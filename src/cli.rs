//! CLI adapter. All media work goes through the shared controller.
use crate::engine::{Event, ExportRequest, MediaItem};
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser};
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "noh",
    group = clap::ArgGroup::new("structured_operation").args(["diagnose", "transcribe", "burn_subtitles", "short", "project"]),
    group = clap::ArgGroup::new("caption_operation").args(["burn_subtitles", "short", "project"]),
    group = clap::ArgGroup::new("short_operation").args(["short", "project"]),
    version = crate::build_info::VERSION,
    about = "Loop a sequence, generate or burn captions, or extract a selected vertical short.",
    after_help = "Positional form: noh VIDEO WAV [--video VIDEO ...]. The positional video comes first.\nFlagged form: noh --soundtrack WAV --video VIDEO --image IMAGE SECONDS [...]. Named items follow their option order.\nWithout fades: copy compatible video without re-encoding. Images are encoded.\nWith fades: high-quality H.264 (CRF 16). --partial-fades copies compatible central loops.\nMP4: AAC 320 kbit/s. MKV: lossless audio copy. Existing files are never overwritten.\nSubtitles: noh --transcribe SOURCE --output captions.srt --transcriber WHISPER --model MODEL --vad-model VAD [--language fr].\nTranscription uses a local GPU when available, otherwise CPU. Review the generated text and timing before use.\nCaption burn: noh --burn-subtitles SOURCE --srt captions.srt --output captioned.mp4 [--caption-size medium] [--caption-placement bottom] [--preview] [--json].\nBurning captions re-encodes video and never overwrites output.\nShort: noh --short SOURCE --start-ms 135 --end-ms 1775 --output short.mp4 [--framing pad|crop] [--srt reviewed.srt] [--preview] [--json].\nShort intervals use source playback milliseconds; optional SRT times refer to the original source.\nProject: noh --project --soundtrack voice.wav --video A.mp4 --video B.mp4 --output selected.mp4 --start-ms 13000 --end-ms 18000 [--restart-loops] [--srt reviewed.srt].\nProject start/end and SRT use the full WAV clock. Restart changes visuals and original-clip audio only. Omit start/end for a full montage; captions remain optional."
)]
pub struct Options {
    /// Print the embedded build identity as JSON, without reading media.
    #[arg(long, exclusive = true)]
    build_info: bool,
    /// Diagnose without exporting. Quick reads headers; exact verifies copy decisions.
    #[arg(long, num_args = 0..=1, default_missing_value = "exact", require_equals = true)]
    diagnose: Option<crate::inspection::Level>,
    /// Emit one versioned diagnosis/subtitle/caption/short result or structured failure.
    #[arg(long, requires = "structured_operation")]
    json: bool,
    /// Generate local UTF-8 SRT from this file's first audio stream (maximum 2 hours).
    #[arg(long, value_name = "SOURCE", requires_all = ["output", "transcriber", "model", "vad_model"], conflicts_with_all = ["video", "wav", "soundtrack", "videos", "images", "fade_in", "fade_out", "partial_fades", "preview", "clip_audio", "force_encode", "diagnose"])]
    transcribe: Option<PathBuf>,
    /// Path to a local whisper.cpp CLI executable; no automatic downloads.
    #[arg(long, requires = "transcribe")]
    transcriber: Option<PathBuf>,
    /// Local whisper.cpp transcription model (.bin).
    #[arg(long, requires = "transcribe")]
    model: Option<PathBuf>,
    /// Local Silero VAD model (.bin), required to reduce hallucinations on silence.
    #[arg(long, requires = "transcribe")]
    vad_model: Option<PathBuf>,
    /// Spoken language code, or auto (default). Independent of the GUI language.
    #[arg(long, requires = "transcribe")]
    language: Option<String>,
    /// Burn reviewed SRT captions into this video's pixels (always re-encodes).
    #[arg(long, value_name = "SOURCE", requires_all = ["srt", "output"], conflicts_with_all = ["video", "wav", "soundtrack", "videos", "images", "fade_in", "fade_out", "partial_fades", "clip_audio", "force_encode", "diagnose", "transcribe", "transcriber", "model", "vad_model", "language"])]
    burn_subtitles: Option<PathBuf>,
    /// Extract a selected source interval onto a vertical canvas (always re-encodes).
    #[arg(long, value_name = "SOURCE", requires_all = ["start_ms", "end_ms", "output"], conflicts_with_all = ["video", "wav", "soundtrack", "videos", "images", "fade_in", "fade_out", "partial_fades", "clip_audio", "force_encode", "diagnose", "transcribe", "burn_subtitles", "transcriber", "model", "vad_model", "language"])]
    short: Option<PathBuf>,
    /// Export the current montage with optional WAV-timed captions or a selected short.
    #[arg(long, requires = "output")]
    project: bool,
    /// Restart the ordered visuals at the selected short's beginning; WAV stays fixed.
    #[arg(long, requires_all = ["project", "start_ms"])]
    restart_loops: bool,
    /// Inclusive selected start in source playback milliseconds (WAV clock with --project).
    #[arg(long, requires_all = ["short_operation", "end_ms"])]
    start_ms: Option<u64>,
    /// Exclusive selected end in source playback milliseconds (WAV clock with --project).
    #[arg(long, requires_all = ["short_operation", "start_ms"])]
    end_ms: Option<u64>,
    /// Preserve the full picture with black padding, or fill by centered crop.
    #[arg(long, value_parser = ["pad", "crop"], requires_all = ["short_operation", "start_ms"])]
    framing: Option<String>,
    /// Reviewed plain UTF-8 SRT, timed from source zero (full WAV zero with --project).
    #[arg(long, value_name = "FILE", requires = "caption_operation")]
    srt: Option<PathBuf>,
    /// Caption size preset (default: medium); text is never silently shrunk.
    #[arg(long, value_parser = ["small", "medium", "large"], requires = "srt")]
    caption_size: Option<String>,
    /// Caption placement preset (default: bottom).
    #[arg(long, value_parser = ["bottom", "top"], requires = "srt")]
    caption_placement: Option<String>,
    /// Protected caption region; conservative platform UI guides (default: none).
    #[arg(long, value_parser = ["none", "youtube_shorts", "tiktok", "reels", "universal"], requires = "srt")]
    caption_safe_area: Option<String>,
    video: Option<PathBuf>,
    wav: Option<PathBuf>,
    /// WAV soundtrack for fully flagged or image-only input.
    #[arg(long, conflicts_with = "wav")]
    soundtrack: Option<PathBuf>,
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    #[arg(long, value_parser = seconds, default_value = "0")]
    fade_in: f64,
    #[arg(long, value_parser = seconds, default_value = "0")]
    fade_out: f64,
    #[arg(long)]
    ffmpeg: Option<PathBuf>,
    #[arg(long)]
    partial_fades: bool,
    #[arg(long)]
    preview: bool,
    #[arg(long = "video")]
    videos: Vec<PathBuf>,
    /// Add a PNG/JPEG occurrence with a finite positive duration in seconds.
    #[arg(long = "image", num_args = 2, value_names = ["PATH", "SECONDS"], allow_hyphen_values = true)]
    images: Vec<OsString>,
    #[arg(long)]
    clip_audio: bool,
    #[arg(long = "reencode")]
    force_encode: bool,
    #[arg(long, hide = true)]
    worker: bool,
    #[arg(long, hide = true)]
    engine_worker: Option<PathBuf>,
    /// Write structured events for progress viewers (creates a new file).
    #[arg(long)]
    events: Option<PathBuf>,
    /// Cancel when this signal file appears; used by the progress window.
    #[arg(long)]
    cancel_file: Option<PathBuf>,
}
fn seconds(value: &str) -> Result<f64, String> {
    let value: f64 = value
        .replace(',', ".")
        .parse()
        .map_err(|_| "Invalid duration")?;
    if !value.is_finite() || value < 0.0 {
        return Err("Fade duration must be nonnegative.".into());
    }
    Ok(value)
}

