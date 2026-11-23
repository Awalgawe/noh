//! Optional local MCP adapter; media work remains in the shared job controller.
pub mod apps;
pub mod machine;
pub mod scene;
pub mod session;
pub mod slot;
mod transport;
pub mod types;

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{common::schema_for_output, tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, JsonObject, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
};
use scene::{SceneCheckInput, SceneClipInput, SceneLookInput};
use serde::Serialize;
use session::Session;
use std::{io, path::PathBuf, sync::Arc};
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

#[derive(Serialize, JsonSchema)]
struct AppErrorReply {
    version: u32,
    error: AppError,
}

#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)] // Schema-only union, as `Reply`.
enum AppReply<T> {
    Success(T),
    Error(AppErrorReply),
}

/// A window tool's call, off the async executor: it may wait for a window.
async fn app_blocking<T, F>(work: F) -> CallToolResult
where
    T: Serialize + Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    let outcome = tokio::task::spawn_blocking(move || match work() {
        Ok(reply) => serde_json::to_value(reply)
            .map(CallToolResult::structured)
            .map_err(|e| AppError::new("mcp.internal", e.to_string())),
        Err(error) => Err(error),
    })
    .await
    .unwrap_or_else(|e| Err(AppError::new("mcp.internal", e.to_string())));
    outcome.unwrap_or_else(|error| {
        CallToolResult::structured_error(serde_json::json!({ "version": 1, "error": error }))
    })
}

#[derive(Clone)]
struct Server {
    session: Session,
    apps: apps::Apps,
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

