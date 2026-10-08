//! Optional local MCP adapter; media work remains in the shared job controller.
pub mod session;
mod transport;
pub mod types;

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{common::schema_for_output, wrapper::Parameters},
    model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
};
use serde::Serialize;
use session::Session;
use std::{io, path::PathBuf};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;
use types::*;

#[derive(Serialize, JsonSchema)]
struct ErrorReply {
    version: u32,
    error: SessionError,
}

#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)] // Schema-only union, replies preserve their flat wire shape.
enum Reply<T> {
    Success(T),
    Error(ErrorReply),
}

fn tool_error(error: SessionError) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({ "version": 1, "error": error }))
}

async fn blocking<T, F>(
    work: F,
    failed_result: impl FnOnce(&T) -> bool + Send + 'static,
) -> CallToolResult
where
    T: Serialize + Send + 'static,
    F: FnOnce() -> Result<T, SessionError> + Send + 'static,
{
    // JSON serialization and fallback formatting can also be substantial for an
    // exact diagnosis. Keep them off the async protocol executor with the call.
    match tokio::task::spawn_blocking(move || {
        let reply = match work() {
            Ok(reply) => reply,
            Err(error) => return tool_error(error),
        };
        let failed = failed_result(&reply);
        match serde_json::to_value(reply) {
            Ok(value) if failed => CallToolResult::structured_error(value),
            Ok(value) => CallToolResult::structured(value),
            Err(error) => {
                let mut detail =
                    SessionError::new("mcp.internal", "The job result could not be serialized.");
                detail.technical = error.to_string();
                tool_error(detail)
            }
        }
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            let mut detail = SessionError::new(
                "mcp.internal",
                "The job adapter could not finish this request.",
            );
            detail.technical = error.to_string();
            tool_error(detail)
        }
    }
}

#[derive(Clone)]
struct Server {
    session: Session,
}

#[tool_router]
impl Server {
    #[tool(name = "noh_inspect", description = "Start inspection of one local video, WAV or PNG/JPEG image. Returns a job ID; poll noh_job for metadata. Image duration belongs to the render request, not the image file.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn inspect(&self, Parameters(input): Parameters<InspectInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.inspect(input), |_| false).await
    }

    #[tool(name = "noh_diagnose", description = "Start quick metadata or exact render diagnosis. Exact is the default and is required for export. No media bytes are returned.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn diagnose(&self, Parameters(input): Parameters<DiagnoseInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.diagnose(input), |_| false).await
    }

    #[tool(name = "noh_export", description = "Start export using a completed exact diagnosis from this session. Source stamps and render settings must still match. Destination must be free; existing files are never overwritten.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn export(&self, Parameters(input): Parameters<ExportInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.export(input), |_| false).await
    }

    #[tool(name = "noh_preview", description = "Render the full montage at reduced preview quality to an explicit free MP4 destination. The adapter sets preview mode; existing files are never overwritten. No player is launched.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn preview(&self, Parameters(input): Parameters<PreviewInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.preview(input), |_| false).await
    }

    #[tool(name = "noh_transcribe", description = "Start local CPU transcription from an explicitly selected audio file or a video's first audio stream. Requires explicit local whisper.cpp executable, model and VAD model paths; nothing is downloaded. Source audio is limited to two hours. Publishes UTF-8 SRT to a free .srt path without overwrite; review the recognized text and timing before use. Returns a job ID; poll noh_job for the track.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn transcribe(&self, Parameters(input): Parameters<TranscribeInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.transcribe(input), |_| false).await
    }

    #[tool(name = "noh_burn_subtitles", description = "Render reviewed plain UTF-8 SRT onto one local video (maximum two hours) and publish a new MP4 without overwriting. Re-encodes video; copies first AAC audio or converts first audio to AAC. Uses bundled fonts and rejects unsupported or oversized text. ASS timing rounds endpoints to 10 ms. Optional preview renders the same composition before reducing quality; no player is launched. Returns a job ID; poll noh_job for warnings and the exported path.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn captions(&self, Parameters(input): Parameters<CaptionsInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.captions(input), |_| false).await
    }

    #[tool(name = "noh_make_short", description = "Extract an explicit start_ms inclusive/end_ms exclusive interval from one local video into a vertical MP4, without overwriting. Pad preserves the full picture on black; crop fills the canvas by centered crop. Always re-encodes video and the first audio stream, preserving its source timing. Optional reviewed SRT captions use original source times and are clipped and shifted to the interval. Preview uses the same composition at reduced resolution. Returns a session job ID; poll noh_job for warnings, measured duration and exported path. No montage diagnosis is required.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn short(&self, Parameters(input): Parameters<ShortInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.short(input), |_| false).await
    }

    #[tool(name = "noh_export_project", description = "Compose original ordered media and WAV directly into a full montage or selected vertical short, with optional reviewed WAV-timed SRT burned after framing. Short start_ms/end_ms select the WAV; restart_loops defaults false and changes visual phase only, including synchronized original clip audio. Fades retain their full-project WAV clock. Selected pieces are prepared privately and losslessly; final composed video is encoded once. Composed output requires MP4 and WAV duration at most two hours. Without short or captions, existing montage behavior is preserved. No diagnosis or full-project intermediate is required. Existing files are never overwritten. Returns a job ID; poll noh_job.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn project(&self, Parameters(input): Parameters<ProjectInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.project(input), |_| false).await
    }

    #[tool(name = "noh_job", description = "Read a session job. Optionally wait up to 30000 ms for a revision change. Request include_result for terminal metadata, diagnosis, export, subtitle track or structured failure; results contain no media bytes.", output_schema = schema_for_output::<Reply<JobReply>>())]
    async fn job(&self, Parameters(input): Parameters<JobInput>) -> CallToolResult {
        let session = self.session.clone();
        let include_result = input.include_result;
        blocking(
            move || session.job(input),
            move |reply| include_result && reply.state == JobState::Failed,
        )
        .await
    }

    #[tool(name = "noh_cancel", description = "Request cooperative cancellation of a session job. A publication already committed remains succeeded. Poll noh_job for the terminal state.", output_schema = schema_for_output::<Reply<JobReply>>())]
    async fn cancel(&self, Parameters(input): Parameters<CancelInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.cancel(input), |_| false).await
    }
}

