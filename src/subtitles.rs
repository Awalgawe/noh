//! One local whisper.cpp operation. The supervisor owns final publication.
use crate::{
    engine::{EngineError, Event, Message},
    inspection::{FileStamp, Snapshot},
    subtitle_track::{SubtitleCue, SubtitleTrack, ValidationError},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[path = "subtitle_audio.rs"]
mod audio;

pub const MAX_AUDIO_MS: u64 = 7_200_000;
pub const MAX_MODEL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_VAD_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;
pub const TOTAL_TIMEOUT_SECS: u64 = 8100;
const PREPARATION_TIMEOUT_SECS: u64 = 600;
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 7200;
const MAX_SHARED_LIBRARIES: usize = 256;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtitleRequest {
    pub source: PathBuf,
    pub output: PathBuf,
    pub ffmpeg: PathBuf,
    pub transcriber: PathBuf,
    pub model: PathBuf,
    pub vad_model: PathBuf,
    pub language: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubtitleResult {
    pub output: PathBuf,
    pub track: SubtitleTrack,
}

/// Metadata-based invalidation, not content hashing or locked model/source IO.
/// Adjacent whisper/ggml shared-library additions and removals invalidate it too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubtitleSnapshot {
    files: Snapshot,
    runtime_directory: PathBuf,
    libraries: Vec<PathBuf>,
}

impl SubtitleRequest {
    /// Syntax only: IO and runtime availability are checked by snapshot().
    pub fn validate(&self) -> Result<(), EngineError> {
        for (path, code) in [
            (&self.source, "error.subtitle_input"),
            (&self.output, "error.subtitle_config"),
            (&self.transcriber, "error.subtitle_config"),
            (&self.model, "error.subtitle_config"),
            (&self.vad_model, "error.subtitle_config"),
        ] {
            if path.as_os_str().is_empty() || path.as_os_str().as_encoded_bytes().contains(&0) {
                return Err(error(
                    code,
                    "validate_subtitles",
                    Some(path),
                    "A nonempty path without NUL characters is required.",
                ));
            }
        }
        if self.ffmpeg.as_os_str().as_encoded_bytes().contains(&0)
            || !self
                .output
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("srt"))
            || !language_code(&self.language)
        {
            return Err(error(
                "error.subtitle_config",
                "validate_subtitles",
                None,
                "Choose an SRT output and language auto or a two/three-letter lowercase code.",
            ));
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<SubtitleSnapshot, EngineError> {
        self.validate()?;
        let source = stamp(&self.source, "error.subtitle_input")?;
        let ffmpeg = crate::inspection::resolve_ffmpeg(&self.ffmpeg)?;
        let ffmpeg = stamp(&ffmpeg, "error.ffmpeg")?;
        let transcriber = stamp(&self.transcriber, "error.subtitle_config")?;
        let model = stamp(&self.model, "error.subtitle_config")?;
        let vad = stamp(&self.vad_model, "error.subtitle_config")?;
        check_model_size(model.bytes, MAX_MODEL_BYTES, &self.model)?;
        check_model_size(vad.bytes, MAX_VAD_BYTES, &self.vad_model)?;
        let runtime_directory = transcriber
            .resolved
            .parent()
            .ok_or_else(|| {
                error(
                    "error.subtitle_config",
                    "snapshot_subtitles",
                    Some(&self.transcriber),
                    "Transcriber has no parent directory.",
                )
            })?
            .to_path_buf();
        let libraries = shared_libraries(&runtime_directory)?;
        let mut files = vec![source, ffmpeg, transcriber, model, vad];
        for library in &libraries {
            files.push(stamp(library, "error.subtitle_config")?);
        }
        let snapshot = SubtitleSnapshot {
            files: Snapshot(files),
            runtime_directory,
            libraries,
        };
        snapshot.verify()?;
        Ok(snapshot)
    }
}

impl SubtitleSnapshot {
    pub fn verify(&self) -> Result<(), EngineError> {
        self.files.verify().map_err(|failure| {
            let source_changed =
                failure.path.as_ref() == self.files.0.first().map(|stamp| &stamp.path);
            error(
                if source_changed {
                    "error.subtitle_input"
                } else {
                    "error.subtitle_config"
                },
                "verify_subtitles",
                failure.path.as_deref(),
                "Subtitle source, runtime or models changed during the operation.",
            )
        })?;
        if shared_libraries(&self.runtime_directory)? != self.libraries {
            return Err(error(
                "error.subtitle_config",
                "verify_subtitles",
                Some(&self.runtime_directory),
                "Adjacent whisper/ggml shared libraries changed during the operation.",
            ));
        }
        Ok(())
    }
}

fn error(code: &str, operation: &str, path: Option<&Path>, detail: impl ToString) -> EngineError {
    EngineError::new(code, operation, path, detail)
}
fn stamp(path: &Path, code: &str) -> Result<FileStamp, EngineError> {
    FileStamp::read(path).map_err(|e| error(code, "snapshot_subtitles", Some(path), e))
}
fn language_code(language: &str) -> bool {
    language == "auto"
        || ((2..=3).contains(&language.len()) && language.bytes().all(|b| b.is_ascii_lowercase()))
}
fn check_model_size(bytes: u64, limit: u64, path: &Path) -> Result<(), EngineError> {
    if bytes == 0 {
        Err(error(
            "error.subtitle_config",
            "validate_model",
            Some(path),
            "Model file is empty.",
        ))
    } else if bytes > limit {
        Err(error(
            "error.subtitle_limit",
            "validate_model",
            Some(path),
            "Model exceeds the supported size limit.",
        ))
    } else {
        Ok(())
    }
}
fn shared_library_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let prefix = [
        "whisper",
        "ggml",
        "libwhisper",
        "libggml",
        "cudart",
        "cublas",
        "nvrtc",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix));
    prefix
        && (name.ends_with(".dll")
            || name.ends_with(".dylib")
            || name.ends_with(".so")
            || name.contains(".so."))
}
fn shared_libraries(directory: &Path) -> Result<Vec<PathBuf>, EngineError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| {
        error(
            "error.subtitle_config",
            "snapshot_runtime",
            Some(directory),
            e,
        )
    })? {
        let entry = entry.map_err(|e| {
            error(
                "error.subtitle_config",
                "snapshot_runtime",
                Some(directory),
                e,
            )
        })?;
        if entry.file_name().to_str().is_some_and(shared_library_name) {
            if files.len() == MAX_SHARED_LIBRARIES {
                return Err(error(
                    "error.subtitle_limit",
                    "snapshot_runtime",
                    Some(directory),
                    "Runtime has too many adjacent shared libraries.",
                ));
            }
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

/// Called inside an isolated worker. Writes subtitle.srt only in its workspace.
/// The controller revalidates inputs/results and exclusively publishes afterward.
pub(crate) fn execute(
    request: &SubtitleRequest,
    workspace: &Path,
    emit: &mut dyn FnMut(Event),
) -> Result<SubtitleResult, EngineError> {
    let snapshot = request.snapshot()?;
    let files = &snapshot.files.0;
    let source = &files[0].resolved;
    let ffmpeg = &files[1].resolved;
    let transcriber = &files[2].resolved;
    let model = &files[3].resolved;
    let workspace = workspace.canonicalize().map_err(|e| {
        error(
            "error.subtitle_config",
            "workspace_subtitles",
            Some(workspace),
            e,
        )
    })?;
    let input = workspace.join("input.wav");
    let json = workspace.join("transcription.json");
    let staged = workspace.join("subtitle.srt");
    for path in [&input, &json, &staged] {
        ensure_fresh(path)?;
    }
    phase(emit, "subtitle.preparing");
    let mut prepare = crate::command(ffmpeg);
    // Keep FFmpeg's normal demux start-time subtraction. Anchor samples to that
    // playback origin, filling a delayed audio start/gaps with silence and
    // trimming negative-PTS encoder preroll instead of discarding timestamps.
    prepare
        .args(["-hide_banner", "-nostdin", "-nostats", "-n", "-i"])
        .arg(source)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-af",
            "aresample=16000:async=1:first_pts=0",
            "-t",
            "7201",
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&input);
    run_process(
        prepare,
        PREPARATION_TIMEOUT_SECS,
        "error.subtitle_input",
        "prepare_subtitle_audio",
        source,
        |_| {},
    )?;
    let seconds = crate::wav_duration(&input).map_err(|e| {
        error(
            "error.subtitle_input",
            "read_subtitle_audio",
            Some(source),
            e,
        )
    })?;
    let duration_ms = audio_duration_ms(seconds, source)?;
    alias_model(model, &workspace.join("model.bin"), MAX_MODEL_BYTES)?;
    snapshot.verify()?;
    phase(emit, "subtitle.transcribing");
    let mut transcribe = transcription_command(transcriber, &workspace, &request.language);
    if let Some(preset) = alignment_preset(model) {
        // DTW requires ordinary attention. The pinned CLI otherwise silently
        // disables alignment when flash attention is enabled.
        transcribe.args(["-ojf", "-dtw", preset, "-nfa"]);
    }
    let mut progress = TranscriptionProgress::default();
    let log = run_process(
        transcribe,
        TRANSCRIPTION_TIMEOUT_SECS,
        "error.subtitle_backend",
        "transcribe_subtitles",
        transcriber,
        |line| {
            if let Some(event) = progress.line(line) {
                emit(event);
            }
        },
    )?;
    let bytes = read_json(&json).map_err(|mut failure| {
        failure.technical = crate::engine::bounded(&log, 8192).into();
        failure
    })?;
    let mut track = parse_track(&bytes, duration_ms).map_err(|mut failure| {
        failure.technical = crate::engine::bounded(&log, 8192).into();
        failure
    })?;
    audio::trim_silence(&input, &mut track).map_err(|e| {
        error(
            "error.subtitle_result",
            "align_subtitle_audio",
            Some(&input),
            e,
        )
    })?;
    snapshot.verify()?;
    phase(emit, "subtitle.writing");
    let srt = track.to_srt().map_err(track_error)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|e| error("error.subtitle_result", "stage_subtitles", Some(&staged), e))?;
    output
        .write_all(srt.as_bytes())
        .and_then(|_| output.flush())
        .map_err(|e| error("error.subtitle_result", "stage_subtitles", Some(&staged), e))?;
    snapshot.verify()?;
    Ok(SubtitleResult {
        output: request.output.clone(),
        track,
    })
}