    #[tool(name = "noh_burn_subtitles", description = "Render reviewed plain UTF-8 SRT (or its NOH lyrics file .noh-lyrics.json) onto one local video (maximum two hours) and publish a new MP4 without overwriting. Before starting, NOH keeps the .srt and its lyrics file identical: it creates the lyrics file beside an .srt given alone; when they differ, `truth` (lyrics or srt) is required and rewrites the other file, keeping the replaced one beside as <name>.replaced…; without it the start is refused (error.lyrics_truth_required) and nothing is written. The start reply lists what was done in `lyrics`. Re-encodes video; copies first AAC audio or converts first audio to AAC. Captions are drawn frame-accurately by NOH's OpenGL engine, as in the app: in the Classique style by default, or in the caption effect named by `effect` (id, preset, values; see noh_list_effects), checked before the job starts. Unsupported characters are rejected, and text leaving the safe area is drawn with a warning. Without an OpenGL context libass draws Classique instead (same line breaks, a warning; its timing rounds endpoints to 10 ms and it rejects oversized text); a chosen effect has no fallback: the job fails and nothing is published. An installed font (a font value system:<family>) missing on this computer is drawn in the effect's own font with warning.effect_font_missing. Lines break between words only: a word wider than the text area stays whole and centred, with warning.effect_word_too_wide. Optional preview renders the same composition before reducing quality; no player is launched. Returns a job ID; poll noh_job for warnings and the exported path.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn captions(&self, Parameters(input): Parameters<CaptionsInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.captions(input), |_| false).await
    }

    #[tool(name = "noh_make_short", description = "Extract an explicit start_ms inclusive/end_ms exclusive interval from one local video into a vertical MP4, without overwriting. Pad preserves the full picture on black; crop fills the canvas by centered crop; experimental framing \"scene\" draws the motion scene in `motion` on the Short, framed by its video() (covering and centred without one), with captions.subtitles and captions.safe_area required (ask the user's platform) and no size, placement or effect. Always re-encodes video and the first audio stream, preserving its source timing. Optional reviewed SRT captions (or their NOH lyrics file; the .srt and lyrics file are kept identical before starting, `truth` required when they differ, as in noh_burn_subtitles) use original source times and are clipped and shifted to the interval; they are drawn by the OpenGL engine in Classique, or in the caption effect named by captions.effect (see noh_list_effects, as in noh_burn_subtitles); libass is Classique's fallback only. Preview uses the same composition at reduced resolution. Returns a session job ID; poll noh_job for warnings, measured duration and exported path. No montage diagnosis is required.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn short(&self, Parameters(input): Parameters<ShortInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.short(input), |_| false).await
    }

    #[tool(name = "noh_export_project", description = "Compose original ordered media and WAV directly into a full montage or selected vertical short, with optional reviewed WAV-timed SRT burned after framing (or its NOH lyrics file; the .srt and lyrics file are kept identical before starting, `truth` required when they differ, as in noh_burn_subtitles). Short start_ms/end_ms select the WAV; restart_loops defaults false and changes visual phase only, including synchronized original clip audio. Fades retain their full-project WAV clock. Selected pieces are prepared privately and losslessly; final composed video is encoded once. Composed output requires MP4 and WAV duration at most two hours. Captions are drawn frame-accurately in NOH's Classique style by its OpenGL engine, as in the app; without an OpenGL context they fall back to libass with the same line breaks and a warning, and a preview render uses libass. captions.effect names another caption effect (see noh_list_effects, as in noh_burn_subtitles): it has no libass fallback and is refused with preview. Without short or captions, existing montage behavior is preserved. No diagnosis or full-project intermediate is required. Existing files are never overwritten. Returns a job ID; poll noh_job.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn project(&self, Parameters(input): Parameters<ProjectInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.project(input), |_| false).await
    }

    #[tool(name = "noh_scene_check", description = "Experimental. Check a motion scene (a script file that draws the lyrics, shapes and particles over a video; its language, functions, caps and the author guide are in noh_list_effects, `motion`) against a video and its reviewed subtitles: every frame of the interval at the video's frame rate is evaluated, checked against the counted caps, and asked again in another order; the layer shaders beside the scene (<scene>.shaders/, WGSL) are validated, then compiled and drawn once in NOH's worker process (a driver that fails on one stops the worker: error.scene_worker_stopped, nothing published). Writes report.json into a new output folder: the first refused frame with its instant and reason (the job fails), or the counted maxima against the caps, warnings (text leaving the frame or the safe area, overlaps), what was not checked, the script's time and the whole frame's time on this machine (information, never a limit). A vertical video needs a safe_area: ask the user which platform the video is for, proposing YouTube Shorts. If you cannot ask yet, draft with universal (the strictest) and ask before exporting. The subtitles are kept identical to their lyrics file as in noh_burn_subtitles: NOH creates the lyrics file beside an .srt given alone. The job's result is kind export, its output the folder. One job at a time: a check is refused (mcp.busy) while another job runs. In this version a scene is drawn over one video (a caption burn): its sound gives the energy and onsets, its length the song's, its size the canvas; scenes over a montage (a song WAV and several clips) come with the project export. Returns a job ID; poll noh_job.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn scene_check(&self, Parameters(input): Parameters<SceneCheckInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.scene_check(input), |_| false).await
    }

    #[tool(name = "noh_scene_look", description = "Experimental. See frames of a motion scene drawn by the export's engine over the video: stills (PNG frames at at_ms), strip (eight frames over one second and a sheet of them), sheet (count frames at the middle of equal parts of the interval, and a sheet; look.json gives each frame's time), or measure (where each text falls, as JSON, no picture). grid draws the frame's edges, a line every tenth of the frame and the safe area; bare shows the video alone, without the scene's draws, to compose on it. The scene is checked first, as noh_scene_check does; a refused scene fails the job. Files go into a new output folder with look.json listing them; open the PNGs to judge them. A vertical video needs a safe_area (ask the user's platform, proposing YouTube Shorts). If you cannot ask yet, draft with universal (the strictest) and ask before exporting. The subtitles are kept identical to their lyrics file as in noh_burn_subtitles: NOH creates the lyrics file beside an .srt given alone. The job's result is kind export, its output the folder. One job at a time: a look is refused (mcp.busy) while another job runs. With app_id (a window you drive), that window first moves its playhead to the instant looked at (waiting while the person navigates) and draws the frame's grid, and on the short the safe area, over its monitor, to show the person where you look; its refusals start nothing. In this version a scene is drawn over one video (a caption burn): its sound gives the energy and onsets, its length the song's, its size the canvas; scenes over a montage (a song WAV and several clips) come with the project export. Returns a job ID; poll noh_job.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn scene_look(&self, Parameters(input): Parameters<SceneLookInput>) -> CallToolResult {
        let session = self.session.clone();
        let apps = self.apps.clone();
        blocking(
            move || {
                // The window shows where the agent looks before the frames
                // are drawn; a window that refuses (the person navigates, it
                // was taken back) starts nothing.
                if let Some(app_id) = &input.app_id {
                    apps.act(app_id, input.marker()).map_err(|e| {
                        let mut error = SessionError::new(&e.code, &e.detail);
                        error.technical = e.details.map(|d| d.to_string()).unwrap_or_default();
                        error
                    })?;
                }
                session.scene_look(input)
            },
            |_| false,
        )
        .await
    }

    #[tool(name = "noh_scene_clip", description = "Experimental. Render a short clip (at most 30 s, preview quality) of the video with a motion scene drawn over it, to judge a rhythm: the same drawing as noh_burn_subtitles with `motion`, on start_ms..end_ms of the source; the scene keeps the source's clock. Checked before it starts; nothing published on a refusal. Publishes a new MP4 without overwriting. Its frames are checked at the clip's own instants (start plus whole frames); lyrics times stay on the source's clock. `vertical` draws on a vertical Short made from the video, as noh_make_short framing \"scene\" exports it. A vertical frame needs a safe_area (ask the user's platform). The subtitles are kept identical to their lyrics file as in noh_burn_subtitles: NOH creates the lyrics file beside an .srt given alone. One job at a time: refused (mcp.busy) while another job runs. Returns a job ID; poll noh_job.", output_schema = schema_for_output::<Reply<StartReply>>())]
    async fn scene_clip(&self, Parameters(input): Parameters<SceneClipInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.scene_clip(input), |_| false).await
    }

    #[tool(name = "noh_list_effects", description = "List the caption effects that `effect.id` can name in noh_burn_subtitles, noh_make_short and noh_export_project: each effect's presets and settings (type, choices, range, default), the folders NOH reads (bundled beside noh.exe, then the user's) and the folders it refused with their reason. In a build with motion scenes it also returns `motion`: the scene format line, every callable function, the caps, the font faces, the safe areas and the author guide (read it before writing a scene for noh_scene_check, noh_scene_look, noh_scene_clip or noh_burn_subtitles `motion`). Reads files only; starts no job.", output_schema = schema_for_output::<Reply<EffectsReply>>())]
    async fn list_effects(&self, Parameters(_): Parameters<ListEffectsInput>) -> CallToolResult {
        blocking(
            || Ok(EffectsReply::from(crate::effect_choice::list())),
            |_| false,
        )
        .await
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

    #[tool(name = "noh_app_open", description = "Open a NOH window that you drive and the person watches: the ordinary NOH application, started by this server with nothing listening. The person can play, pause and move through the video in it, but cannot edit: they ask you in the conversation. They can stop the piloting at any time (« Arrêter le pilotage », or closing the window); the window then becomes theirs, an ordinary NOH. The window stays open after this session ends, as an ordinary NOH. Opens only if the machine has room: free memory and graphics memory are measured, each open window keeps a margin, at most 8 windows; a refusal (app.cap_reached) gives the figures. Returns app_id and every window of this session with its state.", output_schema = schema_for_output::<AppReply<AppOpenReply>>())]
    async fn app_open(&self, Parameters(input): Parameters<AppOpenInput>) -> CallToolResult {
        let apps = self.apps.clone();
        app_blocking(move || apps.open(&input.title)).await
    }

    #[tool(name = "noh_app_close", description = "Close a NOH window you opened and still drive. Pending lyrics changes are written first; nothing is ever discarded. If a question is open in the window, or writing the lyrics needs one (first change of an .srt), nothing closes and the error app.question_open says which. A job running refuses the close (app.job_running). A window the person closed (app.closed) or took back (app.released_by_user) is not yours to close.", output_schema = schema_for_output::<AppReply<AppCloseReply>>())]
    async fn app_close(&self, Parameters(input): Parameters<AppCloseInput>) -> CallToolResult {
        let apps = self.apps.clone();
        app_blocking(move || apps.close(&input.app_id)).await
    }

    #[tool(name = "noh_app_do", description = "One gesture in a NOH window you drive, applied as the matching click: add files (as a drop), remove or move a clip, a picture's duration, fades and the clips' sound, convert all, a lyrics line (set_text, set_start, set_end, shift, add, delete; based_on = the lyrics revision you read, refused with app.state_changed if they were reloaded since), undo, redo, the caption effect and its settings (preset, values, size, placement, show_on_video), the safe area, the short's range and framing, the destination, export (montage or short; the answer says whether it started, or why not, e.g. app.not_started when the destination folder was removed meanwhile; state follows the job), cancel, seek, play, pause, an answer to the open question, or (experimental, a build with motion scenes) set_scene: the scene file you write, reread when it changes, its light check in state.scene. Gestures the interface locks during a job are refused with app.job_running. Seek, play and pause wait while the person navigates (up to 3 s after their last gesture) and are then refused with app.user_navigating and retry_after_ms; never take the playhead from the person. The open question and its kind are in noh_app_state (question); answers that drop work or rewrite a file (modify_srt, keep_lyrics, keep_srt, discard) are given like the others: ask the person first when you judge it right. Returns the state revision the gesture led to.", output_schema = schema_for_output::<AppReply<AppDoReply>>())]
    async fn app_do(&self, Parameters(input): Parameters<AppDoInput>) -> CallToolResult {
        let apps = self.apps.clone();
        app_blocking(move || apps.act(&input.app_id, input.action)).await
    }

    #[tool(name = "noh_app_state", description = "What a NOH window you drive shows, as data: clips (with their ids), song, lyrics (path, revision, lines), effect, safe area, range, whether the export is ready, the job running, the open question with its kind, whether the window is minimized, and user_position: where the person is (playhead, playing, view, clip and lyrics line under the playhead, what they clicked to point it out). When the person says « here » or « this one », read user_position first. Optionally waits up to 30000 ms for a revision after after_revision.", output_schema = schema_for_output::<AppReply<AppStateReply>>())]
    async fn app_state(&self, Parameters(input): Parameters<AppStateInput>) -> CallToolResult {
        let apps = self.apps.clone();
        app_blocking(move || apps.view(input)).await
    }

    #[tool(name = "noh_app_look", description = "A PNG of a NOH window you drive, written to a free absolute .png path (no image bytes are returned): the whole window or the monitor alone. Without at_ms, the screen as it is, where the person is; with at_ms, the playhead moves there first, as seek does (it waits while the person navigates), and stays there. This is NOH's preview (light copies, captions drawn over), not the exported pixels. Refused while the window is minimized (app.hidden).", output_schema = schema_for_output::<AppReply<AppLookReply>>())]
    async fn app_look(&self, Parameters(input): Parameters<AppLookInput>) -> CallToolResult {
        let apps = self.apps.clone();
        app_blocking(move || apps.look(input)).await
    }

    #[tool(name = "noh_cancel", description = "Request cooperative cancellation of a session job. A publication already committed remains succeeded. Poll noh_job for the terminal state.", output_schema = schema_for_output::<Reply<JobReply>>())]
    async fn cancel(&self, Parameters(input): Parameters<CancelInput>) -> CallToolResult {
        let session = self.session.clone();
        blocking(move || session.cancel(input), |_| false).await
    }
}

