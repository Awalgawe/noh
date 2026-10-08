#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app_captions;
mod app_capture;
mod app_destination;
mod app_diagnosis;
mod app_icons;
mod app_job;
mod app_locale;
mod app_lyrics;
mod app_media;
mod app_navigation;
mod app_options;
mod app_preview;
mod app_project;
mod app_project_ops;
mod app_project_ui;
mod app_range;
mod app_resources;
mod app_sheet;
mod app_shorts;
mod app_state;
mod app_strip;
mod app_style;
mod app_subtitles;
mod app_timeline;
mod app_track;
mod app_ui;
#[cfg(feature = "updates")]
mod app_updates;
use app_job::{Event, Job, PreviewFile, Request};
use app_locale::Locale;
use app_media::{Clip, Metadata};
use eframe::egui::{self, RichText};
use noh::i18n::{Language, Message};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct NohApp {
    project: app_project::State,
    project_ui: app_project_ui::State,
    strip: app_strip::State,
    mini_preview: app_preview::State,
    subtitles: app_subtitles::Workspace,
    resources: app_resources::State,
    locale: Locale,
    clips: Vec<Clip>,
    wav: Option<PathBuf>,
    output: Option<PathBuf>,
    output_name: String,
    output_folder: PathBuf,
    /// The name is automatic until the person types one or picks a
    /// folder in the location sheet; `output_base` is the name it counts from.
    output_auto: bool,
    output_base: String,
    /// The location sheet.
    sheet: app_sheet::Sheet,
    /// The most recent export was the short.
    last_export_short: bool,
    /// Executable that runs checks and exports (this one, or the app in tests).
    worker: PathBuf,
    destination: Option<app_destination::Destination>,
    destination_result: Option<app_destination::ResultView>,
    revision: u64,
    diagnosis_revision: Option<u64>,
    operation_revision: u64,
    preview_artifact: Option<app_state::Artifact>,
    export_artifact: Option<app_state::Artifact>,
    notice: Option<Message>,
    outcome: Option<Message>,
    cancelling: bool,
    failure_revision: Option<u64>,
    failure_path: Option<PathBuf>,
    capture_state: Option<String>,
    /// Capture state applied over a real project (layout matrix) and its progress.
    capture_overlay: Option<String>,
    capture_overlay_step: u8,
    ffmpeg: Option<PathBuf>,
    in_enabled: bool,
    out_enabled: bool,
    fade_in: f64,
    fade_out: f64,
    partial: bool,
    clip_audio: bool,
    metadata: Metadata,
    preflight: Option<app_diagnosis::Preflight>,
    diagnosis: Option<Box<noh::inspection::Diagnosis>>,
    diagnosis_error: Option<Message>,
    inspecting: bool,
    analysis_phase: Option<Message>,
    analysis_fraction: f32,
    next_id: u64,
    wav_id: u64,
    wav_seconds: Option<f64>,
    wav_error: Option<Message>,
    dragging: Option<u64>,
    job: Option<Job>,
    captions: app_captions::Workspace,
    shorts: app_shorts::Workspace,
    fraction: f32,
    phase: Message,
    error: Option<Message>,
    warnings: Vec<Message>,
    previewing: bool,
    preview: Option<PreviewFile>,
    preview_ready: bool,
    completed: Option<PathBuf>,
    log: VecDeque<String>,
    started: Option<Instant>,
    close_question: bool,
    settings_open: bool,
    /// Options.
    options: app_options::State,
    #[cfg(feature = "updates")]
    updates: app_updates::Workspace,
    drop_error: Option<Message>,
    /// Lyrics just generated into this file until acted on.
    lyrics_ready: Option<PathBuf>,
    capture_requested: bool,
    capture_project_started: bool,
    // QA geometry deadline begins only after the real captured Job is reaped.
    capture_terminal_at: Option<Instant>,
    opened: Instant,
    scrub_qa: app_capture::ScrubQa,
}