#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("noh-mcp", crate::build_info::VERSION))
            .with_instructions("Local media jobs only. Use absolute paths. Montage requests provide exactly one videos list or ordered items list; image entries require positive duration in seconds. Transcription uses explicit local runtime and model files and downloads nothing. Caption burning takes a video and reviewed SRT directly; it re-encodes video without requiring a montage diagnosis. Short extraction takes an explicit source interval and framing, with optional reviewed source-timed SRT, and needs no montage diagnosis. Start a job, poll noh_job by revision, and obtain a completed exact diagnosis before montage export. Only one job may run per session. Wire format version is 1.")
    }
}

/// Run one stdio session. Cleanup is awaited even when initialization fails.
pub async fn run_stdio(default_ffmpeg: PathBuf) -> io::Result<()> {
    serve_io(default_ffmpeg, tokio::io::stdin(), tokio::io::stdout()).await
}

async fn serve_io<R, W>(default_ffmpeg: PathBuf, read: R, write: W) -> io::Result<()>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let session = Session::new(default_ffmpeg).map_err(io::Error::other)?;
    let token = CancellationToken::new();
    let stop_session = session.clone();
    let shutdown = transport::Shutdown::new(token.clone(), move || {
        stop_session.stop_admission_and_cancel()
    });
    let transport = transport::BoundedStdio::new(read, write, shutdown.clone());
    let result = match (Server {
        session: session.clone(),
    })
    .serve_with_ct(transport, token)
    .await
    {
        Ok(service) => service
            .waiting()
            .await
            .map(|_| ())
            .map_err(io::Error::other),
        Err(error) => Err(io::Error::other(error)),
    };
    shutdown.stop();
    tokio::task::spawn_blocking(move || session.join_and_drop_jobs())
        .await
        .map_err(io::Error::other)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_tools_have_strict_inputs_and_explicit_output_schemas() {
        let tools = Server::tool_router().list_all();
        assert_eq!(tools.len(), 10);
        let transcribe = tools
            .iter()
            .find(|tool| tool.name == "noh_transcribe")
            .unwrap();
        let input_schema = serde_json::to_value(&transcribe.input_schema).unwrap();
        for field in ["source", "output", "transcriber", "model", "vad_model"] {
            assert!(
                input_schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!(field))
            );
        }
        assert_eq!(input_schema["additionalProperties"], false);
        assert!(input_schema["properties"]["ffmpeg"].is_object());
        let captions = tools
            .iter()
            .find(|tool| tool.name == "noh_burn_subtitles")
            .unwrap();
        let schema = serde_json::to_value(&captions.input_schema).unwrap();
        for field in ["source", "subtitles", "output"] {
            assert!(
                schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!(field))
            );
        }
        for tool in tools {
            assert!(tool.name.starts_with("noh_"));
            assert_eq!(
                tool.input_schema.get("additionalProperties"),
                Some(&serde_json::json!(false))
            );
            assert!(
                tool.output_schema
                    .as_ref()
                    .is_some_and(|schema| schema.contains_key("anyOf"))
            );
        }
    }

    #[tokio::test]
    async fn tool_errors_include_matching_json_fallback() {
        let result = blocking(
            || Err::<StartReply, _>(SessionError::new("mcp.busy", "A job is running.")),
            |_| false,
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        let serialized = serde_json::to_value(&result).unwrap();
        let fallback: serde_json::Value =
            serde_json::from_str(serialized["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(fallback, serialized["structuredContent"]);
        assert_eq!(fallback["error"]["code"], "mcp.busy");
    }

    #[tokio::test]
    async fn failed_result_keeps_job_identity_and_sets_tool_error() {
        let result = blocking(
            || {
                Ok(JobReply {
                    version: 1,
                    job_id: "session-job-7".into(),
                    state: JobState::Failed,
                    revision: 3,
                    changed: true,
                    progress: None,
                    warnings: None,
                    diagnostics: None,
                    result: None,
                    error: Some(SessionError::new(
                        "error.ffmpeg",
                        "The engine could not be started.",
                    )),
                })
            },
            |reply| reply.state == JobState::Failed,
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        let body = result.structured_content.unwrap();
        assert_eq!(body["job_id"], "session-job-7");
        assert_eq!(body["revision"], 3);
        assert_eq!(body["error"]["code"], "error.ffmpeg");
    }

    #[tokio::test]
    async fn initialization_eof_returns_after_cleanup() {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            serve_io(
                PathBuf::from("ffmpeg"),
                tokio::io::empty(),
                tokio::io::sink(),
            ),
        )
        .await;
        assert!(result.expect("EOF must not hang initialization").is_err());
    }
}