fn transcription_command(
    transcriber: &Path,
    workspace: &Path,
    language: &str,
) -> std::process::Command {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    let mut transcribe = crate::command(transcriber);
    // Keep the snapshotted runtime and its automatic device selection authoritative.
    // These upstream overrides can load out-of-tree code or select a device.
    transcribe
        .env_remove("GGML_BACKEND_PATH")
        .env_remove("WHISPER_ARG_DEVICE");
    transcribe.current_dir(workspace).args([
        "-m",
        "model.bin",
        "-f",
        "input.wav",
        "-l",
        language,
        "-oj",
        "-of",
        "transcription",
        "-pp",
        "-sns",
        "-nf",
        // Trust Whisper's acoustic no-speech decision even when its decoder
        // confidently invents text for silence/noise. Keep deterministic beam
        // decoding: temperature retries cannot satisfy a zero logprob threshold.
        "-lpt",
        "0",
        "-ml",
        "84",
        "-sow",
        // Carrying the preceding window's text can replace repeated speech
        // after a long instrumental gap with a misplaced earlier phrase.
        "-mc",
        "0",
        // Do not cut audio with a speech-only VAD: sung/whispered passages can
        // be discarded wholesale. Decode on the original, complete audio clock.
        "-t",
        &threads.to_string(),
    ]);
    transcribe
}

