//! Independent local subtitle workspace. No probing or model IO in rendering.
use super::*;
use app_state::{Availability, ContextAction, Operation, StatusView};
use app_style::{self as style, Severity, space};
use std::sync::mpsc::{self, Receiver};

pub(super) fn transcriber_name() -> String {
    format!("whisper-cli{}", std::env::consts::EXE_SUFFIX)
}

#[derive(Clone, Debug)]
pub struct Artifact {
    pub path: PathBuf,
    pub revision: u64,
    pub cues: usize,
}

#[derive(Clone, Copy)]
enum Field {
    Runtime,
    Model,
    Vad,
}

pub struct Workspace {
    pub open: bool,
    pub source: String,
    pub output: String,
    pub transcriber: String,
    pub model: String,
    pub vad_model: String,
    pub language: String,
    pub revision: u64,
    pub operation_revision: u64,
    pub running: bool,
    pub artifact: Option<Artifact>,
    pub failure: Option<Message>,
    pub failure_revision: Option<u64>,
    pub outcome: Option<Message>,
    picker: Option<Receiver<(Field, u64, Option<PathBuf>)>>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            open: false,
            source: String::new(),
            output: std::env::current_dir()
                .unwrap_or_default()
                .join("subtitles.srt")
                .display()
                .to_string(),
            transcriber: String::new(),
            model: String::new(),
            vad_model: String::new(),
            language: "auto".into(),
            revision: 0,
            operation_revision: 0,
            running: false,
            artifact: None,
            failure: None,
            failure_revision: None,
            outcome: None,
            picker: None,
        }
    }
}

impl Workspace {
    /// Discover the portable speech files once, before the first GUI frame.
    /// Model contents are only loaded by the supervised transcription worker.
    pub fn load_bundled(&mut self, folder: &Path) -> bool {
        let portable = [folder.join("bin/speech"), folder.to_path_buf()]
            .into_iter()
            .map(|folder| {
                [
                    folder.join(transcriber_name()),
                    folder.join("ggml-small.bin"),
                    folder.join("ggml-silero-v6.2.0.bin"),
                ]
            });
        // Signed macOS apps keep model data in Resources and executable code
        // in MacOS; putting model files in a nested code directory breaks sealing.
        let macos = [[
            folder.join("bin").join(transcriber_name()),
            folder.join("../Resources/speech/ggml-small.bin"),
            folder.join("../Resources/speech/ggml-silero-v6.2.0.bin"),
        ]];
        let Some(paths) = portable
            .chain(macos)
            .filter(|paths| paths.iter().any(|path| path.is_file()))
            .max_by_key(|paths| paths.iter().filter(|path| path.is_file()).count())
        else {
            return false;
        };
        let complete = paths.iter().all(|path| path.is_file());
        for (value, path) in [&mut self.transcriber, &mut self.model, &mut self.vad_model]
            .into_iter()
            .zip(paths)
        {
            if value.is_empty() {
                *value = path.display().to_string();
            }
        }
        complete
    }

    pub fn configured(&self) -> bool {
        !self.transcriber.is_empty() && !self.model.is_empty() && !self.vad_model.is_empty()
    }