fn image_seconds(value: &std::ffi::OsStr) -> Result<f64, String> {
    let parsed = value
        .to_str()
        .and_then(|value| value.replace(',', ".").parse::<f64>().ok());
    match parsed {
        Some(value) if value.is_finite() && value > 0.0 => Ok(value),
        _ => Err("Image duration must be a finite positive number of seconds.".into()),
    }
}

impl Options {
    fn caption_style(&self) -> crate::captions::CaptionStyle {
        use crate::captions::{CaptionPlacement, CaptionSize, CaptionStyle};
        CaptionStyle {
            safe_area: match self.caption_safe_area.as_deref() {
                Some("youtube_shorts") => crate::safe_area::SafeArea::YoutubeShorts,
                Some("tiktok") => crate::safe_area::SafeArea::Tiktok,
                Some("reels") => crate::safe_area::SafeArea::Reels,
                Some("universal") => crate::safe_area::SafeArea::Universal,
                _ => crate::safe_area::SafeArea::None,
            },
            size: match self.caption_size.as_deref() {
                Some("small") => CaptionSize::Small,
                None | Some("medium") => CaptionSize::Medium,
                Some("large") => CaptionSize::Large,
                _ => unreachable!("Clap validates caption size values"),
            },
            placement: match self.caption_placement.as_deref() {
                None | Some("bottom") => CaptionPlacement::Bottom,
                Some("top") => CaptionPlacement::Top,
                _ => unreachable!("Clap validates caption placement values"),
            },
        }
    }
    fn ordered_items(&self, matches: &ArgMatches) -> Result<Vec<MediaItem>, String> {
        let mut named = Vec::with_capacity(self.videos.len() + self.images.len() / 2);
        if let Some(indices) = matches.indices_of("videos") {
            named.extend(
                indices
                    .zip(&self.videos)
                    .map(|(index, path)| (index, MediaItem::from(path.clone()))),
            );
        }
        if let Some(indices) = matches.indices_of("images") {
            let indices: Vec<_> = indices.collect();
            for (values, positions) in self
                .images
                .as_chunks::<2>()
                .0
                .iter()
                .zip(indices.as_chunks::<2>().0)
            {
                named.push((
                    positions[0],
                    MediaItem::Image {
                        path: PathBuf::from(&values[0]),
                        duration: image_seconds(&values[1])?,
                    },
                ));
            }
        }
        named.sort_by_key(|(index, _)| *index);
        // Preserve the existing positional-first contract even when named flags
        // precede positionals. Fully flagged input has no positional prefix.
        Ok(self
            .video
            .iter()
            .cloned()
            .map(MediaItem::from)
            .chain(named.into_iter().map(|(_, item)| item))
            .collect())
    }
}
fn present(event: Event, machine_output: bool) -> Option<i32> {
    match event {
        Event::Progress { percent, phase } => {
            if machine_output {
                println!("NOH_PROGRESS|{percent}|{}", phase.to_wire());
            } else {
                println!(
                    "Progress: {percent} % - {}",
                    crate::i18n::Language::En.format(&phase.code, &phase.args)
                );
            }
        }
        Event::Warning(message) => {
            if machine_output {
                eprintln!("NOH_WARNING|{}", message.to_wire());
            } else {
                eprintln!(
                    "Warning: {}",
                    crate::i18n::Language::En.format(&message.code, &message.args)
                );
            }
        }
        Event::Diagnostic(text) => eprintln!("{text}"),
        Event::Plan(plan) => {
            for (i, clip) in plan.clips.iter().enumerate() {
                if !clip.reasons.is_empty() {
                    eprintln!(
                        "Plan: clip {}: {:?}; reasons: {}; target {}x{} {}/{} fps ({})",
                        i + 1,
                        clip.treatment,
                        clip.reasons.join(", "),
                        plan.target.width,
                        plan.target.height,
                        plan.target.rate_num,
                        plan.target.rate_den,
                        plan.target.codec
                    );
                }
            }
        }
        Event::Done(Ok(result)) => {
            println!("Done: {}", result.output.display());
            return Some(0);
        }
        Event::Subtitled(Ok(result)) => {
            println!(
                "Subtitles: {} ({} cues). Review the text before use.",
                result.output.display(),
                result.track.cues.len()
            );
            return Some(0);
        }
        Event::Inspected(Ok(crate::inspection::InspectionResult::Plan(result))) => {
            println!(
                "Diagnosis: {:?}; output: {} / {}; duration: {:.3} s",
                result.level, result.container, result.audio, result.duration
            );
            if let Some(plan) = result.plan {
                println!(
                    "Video: {} x {}, {}/{} fps, {}, {}",
                    plan.target.width,
                    plan.target.height,
                    plan.target.rate_num,
                    plan.target.rate_den,
                    plan.target.codec,
                    plan.target.pixel_format
                );
                for (i, clip) in plan.clips.iter().enumerate() {
                    let reasons = clip
                        .reasons
                        .iter()
                        .map(|r| crate::i18n::Language::En.text(&crate::plan::reason_key(r)))
                        .collect::<Vec<_>>()
                        .join("; ");
                    println!(
                        "Clip {}: {} — {:?}{}{}",
                        i + 1,
                        clip.path.display(),
                        clip.treatment,
                        if reasons.is_empty() { "" } else { ": " },
                        reasons
                    );
                    if clip.timestamp_normalization {
                        println!(
                            "  {}",
                            crate::i18n::Language::En.text("diagnosis.timestamps")
                        );
                    }
                }
            }
            for note in result.notes {
                println!("{}", crate::i18n::Language::En.text(&note));
            }
            return Some(0);
        }
        Event::Inspected(Ok(_)) => return Some(0),
        Event::Inspected(Err(error)) => {
            eprintln!("Error: {error}");
            return Some(1);
        }
        Event::Done(Err(error)) | Event::Subtitled(Err(error)) => {
            eprintln!("Error: {error}");
            if !error.technical.is_empty() {
                eprintln!("{}", error.technical);
            }
            if machine_output {
                eprintln!("NOH_ERROR|{}", error.code);
            }
            return Some(1);
        }
        Event::Cancelled => {
            eprintln!("Error: Operation cancelled.");
            return Some(130);
        }
    }
    None
}