/// Read only the GGML model header, never infer architecture from a user filename.
/// Unknown/custom architectures retain the backend's own segment timestamps.
fn alignment_preset(model: &Path) -> Option<&'static str> {
    let mut header = [0u8; 48];
    File::open(model).ok()?.read_exact(&mut header).ok()?;
    alignment_header(&header)
}

fn alignment_header(header: &[u8; 48]) -> Option<&'static str> {
    let n = |index: usize| u32::from_le_bytes(header[index * 4..index * 4 + 4].try_into().unwrap());
    if n(0) != 0x67676d6c || n(2) != 1500 || n(6) != 448 || n(3) != n(7) || n(4) != n(8) {
        return None;
    }
    match (n(1), n(3), n(4), n(5), n(9), n(10)) {
        (51864, 384, 6, 4, 4, 80) => Some("tiny.en"),
        (51865, 384, 6, 4, 4, 80) => Some("tiny"),
        (51864, 512, 8, 6, 6, 80) => Some("base.en"),
        (51865, 512, 8, 6, 6, 80) => Some("base"),
        (51864, 768, 12, 12, 12, 80) => Some("small.en"),
        (51865, 768, 12, 12, 12, 80) => Some("small"),
        (51864, 1024, 16, 24, 24, 80) => Some("medium.en"),
        (51865, 1024, 16, 24, 24, 80) => Some("medium"),
        (51866, 1280, 20, 32, 32, 128) => Some("large.v3"),
        (51866, 1280, 20, 32, 4, 128) => Some("large.v3.turbo"),
        // Large v1/v2 share a header but have different alignment heads.
        _ => None,
    }
}

fn ensure_fresh(path: &Path) -> Result<(), EngineError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error(
            "error.subtitle_result",
            "stage_subtitles",
            Some(path),
            e,
        )),
        Ok(_) => Err(error(
            "error.subtitle_result",
            "stage_subtitles",
            Some(path),
            "Subtitle workspace artifact already exists.",
        )),
    }
}
fn phase(emit: &mut dyn FnMut(Event), code: &str) {
    emit(Event::Progress {
        percent: 0,
        phase: Message {
            code: code.into(),
            args: Vec::new(),
        },
    });
}