    pub fn settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, l: Language, busy: bool) {
        let mut changed = false;
        // Options shows these under its own disclosure.
        ui.add_enabled_ui(!busy && self.picker.is_none(), |ui| {
            for (field, key) in [
                (Field::Runtime, "ui.subtitle_runtime"),
                (Field::Model, "ui.subtitle_model"),
                (Field::Vad, "ui.subtitle_vad"),
            ] {
                ui.label(l.text(key));
                changed |= self.path_row(ui, ctx, l, field, true);
            }
        });
        ui.add(
            egui::Label::new(egui::RichText::new(l.text("ui.subtitle_models_help")).small()).wrap(),
        );
        if changed {
            self.edited();
        }
    }

    pub fn edited(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.failure = None;
        self.failure_revision = None;
        self.outcome = None;
    }

    pub fn issue(&self) -> Option<Message> {
        if self.source.is_empty() {
            return Some("ui.subtitle_source_required".into());
        }
        if self.transcriber.is_empty()
            || self.model.is_empty()
            || self.vad_model.is_empty()
            || self.language.is_empty()
        {
            return Some("ui.subtitle_setup_required".into());
        }
        if self.output.is_empty()
            || !Path::new(&self.output)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("srt"))
        {
            return Some("ui.subtitle_srt_required".into());
        }
        if self.picker.is_some() {
            return Some("ui.subtitle_picker".into());
        }
        self.make_request(None)
            .validate()
            .err()
            .map(|error| error.message_code().into())
    }

    fn make_request(&self, ffmpeg: Option<&PathBuf>) -> noh::subtitles::SubtitleRequest {
        noh::subtitles::SubtitleRequest {
            source: self.source.clone().into(),
            output: self.output.clone().into(),
            ffmpeg: ffmpeg.cloned().unwrap_or_default(),
            transcriber: self.transcriber.clone().into(),
            model: self.model.clone().into(),
            vad_model: self.vad_model.clone().into(),
            language: self.language.clone(),
        }
    }
    pub fn request(
        &self,
        ffmpeg: Option<&PathBuf>,
    ) -> Result<noh::subtitles::SubtitleRequest, Message> {
        if let Some(issue) = self.issue() {
            return Err(issue);
        }
        let request = self.make_request(ffmpeg);
        request
            .validate()
            .map_err(|error| Message::from(error.message_code()))?;
        Ok(request)
    }

    pub fn status(
        &self,
        operation: Operation,
        phase: &Message,
        notice: Option<&Message>,
    ) -> StatusView {
        let current = self
            .artifact
            .as_ref()
            .is_some_and(|artifact| artifact.revision == self.revision);
        let issue = self.issue();
        let locked = operation != Operation::Idle;
        let reason: Message = if locked {
            "ui.locked".into()
        } else {
            issue.clone().unwrap_or_else(|| "ui.available".into())
        };
        let mut actions = Vec::new();
        let (severity, headline, detail) = match operation {
            Operation::Transcribing => {
                (Severity::Info, "ui.subtitle_running".into(), phase.clone())
            }
            Operation::CancellingSubtitles => (
                Severity::Warning,
                "ui.cancel_requested".into(),
                "ui.cancel_wait".into(),
            ),
            Operation::Running { .. }
            | Operation::Burning { .. }
            | Operation::Shortening { .. } => (Severity::Info, "ui.locked".into(), phase.clone()),
            Operation::Cancelling { .. }
            | Operation::CancellingCaptions { .. }
            | Operation::CancellingShorts { .. } => (
                Severity::Warning,
                "ui.cancel_requested".into(),
                "ui.cancel_wait".into(),
            ),
            _ if self.failure_revision == Some(self.revision) && self.failure.is_some() => (
                Severity::Error,
                "ui.failed".into(),
                self.failure.clone().unwrap(),
            ),
            _ if current => {
                actions.extend([
                    ContextAction::OpenSubtitle,
                    ContextAction::OpenSubtitleFolder,
                ]);
                let artifact = self.artifact.as_ref().unwrap();
                (
                    Severity::Success,
                    "ui.subtitle_succeeded".into(),
                    Message::new(
                        "ui.subtitle_result",
                        &[
                            artifact.cues.to_string(),
                            artifact
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        ],
                    ),
                )
            }
            _ if issue.is_some() => (Severity::Info, "ui.subtitle_setup".into(), issue.unwrap()),
            _ => (
                Severity::Info,
                "ui.subtitle_ready".into(),
                "ui.subtitle_review".into(),
            ),
        };
        StatusView {
            destination: false,
            severity,
            headline,
            detail,
            third: if locked {
                Some("ui.locked".into())
            } else {
                notice
                    .cloned()
                    .or_else(|| self.outcome.clone())
                    .or_else(|| {
                        self.artifact
                            .as_ref()
                            .filter(|a| a.revision != self.revision)
                            .map(|_| "ui.subtitle_stale".into())
                    })
            },
            export: Availability {
                enabled: !locked && self.issue().is_none(),
                reason: reason.clone(),
            },
            preview: Availability {
                enabled: false,
                reason,
            },
            actions,
            operation,
            montage: None,
        }
    }

    fn choose(&mut self, field: Field, ctx: &egui::Context, l: Language) {
        if self.picker.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let revision = self.revision;
        let title = l
            .text(match field {
                Field::Runtime => "ui.subtitle_runtime",
                Field::Model => "ui.subtitle_model",
                Field::Vad => "ui.subtitle_vad",
            })
            .to_owned();
        let ctx = ctx.clone();
        if std::thread::Builder::new()
            .name("subtitle-picker".into())
            .spawn(move || {
                let dialog = rfd::FileDialog::new().set_title(title);
                let path = match field {
                    Field::Model | Field::Vad => dialog.add_filter("Model", &["bin"]).pick_file(),
                    Field::Runtime => dialog.pick_file(),
                };
                let _ = tx.send((field, revision, path));
                ctx.request_repaint();
            })
            .is_ok()
        {
            self.picker = Some(rx);
        } else {
            self.failure = Some("error.subtitle_config".into());
            self.failure_revision = Some(self.revision);
        }
    }

    pub fn receive_picker(&mut self) {
        let result = self.picker.as_ref().map(|rx| rx.try_recv());
        match result {
            Some(Ok((field, revision, path))) => {
                self.picker = None;
                if revision != self.revision {
                    return;
                }
                if let Some(path) = path {
                    let value = path.display().to_string();
                    let target = match field {
                        Field::Runtime => &mut self.transcriber,
                        Field::Model => &mut self.model,
                        Field::Vad => &mut self.vad_model,
                    };
                    if *target != value {
                        *target = value;
                        self.edited();
                    }
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.picker = None;
                self.failure = Some("error.subtitle_config".into());
                self.failure_revision = Some(self.revision);
            }
            _ => {}
        }
    }

    fn path_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        l: Language,
        field: Field,
        compact: bool,
    ) -> bool {
        let mut changed = false;
        let mut choose = false;
        let (value, id) = match field {
            Field::Runtime => (&mut self.transcriber, "subtitle-runtime"),
            Field::Model => (&mut self.model, "subtitle-model"),
            Field::Vad => (&mut self.vad_model, "subtitle-vad"),
        };
        let label = l.text("ui.subtitle_choose");
        let button_width = ui
            .painter()
            .layout_no_wrap(
                label.into(),
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            )
            .size()
            .x
            + 2.0 * ui.spacing().button_padding.x;
        if compact {
            let response = ui.add_sized(
                egui::vec2(ui.available_width(), style::CONTROL_H),
                egui::TextEdit::singleline(value).id(egui::Id::new(id)),
            );
            changed = response.changed();
            style::focus(ui, &response);
            choose = style::button(ui, label, true, false, 0.0).clicked();
        } else {
            ui.horizontal(|ui| {
                let response = ui.add_sized(
                    egui::vec2(
                        (ui.available_width() - button_width - space::S).max(80.0),
                        style::CONTROL_H,
                    ),
                    egui::TextEdit::singleline(value).id(egui::Id::new(id)),
                );
                changed = response.changed();
                style::focus(ui, &response);
                choose = style::button(ui, label, true, false, button_width).clicked();
            });
        }
        if choose {
            self.choose(field, ctx, l);
        }
        changed
    }
}

