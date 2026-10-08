//! Frame limits and connection-failure shutdown around the SDK's JSON-RPC parser.
use rmcp::{
    RoleServer,
    model::{ErrorData, JsonRpcMessage, ServerResult},
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use std::{
    io::{self, Write},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::sync::CancellationToken;

const INBOUND_LIMIT: usize = 4 * 1024 * 1024;
const OUTBOUND_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Shutdown {
    token: CancellationToken,
    stopped: Arc<AtomicBool>,
    cancel_jobs: Arc<dyn Fn() + Send + Sync>,
}

impl Shutdown {
    pub(super) fn new(
        token: CancellationToken,
        cancel_jobs: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            token,
            stopped: Arc::new(AtomicBool::new(false)),
            cancel_jobs: Arc::new(cancel_jobs),
        }
    }

    pub(super) fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            // Admission stops before service cancellation can discard pending handlers.
            (self.cancel_jobs)();
            self.token.cancel();
        }
    }
}

struct FrameLimitedRead<R> {
    inner: R,
    frame_bytes: usize,
    limit: usize,
    shutdown: Shutdown,
}

impl<R: AsyncRead + Unpin> AsyncRead for FrameLimitedRead<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        // One byte beyond the remaining budget detects oversized unterminated frames.
        // The SDK never sees that chunk, so its persistent line buffer stays bounded.
        let mut scratch = [0u8; 8192];
        let capacity = output
            .remaining()
            .min(scratch.len())
            .min(this.limit - this.frame_bytes + 1);
        let mut read = ReadBuf::new(&mut scratch[..capacity]);
        match Pin::new(&mut this.inner).poll_read(cx, &mut read) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => {
                this.shutdown.stop();
                Poll::Ready(Err(error))
            }
            Poll::Ready(Ok(())) => {
                if read.filled().is_empty() {
                    this.shutdown.stop();
                }
                let mut frame_bytes = this.frame_bytes;
                for &byte in read.filled() {
                    frame_bytes += 1;
                    if frame_bytes > this.limit {
                        this.shutdown.stop();
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "MCP inbound frame exceeds 4 MiB",
                        )));
                    }
                    if byte == b'\n' {
                        frame_bytes = 0;
                    }
                }
                this.frame_bytes = frame_bytes;
                output.put_slice(read.filled());
                Poll::Ready(Ok(()))
            }
        }
    }
}

struct WireBudget {
    remaining: usize,
}

impl Write for WireBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP outbound envelope exceeds 8 MiB",
            ));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn within_budget(item: &TxJsonRpcMessage<RoleServer>, limit: usize) -> bool {
    // The SDK encoder appends a newline. Count its byte as part of the envelope.
    limit
        .checked_sub(1)
        .is_some_and(|remaining| serde_json::to_writer(WireBudget { remaining }, item).is_ok())
}

fn reject_large(item: TxJsonRpcMessage<RoleServer>) -> TxJsonRpcMessage<RoleServer> {
    let error = super::types::SessionError::new(
        "mcp.result_too_large",
        "The complete result exceeds the 8 MiB JSON-RPC wire limit; no result was truncated.",
    );
    match item {
        JsonRpcMessage::Response(response)
            if matches!(response.result, ServerResult::CallToolResult(_)) =>
        {
            JsonRpcMessage::response(
                ServerResult::CallToolResult(super::tool_error(error)),
                response.id,
            )
        }
        JsonRpcMessage::Response(response) => JsonRpcMessage::error(
            ErrorData::internal_error(error.detail, Some(serde_json::json!({"code": error.code}))),
            Some(response.id),
        ),
        JsonRpcMessage::Error(error_response) => JsonRpcMessage::error(
            ErrorData::internal_error(error.detail, Some(serde_json::json!({"code": error.code}))),
            error_response.id,
        ),
        _ => JsonRpcMessage::error(
            ErrorData::internal_error(error.detail, Some(serde_json::json!({"code": error.code}))),
            None,
        ),
    }
}

pub(super) struct BoundedStdio<R: AsyncRead + Unpin, W: AsyncWrite> {
    inner: AsyncRwTransport<RoleServer, FrameLimitedRead<R>, W>,
    shutdown: Shutdown,
    outbound_limit: usize,
}

impl<R, W> BoundedStdio<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    pub(super) fn new(read: R, write: W, shutdown: Shutdown) -> Self {
        let read = FrameLimitedRead {
            inner: read,
            frame_bytes: 0,
            limit: INBOUND_LIMIT,
            shutdown: shutdown.clone(),
        };
        Self {
            inner: AsyncRwTransport::new_server(read, write),
            shutdown,
            outbound_limit: OUTBOUND_LIMIT,
        }
    }
}

