//! Small real-stdio protocol client with owned process-tree cleanup and deadlines.
use process_wrap::std::{ChildWrapper, CommandWrap};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Client {
    child: Box<dyn ChildWrapper>,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<Result<Value, String>>,
    diagnostics: Arc<Mutex<Vec<u8>>>,
    readers: Vec<JoinHandle<()>>,
    close_output: Arc<AtomicBool>,
    next_id: u64,
}

impl Client {
    pub fn spawn(engine: Option<&Path>) -> Self {
        let executable =
            std::env::var_os("NOH_MCP_EXE").unwrap_or_else(|| env!("CARGO_BIN_EXE_noh-mcp").into());
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(engine) = engine {
            command.env("NOH_FFMPEG", engine);
        }
        let mut command = CommandWrap::from(command);
        #[cfg(windows)]
        {
            use process_wrap::std::{CreationFlags, JobObject};
            let mut flags = CreationFlags(Default::default());
            flags.0.0 = 0x08000000;
            command.wrap(flags).wrap(JobObject);
        }
        #[cfg(unix)]
        command.wrap(process_wrap::std::ProcessSession);
        let mut child = command.spawn().expect("Start MCP process");
        let input = child.stdin().take();
        let output = child.stdout().take().unwrap();
        let mut error = child.stderr().take().unwrap();
        let (tx, replies) = mpsc::sync_channel(64);
        let close_output = Arc::new(AtomicBool::new(false));
        let reader_close = close_output.clone();
        let reader = thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                let size = output
                    .by_ref()
                    .take(8 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut line);
                match size {
                    Ok(0) => break,
                    Ok(n) if n > 8 * 1024 * 1024 => {
                        let _ = tx.try_send(Err("MCP response exceeds wire limit".into()));
                        break;
                    }
                    Ok(_) => {
                        if reader_close.load(Ordering::Acquire) {
                            let _ = tx.try_send(Err("Output closed by test".into()));
                            break;
                        }
                        let result = serde_json::from_slice(&line)
                            .map_err(|e| format!("Non-protocol stdout: {e}"));
                        if tx.try_send(result).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.try_send(Err(format!("MCP stdout: {e}")));
                        break;
                    }
                }
            }
        });
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let captured = diagnostics.clone();
        let stderr = thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(n) = error.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let mut bytes = captured.lock().unwrap();
                let keep = n.min(16384usize.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buffer[..keep]);
            }
        });
        Self {
            child,
            input,
            replies,
            diagnostics,
            readers: vec![reader, stderr],
            close_output,
            next_id: 0,
        }
    }

    pub fn initialize(&mut self, version: &str) -> Value {
        let reply = self.call(
            "initialize",
            json!({
                "protocolVersion":version,"capabilities":{},
                "clientInfo":{"name":"noh-regression","version":"1"}
            }),
        );
        assert!(reply.get("error").is_none(), "{reply}");
        self.notify("notifications/initialized", json!({}));
        reply["result"].clone()
    }

    pub fn send(&mut self, value: &Value) {
        let input = self.input.as_mut().expect("Open MCP input");
        serde_json::to_writer(&mut *input, value).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    pub fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let reply = self
                .replies
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| panic!("MCP response {id}: {e}; stderr: {}", self.stderr()))
                .unwrap_or_else(|e| panic!("{e}; stderr: {}", self.stderr()));
            if reply.get("id").is_none() {
                continue;
            }
            assert_eq!(reply["id"], id, "Unexpected response: {reply}");
            return reply;
        }
    }

    pub fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.call("tools/call", json!({"name":name,"arguments":arguments}));
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.diagnostics.lock().unwrap()).into_owned()
    }

    pub fn close_input(&mut self) {
        self.input.take();
    }

    pub fn close_output(&mut self) {
        self.close_output.store(true, Ordering::Release);
        self.send(&json!({"jsonrpc":"2.0","id":9998,"method":"ping"}));
        let reply = self.replies.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(reply, Err("Output closed by test".into()));
        self.readers.remove(0).join().unwrap();
    }

    pub fn raw(&mut self, bytes: &[u8]) {
        let input = self.input.as_mut().expect("Open MCP input");
        let _ = input.write_all(bytes);
        let _ = input.flush();
    }

    pub fn wait_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if self.child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "MCP did not stop after EOF: {}",
                self.stderr()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.start_kill();
        let _ = self.child.inner_mut().wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

pub fn structured(result: &Value) -> Value {
    let data = result
        .get("structuredContent")
        .expect("Structured tool output");
    let text = result["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "text")
        .and_then(|c| c["text"].as_str())
        .expect("JSON text fallback");
    assert_eq!(&serde_json::from_str::<Value>(text).unwrap(), data);
    data.clone()
}