impl Default for NohApp {
    fn default() -> Self {
        Self {
            project: app_project::State::default(),
            scrub_qa: app_capture::ScrubQa::default(),
            project_ui: app_project_ui::State::default(),
            strip: app_strip::State::default(),
            mini_preview: app_preview::State::default(),
            subtitles: app_subtitles::Workspace::default(),
            resources: app_resources::State::default(),
            locale: Locale::load(),
            clips: Vec::new(),
            wav: None,
            output: None,
            output_name: "montage.mp4".into(),
            output_folder: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            output_auto: true,
            output_base: "montage.mp4".into(),
            sheet: app_sheet::Sheet::default(),
            last_export_short: false,
            worker: std::env::current_exe().unwrap_or_default(),
            destination: None,
            destination_result: None,
            revision: 0,
            diagnosis_revision: None,
            operation_revision: 0,
            preview_artifact: None,
            export_artifact: None,
            notice: None,
            outcome: None,
            cancelling: false,
            failure_revision: None,
            failure_path: None,
            capture_state: None,
            capture_overlay: None,
            capture_overlay_step: 0,
            ffmpeg: None,
            in_enabled: true,
            out_enabled: true,
            fade_in: 1.0,
            fade_out: 2.0,
            partial: true,
            clip_audio: false,
            job: None,
            captions: app_captions::Workspace::default(),
            shorts: app_shorts::Workspace::default(),
            metadata: Metadata::default(),
            preflight: None,
            diagnosis: None,
            diagnosis_error: None,
            inspecting: false,
            analysis_phase: None,
            analysis_fraction: 0.0,
            next_id: 0,
            wav_id: 0,
            wav_seconds: None,
            wav_error: None,
            dragging: None,
            fraction: 0.0,
            phase: "status.ready".into(),
            error: None,
            warnings: Vec::new(),
            previewing: false,
            preview: None,
            preview_ready: false,
            completed: None,
            log: VecDeque::new(),
            started: None,
            close_question: false,
            options: app_options::State::default(),
            #[cfg(feature = "updates")]
            updates: app_updates::Workspace::default(),
            settings_open: std::env::var_os("NOH_CAPTURE_UI").is_some()
                && std::env::var_os("NOH_CAPTURE_SETTINGS").is_some(),
            drop_error: None,
            lyrics_ready: None,
            capture_requested: false,
            capture_project_started: false,
            capture_terminal_at: None,
            opened: Instant::now(),
        }
    }
}

impl NohApp {
    fn pick_wav(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.wav.is_none() && self.output_auto {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            self.output_base = format!("{stem}_noh.mp4");
            self.output_name = self.output_base.clone();
            self.output_folder = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            self.destination_edited();
        }
        self.next_id += 1;
        self.wav_id = self.next_id;
        self.wav_seconds = None;
        self.wav_error = None;
        self.metadata
            .retain(self.clips.iter().map(|c| c.request_id).chain([self.wav_id]));
        self.metadata
            .request(self.wav_id, path.clone(), true, self.ffmpeg.clone(), ctx);
        self.wav = Some(path);
        self.drop_error = None;
    }

    /// Drops, pickers and command-line files all land here: sorted by type
    ///, refused whole with a reason, or applied at once.
    fn drop_files(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let paths: Vec<Option<PathBuf>> = paths.into_iter().map(Some).collect();
        let plan = match app_media::classify(&paths, self.wav.is_some(), true) {
            Ok(plan) => plan,
            Err(error) => {
                self.drop_error = Some(error);
                return;
            }
        };
        let rendering = (self.job.is_some() && !self.subtitles.running)
            || self.cancelling
            || self.project.running.is_some()
            || self.captions.running.is_some()
            || self.shorts.running.is_some();
        // Transcription keeps the montage editable but not its song or lyrics.
        let generating = self.subtitles.running || self.project.generating;
        if rendering || (generating && (plan.song.is_some() || plan.lyrics.is_some())) {
            self.drop_error = Some("drop.busy".into());
            return;
        }
        if self.clips.len() + plan.visuals.len() > 4096 {
            self.drop_error = Some("diagnosis.clip_limit".into());
            return;
        }
        for path in plan.visuals {
            let item = app_media::item_for_path(path);
            self.next_id += 1;
            self.metadata
                .request_media(self.next_id, item.clone(), self.ffmpeg.clone(), ctx);
            self.clips.push(Clip {
                id: self.next_id,
                request_id: self.next_id,
                item,
                info: None,
                error: None,
            });
        }
        if let Some(song) = plan.song {
            self.pick_wav(song, ctx);
        }
        if let Some(lyrics) = plan.lyrics {
            self.project.attach(lyrics, self.wav.as_ref());
        }
        self.drop_error = None;
        self.invalidated();
    }