impl<R, W> Transport<RoleServer> for BoundedStdio<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let item = if within_budget(&item, self.outbound_limit) {
            item
        } else {
            reject_large(item)
        };
        let allowed = within_budget(&item, self.outbound_limit);
        let shutdown = self.shutdown.clone();
        let send = allowed.then(|| self.inner.send(item));
        async move {
            let result = match send {
                Some(send) => send.await,
                None => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "MCP response cannot fit the wire limit",
                )),
            };
            if result.is_err() {
                shutdown.stop();
            }
            result
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let result = self.inner.receive().await;
        if result.is_none() {
            self.shutdown.stop();
        }
        result
    }

    async fn close(&mut self) -> io::Result<()> {
        self.shutdown.stop();
        self.inner.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{CallToolResult, RequestId};
    use std::{sync::atomic::AtomicUsize, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    fn shutdown() -> (Shutdown, Arc<AtomicUsize>, CancellationToken) {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let token = CancellationToken::new();
        (
            Shutdown::new(token.clone(), move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
            calls,
            token,
        )
    }

    struct BrokenRead;

    impl AsyncRead for BrokenRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "input pipe failed",
            )))
        }
    }

    #[tokio::test]
    async fn read_failure_stops_admission_before_cancelling_service() {
        let token = CancellationToken::new();
        let observed_token = token.clone();
        let admission_stopped = Arc::new(AtomicBool::new(false));
        let observed_admission = admission_stopped.clone();
        let stop = Shutdown::new(token.clone(), move || {
            assert!(!observed_token.is_cancelled());
            observed_admission.store(true, Ordering::Release);
        });
        let mut transport = BoundedStdio::new(BrokenRead, tokio::io::sink(), stop);
        assert!(transport.receive().await.is_none());
        assert!(admission_stopped.load(Ordering::Acquire));
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn newlines_reset_budget_and_eof_stops_once() {
        let (stop, calls, token) = shutdown();
        let mut read = FrameLimitedRead {
            inner: &b"1234\n5678\n"[..],
            frame_bytes: 0,
            limit: 5,
            shutdown: stop.clone(),
        };
        let mut bytes = Vec::new();
        read.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"1234\n5678\n");
        assert!(token.is_cancelled());
        stop.stop();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_partial_read_keeps_frame_budget() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let (stop, _, token) = shutdown();
        let limited = FrameLimitedRead {
            inner: reader,
            frame_bytes: 0,
            limit: 8,
            shutdown: stop,
        };
        let mut reader = BufReader::new(limited);
        writer.write_all(b"123456").await.unwrap();
        let mut frame = Vec::new();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                reader.read_until(b'\n', &mut frame)
            )
            .await
            .is_err()
        );
        assert_eq!(frame, b"123456");
        writer.write_all(b"789").await.unwrap();
        let error = reader.read_until(b'\n', &mut frame).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(frame.len() <= 8);
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn broken_writer_stops_without_waiting_for_stdin_eof() {
        let (keep_stdin_open, reader) = tokio::io::duplex(64);
        let (writer, broken_peer) = tokio::io::duplex(64);
        drop(broken_peer);
        let (stop, calls, token) = shutdown();
        let mut transport = BoundedStdio::new(reader, writer, stop);
        let message = JsonRpcMessage::response(
            ServerResult::CallToolResult(CallToolResult::structured(
                serde_json::json!({"version":1}),
            )),
            RequestId::Number(1),
        );
        assert!(transport.send(message).await.is_err());
        assert!(token.is_cancelled());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(keep_stdin_open);
    }

    #[tokio::test]
    async fn oversized_result_is_explicit_and_preserves_request_id() {
        let (keep_stdin_open, reader) = tokio::io::duplex(64);
        let (writer, mut peer) = tokio::io::duplex(4096);
        let (stop, _, token) = shutdown();
        let mut transport = BoundedStdio::new(reader, writer, stop);
        transport.outbound_limit = 1024;
        let message = JsonRpcMessage::response(
            ServerResult::CallToolResult(CallToolResult::structured(
                serde_json::json!({"diagnosis":"x".repeat(600)}),
            )),
            RequestId::Number(42),
        );
        assert!(!within_budget(&message, 1024)); // Includes structured AND escaped fallback.
        transport.send(message).await.unwrap();
        let mut line = String::new();
        BufReader::new(&mut peer)
            .read_line(&mut line)
            .await
            .unwrap();
        assert!(line.len() <= 1024);
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], 42);
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "mcp.result_too_large"
        );
        assert!(!token.is_cancelled());
        drop(keep_stdin_open);
    }

    #[test]
    fn envelope_budget_includes_newline_and_json_escaping() {
        let item = JsonRpcMessage::response(
            ServerResult::CallToolResult(CallToolResult::structured(
                serde_json::json!({"text":"\"\\\n"}),
            )),
            RequestId::Number(1),
        );
        let length = serde_json::to_vec(&item).unwrap().len();
        assert!(within_budget(&item, length + 1));
        assert!(!within_budget(&item, length));
    }
}