/// Return an exit code; only the executable may terminate its caller.
pub fn run(arguments: impl IntoIterator<Item = OsString>) -> i32 {
    let matches = match Options::command().try_get_matches_from(arguments) {
        Ok(matches) => matches,
        Err(error) => {
            if error.kind() == clap::error::ErrorKind::UnknownArgument
                && let Some(argument) = error.get(clap::error::ContextKind::InvalidArg)
            {
                eprintln!("Error: Unknown option: {argument}");
                return 1;
            }
            let code = i32::from(error.use_stderr());
            let _ = error.print();
            return code;
        }
    };
    let args = match Options::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(error) => {
            let code = i32::from(error.use_stderr());
            let _ = error.print();
            return code;
        }
    };
    if args.build_info {
        println!("{}", crate::build_info::current().json());
        return 0;
    }
    if let Some(path) = args.engine_worker {
        return crate::jobs::worker(&path);
    }
    if args.transcribe.is_some() {
        return transcribe(args);
    }
    if args.burn_subtitles.is_some() {
        return burn_subtitles(args);
    }
    if args.short.is_some() {
        return make_short(args);
    }
    if args.video.is_none()
        && args.wav.is_none()
        && args.soundtrack.is_none()
        && args.videos.is_empty()
        && args.images.is_empty()
    {
        let _ = Options::command().print_help();
        return 0;
    }
    let items = match args.ordered_items(&matches) {
        Ok(items) if !items.is_empty() => items,
        Ok(_) => {
            eprintln!("Error: Provide at least one video or image. See --help.");
            return 1;
        }
        Err(error) => {
            eprintln!("Error: {error}");
            return 1;
        }
    };
    let project_short =
        args.start_ms
            .zip(args.end_ms)
            .map(|(start_ms, end_ms)| crate::project::ProjectShort {
                start_ms,
                end_ms,
                restart_loops: args.restart_loops,
                framing: if args.framing.as_deref() == Some("crop") {
                    crate::shorts::Framing::Crop
                } else {
                    crate::shorts::Framing::Pad
                },
            });
    let project_captions = args
        .srt
        .as_ref()
        .map(|subtitles| crate::shorts::ShortCaptions {
            subtitles: subtitles.clone(),
            style: args.caption_style(),
        });
    let Some(wav) = args.soundtrack.or(args.wav) else {
        eprintln!(
            "Error: Provide a WAV soundtrack. Use VIDEO WAV or --soundtrack WAV. See --help."
        );
        return 1;
    };
    let ffmpeg = match crate::find_ffmpeg(args.ffmpeg.map(PathBuf::into_os_string)) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };
    let output = args.output.unwrap_or_else(|| {
        let mut name = wav.file_stem().unwrap_or_default().to_os_string();
        name.push("_noh.mp4");
        wav.with_file_name(name)
    });
    let request = ExportRequest {
        items,
        wav,
        output,
        ffmpeg,
        fade_in: args.fade_in,
        fade_out: args.fade_out,
        partial_fades: args.partial_fades,
        preview: args.preview,
        clip_audio: args.clip_audio,
        force_encode: args.force_encode,
    };
    let operation = if args.project {
        CliOperation::Project(crate::project::ProjectRequest {
            montage: request,
            short: project_short,
            captions: project_captions,
        })
    } else if let Some(level) = args.diagnose {
        CliOperation::Inspect(crate::inspection::InspectionRequest::Plan { request, level })
    } else {
        CliOperation::Export(request)
    };
    run_job(
        operation,
        args.events,
        args.cancel_file,
        args.json,
        args.worker,
    )
}