impl NohApp {
    pub(super) fn start_subtitles(&mut self, ctx: &egui::Context) {
        if self.job.is_some()
            || self.cancelling
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some()
        {
            return;
        }
        if let Some(issue) = self.resources.speech_issue() {
            self.subtitles.failure = Some(issue);
            self.subtitles.failure_revision = Some(self.subtitles.revision);
            self.settings_open = true;
            return;
        }
        let request = match self.subtitles.request(self.ffmpeg.as_ref()) {
            Ok(request) => request,
            Err(error) => {
                self.subtitles.failure = Some(error);
                self.subtitles.failure_revision = Some(self.subtitles.revision);
                return;
            }
        };
        if let Some(preflight) = &self.preflight {
            preflight.cancel();
        }
        self.inspecting = false;
        let ctx = ctx.clone();
        self.job = Some(Job::transcribe(request, move || ctx.request_repaint()));
        self.subtitles.running = true;
        self.subtitles.operation_revision = self.subtitles.revision;
        self.subtitles.failure = None;
        self.subtitles.failure_revision = None;
        self.subtitles.outcome = None;
        self.previewing = false;
        self.cancelling = false;
        self.notice = None;
        self.started = Some(Instant::now());
        self.fraction = 0.0;
        self.phase = "subtitle.preparing".into();
        self.log.clear();
        self.warnings.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_bundle_keeps_the_expected_missing_file_path_for_repair() {
        let folder = tempfile::tempdir().unwrap();
        let speech = folder.path().join("bin/speech");
        std::fs::create_dir_all(&speech).unwrap();
        std::fs::write(speech.join(transcriber_name()), b"synthetic runtime").unwrap();
        std::fs::write(speech.join("ggml-small.bin"), b"synthetic model").unwrap();
        let mut workspace = Workspace::default();
        assert!(!workspace.load_bundled(folder.path()));
        assert_eq!(
            Path::new(&workspace.vad_model),
            speech.join("ggml-silero-v6.2.0.bin")
        );
        assert_eq!(Path::new(&workspace.model), speech.join("ggml-small.bin"));
    }
    #[test]
    fn portable_setup_is_ready_without_manual_paths_and_preserves_overrides() {
        let folder = tempfile::tempdir().unwrap();
        let mut workspace = Workspace::default();
        assert!(!workspace.load_bundled(folder.path()));
        assert!(!workspace.configured());
        let speech = folder.path().join("bin/speech");
        std::fs::create_dir_all(&speech).unwrap();
        for file in [
            &transcriber_name(),
            "ggml-small.bin",
            "ggml-silero-v6.2.0.bin",
        ] {
            std::fs::write(speech.join(file), []).unwrap();
        }
        assert!(workspace.load_bundled(folder.path()));
        assert!(workspace.configured());
        assert_eq!(
            Path::new(&workspace.transcriber),
            speech.join(transcriber_name())
        );
        assert_eq!(Path::new(&workspace.model), speech.join("ggml-small.bin"));
        workspace.model = "custom-model.bin".into();
        assert!(workspace.load_bundled(folder.path()));
        assert_eq!(workspace.model, "custom-model.bin");
        // Neither discovery nor inspecting settings starts transcription.
        assert!(!workspace.running && workspace.artifact.is_none());
    }
    #[test]
    fn macos_bundle_discovers_sealed_resources_after_relocation() {
        let temporary = tempfile::tempdir().unwrap();
        let original = temporary.path().join("Original.app");
        let macos = original.join("Contents/MacOS");
        let resources = original.join("Contents/Resources/speech");
        std::fs::create_dir_all(macos.join("bin")).unwrap();
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(macos.join("bin").join(transcriber_name()), []).unwrap();
        std::fs::write(resources.join("ggml-small.bin"), []).unwrap();
        assert!(!Workspace::default().load_bundled(&macos));
        std::fs::write(resources.join("ggml-silero-v6.2.0.bin"), []).unwrap();
        let relocated = temporary.path().join("Moved app 日本語.app");
        std::fs::rename(original, &relocated).unwrap();
        let mut workspace = Workspace::default();
        assert!(workspace.load_bundled(&relocated.join("Contents/MacOS")));
        assert!(workspace.configured());
        for path in [
            &workspace.transcriber,
            &workspace.model,
            &workspace.vad_model,
        ] {
            assert!(
                Path::new(path)
                    .canonicalize()
                    .unwrap()
                    .starts_with(relocated.canonicalize().unwrap())
            );
        }
    }

    fn ready() -> Workspace {
        Workspace {
            source: "source.wav".into(),
            transcriber: "whisper-cli.exe".into(),
            model: "model.bin".into(),
            vad_model: "vad.bin".into(),
            ..Default::default()
        }
    }
    #[test]
    fn result_is_revision_bound_and_independent_of_montage_edits() {
        let mut app = NohApp::default();
        app.subtitles = ready();
        app.subtitles.artifact = Some(Artifact {
            path: "subtitles.srt".into(),
            revision: 0,
            cues: 2,
        });
        let phase = "subtitle.writing".into();
        assert_eq!(
            app.subtitles
                .status(Operation::Idle, &phase, None)
                .headline
                .key,
            "ui.subtitle_succeeded"
        );
        app.invalidated();
        assert_eq!(
            app.subtitles
                .status(Operation::Idle, &phase, None)
                .headline
                .key,
            "ui.subtitle_succeeded"
        );
        app.subtitles.language = "fr".into();
        app.subtitles.edited();
        assert_ne!(
            app.subtitles
                .status(Operation::Idle, &phase, None)
                .headline
                .key,
            "ui.subtitle_succeeded"
        );
        assert!(app.subtitles.artifact.is_some());
        assert_eq!(
            app.subtitles
                .status(Operation::Idle, &phase, None)
                .third
                .unwrap()
                .key,
            "ui.subtitle_stale"
        );
    }
    #[test]
    fn active_subtitles_and_cancel_lock_generation_without_claiming_success() {
        let workspace = ready();
        let phase = "subtitle.transcribing".into();
        for operation in [Operation::Transcribing, Operation::CancellingSubtitles] {
            let view = workspace.status(operation, &phase, None);
            assert!(!view.export.enabled);
            assert!(view.actions.is_empty());
            assert!(!view.headline.key.contains('%'));
        }
    }

    #[test]
    fn explicit_source_and_spoken_language_do_not_follow_montage_or_locale() {
        let mut app = NohApp::default();
        app.wav = Some("soundtrack.wav".into());
        app.locale.language = Language::Ja;
        assert!(app.subtitles.source.is_empty());
        assert_eq!(app.subtitles.language, "auto");
        app.subtitles = ready();
        app.subtitles.language = "fr".into();
        app.locale.language = Language::De;
        assert_eq!(app.subtitles.language, "fr");
        app.subtitles.output = "wrong.mp4".into();
        assert_eq!(
            app.subtitles.issue().unwrap().key,
            "ui.subtitle_srt_required"
        );
    }

    #[test]
    fn running_subtitles_prevent_both_media_and_subtitle_restarts() {
        let mut app = NohApp::default();
        app.subtitles = ready();
        app.subtitles.running = true;
        let ctx = egui::Context::default();
        app.start(&ctx, false);
        app.start(&ctx, true);
        app.start_subtitles(&ctx);
        assert!(app.job.is_none());
        assert!(app.error.is_none());
        assert!(app.subtitles.failure.is_none());
    }

    #[test]
    fn settings_fields_stay_inside_narrow_and_normal_widths_in_all_languages() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        for language in Language::ALL {
            for width in [420.0, 980.0] {
                let mut workspace = ready();
                workspace.model = format!(
                    "C:/noh-qa/{}/{}.wav",
                    "long folder/".repeat(12),
                    "Speech model".repeat(12)
                );
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 540.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                workspace.settings(ui, &ctx, language, false);
                                assert!(
                                    ui.min_rect().right() <= width + 0.5,
                                    "{} {width}: fields overflow {:?}",
                                    language.code(),
                                    ui.min_rect()
                                );
                            });
                    },
                );
                output.textures_delta.clear();
            }
        }
    }

    #[test]
    fn obsolete_picker_cannot_overwrite_edits_and_identical_selection_keeps_revision() {
        let mut workspace = ready();
        let (tx, rx) = mpsc::channel();
        workspace.picker = Some(rx);
        tx.send((Field::Model, 0, Some("old-model.bin".into())))
            .unwrap();
        workspace.model = "new-model.bin".into();
        workspace.edited();
        workspace.receive_picker();
        assert_eq!(workspace.model, "new-model.bin");
        let (tx, rx) = mpsc::channel();
        workspace.picker = Some(rx);
        tx.send((Field::Model, 1, Some("new-model.bin".into())))
            .unwrap();
        workspace.receive_picker();
        assert_eq!(workspace.revision, 1);
        assert!(workspace.picker.is_none());
    }
}