impl Server {
    /// The tools as served. schemars marks unsigned integers with
    /// `"format": "uint8" | "uint32" | "uint64"`, which JSON Schema does not
    /// define: Claude Code 2.1.323 printed « unknown format "uint32" ignored »
    /// for each such field. The `minimum` (and any `maximum`) beside it keeps
    /// the bound, so the format is dropped from every input and output schema.
    fn router() -> ToolRouter<Self> {
        let mut router = Self::tool_router();
        // A build without motion scenes does not offer their tools.
        if !cfg!(feature = "motion") {
            router.map.retain(|name, _| !name.starts_with("noh_scene_"));
        }
        for route in router.map.values_mut() {
            let tool = &mut route.attr;
            tool.input_schema = Arc::new(without_unsigned_formats(&tool.input_schema));
            if let Some(schema) = &tool.output_schema {
                tool.output_schema = Some(Arc::new(without_unsigned_formats(schema)));
            }
        }
        router
    }
}

fn without_unsigned_formats(schema: &JsonObject) -> JsonObject {
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                if map
                    .get("format")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|format| format.starts_with("uint"))
                {
                    map.remove("format");
                }
                map.values_mut().for_each(strip);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value = serde_json::Value::Object(schema.clone());
    strip(&mut value);
    match value {
        serde_json::Value::Object(map) => map,
        _ => unreachable!("an object stays an object"),
    }
}