enum CliOperation {
    Export(ExportRequest),
    Inspect(crate::inspection::InspectionRequest),
    Transcribe(crate::subtitles::SubtitleRequest),
    Burn(crate::captions::CaptionRequest),
    Short(crate::shorts::ShortRequest),
    Project(crate::project::ProjectRequest),
}

impl Options {
    fn short_request(&self, ffmpeg: PathBuf) -> crate::shorts::ShortRequest {
        crate::shorts::ShortRequest {
            source: self.short.clone().unwrap(),
            output: self.output.clone().unwrap(),
            ffmpeg,
            start_ms: self.start_ms.unwrap(),
            end_ms: self.end_ms.unwrap(),
            framing: match self.framing.as_deref() {
                Some("crop") => crate::shorts::Framing::Crop,
                _ => crate::shorts::Framing::Pad,
            },
            captions: self
                .srt
                .clone()
                .map(|subtitles| crate::shorts::ShortCaptions {
                    subtitles,
                    style: self.caption_style(),
                }),
            preview: self.preview,
        }
    }
}

fn make_short(args: Options) -> i32 {
    let ffmpeg = match crate::find_ffmpeg(args.ffmpeg.clone().map(PathBuf::into_os_string)) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Error: {error}");
            return 1;
        }
    };
    let request = args.short_request(ffmpeg);
    run_job(
        CliOperation::Short(request),
        args.events,
        args.cancel_file,
        args.json,
        args.worker,
    )
}