    fn start(&mut self, ctx: &egui::Context, previewing: bool) {
        if self.job.is_some()
            || self.cancelling
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some()
        {
            return;
        }
        self.error = None;
        // Setup can fail before a controller event supplies its revision.
        self.failure_revision = Some(self.revision);
        self.failure_path = None;
        let Some(wav) = &self.wav else {
            self.error = Some("error.select_files".into());
            return;
        };
        let output = if previewing {
            match PreviewFile::new() {
                Ok(preview) => {
                    let path = preview.path.clone();
                    self.preview = Some(preview);
                    self.preview_ready = false;
                    path
                }
                Err(error) => {
                    self.error = Some(Message::new("error.preview", &[error.to_string()]));
                    return;
                }
            }
        } else if let Some(output) = &self.output {
            output.clone()
        } else {
            self.error = Some("error.choose_output".into());
            return;
        };
        if output
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| !s.eq_ignore_ascii_case("mp4"))
            .unwrap_or(true)
        {
            self.error = Some("error.mp4".into());
            return;
        }
        let request = Request {
            items: self.clips.iter().map(|c| c.item.clone()).collect(),
            clip_audio: self.clip_audio,
            wav: wav.clone(),
            output,
            fade_in: if self.in_enabled { self.fade_in } else { 0.0 },
            fade_out: if self.out_enabled { self.fade_out } else { 0.0 },
            partial_fades: self.partial,
            preview: previewing,
            ffmpeg: self.ffmpeg.clone().unwrap_or_default(),
            force_encode: !self.partial,
        };
        let ctx = ctx.clone();
        if !previewing {
            let Some(diagnosis) = self.diagnosis.as_ref().filter(|d| {
                self.diagnosis_revision == Some(self.revision)
                    && d.request.same_render_settings(&request)
            }) else {
                self.error = Some("diagnosis.required".into());
                return;
            };
            self.job = Some(Job::start_checked_with_worker(
                request,
                *diagnosis.clone(),
                self.worker.clone(),
                move || ctx.request_repaint(),
            ));
        } else {
            self.job = Some(Job::start(request, move || ctx.request_repaint()));
        }
        if self.inspecting {
            if let Some(preflight) = &self.preflight {
                preflight.cancel();
            }
            self.inspecting = false;
        }
        self.previewing = previewing;
        self.operation_revision = self.revision;
        self.cancelling = false;
        self.failure_revision = None;
        self.failure_path = None;
        self.notice = None;
        self.outcome = None;
        self.started = Some(Instant::now());
        self.fraction = 0.0;
        self.phase = if previewing {
            "status.preview_start"
        } else {
            "status.export_start"
        }
        .into();
        self.log.clear();
        self.warnings.clear();
    }

    fn invalidate_clip_metadata(&mut self) {
        for clip in &mut self.clips {
            self.next_id += 1;
            clip.request_id = self.next_id;
            clip.info = None;
            clip.error = None;
        }
        self.invalidated();
    }

    fn set_ffmpeg(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.ffmpeg.as_ref() != Some(&path) {
            self.subtitles.edited();
            self.captions.edited();
            self.shorts.edited();
        }
        self.ffmpeg = Some(path);
        self.refresh_metadata(ctx);
    }

    fn refresh_metadata(&mut self, ctx: &egui::Context) {
        self.invalidate_clip_metadata();
        self.metadata.retain(
            self.clips
                .iter()
                .map(|c| c.request_id)
                .chain(self.wav.iter().map(|_| self.wav_id)),
        );
        for clip in &self.clips {
            self.metadata.request_media(
                clip.request_id,
                clip.item.clone(),
                self.ffmpeg.clone(),
                ctx,
            );
        }
        if let Some(wav) = &self.wav {
            self.next_id += 1;
            self.wav_id = self.next_id;
            self.wav_seconds = None;
            self.wav_error = None;
            self.metadata
                .request(self.wav_id, wav.clone(), true, self.ffmpeg.clone(), ctx);
        }
    }

    fn receive_analysis(&mut self, event: app_media::Analysis) {
        if event
            .result
            .as_ref()
            .is_err_and(|error| matches!(error.key, "error.ffmpeg" | "error.start"))
        {
            self.resources.invalidate();
        }
        if event.wav && event.id == self.wav_id {
            self.wav_seconds = None;
            self.wav_error = None;
            match event.result {
                Ok(info) => self.wav_seconds = Some(info.seconds),
                Err(error) => {
                    self.log.push_back(error.detail);
                    self.wav_error = Some(error.key.into());
                }
            }
        } else if !event.wav
            && let Some(clip) = self.clips.iter_mut().find(|c| c.request_id == event.id)
        {
            clip.info = None;
            clip.error = None;
            match event.result {
                Ok(info) => clip.info = Some(info),
                Err(error) => {
                    self.log.push_back(error.detail);
                    clip.error = Some(error.key.into());
                }
            }
        }
    }

    fn receive(&mut self) {
        self.subtitles.receive_picker();
        self.metadata.retain(
            self.clips
                .iter()
                .map(|c| c.request_id)
                .chain(self.wav.iter().map(|_| self.wav_id)),
        );
        while let Ok(event) = self.metadata.events.try_recv() {
            self.receive_analysis(event);
            while self.log.len() > 300 {
                self.log.pop_front();
            }
            if self.job.is_none()
                && !self.subtitles.running
                && self.captions.running.is_none()
                && self.shorts.running.is_none()
                && self.ready()
            {
                self.phase = "status.ready_export".into();
            }
        }
        let events: Vec<_> = self
            .job
            .as_ref()
            .map(|job| std::iter::from_fn(|| job.events.try_recv().ok()).collect())
            .unwrap_or_default();
        let mut done = false;
        for event in events {
            done |= self.receive_job_event(event);
        }
        if done {
            self.job.take();
            self.started = None;
            self.cancelling = false;
            self.subtitles.running = false;
            self.captions.reaped();
            self.shorts.reaped();
        }
    }

    fn receive_job_event(&mut self, event: Event) -> bool {
        if matches!(&event, Event::Done(Err(error)) | Event::Subtitled(Err(error))
            if matches!(error.code.as_str(), "error.ffmpeg" | "error.start" | "error.dependencies" | "error.subtitle_config" | "error.subtitle_backend"))
        {
            self.resources.invalidate();
        }
        if self.receive_project_event(&event) {
            return matches!(
                event,
                Event::Done(_) | Event::Cancelled | Event::Subtitled(_)
            );
        }
        let mut done = false;
        match event {
            Event::Progress { percent, phase } => {
                if !self.cancelling {
                    self.fraction = self.fraction.max(percent as f32 / 100.0);
                    self.phase = phase.to_wire().into();
                }
            }
            Event::Diagnostic(line) => {
                self.log.push_back(line);
                if self.log.len() > 300 {
                    self.log.pop_front();
                }
            }
            Event::Warning(reason) => {
                let message = Message::from(reason.to_wire());
                if self.shorts.running.is_some() {
                    if !self.shorts.warnings.contains(&message) {
                        self.shorts.warnings.push(message);
                    }
                    return done;
                }
                if self.captions.running.is_some() {
                    if !self.captions.warnings.contains(&message) {
                        self.captions.warnings.push(message);
                    }
                    return done;
                }
                if !self.warnings.contains(&message) {
                    self.warnings.push(message);
                }
            }
            Event::Done(result) => {
                done = true;
                if self.shorts.running.is_some() {
                    if let Err(error) = &result {
                        self.log.push_back(format!(
                            "{}: {}\n{}",
                            error.operation, error.detail, error.technical
                        ));
                    }
                    self.shorts.completed(result);
                    return done;
                }
                if self.captions.running.is_some() {
                    if let Err(error) = &result {
                        self.log.push_back(format!(
                            "{}: {}\n{}",
                            error.operation, error.detail, error.technical
                        ));
                    }
                    self.captions.completed(result);
                    return done;
                }
                match result {
                    Ok(result) if self.previewing => {
                        self.fraction = 1.0;
                        self.preview_ready = true;
                        self.preview_artifact = Some(app_state::Artifact {
                            path: result.output,
                            revision: self.operation_revision,
                            bytes: None,
                        });
                        self.phase = "status.preview_ready".into();
                    }
                    Ok(result) => {
                        self.fraction = 1.0;
                        self.phase = "status.done".into();
                        self.last_export_short = false;
                        self.completed = Some(result.output.clone());
                        self.export_artifact = Some(app_state::Artifact {
                            path: result.output,
                            revision: self.operation_revision,
                            bytes: None,
                        });
                        self.destination_result =
                            self.output
                                .as_ref()
                                .map(|path| app_destination::ResultView {
                                    path: path.clone(),
                                    issue: Some(app_destination::Issue::Exists),
                                    suggestion: None,
                                    bytes: None,
                                });
                        if let Some(destination) = &self.destination {
                            destination.update(self.output.clone(), true);
                        }
                    }
                    Err(error) => {
                        self.phase = if self.previewing {
                            "status.preview_failed"
                        } else {
                            "status.export_failed"
                        }
                        .into();
                        self.log.push_back(format!(
                            "{}: {}\n{}",
                            error.operation, error.detail, error.technical
                        ));
                        self.error = Some(error.message_code().into());
                        // Publication refused (a file appeared after the check): ask
                        // where to write instead.
                        if error.code == "error.output_exists" && !self.previewing {
                            self.open_sheet(false, None);
                        }
                        self.failure_revision = Some(self.operation_revision);
                        self.failure_path = error.path;
                    }
                }
            }
            Event::Plan(plan) => {
                self.log.push_back(format!(
                    "Plan: {}",
                    serde_json::to_string(&plan).unwrap_or_default()
                ));
            }
            Event::Inspected(_) => {}
            Event::Subtitled(result) => {
                done = true;
                let attached = self.project_subtitled(&result);
                match result {
                    Ok(result) => {
                        if attached {
                            self.lyrics_ready = Some(result.output.clone());
                        }
                        self.subtitles.artifact = Some(app_subtitles::Artifact {
                            path: result.output,
                            revision: self.subtitles.operation_revision,
                            cues: result.track.cues.len(),
                        });
                        self.subtitles.failure = None;
                        self.subtitles.failure_revision = None;
                        self.subtitles.outcome = None;
                        self.phase = "ui.subtitle_succeeded".into();
                    }
                    Err(error) => {
                        self.log.push_back(format!(
                            "{}: {}\n{}",
                            error.operation, error.detail, error.technical
                        ));
                        self.subtitles.failure = Some(error.message_code().into());
                        self.subtitles.failure_revision = Some(self.subtitles.operation_revision);
                    }
                }
            }
            Event::Cancelled => {
                done = true;
                if self.shorts.running.is_some() {
                    self.shorts.outcome = Some("ui.short_cancelled".into());
                    return done;
                }
                if self.captions.running.is_some() {
                    self.captions.outcome = Some("ui.caption_cancelled".into());
                    return done;
                }
                if self.subtitles.running {
                    self.subtitles.outcome = Some("ui.subtitle_cancelled".into());
                    self.phase = "ui.subtitle_cancelled".into();
                    return done;
                }
                self.outcome = Some(
                    if self.previewing {
                        "ui.preview_cancelled_confirmed"
                    } else {
                        "ui.export_cancelled_confirmed"
                    }
                    .into(),
                );
                self.phase = if self.previewing {
                    "status.preview_cancelled"
                } else {
                    "status.export_cancelled"
                }
                .into();
            }
        }
        done
    }
    fn open(&mut self, path: &Path) {
        if let Err(error) = open::that(path) {
            self.notice = Some(Message::new("error.open", &[error.to_string()]));
        } else {
            self.notice = Some("ui.opened".into());
        }
    }
}