#[tool_handler(router = Self::router())]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("noh-mcp", crate::build_info::VERSION))
            .with_instructions("Local media jobs only. Use absolute paths. Montage requests provide exactly one videos list or ordered items list; image entries require positive duration in seconds. Transcription uses explicit local runtime and model files and downloads nothing. Caption burning takes a video and reviewed SRT directly; it re-encodes video without requiring a montage diagnosis. Short extraction takes an explicit source interval and framing, with optional reviewed source-timed SRT, and needs no montage diagnosis. Start a job, poll noh_job by revision, and obtain a completed exact diagnosis before montage export. Only one job may run per session. noh_app_open opens a NOH window you drive while the person watches; they cannot edit in it and ask you instead. Wire format version is 1.")
    }
}

/// Run one stdio session. Cleanup is awaited even when initialization fails.
pub async fn run_stdio(default_ffmpeg: PathBuf) -> io::Result<()> {
    crate::driven::keep_standard_handles_private();
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
    let watch = tokio::spawn(stop_for_update(session.clone(), shutdown.clone()));
    let apps = apps::Apps::new(
        Box::new(apps::Beside),
        Box::new(machine::read),
        apps::Timing::default(),
        session.slot(),
    );
    let result = match (Server {
        session: session.clone(),
        apps: apps.clone(),
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
    watch.abort();
    shutdown.stop();
    tokio::task::spawn_blocking(move || session.join_and_drop_jobs())
        .await
        .map_err(io::Error::other)?;
    // The windows stay open: they are told, and their pipes closed.
    tokio::task::spawn_blocking(move || apps.end_session())
        .await
        .map_err(io::Error::other)?;
    result
}

/// How often a server looks for an update's stop request.
const UPDATE_POLL: std::time::Duration = std::time::Duration::from_secs(1);
/// How long a finished job's result stays readable before an update may stop
/// the server: a client polling `noh_job` reads it within a few seconds.
const UPDATE_QUIET: std::time::Duration = std::time::Duration::from_secs(15);

/// Ends the session for an update of the copy this server runs from
/// (`crate::update_stop`): new jobs are refused while the request stands, and
/// the server stops once no job runs, never in the middle of one. The client
/// sees the server exit; an interactive client must reconnect it afterwards.
async fn stop_for_update(session: Session, shutdown: transport::Shutdown) {
    let mut tick = tokio::time::interval(UPDATE_POLL);
    loop {
        tick.tick().await;
        let requested = tokio::task::spawn_blocking(crate::update_stop::requested_here)
            .await
            .unwrap_or(false);
        session.set_draining(requested);
        if requested && session.close_if_idle(UPDATE_QUIET) {
            shutdown.stop();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_tools_have_strict_inputs_and_explicit_output_schemas() {
        let tools = Server::router().list_all();
        // Sixteen, and the three scene tools when motion scenes are built in.
        assert_eq!(tools.len(), if cfg!(feature = "motion") { 19 } else { 16 });
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

    #[test]
    fn served_schemas_name_no_unsigned_format_and_keep_their_bounds() {
        fn formats(value: &serde_json::Value, found: &mut Vec<String>) {
            match value {
                serde_json::Value::Object(map) => {
                    if let Some(format) = map.get("format").and_then(|f| f.as_str()) {
                        found.push(format.to_owned());
                    }
                    map.values().for_each(|v| formats(v, found));
                }
                serde_json::Value::Array(items) => items.iter().for_each(|v| formats(v, found)),
                _ => {}
            }
        }
        let raw = Server::tool_router().list_all();
        let served = Server::router().list_all();
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for tool in &raw {
            formats(&serde_json::to_value(tool).unwrap(), &mut before);
        }
        for tool in &served {
            formats(&serde_json::to_value(tool).unwrap(), &mut after);
        }
        // The control: schemars does emit them, so the strip has work to do.
        assert!(before.iter().any(|f| f.starts_with("uint")), "{before:?}");
        assert!(!after.iter().any(|f| f.starts_with("uint")), "{after:?}");
        // Other formats are left alone.
        assert!(after.iter().any(|f| f == "double"), "{after:?}");
        let tool = |name: &str| served.iter().find(|t| t.name == name).unwrap();
        let job = serde_json::to_value(tool("noh_job")).unwrap();
        let wait = &job["inputSchema"]["properties"]["wait_ms"];
        assert_eq!(
            (&wait["minimum"], &wait["maximum"]),
            (&0.into(), &30000.into())
        );
        let revision = &job["inputSchema"]["properties"]["after_revision"];
        assert_eq!(revision["minimum"], 0);
        let effects = serde_json::to_value(tool("noh_list_effects")).unwrap();
        let level = &effects["outputSchema"]["$defs"]["EffectSetting"]["properties"]["level"];
        assert_eq!(
            (&level["minimum"], &level["maximum"]),
            (&0.into(), &255.into())
        );
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