#[derive(Default)]
struct TranscriptionProgress {
    percent: u32,
    gpu: Option<bool>,
}
impl TranscriptionProgress {
    fn line(&mut self, line: &str) -> Option<Event> {
        let device = if line == "whisper_backend_init_gpu: no GPU found"
            || line.starts_with("whisper_backend_init_gpu: failed to initialize ")
        {
            Some(false)
        } else if line.starts_with("whisper_backend_init_gpu: using ") && line.ends_with(" backend")
        {
            Some(true)
        } else {
            None
        };
        if let Some(device) = device {
            if self.gpu == Some(device) {
                return None;
            }
            self.gpu = Some(device);
        } else {
            let percent: u32 = line
                .strip_prefix("whisper_print_progress_callback: progress =")?
                .trim()
                .strip_suffix('%')?
                .parse()
                .ok()?;
            if percent > 100 || percent.min(99) <= self.percent {
                return None;
            }
            // Completion belongs to the supervisor after validated SRT publication.
            self.percent = percent.min(99);
        }
        Some(Event::Progress {
            percent: self.percent,
            phase: Message {
                code: match self.gpu {
                    Some(true) => "subtitle.transcribing_gpu",
                    Some(false) => "subtitle.transcribing_cpu",
                    None => "subtitle.transcribing",
                }
                .into(),
                args: Vec::new(),
            },
        })
    }
}
fn run_process(
    command: std::process::Command,
    timeout: u64,
    code: &str,
    operation: &str,
    path: &Path,
    mut on_line: impl FnMut(&str),
) -> Result<String, EngineError> {
    let (status, log) =
        crate::process::stream_stderr(command, Duration::from_secs(timeout), |line| {
            on_line(line);
            Ok(())
        })
        .map_err(|e| {
            let timed_out = e
                .downcast_ref::<EngineError>()
                .is_some_and(|e| e.code == "error.timeout");
            let mut failure = error(
                if timed_out {
                    "error.subtitle_limit"
                } else {
                    code
                },
                operation,
                Some(path),
                if timed_out {
                    "Subtitle process deadline exceeded."
                } else {
                    "Subtitle process could not complete within its execution limits."
                },
            );
            failure.technical = crate::engine::bounded(&e.to_string(), 8192).into();
            failure
        })?;
    if !status.success() {
        let dependencies = crate::resources::dependency_startup_failure(status.code());
        let mut failure = error(
            if dependencies {
                "error.dependencies"
            } else {
                code
            },
            operation,
            Some(path),
            if dependencies {
                format!(
                    "Transcription runtime could not load its dependencies ({status}). Keep the complete application folder together. On Windows, install or repair the Microsoft Visual C++ v14 Redistributable (x64) from https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist before retrying. No final subtitle was published."
                )
            } else {
                format!("Subtitle process failed ({status}). No final subtitle was published.")
            },
        );
        failure.technical = crate::engine::bounded(&log, 8192).into();
        return Err(failure);
    }
    Ok(log)
}
fn audio_duration_ms(seconds: f64, source: &Path) -> Result<u64, EngineError> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(error(
            "error.subtitle_input",
            "subtitle_duration",
            Some(source),
            "Audio duration is unavailable or empty.",
        ));
    }
    if seconds > MAX_AUDIO_MS as f64 / 1000.0 {
        return Err(error(
            "error.subtitle_limit",
            "subtitle_duration",
            Some(source),
            "Transcription supports audio up to two hours.",
        ));
    }
    let milliseconds = (seconds * 1000.0).round() as u64;
    if milliseconds == 0 {
        return Err(error(
            "error.subtitle_input",
            "subtitle_duration",
            Some(source),
            "Audio is too short to represent in integer milliseconds.",
        ));
    }
    Ok(milliseconds)
}

fn alias_model(source: &Path, alias: &Path, limit: u64) -> Result<(), EngineError> {
    alias_model_with(source, alias, limit, |source, alias| {
        fs::hard_link(source, alias)
    })
}
fn alias_model_with(
    source: &Path,
    alias: &Path,
    limit: u64,
    link: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<(), EngineError> {
    let bytes = fs::metadata(source)
        .map_err(|e| error("error.subtitle_config", "alias_model", Some(source), e))?
        .len();
    check_model_size(bytes, limit, source)?;
    ensure_fresh(alias)?;
    if link(source, alias).is_ok() {
        return Ok(());
    }
    // The private worker can be terminated throughout a cross-volume copy.
    // create_new never replaces even an unexpected workspace alias; take bounds
    // a concurrently growing source independently of its earlier metadata.
    let input = File::open(source)
        .map_err(|e| error("error.subtitle_config", "alias_model", Some(source), e))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(alias)
        .map_err(|e| error("error.subtitle_config", "alias_model", Some(alias), e))?;
    let count = io::copy(&mut input.take(limit + 1), &mut output)
        .map_err(|e| error("error.subtitle_config", "alias_model", Some(alias), e))?;
    check_model_size(count, limit, source)?;
    output
        .flush()
        .map_err(|e| error("error.subtitle_config", "alias_model", Some(alias), e))?;
    Ok(())
}
fn read_json(path: &Path) -> Result<Vec<u8>, EngineError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| {
        error(
            "error.subtitle_backend",
            "read_transcription",
            Some(path),
            format!("Transcriber did not produce fresh JSON: {e}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(error(
            "error.subtitle_result",
            "read_transcription",
            Some(path),
            "Transcription JSON is not a regular file.",
        ));
    }
    if metadata.len() > MAX_JSON_BYTES {
        return Err(error(
            "error.subtitle_limit",
            "read_transcription",
            Some(path),
            "Transcription JSON exceeds 32 MiB.",
        ));
    }
    let input = File::open(path)
        .map_err(|e| error("error.subtitle_result", "read_transcription", Some(path), e))?;
    let mut bytes = Vec::new();
    input
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| error("error.subtitle_result", "read_transcription", Some(path), e))?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(error(
            "error.subtitle_limit",
            "read_transcription",
            Some(path),
            "Transcription JSON exceeds 32 MiB.",
        ));
    }
    Ok(bytes)
}

