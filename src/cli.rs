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
    after_help = "Positional form: noh VIDEO WAV [--video VIDEO ...]. The positional video comes first.\nFlagged form: noh --soundtrack WAV --video VIDEO --image IMAGE SECONDS [...]. Named items follow their option order.\nWithout fades: copy compatible video without re-encoding. Images are encoded.\nWith fades: high-quality H.264 (CRF 16). --partial-fades copies compatible central loops.\nMP4: AAC 320 kbit/s. MKV: lossless audio copy. Existing files are never overwritten.\nSubtitles: noh --transcribe SOURCE --output captions.srt --transcriber WHISPER --model MODEL --vad-model VAD [--language fr].\nTranscription uses a local GPU when available, otherwise CPU. Review the generated text and timing before use.\nCaption burn: noh --burn-subtitles SOURCE --srt captions.srt --output captioned.mp4 [--caption-size medium] [--caption-placement bottom] [--preview] [--json].\nBurning captions re-encodes video and never overwrites output. Captions (burn, short, project) use the Classique style by default, drawn by the OpenGL engine as in the app; without OpenGL, libass draws them, with the same line breaks.
Effects: noh --list-effects shows the caption effects; --effect ID [--effect-preset NAME] [--effect-value KEY=VALUE ...] draws one instead of Classique, with OpenGL only (no libass fallback). An installed font missing on this computer is drawn in the effect's own font, with a warning. A project --preview cannot draw an effect.\nShort: noh --short SOURCE --start-ms 135 --end-ms 1775 --output short.mp4 [--framing pad|crop] [--srt reviewed.srt] [--preview] [--json].\nShort intervals use source playback milliseconds; optional SRT times refer to the original source.\nProject: noh --project --soundtrack voice.wav --video A.mp4 --video B.mp4 --output selected.mp4 --start-ms 13000 --end-ms 18000 [--restart-loops] [--srt reviewed.srt].\nProject start/end and SRT use the full WAV clock. Restart changes visuals and original-clip audio only. Omit start/end for a full montage; captions remain optional.
A project --preview draws its captions with libass."
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
    /// Reviewed plain UTF-8 SRT, or its NOH lyrics file (.noh-lyrics.json), timed
    /// from source zero (full WAV zero with --project). The lyrics file is created
    /// beside an .srt given alone; the two are kept identical (see --truth).
    #[arg(long, value_name = "FILE", requires = "caption_operation")]
    srt: Option<PathBuf>,
    /// When the .srt and its lyrics file differ: which one is the source of truth.
    /// lyrics rewrites the .srt, srt replaces the lyrics; the replaced file is kept
    /// beside as <name>.replaced…. Asked in a terminal; required otherwise.
    #[arg(long, value_parser = ["lyrics", "srt"], requires = "srt")]
    truth: Option<String>,
    /// Caption size preset (default: medium); text is never silently shrunk.
    #[arg(long, value_parser = ["small", "medium", "large"], requires = "srt")]
    caption_size: Option<String>,
    /// Caption placement preset (default: bottom).
    #[arg(long, value_parser = ["bottom", "top"], requires = "srt")]
    caption_placement: Option<String>,
    /// Protected caption region; conservative platform UI guides (default: none).
    #[arg(long, value_parser = ["none", "youtube_shorts", "tiktok", "reels", "universal"], requires = "srt")]
    caption_safe_area: Option<String>,
    /// Caption effect by id instead of Classique (see --list-effects). Its own
    /// settings come with --effect-value; --caption-size and --caption-placement
    /// are Classique's and are refused with another effect.
    #[arg(long, value_name = "ID", requires = "srt")]
    effect: Option<String>,
    /// One of the effect's presets (see --list-effects).
    #[arg(long, value_name = "NAME", requires = "effect")]
    effect_preset: Option<String>,
    /// One effect setting, read by its declared type: a number (decimal comma
    /// accepted), true/false, a palette as comma-separated colors, a font as a
    /// bundled face or system:<installed family>, a landscape/portrait pair as
    /// JSON. Repeatable.
    #[arg(long = "effect-value", value_name = "KEY=VALUE", requires = "effect")]
    effect_values: Vec<String>,
    /// List the caption effects --effect can name, their presets and settings,
    /// the folders read and the folders refused (=json for JSON).
    #[arg(long, num_args = 0..=1, default_missing_value = "text", require_equals = true, value_parser = ["text", "json"], exclusive = true)]
    list_effects: Option<String>,
    /// The caption style once the effect is resolved (Classique's values folded in).
    #[arg(skip)]
    chosen_style: Option<crate::captions::CaptionStyle>,
    /// The effect plugin resolved from --effect, if not Classique.
    #[arg(skip)]
    chosen_effect: Option<crate::project::EffectChoice>,
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
        if let Some(style) = self.chosen_style {
            return style;
        }
        CaptionStyle {
            line_width: None,
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
                println!("{}", note_text(&note));
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
    if let Some(format) = args.list_effects.as_deref() {
        let listing = crate::effect_choice::list();
        if format == "json" {
            println!("{}", listing_json(&listing));
        } else {
            print!("{}", crate::effect_choice::listing_text(&listing));
        }
        return 0;
    }
    if args.transcribe.is_some() {
        return transcribe(args);
    }
    // The effect is checked first: a refused one leaves the subtitles and their
    // lyrics file untouched and asks nothing.
    let mut args = args;
    if let Err(error) = args.resolve_effect() {
        return start_error(&error, args.json);
    }
    // The subtitles given are kept in step with their lyrics file before any
    // job starts (lyrics plan 10).
    match args.resolve_subtitles() {
        Ok(srt) => args.srt = srt,
        Err(code) => return code,
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
            effect: args.chosen_effect.clone(),
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

/// `--list-effects=json`: the MCP's `noh_list_effects` shape (version, folders,
/// effects, refused), flat.
fn listing_json(listing: &crate::effect_choice::Listing) -> serde_json::Value {
    let mut json = serde_json::to_value(listing).unwrap_or_default();
    if let Some(fields) = json.as_object_mut() {
        fields.insert("version".into(), 1.into());
    }
    json
}

/// A failure before any job starts, printed as the job's own would be (JSON with
/// --json): the exit code.
fn start_error(error: &crate::engine::EngineError, json: bool) -> i32 {
    if json {
        println!(
            "{}",
            serde_json::json!({ "version": 1, "error": {
                "code": error.code,
                "path": error.path,
                "detail": error.detail.as_ref(),
                "technical": serde_json::from_str::<serde_json::Value>(&error.technical)
                    .unwrap_or_default(),
            }})
        );
    } else {
        eprintln!("Error [{}]: {}", error.code, error.detail);
    }
    1
}

impl Options {
    /// `--srt` (an .srt or a lyrics file) kept in step with its lyrics file: the
    /// .srt the job reads. A difference is answered by `--truth` or the terminal
    /// (both stdin and stderr, never with --json); without an answer nothing is
    /// written and the command stops. What was done goes to stderr.
    fn resolve_subtitles(&self) -> Result<Option<PathBuf>, i32> {
        let Some(input) = &self.srt else {
            return Ok(None);
        };
        let truth = self.truth.as_deref().and_then(|t| t.parse().ok());
        let json = self.json;
        match crate::lyrics_input::resolve(input, truth, |difference| {
            crate::lyrics_input::ask_terminal(difference, json)
        }) {
            Ok(resolved) => {
                for note in &resolved.notes {
                    eprintln!("{}", crate::lyrics_input::note_text(note));
                }
                Ok(Some(resolved.srt))
            }
            Err(error) => Err(start_error(&error, json)),
        }
    }

    /// `--effect` resolved before any job starts (effect_choice): Classique's
    /// values become the caption style, another effect is kept for the request.
    fn resolve_effect(&mut self) -> Result<(), crate::engine::EngineError> {
        use crate::effect_choice::{Chosen, Flags, Given, Wanted};
        let Some(id) = self.effect.clone() else {
            return Ok(());
        };
        let mut values = Vec::new();
        for pair in &self.effect_values {
            let Some((key, value)) = pair.split_once('=') else {
                return Err(crate::engine::EngineError::new(
                    "error.effect",
                    "choose_effect",
                    None,
                    format!("--effect-value takes KEY=VALUE: \"{pair}\" has no \"=\"."),
                ));
            };
            values.push((key.trim().to_string(), Given::Text(value.to_string())));
        }
        let wanted = Wanted {
            id,
            preset: self.effect_preset.clone(),
            values,
        };
        // Given on the command line, not defaulted.
        let flags = Flags {
            size: self.caption_size.is_some(),
            placement: self.caption_placement.is_some(),
        };
        let style = self.caption_style();
        match crate::effect_choice::resolve(&wanted, style, flags, self.project && self.preview)? {
            Chosen::Classic(style) => self.chosen_style = Some(style),
            Chosen::Plugin(choice) => self.chosen_effect = Some(choice),
        }
        Ok(())
    }

    fn short_request(&self, ffmpeg: PathBuf) -> crate::shorts::ShortRequest {
        crate::shorts::ShortRequest {
            effect: self.chosen_effect.clone(),
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
        effect: args.chosen_effect.clone(),
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

    #[test]
    fn truth_needs_srt_and_takes_lyrics_or_srt() {
        let (options, _) = parse(&[
            "noh",
            "--burn-subtitles",
            "source.mp4",
            "--srt",
            "song.noh-lyrics.json",
            "--truth",
            "srt",
            "--output",
            "out.mp4",
        ])
        .unwrap();
        assert_eq!(options.truth.as_deref(), Some("srt"));
        assert_eq!(options.srt, Some("song.noh-lyrics.json".into()));
        for bad in [
            vec![
                "noh",
                "--burn-subtitles",
                "s.mp4",
                "--srt",
                "a.srt",
                "--truth",
                "paroles",
                "--output",
                "o.mp4",
            ],
            vec![
                "noh",
                "--short",
                "s.mp4",
                "--start-ms",
                "0",
                "--end-ms",
                "10",
                "--output",
                "o.mp4",
                "--truth",
                "lyrics",
            ],
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }

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

/// A diagnosis note in English: a key, or `key|arg|...` (the colour matrix warning).
fn note_text(note: &str) -> String {
    crate::i18n::Message::from(note).render(crate::i18n::Language::En)
}

#[cfg(test)]
mod note_tests {
    #[test]
    fn notes_with_arguments_are_rendered_not_looked_up_whole() {
        assert_eq!(
            super::note_text("warning.color_matrix|BT.709|2|BT.601"),
            "The videos do not use the same color standard. The whole export uses BT.709; clips converted from BT.601: 2."
        );
        assert_eq!(
            super::note_text("diagnosis.color_limit"),
            crate::i18n::Language::En.text("diagnosis.color_limit")
        );
    }

    /// The command line says which installed family was missing, by name.
    #[test]
    fn a_missing_installed_font_is_named_in_the_terminal() {
        let text = super::note_text("warning.effect_font_missing|NOH Absent Family");
        assert!(text.contains("NOH Absent Family"), "{text}");
        assert!(!text.contains("warning.effect_font_missing"), "{text}");
        // A word wider than the text area: the terminal gives its time.
        let text = super::note_text(&crate::captions::word_too_wide_warning(42.4));
        assert!(text.contains("0:42") && text.contains("NOH"), "{text}");
        assert!(!text.contains("warning."), "{text}");
    }
}

#[cfg(test)]
mod effect_tests {
    use super::Options;
    use clap::{CommandFactory, FromArgMatches};

    fn options(arguments: &[&str]) -> Result<Options, clap::Error> {
        let matches = Options::command().try_get_matches_from(arguments)?;
        Options::from_arg_matches(&matches)
    }
    const BURN: [&str; 7] = [
        "noh",
        "--burn-subtitles",
        "in.mp4",
        "--srt",
        "x.srt",
        "--output",
        "out.mp4",
    ];

    /// --effect needs captions; its preset and values need --effect; the
    /// listing stands alone (=json for JSON).
    #[test]
    fn effect_options_need_their_parents_and_the_listing_stands_alone() {
        assert!(
            options(&[
                "noh",
                "--short",
                "in.mp4",
                "--start-ms",
                "0",
                "--end-ms",
                "9",
                "--output",
                "o.mp4",
                "--effect",
                "noh.drop"
            ])
            .is_err()
        );
        let mut alone = BURN.to_vec();
        alone.extend(["--effect-value", "font=sans"]);
        assert!(options(&alone).is_err());
        let mut preset = BURN.to_vec();
        preset.extend(["--effect-preset", "white"]);
        assert!(options(&preset).is_err());
        assert_eq!(
            options(&["noh", "--list-effects"])
                .unwrap()
                .list_effects
                .as_deref(),
            Some("text")
        );
        assert_eq!(
            options(&["noh", "--list-effects=json"])
                .unwrap()
                .list_effects
                .as_deref(),
            Some("json")
        );
        assert!(options(&["noh", "--list-effects=xml"]).is_err());
        // The JSON listing is flat, as noh_list_effects replies.
        let json = super::listing_json(&crate::effect_choice::list());
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["effects", "folders", "refused", "version"]);
        assert_eq!(json["version"], 1);
        assert!(
            json["effects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == "noh.classic")
        );
        let mut both = BURN.to_vec();
        both.push("--list-effects");
        assert!(options(&both).is_err());
    }

    /// Classique by id: its values become the style the burn draws (and libass
    /// would); an option given twice is refused before any job.
    #[test]
    fn classique_by_id_folds_into_the_style_and_twice_is_refused() {
        let mut args = BURN.to_vec();
        args.extend([
            "--effect",
            "noh.classic",
            "--effect-value",
            "size=large",
            "--effect-value",
            "width=75,5",
        ]);
        let mut o = options(&args).unwrap();
        o.resolve_effect().unwrap();
        assert!(o.chosen_effect.is_none());
        let style = o.caption_style();
        assert_eq!(style.size, crate::captions::CaptionSize::Large);
        assert_eq!(style.line_width, Some(7_550));
        args.extend(["--caption-size", "small"]);
        let mut o = options(&args).unwrap();
        let e = o.resolve_effect().unwrap_err();
        assert!(e.detail.contains("given twice"), "{}", e.detail);
    }

    /// Refused before any job: Classique's options with another effect, a
    /// project preview with one, a value without "=". Nothing is dropped.
    #[test]
    fn wrong_effect_requests_are_refused_before_any_job() {
        let mut sized = BURN.to_vec();
        sized.extend(["--effect", "noh.drop", "--caption-placement", "top"]);
        let e = options(&sized).unwrap().resolve_effect().unwrap_err();
        assert!(e.detail.contains("Classique's options"), "{}", e.detail);
        let preview = [
            "noh",
            "--project",
            "--soundtrack",
            "s.wav",
            "--video",
            "v.mp4",
            "--output",
            "o.mp4",
            "--srt",
            "x.srt",
            "--preview",
            "--effect",
            "noh.drop",
        ];
        let e = options(&preview).unwrap().resolve_effect().unwrap_err();
        assert_eq!(e.code, "error.effect_preview");
        // A project export (no preview) and a burn preview may draw an effect:
        // nothing refuses them before the folders are read.
        let mut no_equals = BURN.to_vec();
        no_equals.extend(["--effect", "noh.classic", "--effect-value", "large"]);
        let e = options(&no_equals).unwrap().resolve_effect().unwrap_err();
        assert!(e.detail.contains("KEY=VALUE"), "{}", e.detail);
    }
}