fn main() -> eframe::Result {
    #[cfg(all(windows, feature = "updates"))]
    if let Some(code) = noh::update::lifecycle::hook_exit(std::env::args_os()) {
        std::process::exit(code);
    }
    #[cfg(all(windows, feature = "updates"))]
    let _update_lease = noh::update::windows::RuntimeLease::for_current_executable()
        .unwrap_or_else(|error| {
            eprintln!("noh-app: managed installation is unavailable: {error}");
            std::process::exit(1);
        });
    if std::env::args_os()
        .nth(1)
        .is_some_and(|s| s == "--version" || s == "-V")
    {
        println!("noh-app {}", noh::build_info::VERSION);
        return Ok(());
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|s| s == "--worker" || s == "--engine-worker" || s == "--build-info")
    {
        std::process::exit(noh::cli::run(std::env::args_os()));
    }
    let mut app = NohApp::default();
    #[cfg(feature = "updates")]
    if let Err(error) = app.load_update_capture_fixture() {
        eprintln!("noh-app: {error}");
        std::process::exit(1);
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(folder) = executable.parent()
    {
        app.subtitles.load_bundled(folder);
    }
    // Populate the actual diagnostic path for reproducible render QA only.
    if std::env::var_os("NOH_CAPTURE_UI").is_some()
        && let Some(path) = std::env::var_os("NOH_CAPTURE_PROJECT")
    {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("Capture project file"))
                .expect("Capture project JSON");
        let request: Request = if value.get("montage").is_some() {
            let project: noh::project::ProjectRequest =
                serde_json::from_value(value).expect("Capture composition request");
            if let Some(short) = project.short {
                if std::env::var("NOH_CAPTURE_SCRUB_VIEW").as_deref() == Ok("short") {
                    app.mini_preview.short = true;
                }
                app.project.range = Some(noh::timeline::Range {
                    start_ms: short.start_ms,
                    end_ms: short.end_ms,
                });
                app.project.restart_loops = short.restart_loops;
                app.project.framing = short.framing;
            }
            if let Some(captions) = project.captions {
                app.project.apply_subtitles = true;
                app.project.caption_style = captions.style;
                app.project.short_safe_area = captions.style.safe_area;
                app.project.attach(captions.subtitles, None);
            }
            project.montage
        } else {
            serde_json::from_value(value).expect("Capture project request")
        };
        app.wav = Some(request.wav);
        app.output_name = request
            .output
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // The free-name rule counts from this base, as after adding a song.
        app.output_base = app.output_name.clone();
        app.output_folder = request
            .output
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        app.output = Some(request.output);
        app.ffmpeg = (!request.ffmpeg.as_os_str().is_empty()).then_some(request.ffmpeg);
        app.fade_in = request.fade_in;
        app.fade_out = request.fade_out;
        app.in_enabled = request.fade_in > 0.0;
        app.out_enabled = request.fade_out > 0.0;
        app.partial = !request.force_encode;
        app.clip_audio = request.clip_audio;
        for item in request.items {
            app.next_id += 1;
            app.clips.push(Clip {
                id: app.next_id,
                request_id: app.next_id,
                item,
                info: None,
                error: None,
            });
        }
    }
    app.install_capture_fixture();
    if std::env::var_os("NOH_CAPTURE_UI").is_some()
        && let Some(path) = std::env::var_os("NOH_CAPTURE_SPEECH_SETUP")
    {
        let setup: noh::subtitles::SubtitleRequest =
            serde_json::from_slice(&std::fs::read(path).expect("Capture speech setup"))
                .expect("Capture speech request");
        app.subtitles.transcriber = setup.transcriber.display().to_string();
        app.subtitles.model = setup.model.display().to_string();
        app.subtitles.vad_model = setup.vad_model.display().to_string();
        app.subtitles.language = setup.language;
    }
    let capture_width = if std::env::var_os("NOH_CAPTURE_UI").is_some() {
        std::env::var("NOH_CAPTURE_WIDTH")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|w| w.is_finite())
            .map(|w| w.clamp(420.0, 1920.0))
            .unwrap_or(980.0)
    } else {
        980.0
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/noh-icon.png"))
                    .expect("The bundled NOH window icon must be valid PNG"),
            )
            .with_inner_size([
                capture_width,
                app_capture::number("NOH_CAPTURE_HEIGHT", 850.0, 540.0, 2160.0),
            ])
            .with_drag_and_drop(true)
            .with_min_inner_size([420.0, 540.0])
            .with_title(app.locale.language.text("title")),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "NOH",
        options,
        Box::new(move |cc| {
            if std::env::var_os("NOH_CAPTURE_UI").is_some()
                && std::env::var_os("NOH_CAPTURE_PROJECT").is_some()
            {
                for clip in &app.clips {
                    app.metadata.request_media(
                        clip.request_id,
                        clip.item.clone(),
                        app.ffmpeg.clone(),
                        &cc.egui_ctx,
                    );
                }
                if let Some(wav) = &app.wav {
                    app.next_id += 1;
                    app.wav_id = app.next_id;
                    app.metadata.request(
                        app.wav_id,
                        wav.clone(),
                        true,
                        app.ffmpeg.clone(),
                        &cc.egui_ctx,
                    );
                }
            }
            app_locale::install_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::ThemePreference::System);
            if std::env::var_os("NOH_CAPTURE_UI").is_some() {
                cc.egui_ctx.set_pixels_per_point(app_capture::number(
                    "NOH_CAPTURE_SCALE",
                    1.0,
                    1.0,
                    2.0,
                ));
                cc.egui_ctx.set_theme(
                    if std::env::var("NOH_CAPTURE_THEME").as_deref() == Ok("light") {
                        egui::ThemePreference::Light
                    } else {
                        egui::ThemePreference::Dark
                    },
                );
            }
            app_style::apply(&cc.egui_ctx);
            let mut app = app;
            let mut preview_issue = Some(noh::resources::Issue {
                component: noh::resources::Component::Preview,
                path: None,
                detail:
                    "libmpv was not found. Restore the complete application folder and restart NOH."
                        .into(),
            });
            for runtime in noh::scrub::runtime_candidates() {
                match noh::scrub::Scrubber::new(cc, runtime.clone()) {
                    Ok(scrubber) => {
                        app.mini_preview.scrubber = Some(scrubber);
                        preview_issue = None;
                        break;
                    }
                    Err(error) => {
                        tracing::warn!("Persistent preview unavailable: {error}");
                        preview_issue = Some(noh::resources::Issue {
                            component: noh::resources::Component::Preview,
                            path: Some(runtime),
                            detail: error.to_string(),
                        });
                    }
                }
            }
            app.resources.preview = preview_issue;
            // Arguments also allow opening files from a shortcut, sorted by type.
            let arguments: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
            #[cfg(all(windows, feature = "updates"))]
            let arguments = if arguments.len() == 2
                && matches!(
                    arguments[0].to_str(),
                    Some("--install-retained-update" | "--noh-update-ready-pipe")
                ) {
                Vec::new()
            } else {
                arguments
            };
            if !arguments.is_empty() {
                app.drop_files(arguments, &cc.egui_ctx);
            }
            if app_capture::transcribe_enabled() {
                app.start_subtitles(&cc.egui_ctx);
            }
            if let Some(preview) = app_capture::burn_enabled() {
                app.start_caption(&cc.egui_ctx, preview);
            }
            if let Some(preview) = app_capture::short_enabled() {
                app.start_short(&cc.egui_ctx, preview);
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    use app_media::{Analysis, Failure};
    use noh::media::MediaInfo;

    fn app() -> NohApp {
        let mut app = NohApp {
            next_id: 1,
            wav_seconds: Some(3.0),
            ..Default::default()
        };
        app.clips.push(Clip {
            id: 1,
            request_id: 1,
            item: "clip.mp4".into(),
            info: None,
            error: None,
        });
        app
    }

    fn success(id: u64) -> Analysis {
        Analysis {
            id,
            wav: false,
            result: Ok(MediaInfo::default()),
        }
    }

    fn failure(id: u64) -> Analysis {
        Analysis {
            id,
            wav: false,
            result: Err(Failure {
                key: "error.ffmpeg",
                detail: "engine unavailable".into(),
            }),
        }
    }

    #[test]
    fn image_duration_is_per_clip_and_invalidates_current_diagnosis() {
        let mut app = app();
        app.clips[0].item = noh::input::MediaItem::Image {
            path: "still.png".into(),
            duration: 3.0,
        };
        app.clips.push(Clip {
            id: 2,
            request_id: 2,
            item: noh::input::MediaItem::Image {
                path: "other.jpg".into(),
                duration: 7.0,
            },
            info: None,
            error: None,
        });
        app.revision = 8;
        app.diagnosis_revision = Some(8);
        assert!(app.set_image_duration(1, 4.5));
        assert_eq!(app.revision, 9);
        assert_eq!(app.diagnosis_revision, None);
        app.clips.swap(0, 1);
        let durations: Vec<f64> = app
            .clips
            .iter()
            .map(|clip| match &clip.item {
                noh::input::MediaItem::Image { duration, .. } => *duration,
                noh::input::MediaItem::Video { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(durations, [7.0, 4.5]);
    }

    #[test]
    fn old_engine_success_cannot_revive_a_clip_after_new_engine_failure() {
        let mut app = app();
        app.receive_analysis(success(1));
        assert!(app.ready());
        app.invalidate_clip_metadata();
        assert_eq!(
            app.clips[0].id, 1,
            "The visible clip identity must stay stable"
        );
        assert!(!app.ready());
        let current = app.clips[0].request_id;
        assert_ne!(current, 1);
        app.receive_analysis(success(1));
        assert!(!app.ready());
        app.receive_analysis(failure(current));
        app.receive_analysis(success(1));
        assert!(!app.ready());
        assert!(app.clips[0].info.is_none());
        assert_eq!(app.clips[0].error.as_ref().unwrap().key, "error.ffmpeg");
    }

    #[test]
    fn stale_failures_do_not_replace_current_success_and_errors_are_cleared() {
        let mut app = app();
        app.receive_analysis(failure(1));
        app.invalidate_clip_metadata();
        let current = app.clips[0].request_id;
        app.receive_analysis(success(current));
        app.receive_analysis(failure(1));
        assert!(app.ready());
        assert!(app.clips[0].error.is_none());
        app.receive_analysis(failure(current));
        assert!(!app.ready());
        assert!(app.clips[0].info.is_none());
        app.receive_analysis(success(current));
        assert!(app.clips[0].error.is_none());
        assert!(app.ready());
    }
    #[test]
    fn cancel_freezes_progress_but_publication_success_is_retained() {
        let mut app = app();
        app.output = Some("result.mp4".into());
        app.cancelling = true;
        app.fraction = 0.46;
        app.phase = "progress.assemble".into();
        assert!(!app.receive_job_event(Event::Progress {
            percent: 80,
            phase: noh::engine::Message {
                code: "progress.encode".into(),
                args: vec![]
            }
        }));
        assert_eq!(app.fraction, 0.46);
        assert_eq!(app.phase.key, "progress.assemble");
        assert!(
            app.receive_job_event(Event::Done(Ok(noh::engine::ExportResult {
                output: "result.mp4".into(),
                duration: 3.0
            })))
        );
        assert!(app.export_artifact.is_some());
        assert!(app.outcome.is_none());
        assert_eq!(
            app.destination_result.as_ref().unwrap().issue,
            Some(app_destination::Issue::Exists)
        );
        app.cancelling = false;
        app.invalidated();
        assert_ne!(app.export_artifact.as_ref().unwrap().revision, app.revision);
        assert!(
            app.export_artifact.is_some(),
            "Published history remains available after an edit"
        );
    }
    #[test]
    fn confirmed_cancellation_has_no_published_artifact() {
        let mut app = app();
        app.cancelling = true;
        assert!(app.receive_job_event(Event::Cancelled));
        assert!(app.export_artifact.is_none());
        assert_eq!(
            app.outcome.as_ref().unwrap().key,
            "ui.export_cancelled_confirmed"
        );
    }

    #[test]
    fn committed_subtitles_survive_late_cancel_and_preserve_montage_artifacts() {
        let mut app = app();
        app.subtitles.running = true;
        app.subtitles.revision = 7;
        app.subtitles.operation_revision = 7;
        app.cancelling = true;
        app.preview_artifact = Some(app_state::Artifact {
            path: "preview.mp4".into(),
            revision: 0,
            bytes: None,
        });
        app.export_artifact = Some(app_state::Artifact {
            path: "export.mp4".into(),
            revision: 0,
            bytes: None,
        });
        let result = noh::subtitles::SubtitleResult {
            output: "speech.srt".into(),
            track: noh::subtitle_track::SubtitleTrack {
                language: None,
                duration_ms: 1000,
                cues: vec![],
            },
        };
        assert!(app.receive_job_event(Event::Subtitled(Ok(result))));
        assert_eq!(app.subtitles.artifact.as_ref().unwrap().revision, 7);
        assert_eq!(app.subtitles.artifact.as_ref().unwrap().cues, 0);
        assert!(app.subtitles.outcome.is_none());
        assert!(app.preview_artifact.is_some() && app.export_artifact.is_some());
        app.notice = Some("ui.opened".into());
        app.cancelling = false;
        app.subtitles.running = false;
        assert_eq!(
            app.subtitles
                .status(app_state::Operation::Idle, &app.phase, app.notice.as_ref())
                .headline
                .key,
            "ui.subtitle_succeeded"
        );
    }

    #[test]
    fn subtitle_cancellation_keeps_montage_outcome_separate() {
        let mut app = app();
        app.subtitles.running = true;
        app.outcome = Some("ui.export_cancelled_confirmed".into());
        assert!(app.receive_job_event(Event::Cancelled));
        assert!(app.subtitles.artifact.is_none());
        assert_eq!(
            app.subtitles.outcome.as_ref().unwrap().key,
            "ui.subtitle_cancelled"
        );
        assert_eq!(
            app.outcome.as_ref().unwrap().key,
            "ui.export_cancelled_confirmed"
        );
    }

    #[test]
    fn subtitle_controller_failures_use_translated_catalog_fallback() {
        let mut app = app();
        app.subtitles.operation_revision = 4;
        for code in ["error.protocol", "error.publish", "error.timeout"] {
            assert!(
                app.receive_job_event(Event::Subtitled(Err(noh::engine::EngineError::new(
                    code,
                    "transcribe",
                    None,
                    "Technical detail"
                ))))
            );
            let failure = app.subtitles.failure.as_ref().unwrap();
            assert_eq!(failure.key, "error.engine");
            assert_eq!(app.subtitles.failure_revision, Some(4));
            for language in Language::ALL {
                assert_ne!(failure.render(language), "error.engine");
                assert!(!failure.render(language).contains(code));
            }
            assert!(app.log.back().unwrap().contains("Technical detail"));
        }
    }
}