fn burn_subtitles(args: Options) -> i32 {
    let style = args.caption_style();
    let ffmpeg = match crate::find_ffmpeg(args.ffmpeg.map(PathBuf::into_os_string)) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Error: {error}");
            return 1;
        }
    };
    // Clap requires the fields; validation, rendering and publication are shared.
    let request = crate::captions::CaptionRequest {
        source: args.burn_subtitles.unwrap(),
        subtitles: args.srt.unwrap(),
        output: args.output.unwrap(),
        ffmpeg,
        style,
        preview: args.preview,
    };
    run_job(
        CliOperation::Burn(request),
        args.events,
        args.cancel_file,
        args.json,
        args.worker,
    )
}

fn transcribe(args: Options) -> i32 {
    let ffmpeg = match crate::find_ffmpeg(args.ffmpeg.map(PathBuf::into_os_string)) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Error: {error}");
            return 1;
        }
    };
    // Clap requires these fields together; the shared controller validates them
    // and owns all media work, cancellation, staging and publication.
    let request = crate::subtitles::SubtitleRequest {
        source: args.transcribe.unwrap(),
        output: args.output.unwrap(),
        ffmpeg,
        transcriber: args.transcriber.unwrap(),
        model: args.model.unwrap(),
        vad_model: args.vad_model.unwrap(),
        language: args.language.unwrap_or_else(|| "auto".into()),
    };
    run_job(
        CliOperation::Transcribe(request),
        args.events,
        args.cancel_file,
        args.json,
        args.worker,
    )
}