#[derive(Deserialize)]
struct BackendJson {
    result: BackendLanguage,
    transcription: Vec<BackendCue>,
}
#[derive(Deserialize)]
struct BackendLanguage {
    language: String,
}
#[derive(Deserialize)]
struct BackendCue {
    offsets: BackendOffsets,
    text: String,
    #[serde(default)]
    tokens: Vec<BackendToken>,
}
#[derive(Deserialize)]
struct BackendToken {
    id: u32,
    t_dtw: f64,
}
#[derive(Deserialize)]
struct BackendOffsets {
    from: u64,
    to: u64,
}

fn track_error(failure: ValidationError) -> EngineError {
    let code = if matches!(
        failure,
        ValidationError::CueCount
            | ValidationError::CueTextLimit { .. }
            | ValidationError::TrackTextLimit
    ) {
        "error.subtitle_limit"
    } else {
        "error.subtitle_result"
    };
    error(code, "validate_transcription", None, failure)
}
fn parse_track(bytes: &[u8], duration_ms: u64) -> Result<SubtitleTrack, EngineError> {
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(error(
            "error.subtitle_limit",
            "parse_transcription",
            None,
            "Transcription JSON exceeds 32 MiB.",
        ));
    }
    let document: BackendJson = serde_json::from_slice(bytes)
        .map_err(|e| error("error.subtitle_result", "parse_transcription", None, e))?;
    if document.transcription.len() > crate::subtitle_track::MAX_CUES {
        return Err(track_error(ValidationError::CueCount));
    }
    let aligned: Vec<_> = document
        .transcription
        .iter()
        .map(|cue| {
            let times: Vec<_> = cue
                .tokens
                .iter()
                .filter(|token| token.id < 50256 && token.t_dtw >= 0.0 && token.t_dtw.is_finite())
                .map(|token| (token.t_dtw * 10.0).round() as u64)
                .collect();
            match (times.first(), times.last()) {
                (Some(&first), Some(&last))
                    if first <= last
                        && last <= duration_ms.saturating_add(1000)
                        && times.windows(2).all(|p| p[0] <= p[1]) =>
                {
                    Some((first, last))
                }
                _ => None,
            }
        })
        .collect();
    let language = if document.transcription.is_empty() {
        None
    } else if document.result.language != "auto" && language_code(&document.result.language) {
        Some(document.result.language)
    } else {
        return Err(error(
            "error.subtitle_result",
            "parse_transcription",
            None,
            "Backend did not report a valid spoken language.",
        ));
    };
    let mut track = SubtitleTrack {
        language,
        duration_ms,
        cues: document
            .transcription
            .into_iter()
            .map(|cue| SubtitleCue {
                start_ms: cue.offsets.from,
                end_ms: cue.offsets.to,
                text: cue.text.trim().to_owned(),
            })
            .collect(),
    };
    // Whisper can round its final segment beyond the available samples (e.g.
    // 12.00 s for 11.59 s of speech). Allow only that bounded final overhang;
    // reversed, overlapping, wholly out-of-source and larger errors still fail.
    if let Some(last) = track.cues.last_mut()
        && last.start_ms < duration_ms
        && last.end_ms > duration_ms
        && last.end_ms - duration_ms <= 1000
    {
        last.end_ms = duration_ms;
    }
    track.validate().map_err(track_error)?;
    // Cross-attention anchors locate words even over instrumental audio. Keep
    // nearby native ends for held syllables; trim only implausibly long tails
    // (>3 seconds after the final token) to a short reading hold. DTW anchors
    // are coarse token positions, so allow 200 ms before the first token.
    for (cue, anchor) in track.cues.iter_mut().zip(aligned) {
        if let Some((first, last)) = anchor {
            let start = first.saturating_sub(200);
            let end = if cue.end_ms > last.saturating_add(3000) {
                last.saturating_add(500)
            } else {
                cue.end_ms.max(last.saturating_add(100))
            }
            .min(duration_ms);
            if start < end {
                cue.start_ms = start;
                cue.end_ms = end;
            }
        }
    }
    for index in 1..track.cues.len() {
        // Small token padding may touch the next phrase. Never reorder words.
        if track.cues[index - 1].end_ms > track.cues[index].start_ms {
            track.cues[index - 1].end_ms = track.cues[index].start_ms;
        }
    }
    track.validate().map_err(track_error)?;
    Ok(track)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn child_dll_loader_failure_has_actionable_shared_error() {
        let path = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let mut command = crate::command(&path);
        command.args(["/C", "exit", "/b", "-1073741515"]);
        let failure = run_process(
            command,
            5,
            "error.subtitle_backend",
            "transcribe",
            &path,
            |_| {},
        )
        .unwrap_err();
        assert_eq!(failure.code, "error.dependencies");
        assert!(
            failure
                .detail
                .contains("Keep the complete application folder together")
        );
        assert!(
            failure
                .detail
                .contains("Microsoft Visual C++ v14 Redistributable (x64)")
        );
        assert!(
            failure.detail.contains(
                "https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist"
            )
        );
        assert!(failure.detail.contains("No final subtitle was published"));
    }
    #[test]
    fn progress_uses_only_backend_reports_and_reserves_published_completion() {
        let mut progress = TranscriptionProgress::default();
        for line in [
            "progress = 10%",
            "whisper_print_progress_callback: progress = -1%",
            "whisper_print_progress_callback: progress = 101%",
            "transcribed words 50%",
        ] {
            assert!(progress.line(line).is_none());
        }
        let Some(Event::Progress { percent, phase }) =
            progress.line("whisper_backend_init_gpu: using CUDA0 backend")
        else {
            panic!("GPU detection")
        };
        assert_eq!(percent, 0);
        assert_eq!(phase.code, "subtitle.transcribing_gpu");
        for percent in [5, 46, 100] {
            let Some(Event::Progress {
                percent: actual, ..
            }) = progress.line(&format!(
                "whisper_print_progress_callback: progress = {percent:3}%"
            ))
            else {
                panic!("missing progress")
            };
            assert_eq!(actual, percent.min(99));
        }
        assert!(
            progress
                .line("whisper_print_progress_callback: progress =  46%")
                .is_none()
        );
        let Some(Event::Progress { percent, phase }) =
            progress.line("whisper_backend_init_gpu: no GPU found")
        else {
            panic!("CPU detection")
        };
        assert_eq!(percent, 99);
        assert_eq!(phase.code, "subtitle.transcribing_cpu");
        for name in [
            "ggml-cuda.dll",
            "cublas64_11.dll",
            "cublasLt64_11.dll",
            "cudart64_110.dll",
            "nvrtc64_112_0.dll",
        ] {
            assert!(shared_library_name(name));
        }
    }
    #[test]
    fn alignment_selects_known_architecture_without_trusting_filenames() {
        let fields = [
            0x67676d6cu32,
            51865,
            1500,
            768,
            12,
            12,
            448,
            768,
            12,
            12,
            80,
            1,
        ];
        let mut header = [0; 48];
        for (bytes, value) in header.chunks_exact_mut(4).zip(fields) {
            bytes.copy_from_slice(&value.to_le_bytes());
        }
        assert_eq!(alignment_header(&header), Some("small"));
        header[44..48].copy_from_slice(&1007u32.to_le_bytes()); // Quantized weights, same heads.
        assert_eq!(alignment_header(&header), Some("small"));
        header[4..8].copy_from_slice(&51864u32.to_le_bytes());
        assert_eq!(alignment_header(&header), Some("small.en"));
        header[36..40].copy_from_slice(&4u32.to_le_bytes()); // Custom distilled architecture.
        assert_eq!(alignment_header(&header), None);
        assert_eq!(alignment_header(&[0; 48]), None);
    }

    #[test]
    fn token_alignment_restores_late_onsets_and_removes_instrumental_caption_tails() {
        let bytes = json(serde_json::json!([
            {"offsets":{"from":0,"to":39740},"text":"first phrase",
             "tokens":[{"id":50364,"t_dtw":-1},{"id":123,"t_dtw":354},
                       {"id":456,"t_dtw":1468}]},
            {"offsets":{"from":39940,"to":55660},"text":"late phrase",
             "tokens":[{"id":123,"t_dtw":4552},{"id":456,"t_dtw":5582}]}
        ]));
        let track = parse_track(&bytes, 58000).unwrap();
        assert_eq!(
            (track.cues[0].start_ms, track.cues[0].end_ms),
            (3340, 15180)
        );
        assert_eq!(
            (track.cues[1].start_ms, track.cues[1].end_ms),
            (45320, 55920)
        );
        assert_eq!(track.duration_ms, 58000);
    }

    #[test]
    fn backend_command_removes_overrides_and_uses_relative_aliases_without_translation() {
        let runtime = Path::new("runtime/whisper-cli");
        let workspace = Path::new("workspace-字幕");
        let command = transcription_command(runtime, workspace, "fr");
        for key in ["GGML_BACKEND_PATH", "WHISPER_ARG_DEVICE"] {
            assert!(
                command
                    .get_envs()
                    .any(|(name, value)| name == key && value.is_none()),
                "{key} must be removed from child environment"
            );
        }
        assert_eq!(command.get_program(), runtime.as_os_str());
        assert_eq!(command.get_current_dir(), Some(workspace));
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        for pair in [
            ["-m", "model.bin"],
            ["-f", "input.wav"],
            ["-of", "transcription"],
            ["-l", "fr"],
        ] {
            assert!(args.windows(2).any(|values| values == pair));
        }
        assert!(!args.contains(&"-ng"));
        assert!(args.contains(&"-pp"));
        assert!(args.windows(2).any(|values| values == ["-lpt", "0"]));
        assert!(args.contains(&"-nf"));
        assert!(!args.iter().any(|&arg| matches!(arg, "--vad" | "-vm")));
        assert!(
            !args
                .iter()
                .any(|&arg| matches!(arg, "-tr" | "--translate" | "-dev" | "--device"))
        );
        let threads = args.windows(2).find(|values| values[0] == "-t").unwrap()[1]
            .parse::<usize>()
            .unwrap();
        assert!((1..=4).contains(&threads));
    }
    fn request(folder: &Path) -> SubtitleRequest {
        SubtitleRequest {
            source: folder.join("source.wav"),
            output: folder.join("output.srt"),
            ffmpeg: folder.join("ffmpeg"),
            transcriber: folder.join("whisper-cli"),
            model: folder.join("model"),
            vad_model: folder.join("vad"),
            language: "auto".into(),
        }
    }
    fn fixture() -> (tempfile::TempDir, SubtitleRequest) {
        let folder = tempfile::tempdir().unwrap();
        let request = request(folder.path());
        for path in [
            &request.source,
            &request.ffmpeg,
            &request.transcriber,
            &request.model,
            &request.vad_model,
        ] {
            fs::write(path, b"fixture").unwrap();
        }
        (folder, request)
    }
    fn json(cues: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"result":{"language":"fr"},"transcription":cues,"model":{"ignored":true}})).unwrap()
    }
    #[test]
    fn parser_uses_integer_offsets_and_preserves_unicode_inside_generated_text() {
        let bytes = json(
            serde_json::json!([{"offsets":{"from":123,"to":999},"timestamps":{"from":"ignored"},"text":"  Café 👋\r\n字幕  "},{"offsets":{"from":999,"to":1000},"text":" fin "}]),
        );
        let track = parse_track(&bytes, 1000).unwrap();
        assert_eq!(track.language.as_deref(), Some("fr"));
        assert_eq!((track.cues[0].start_ms, track.cues[0].end_ms), (123, 999));
        assert_eq!(track.cues[0].text, "Café 👋\r\n字幕");
        assert!(
            track
                .to_srt()
                .unwrap()
                .contains("00:00:00,123 --> 00:00:00,999\nCafé 👋\n字幕")
        );
        let silent = parse_track(&json(serde_json::json!([])), 1000).unwrap();
        assert_eq!(silent.language, None);
        assert_eq!(silent.to_srt().unwrap(), "");
    }
    #[test]
    fn parser_rejects_bad_boundaries_controls_and_missing_machine_results() {
        for cues in [
            serde_json::json!([{"offsets":{"from":-1,"to":1},"text":"x"}]),
            serde_json::json!([{"offsets":{"from":0,"to":2001},"text":"x"}]),
            serde_json::json!([{"offsets":{"from":1,"to":1},"text":"x"}]),
            serde_json::json!([{"offsets":{"from":2,"to":3},"text":"x"},{"offsets":{"from":0,"to":1},"text":"x"}]),
            serde_json::json!([{"offsets":{"from":0,"to":2},"text":"x"},{"offsets":{"from":1,"to":3},"text":"x"}]),
            serde_json::json!([{"offsets":{"from":0,"to":1},"text":"x\n\nx"}]),
            serde_json::json!([{"offsets":{"from":0,"to":1},"text":"x\u{0}"}]),
            serde_json::json!([{"offsets":{"from":0,"to":1},"text":"  "}]),
            serde_json::json!([{"text":"x","timestamps":{"from":"00:00:00,000","to":"00:00:00,001"}}]),
        ] {
            assert_eq!(
                parse_track(&json(cues), 1000).unwrap_err().code,
                "error.subtitle_result"
            );
        }
        for bytes in [
            b"{}".as_slice(),
            b"garbage",
            br#"{"result":{"language":"en"}}"#,
        ] {
            assert!(parse_track(bytes, 1000).is_err());
        }
    }
    #[test]
    fn parser_and_audio_limits_are_checked_without_clamping() {
        let bytes =
            json(serde_json::json!([{"offsets":{"from":0,"to":1},"text":"x".repeat(4097)}]));
        assert_eq!(
            parse_track(&bytes, 1000).unwrap_err().code,
            "error.subtitle_limit"
        );
        assert_eq!(
            parse_track(&vec![b' '; MAX_JSON_BYTES as usize + 1], 1000)
                .unwrap_err()
                .code,
            "error.subtitle_limit"
        );
        let path = Path::new("source.wav");
        assert_eq!(audio_duration_ms(7200.0, path).unwrap(), MAX_AUDIO_MS);
        assert_eq!(audio_duration_ms(1.23456, path).unwrap(), 1235);
        assert_eq!(
            audio_duration_ms(7200.0000625, path).unwrap_err().code,
            "error.subtitle_limit"
        );
        for seconds in [0.0, 0.0000625, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                audio_duration_ms(seconds, path).unwrap_err().code,
                "error.subtitle_input"
            );
        }
    }
    #[test]
    fn backend_final_rounding_is_clipped_without_stretching_or_hiding_invalid_cues() {
        let bytes = json(serde_json::json!([
            {"offsets":{"from":3000,"to":12000},"text":"late speech"}
        ]));
        let track = parse_track(&bytes, 11598).unwrap();
        assert_eq!(track.cues[0].start_ms, 3000);
        assert_eq!(track.cues[0].end_ms, 11598);
        assert_eq!(parse_track(&bytes, 15000).unwrap().cues[0].end_ms, 12000);
        for cues in [
            serde_json::json!([{"offsets":{"from":11598,"to":12000},"text":"outside"}]),
            serde_json::json!([{"offsets":{"from":0,"to":13000},"text":"too far"}]),
            serde_json::json!([{"offsets":{"from":0,"to":12000},"text":"overlap"},
                              {"offsets":{"from":11000,"to":12000},"text":"last"}]),
        ] {
            assert!(parse_track(&json(cues), 11598).is_err());
        }
    }
    #[test]
    fn syntax_is_pure_and_models_are_size_bounded() {
        let mut request = request(Path::new("does-not-exist"));
        request.validate().unwrap();
        request.ffmpeg = PathBuf::new();
        request.output = "out.SRT".into();
        request.language = "zz".into();
        request.validate().unwrap();
        for language in ["", "EN", "en-US", "fr\0", "english"] {
            request.language = language.into();
            assert_eq!(
                request.validate().unwrap_err().code,
                "error.subtitle_config"
            );
        }
        for limit in [MAX_MODEL_BYTES, MAX_VAD_BYTES] {
            let path = Path::new("model");
            check_model_size(limit, limit, path).unwrap();
            assert_eq!(
                check_model_size(limit + 1, limit, path).unwrap_err().code,
                "error.subtitle_limit"
            );
            assert_eq!(
                check_model_size(0, limit, path).unwrap_err().code,
                "error.subtitle_config"
            );
        }
    }
    #[test]
    fn metadata_snapshots_detect_source_models_runtime_and_library_set_changes() {
        for changed in 0..8 {
            let (folder, request) = fixture();
            let library = folder.path().join("ggml-base.dll");
            fs::write(&library, b"library").unwrap();
            let snapshot = request.snapshot().unwrap();
            snapshot.verify().unwrap();
            match changed {
                0 => fs::write(&request.source, b"changed source").unwrap(),
                1 => fs::write(&request.ffmpeg, b"changed ffmpeg").unwrap(),
                2 => fs::write(&request.transcriber, b"changed transcriber").unwrap(),
                3 => fs::write(&request.model, b"changed model").unwrap(),
                4 => fs::write(&request.vad_model, b"changed VAD").unwrap(),
                5 => fs::write(&library, b"changed library").unwrap(),
                6 => fs::remove_file(&library).unwrap(),
                _ => fs::write(folder.path().join("whisper.dll"), b"added library").unwrap(),
            }
            assert!(snapshot.verify().is_err(), "change {changed}");
        }
        let (folder, request) = fixture();
        let snapshot = request.snapshot().unwrap();
        fs::write(folder.path().join("unrelated.txt"), b"irrelevant").unwrap();
        snapshot.verify().unwrap();
        for name in [
            "whisper.dll",
            "GGML-CPU.DLL",
            "libwhisper.so.1",
            "libggml.dylib",
        ] {
            assert!(shared_library_name(name));
        }
        for name in ["model.bin", "whisper-cli.exe", "unrelated.dll"] {
            assert!(!shared_library_name(name));
        }
    }
    #[test]
    fn ascii_model_aliases_support_links_copy_fallback_and_refuse_replacement() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("模型-é.bin");
        fs::write(&source, b"model contents").unwrap();
        let linked = folder.path().join("model.bin");
        alias_model(&source, &linked, MAX_MODEL_BYTES).unwrap();
        assert_eq!(fs::read(&linked).unwrap(), b"model contents");
        let copied = folder.path().join("vad.bin");
        alias_model_with(&source, &copied, MAX_VAD_BYTES, |_, _| {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "forced cross-volume fallback",
            ))
        })
        .unwrap();
        assert_eq!(fs::read(&copied).unwrap(), b"model contents");
        assert!(alias_model(&source, &copied, MAX_VAD_BYTES).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"model contents");
        assert_eq!(fs::read(&copied).unwrap(), b"model contents");
    }
    #[test]
    fn fallback_copy_bounds_a_model_that_grows_after_its_size_check() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("source.bin");
        let alias = folder.path().join("model.bin");
        fs::write(&source, b"small").unwrap();
        let failure = alias_model_with(&source, &alias, 16, |source, _| {
            fs::write(source, [b'x'; 64])?;
            Err(io::Error::new(io::ErrorKind::Unsupported, "forced copy"))
        })
        .unwrap_err();
        assert_eq!(failure.code, "error.subtitle_limit");
        assert_eq!(fs::metadata(alias).unwrap().len(), 17);
    }
    #[test]
    fn json_must_be_fresh_regular_and_bounded() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("transcription.json");
        ensure_fresh(&path).unwrap();
        assert_eq!(read_json(&path).unwrap_err().code, "error.subtitle_backend");
        fs::write(&path, json(serde_json::json!([]))).unwrap();
        assert!(ensure_fresh(&path).is_err());
        assert!(
            parse_track(&read_json(&path).unwrap(), 1000)
                .unwrap()
                .cues
                .is_empty()
        );
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_JSON_BYTES + 1)
            .unwrap();
        assert_eq!(read_json(&path).unwrap_err().code, "error.subtitle_limit");
    }
}