fn run_job(
    operation: CliOperation,
    events: Option<PathBuf>,
    cancel_file: Option<PathBuf>,
    json_output: bool,
    machine_output: bool,
) -> i32 {
    // Preserve an already-present cancellation signal before the worker can launch.
    let cancel = Arc::new(AtomicBool::new(
        cancel_file.as_ref().is_some_and(|path| path.exists()),
    ));
    let stop = cancel.clone();
    if let Err(error) = ctrlc::set_handler(move || {
        stop.store(true, std::sync::atomic::Ordering::Release);
    }) {
        eprintln!("Error: Cannot install cancellation handler: {error}");
        return 1;
    }
    let worker = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };
    let mut events_file = match events {
        Some(path) => match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("Error: Cannot create event file {}: {e}", path.display());
                return 1;
            }
        },
        None => None,
    };
    let job = match operation {
        CliOperation::Inspect(request) => {
            crate::jobs::Job::inspect_with_worker(request, worker, cancel, || {})
        }
        CliOperation::Export(request) => {
            crate::jobs::Job::start_with_cancel(request, worker, cancel, || {})
        }
        CliOperation::Transcribe(request) => {
            crate::jobs::Job::transcribe_with_cancel(request, worker, cancel, || {})
        }
        CliOperation::Burn(request) => {
            crate::jobs::Job::burn_with_cancel(request, worker, cancel, || {})
        }
        CliOperation::Short(request) => {
            crate::jobs::Job::short_with_cancel(request, worker, cancel, || {})
        }
        CliOperation::Project(request) => {
            crate::jobs::Job::project_with_cancel(request, worker, cancel, || {})
        }
    };
    loop {
        if cancel_file.as_ref().is_some_and(|path| path.exists()) {
            job.cancel();
        }
        match job.events.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                if let Some(file) = &mut events_file {
                    use std::io::Write;
                    let mut json = serde_json::to_value(&event).unwrap();
                    let display = match &event {
                        Event::Progress { phase, .. } | Event::Warning(phase) => {
                            crate::i18n::Language::En.format(&phase.code, &phase.args)
                        }
                        Event::Done(Ok(_)) => "Export complete".into(),
                        Event::Subtitled(Ok(_)) => "Subtitles complete".into(),
                        Event::Done(Err(error)) | Event::Subtitled(Err(error)) => error.to_string(),
                        Event::Cancelled => "Operation cancelled".into(),
                        _ => String::new(),
                    };
                    json.as_object_mut()
                        .unwrap()
                        .insert("display".into(), display.into());
                    if let Err(e) = serde_json::to_writer(&mut *file, &json)
                        .map_err(std::io::Error::other)
                        .and_then(|()| file.write_all(b"\n"))
                        .and_then(|()| file.flush())
                    {
                        eprintln!("Error: Cannot write events: {e}");
                        return 1;
                    }
                }
                if json_output {
                    if event.is_terminal() {
                        let code = match &event {
                            Event::Done(Ok(_))
                            | Event::Inspected(Ok(_))
                            | Event::Subtitled(Ok(_)) => 0,
                            Event::Cancelled => 130,
                            _ => 1,
                        };
                        println!(
                            "{}",
                            serde_json::to_string(
                                &serde_json::json!({ "version": 1, "result": event })
                            )
                            .unwrap()
                        );
                        return code;
                    }
                    if matches!(event, Event::Warning(_)) {
                        // Stdout remains one terminal JSON envelope. Warnings are
                        // still visible on stderr, including mandatory re-encoding.
                        present(event, machine_output);
                    }
                    continue;
                }
                if let Some(code) = present(event, machine_output) {
                    return code;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => {
                eprintln!("Error: Controller stopped without a result");
                return 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<(Options, ArgMatches), clap::Error> {
        let matches = Options::command().try_get_matches_from(arguments)?;
        let options = Options::from_arg_matches(&matches)?;
        Ok((options, matches))
    }

    #[test]
    fn short_cli_maps_intervals_framing_optional_captions_and_preview() {
        let base = [
            "noh",
            "--short",
            "source film.mp4",
            "--start-ms",
            "135",
            "--end-ms",
            "1775",
            "--output",
            "short.mp4",
            "--json",
        ];
        let (options, _) = parse(&base).unwrap();
        let request = options.short_request("ffmpeg.exe".into());
        assert_eq!(request.source, PathBuf::from("source film.mp4"));
        assert_eq!((request.start_ms, request.end_ms), (135, 1775));
        assert_eq!(request.framing, crate::shorts::Framing::Pad);
        assert!(request.captions.is_none() && !request.preview && options.json);
        let mut flags = base.to_vec();
        flags.extend([
            "--framing",
            "crop",
            "--srt",
            "reviewed.srt",
            "--caption-size",
            "small",
            "--caption-placement",
            "top",
            "--preview",
        ]);
        let (options, _) = parse(&flags).unwrap();
        let request = options.short_request("ffmpeg.exe".into());
        assert_eq!(request.framing, crate::shorts::Framing::Crop);
        assert!(request.preview);
        let captions = request.captions.unwrap();
        assert_eq!(captions.subtitles, PathBuf::from("reviewed.srt"));
        assert_eq!(captions.style.size, crate::captions::CaptionSize::Small);
        assert_eq!(
            captions.style.placement,
            crate::captions::CaptionPlacement::Top
        );
        for required in ["--start-ms", "--end-ms", "--output"] {
            let mut incomplete = base.to_vec();
            let index = incomplete.iter().position(|arg| *arg == required).unwrap();
            incomplete.drain(index..index + 2);
            assert!(parse(&incomplete).is_err(), "{required}");
        }
        for conflicting in [
            vec!["clip.mp4", "music.wav"],
            vec!["--diagnose=quick"],
            vec!["--burn-subtitles", "source.mp4"],
            vec!["--transcribe", "speech.wav"],
            vec!["--video", "other.mp4"],
            vec!["--image", "photo.png", "1"],
            vec!["--fade-in", "0"],
            vec!["--soundtrack", "music.wav"],
            vec!["--caption-size", "large"],
            vec!["--caption-placement", "top"],
            vec!["--framing", "automatic"],
            vec!["--start-ms", "-1"],
        ] {
            let mut args = base.to_vec();
            args.extend(&conflicting);
            assert!(parse(&args).is_err(), "{conflicting:?}");
        }
        for unused in [
            ["--start-ms", "0"],
            ["--end-ms", "1000"],
            ["--framing", "crop"],
        ] {
            let mut args = vec!["noh", "source.mp4", "music.wav"];
            args.extend(unused);
            assert!(parse(&args).is_err(), "{unused:?}");
        }
        let mut negative = base.to_vec();
        negative[4] = "-1";
        assert!(parse(&negative).is_err());
        let (positional, _) = parse(&["noh", "short", "music.wav"]).unwrap();
        assert_eq!(positional.video, Some("short".into()));
        assert!(positional.short.is_none());
        let (options, _) = parse(&[
            "noh",
            "--short",
            "source.mp4",
            "--start-ms",
            "1000",
            "--end-ms",
            "1000",
            "--output",
            "short.mp4",
        ])
        .unwrap();
        assert_eq!(
            options
                .short_request("ffmpeg".into())
                .validate()
                .unwrap_err()
                .code,
            "error.short_range"
        );
    }

    #[test]
    fn caption_burn_requires_srt_output_and_defaults_to_medium_bottom() {
        let base = [
            "noh",
            "--burn-subtitles",
            "source film.mp4",
            "--srt",
            "reviewed captions.srt",
            "--output",
            "captioned.mp4",
            "--json",
        ];
        let (options, _) = parse(&base).unwrap();
        assert_eq!(options.burn_subtitles, Some("source film.mp4".into()));
        assert_eq!(options.srt, Some("reviewed captions.srt".into()));
        assert_eq!(options.output, Some("captioned.mp4".into()));
        assert!(options.json && !options.preview);
        let style = options.caption_style();
        assert!(matches!(style.size, crate::captions::CaptionSize::Medium));
        assert!(matches!(
            style.placement,
            crate::captions::CaptionPlacement::Bottom
        ));
        for required in ["--srt", "--output"] {
            let mut incomplete = base.to_vec();
            let index = incomplete
                .iter()
                .position(|value| *value == required)
                .unwrap();
            incomplete.drain(index..index + 2);
            assert!(parse(&incomplete).is_err(), "{required}");
        }
    }

    #[test]
    fn caption_presets_and_preview_are_explicit_validated_options() {
        let base = [
            "noh",
            "--burn-subtitles",
            "source.mp4",
            "--srt",
            "captions.srt",
            "--output",
            "captioned.mp4",
        ];
        for area in crate::safe_area::SafeArea::ALL {
            let value = serde_json::to_value(area).unwrap();
            let mut arguments = base.to_vec();
            arguments.extend(["--caption-safe-area", value.as_str().unwrap()]);
            let (options, _) = parse(&arguments).unwrap();
            assert_eq!(options.caption_style().safe_area, area);
        }
        for size in ["small", "medium", "large"] {
            for placement in ["bottom", "top"] {
                let mut arguments = base.to_vec();
                arguments.extend([
                    "--caption-size",
                    size,
                    "--caption-placement",
                    placement,
                    "--caption-safe-area",
                    "universal",
                    "--preview",
                ]);
                let (options, _) = parse(&arguments).unwrap();
                assert!(options.preview);
                let style = options.caption_style();
                assert_eq!(style.safe_area, crate::safe_area::SafeArea::Universal);
                assert!(matches!(
                    (size, style.size),
                    ("small", crate::captions::CaptionSize::Small)
                        | ("medium", crate::captions::CaptionSize::Medium)
                        | ("large", crate::captions::CaptionSize::Large)
                ));
                assert!(matches!(
                    (placement, style.placement),
                    ("bottom", crate::captions::CaptionPlacement::Bottom)
                        | ("top", crate::captions::CaptionPlacement::Top)
                ));
            }
        }
        for invalid in [
            ["--caption-size", "automatic"],
            ["--caption-placement", "middle"],
            ["--caption-safe-area", "automatic"],
        ] {
            let mut arguments = base.to_vec();
            arguments.extend(invalid);
            assert!(parse(&arguments).is_err());
        }
    }

    #[test]
    fn caption_burn_cannot_mix_operations_or_leak_unused_caption_flags() {
        let base = [
            "noh",
            "--burn-subtitles",
            "source.mp4",
            "--srt",
            "captions.srt",
            "--output",
            "captioned.mp4",
        ];
        for conflicting in [
            vec!["clip.mp4", "music.wav"],
            vec!["--video", "other.mp4"],
            vec!["--image", "photo.png", "2"],
            vec!["--soundtrack", "music.wav"],
            vec!["--fade-in", "0"],
            vec!["--fade-out", "0"],
            vec!["--partial-fades"],
            vec!["--clip-audio"],
            vec!["--reencode"],
            vec!["--diagnose=quick"],
            vec!["--transcribe", "speech.wav"],
            vec!["--transcriber", "whisper-cli"],
            vec!["--model", "model.bin"],
            vec!["--vad-model", "vad.bin"],
            vec!["--language", "fr"],
        ] {
            let mut arguments = base.to_vec();
            arguments.extend(&conflicting);
            assert!(parse(&arguments).is_err(), "{conflicting:?}");
        }
        for unused in [
            ["--srt", "captions.srt"],
            ["--caption-size", "medium"],
            ["--caption-placement", "bottom"],
        ] {
            let mut positional = vec!["noh", "source.mp4", "music.wav"];
            positional.extend(unused);
            assert!(parse(&positional).is_err(), "{unused:?}");
        }
        let (positional, _) = parse(&["noh", "burn-subtitles", "music.wav"]).unwrap();
        assert_eq!(positional.video, Some("burn-subtitles".into()));
        assert!(positional.burn_subtitles.is_none());
    }

    #[test]
    fn transcription_requires_explicit_runtime_and_preserves_montage_parsing() {
        let base = [
            "noh",
            "--transcribe",
            "speech.wav",
            "--output",
            "captions.srt",
            "--transcriber",
            "whisper-cli",
            "--model",
            "ggml.bin",
            "--vad-model",
            "vad.bin",
        ];
        let (options, _) = parse(&base).unwrap();
        assert_eq!(options.transcribe, Some("speech.wav".into()));
        assert!(options.language.is_none());
        let mut json = base.to_vec();
        json.extend(["--json", "--language", "fr"]);
        assert!(parse(&json).unwrap().0.json);
        for option in ["--preview", "--reencode", "--partial-fades", "--diagnose"] {
            let mut mixed = base.to_vec();
            mixed.push(option);
            assert!(parse(&mixed).is_err(), "{option}");
        }
        for required in ["--output", "--transcriber", "--model", "--vad-model"] {
            let mut incomplete = base.to_vec();
            let index = incomplete.iter().position(|s| *s == required).unwrap();
            incomplete.drain(index..index + 2);
            assert!(parse(&incomplete).is_err(), "{required}");
        }
        assert!(parse(&["noh", "--model", "ggml.bin"]).is_err());
        // Transcription is a flag: a positional video called 'transcribe' remains valid.
        let (positional, _) = parse(&["noh", "transcribe", "music.wav"]).unwrap();
        assert_eq!(positional.video, Some("transcribe".into()));
        assert!(positional.transcribe.is_none());
    }

    #[test]
    fn positional_video_remains_first() {
        let (options, matches) = parse(&[
            "noh",
            "--video",
            "b.mp4",
            "a.mp4",
            "music.wav",
            "--video",
            "c.mp4",
        ])
        .unwrap();
        assert_eq!(
            options.ordered_items(&matches).unwrap(),
            vec!["a.mp4".into(), "b.mp4".into(), "c.mp4".into()]
        );
        assert_eq!(options.wav, Some("music.wav".into()));
    }

    #[test]
    fn flagged_items_preserve_occurrence_order_and_independent_durations() {
        let (options, matches) = parse(&[
            "noh",
            "--soundtrack",
            "music.wav",
            "--image",
            "same photo.png",
            "1.25",
            "--video=middle.mp4",
            "--image",
            "same photo.png",
            "2,5",
        ])
        .unwrap();
        let items = options.ordered_items(&matches).unwrap();
        assert_eq!(
            items,
            vec![
                MediaItem::Image {
                    path: "same photo.png".into(),
                    duration: 1.25
                },
                "middle.mp4".into(),
                MediaItem::Image {
                    path: "same photo.png".into(),
                    duration: 2.5
                },
            ]
        );
        assert_eq!(options.soundtrack, Some("music.wav".into()));
        assert!(options.video.is_none());
    }

    #[test]
    fn image_only_input_and_positional_mixed_input_are_supported() {
        let (options, matches) = parse(&[
            "noh",
            "--image",
            "one.jpg",
            "3",
            "--soundtrack",
            "music.wav",
        ])
        .unwrap();
        assert_eq!(
            options.ordered_items(&matches).unwrap(),
            vec![MediaItem::Image {
                path: "one.jpg".into(),
                duration: 3.0
            }]
        );
        let (options, matches) = parse(&[
            "noh",
            "first.mp4",
            "music.wav",
            "--image",
            "photo.png",
            "4",
            "--video",
            "last.mp4",
        ])
        .unwrap();
        assert_eq!(
            options.ordered_items(&matches).unwrap(),
            vec![
                "first.mp4".into(),
                MediaItem::Image {
                    path: "photo.png".into(),
                    duration: 4.0
                },
                "last.mp4".into()
            ]
        );
    }

    #[test]
    fn invalid_image_durations_and_incomplete_pairs_are_rejected() {
        for duration in ["0", "-1", "NaN", "inf", "-inf", "1e999", "abc"] {
            let (options, matches) = parse(&[
                "noh",
                "--soundtrack",
                "music.wav",
                "--image",
                "photo.png",
                duration,
            ])
            .unwrap();
            assert_eq!(
                options.ordered_items(&matches).unwrap_err(),
                "Image duration must be a finite positive number of seconds.",
                "{duration}"
            );
        }
        assert!(parse(&["noh", "--image", "photo.png"]).is_err());
    }

    #[test]
    fn soundtrack_ambiguity_and_build_identity_exclusivity_are_rejected() {
        assert!(parse(&["noh", "first.mp4", "music.wav", "--soundtrack", "other.wav"]).is_err());
        assert!(parse(&["noh", "--build-info", "--image", "photo.png", "1"]).is_err());
        let (options, _) = parse(&["noh", "--engine-worker", "request.json"]).unwrap();
        assert_eq!(options.engine_worker, Some("request.json".into()));
    }
}
