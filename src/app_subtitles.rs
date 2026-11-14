//! Independent local subtitle workspace. No probing or model IO in rendering.
use super::*;
use app_state::{Availability, ContextAction, Operation, StatusView};
use app_style::{self as style, Severity, space};
use std::sync::mpsc::{self, Receiver};

pub(super) fn transcriber_name() -> String {
    format!("whisper-cli{}", std::env::consts::EXE_SUFFIX)
}

/// The Whisper model's name for the interface (`ggml-large-v3.bin` → `large-v3`).
pub(super) fn model_name(model: &str) -> Option<String> {
    let stem = Path::new(model).file_stem()?.to_string_lossy().into_owned();
    let name = stem.strip_prefix("ggml-").unwrap_or(&stem);
    (!name.is_empty()).then(|| name.to_string())
}

/// Whisper sizes, most accurate first (whisper.cpp file names `ggml-<size>.bin`).
const MODEL_RANK: [&str; 13] = [
    "large-v3",
    "large-v3-turbo",
    "large-v2",
    "large-v1",
    "large",
    "medium",
    "medium.en",
    "small",
    "small.en",
    "base",
    "base.en",
    "tiny",
    "tiny.en",
];

/// A model file's place in MODEL_RANK; quantized files (`ggml-large-v3-q5_0.bin`)
/// rank after their full-size model.
fn rank(name: &str) -> Option<(usize, bool)> {
    let size = name.strip_prefix("ggml-")?.strip_suffix(".bin")?;
    MODEL_RANK
        .iter()
        .position(|m| *m == size)
        .map(|i| (i, false))
        .or_else(|| {
            // The longest size the name starts with: large-v3-turbo-q5_0 is a turbo.
            (0..MODEL_RANK.len())
                .filter(|i| size.starts_with(&format!("{}-", MODEL_RANK[*i])))
                .max_by_key(|i| MODEL_RANK[*i].len())
                .map(|i| (i, true))
        })
}

/// Large-class models (large-v3, turbo, v2, v1): fast only on a capable GPU.
fn is_large(name: &str) -> bool {
    rank(name).is_some_and(|(i, _)| MODEL_RANK[i].starts_with("large"))
}

/// The Whisper models in a folder, most accurate first, with their size on disk.
pub(super) fn installed_models(folder: &Path) -> Vec<(String, u64)> {
    let mut models: Vec<_> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rank = rank(&name)?;
            let metadata = entry.metadata().ok().filter(|m| m.is_file())?;
            Some((rank, name, metadata.len()))
        })
        .collect();
    models.sort();
    models
        .into_iter()
        .map(|(_, name, bytes)| (name, bytes))
        .collect()
}

/// GPU memory a large model needs beyond its file: the compute buffers seen
/// with large-v3 on an RTX 4060 (kv 343 MB, compute about 860 MB) and a margin.
const GPU_MARGIN: u64 = 2 << 30;

/// Whether a model runs at a usable speed here: a large one needs an NVIDIA
/// GPU with room for it (`vram`: device 0's total memory).
fn usable(name: &str, bytes: u64, vram: Option<u64>) -> bool {
    !is_large(name) || vram.is_some_and(|v| v >= bytes.saturating_add(GPU_MARGIN))
}

/// Automatique: the most accurate model that runs at a usable speed here; a
/// large model without a capable GPU only when nothing else is installed.
fn auto_model(models: &[(String, u64)], vram: Option<u64>) -> Option<&str> {
    models
        .iter()
        .find(|(name, bytes)| usable(name, *bytes, vram))
        .or(models.first())
        .map(|(name, _)| name.as_str())
}

/// Automatique's model in a folder. Without any, the bundled Small's path
/// (reported missing for repair). The engine reads the size from the header.
fn best_model(folder: &Path, vram: Option<u64>) -> PathBuf {
    auto_model(&installed_models(folder), vram)
        .map_or_else(|| folder.join("ggml-small.bin"), |name| folder.join(name))
}

/// Device 0's total memory from `whisper-cli --help`, which loads the backends
/// and prints `Device 0: NVIDIA ..., VRAM: 8187 MiB`. None without a CUDA device.
pub(super) fn parse_vram(help: &str) -> Option<u64> {
    help.lines().find_map(|line| {
        let device = line.trim_start().strip_prefix("Device 0:")?;
        let mib = device.rsplit_once("VRAM:")?.1.trim();
        let mib: u64 = mib.strip_suffix("MiB")?.trim().parse().ok()?;
        Some(mib << 20)
    })
}

/// The saved transcription model: `auto`, a file name in the bundled speech
/// folder, or an absolute path (`noh/speech-model`, beside the language).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ModelChoice {
    #[default]
    Auto,
    Installed(String),
    File(String),
}
impl ModelChoice {
    pub fn parse(value: &str) -> Self {
        let value = value.trim();
        if Path::new(value).is_absolute() {
            Self::File(value.into())
        } else if value.is_empty()
            || value == "auto"
            || Path::new(value).file_name() != Some(std::ffi::OsStr::new(value))
        {
            Self::Auto
        } else {
            Self::Installed(value.into())
        }
    }
    pub fn code(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Installed(value) | Self::File(value) => value,
        }
    }
}
fn choice_path() -> Option<PathBuf> {
    Some(noh::i18n::preferences_path()?.with_file_name("speech-model"))
}

/// A size on disk for the model list (`2,9 Go`).
fn size_text(bytes: u64, l: Language) -> String {
    let number = format!("{:.1}", bytes as f64 / 1e9);
    let number = match l.code() {
        "fr" | "de" | "es" => number.replace('.', ","),
        _ => number,
    };
    l.format("ui.size_gb", &[number])
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
    /// The saved transcription model choice, and where it is saved: set only
    /// by `load_choice` (startup), so tests never touch the user's preferences.
    pub choice: ModelChoice,
    prefs: Option<PathBuf>,
    /// The bundled speech folder and its models, read at discovery.
    speech_folder: Option<PathBuf>,
    pub models: Vec<(String, u64)>,
    /// Device 0's memory, read once per session off the UI thread: None until
    /// read, Some(None) without a usable NVIDIA GPU.
    pub vram: Option<Option<u64>>,
    probe: Option<Receiver<Option<u64>>>,
    /// The path Automatique last chose: it only ever replaces its own choice.
    auto_model: Option<String>,
    /// Shown under the setting (a saved model that is gone).
    pub model_notice: Option<Message>,
    /// The size of a model outside the bundled folder, read once per path.
    model_size: std::cell::RefCell<Option<(String, u64)>>,
    /// A large model failed under Automatique: generate again with a smaller one.
    pub retry: Option<Message>,
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
            choice: ModelChoice::Auto,
            prefs: None,
            speech_folder: None,
            models: Vec::new(),
            vram: None,
            probe: None,
            auto_model: None,
            model_notice: None,
            model_size: Default::default(),
            retry: None,
        }
    }
}

impl Workspace {
    /// Discover the portable speech files once, before the first GUI frame.
    /// Model contents are only loaded by the supervised transcription worker.
    pub fn load_bundled(&mut self, folder: &Path) -> bool {
        let vram = self.vram.flatten();
        let best_model = |folder: &Path| best_model(folder, vram);
        let portable = [folder.join("bin/speech"), folder.to_path_buf()]
            .into_iter()
            .map(|folder| {
                [
                    folder.join(transcriber_name()),
                    best_model(&folder),
                    folder.join("ggml-silero-v6.2.0.bin"),
                ]
            });
        // Signed macOS apps keep model data in Resources and executable code
        // in MacOS; putting model files in a nested code directory breaks sealing.
        let resources = folder.join("../Resources/speech");
        let macos = [[
            folder.join("bin").join(transcriber_name()),
            best_model(&resources),
            resources.join("ggml-silero-v6.2.0.bin"),
        ]];
        let Some(paths) = portable
            .chain(macos)
            .filter(|paths| paths.iter().any(|path| path.is_file()))
            .max_by_key(|paths| paths.iter().filter(|path| path.is_file()).count())
        else {
            return false;
        };
        let complete = paths.iter().all(|path| path.is_file());
        if let Some(speech) = paths[1].parent() {
            self.models = installed_models(speech);
            self.speech_folder = Some(speech.to_path_buf());
        }
        if self.model.is_empty() {
            self.auto_model = Some(paths[1].display().to_string());
        }
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

    /// Reads the saved model choice (startup), and saves later choices there.
    pub fn load_choice(&mut self) {
        self.prefs = choice_path();
        if let Some(value) = self
            .prefs
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        {
            self.choice = ModelChoice::parse(&value);
        }
    }
    fn save_choice(&self) {
        if let Some(path) = &self.prefs {
            if let Some(folder) = path.parent() {
                let _ = std::fs::create_dir_all(folder);
            }
            let _ = std::fs::write(path, self.choice.code());
        }
    }

    /// Applies the choice to the model path. A saved model that is gone falls
    /// back to Automatique for this session, with a notice; the saved choice is
    /// kept (its drive may only be unplugged).
    pub fn resolve_choice(&mut self) {
        let wanted = match &self.choice {
            ModelChoice::Auto => None,
            ModelChoice::Installed(name) => self.speech_folder.as_ref().map(|f| f.join(name)),
            ModelChoice::File(path) => Some(PathBuf::from(path)),
        };
        let path = match wanted {
            Some(path) if path.is_file() => {
                self.auto_model = None;
                path
            }
            _ => {
                if self.choice != ModelChoice::Auto {
                    self.model_notice = Some("ui.subtitle_model_missing".into());
                }
                let Some(folder) = &self.speech_folder else {
                    return;
                };
                let name =
                    auto_model(&self.models, self.vram.flatten()).unwrap_or("ggml-small.bin");
                let path = folder.join(name);
                self.auto_model = Some(path.display().to_string());
                path
            }
        };
        let value = path.display().to_string();
        if self.model != value {
            self.model = value;
            self.edited();
        }
    }

    /// Reads the GPU's memory once per session, off the UI thread (about 0.7 s:
    /// the transcriber loads its backends, with no model and no audio). Windows
    /// only: elsewhere the bundled transcriber runs on the CPU.
    pub fn start_probe(&mut self) {
        if self.vram.is_some() || self.probe.is_some() || self.transcriber.is_empty() {
            return;
        }
        if !cfg!(windows) {
            self.vram = Some(None);
            return;
        }
        let transcriber = PathBuf::from(&self.transcriber);
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("speech-gpu-probe".into())
            .spawn(move || {
                let mut command = noh::command(&transcriber);
                // The same device selection as transcription (subtitles.rs).
                command
                    .env_remove("GGML_BACKEND_PATH")
                    .env_remove("WHISPER_ARG_DEVICE")
                    .arg("--help")
                    .stdin(std::process::Stdio::null());
                let vram = command.output().ok().and_then(|output| {
                    parse_vram(&String::from_utf8_lossy(&output.stderr))
                        .or_else(|| parse_vram(&String::from_utf8_lossy(&output.stdout)))
                });
                let _ = tx.send(vram);
            });
        match spawned {
            Ok(_) => self.probe = Some(rx),
            Err(_) => self.vram = Some(None),
        }
    }

    /// The probe's answer: Automatique may now take a large model.
    pub fn receive_probe(&mut self) {
        let vram = match self.probe.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(vram)) => vram,
            Some(Err(mpsc::TryRecvError::Disconnected)) => None,
            _ => return,
        };
        self.probe = None;
        self.vram = Some(vram);
        if self.choice == ModelChoice::Auto
            && !self.running
            && self.auto_model.as_deref() == Some(self.model.as_str())
        {
            self.resolve_choice();
        }
    }

    /// Under Automatique, a large model that failed in the transcriber (at load
    /// or out of memory) is retried once with the most accurate other model
    /// installed; `retry` then names both. False when there is nothing to retry.
    pub fn fall_back(&mut self, code: &str) -> bool {
        let Some(failed) = Path::new(&self.model)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            return false;
        };
        if self.choice != ModelChoice::Auto
            || code != "error.subtitle_backend"
            || !is_large(&failed)
        {
            return false;
        }
        let (Some(folder), Some((smaller, _))) = (
            self.speech_folder.clone(),
            self.models
                .iter()
                .find(|(name, _)| !is_large(name))
                .cloned(),
        ) else {
            return false;
        };
        let path = folder.join(&smaller).display().to_string();
        self.model = path.clone();
        self.auto_model = Some(path);
        self.edited();
        self.retry = Some(Message::new(
            "lyrics.retry_smaller",
            &[
                model_name(&failed).unwrap_or(failed),
                model_name(&smaller).unwrap_or(smaller),
            ],
        ));
        true
    }

    /// The setting: Automatique (naming its pick), each model found beside the
    /// transcriber with its size, and « Autre fichier… » (the file picker).
    pub fn model_selector(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        l: Language,
        busy: bool,
    ) {
        let vram = self.vram.flatten();
        let auto = auto_model(&self.models, vram)
            .and_then(model_name)
            .unwrap_or_else(|| "small".into());
        // Beside the « Paroles » heading: « Automatique (Whisper large-v3) ».
        let current = format!("Whisper {}", model_name(&self.model).unwrap_or_default());
        let selected = match &self.choice {
            ModelChoice::Auto => l.format("ui.subtitle_model_auto", &[current]),
            _ => current,
        };
        let before = self.choice.clone();
        let mut other = false;
        ui.add_enabled_ui(!busy && self.picker.is_none(), |ui| {
            style::framed_select(ui, |ui| {
                egui::ComboBox::from_id_salt("speech-model")
                    .icon(style::select_chevron)
                    .selected_text(selected)
                    .width(ui.available_width().min(260.0))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.choice,
                            ModelChoice::Auto,
                            l.format("ui.subtitle_model_auto", &[auto]),
                        );
                        ui.separator();
                        for (name, bytes) in &self.models {
                            let mut text = format!(
                                "{} · {}",
                                model_name(name).unwrap_or_default(),
                                size_text(*bytes, l)
                            );
                            if !usable(name, *bytes, vram) {
                                text = format!("{text} · {}", l.text("ui.subtitle_model_slow"));
                            }
                            ui.selectable_value(
                                &mut self.choice,
                                ModelChoice::Installed(name.clone()),
                                text,
                            );
                        }
                        ui.separator();
                        other = ui
                            .selectable_label(
                                matches!(self.choice, ModelChoice::File(_)),
                                l.text("ui.subtitle_model_other"),
                            )
                            .clicked();
                    })
                    .response
                    .on_hover_text(l.text("ui.subtitle_model"))
            });
        });
        if other {
            self.choose(Field::Model, ctx, l);
        }
        if self.choice != before {
            self.model_notice = None;
            self.resolve_choice();
            self.save_choice();
        }
    }

    /// A large model that will be slow here (no NVIDIA GPU with room for it):
    /// the lyrics menu says so before generating.
    pub fn slow_here(&self) -> bool {
        let Some(name) = Path::new(&self.model).file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        let listed = self
            .models
            .iter()
            .find(|(model, _)| model == name)
            .map(|(_, bytes)| *bytes);
        let bytes = listed.unwrap_or_else(|| {
            // Another file: its size, read once (not on every frame).
            let mut cached = self.model_size.borrow_mut();
            match &*cached {
                Some((path, bytes)) if *path == self.model => *bytes,
                _ => {
                    let bytes = std::fs::metadata(&self.model).map_or(0, |m| m.len());
                    *cached = Some((self.model.clone(), bytes));
                    bytes
                }
            }
        });
        self.vram.is_some() && !usable(name, bytes, self.vram.flatten())
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
                    if matches!(field, Field::Model) {
                        // A chosen model file becomes the saved choice.
                        self.choice = ModelChoice::File(value.clone());
                        self.auto_model = None;
                        self.model_notice = None;
                        self.save_choice();
                    }
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
        // The model is set by the selector or the picker, which save the choice.
        let editable = !matches!(field, Field::Model);
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
                egui::TextEdit::singleline(value)
                    .id(egui::Id::new(id))
                    .interactive(editable),
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
                    egui::TextEdit::singleline(value)
                        .id(egui::Id::new(id))
                        .interactive(editable),
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
    fn the_most_accurate_whisper_model_beside_the_transcriber_is_used() {
        let folder = tempfile::tempdir().unwrap();
        let speech = folder.path().join("bin/speech");
        std::fs::create_dir_all(&speech).unwrap();
        for file in [
            &transcriber_name(),
            "ggml-silero-v6.2.0.bin",
            "ggml-small.bin",
        ] {
            std::fs::write(speech.join(file), []).unwrap();
        }
        let chosen = |speech: &Path| {
            let mut workspace = Workspace::default();
            // A GPU with room for every model: the most accurate one wins.
            workspace.vram = Some(Some(8 << 30));
            assert!(workspace.load_bundled(speech.parent().unwrap().parent().unwrap()));
            Path::new(&workspace.model)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(chosen(&speech), "ggml-small.bin");
        for (added, expected) in [
            ("ggml-tiny.bin", "ggml-small.bin"),
            ("ggml-medium.bin", "ggml-medium.bin"),
            (
                "ggml-large-v3-turbo-q5_0.bin",
                "ggml-large-v3-turbo-q5_0.bin",
            ),
            ("ggml-large-v3-turbo.bin", "ggml-large-v3-turbo.bin"),
            ("ggml-large-v3-q5_0.bin", "ggml-large-v3-q5_0.bin"),
            ("ggml-large-v3.bin", "ggml-large-v3.bin"),
        ] {
            std::fs::write(speech.join(added), []).unwrap();
            assert_eq!(chosen(&speech), expected, "after adding {added}");
        }
        // Small is not required: any model completes the setup.
        std::fs::remove_file(speech.join("ggml-small.bin")).unwrap();
        assert_eq!(chosen(&speech), "ggml-large-v3.bin");
        // The VAD model is never taken for a speech model.
        let only_vad = tempfile::tempdir().unwrap();
        assert_eq!(
            best_model(only_vad.path(), None),
            only_vad.path().join("ggml-small.bin")
        );
        std::fs::write(only_vad.path().join("ggml-silero-v6.2.0.bin"), []).unwrap();
        assert_eq!(
            best_model(only_vad.path(), None),
            only_vad.path().join("ggml-small.bin")
        );
    }

    #[test]
    fn a_picked_model_is_saved_only_where_the_preferences_were_loaded_from() {
        // A workspace never loaded from the preferences writes nothing (tests).
        assert!(Workspace::default().prefs.is_none());
        let folder = tempfile::tempdir().unwrap();
        let saved = folder.path().join("noh/speech-model");
        let mut workspace = ready();
        workspace.prefs = Some(saved.clone());
        let picked = folder.path().join("ggml-medium.bin").display().to_string();
        let (tx, rx) = mpsc::channel();
        workspace.picker = Some(rx);
        tx.send((Field::Model, 0, Some(picked.clone().into())))
            .unwrap();
        workspace.receive_picker();
        assert_eq!(workspace.model, picked);
        assert_eq!(workspace.choice, ModelChoice::File(picked.clone()));
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), picked);
    }

    #[test]
    fn a_large_model_is_flagged_slow_only_without_a_gpu_with_room_for_it() {
        let mut workspace = ready();
        workspace.models = vec![("ggml-large-v3.bin".into(), 3_095_033_483)];
        workspace.model = "C:/noh/bin/speech/ggml-large-v3.bin".into();
        // Unknown until the probe answers: no warning yet.
        assert!(!workspace.slow_here());
        workspace.vram = Some(None);
        assert!(workspace.slow_here());
        workspace.vram = Some(Some(8187 << 20));
        assert!(!workspace.slow_here());
        workspace.model = "C:/noh/bin/speech/ggml-small.bin".into();
        workspace.vram = Some(None);
        assert!(!workspace.slow_here());
        // A large model picked elsewhere is measured: 3 GB does not fit 4 GiB + buffers.
        let folder = tempfile::tempdir().unwrap();
        let elsewhere = folder.path().join("ggml-large-v3.bin");
        std::fs::File::create(&elsewhere)
            .unwrap()
            .set_len(3_095_033_483)
            .unwrap();
        workspace.model = elsewhere.display().to_string();
        workspace.vram = Some(Some(4 << 30));
        assert!(workspace.slow_here());
        workspace.vram = Some(Some(8187 << 20));
        assert!(!workspace.slow_here());
    }

    #[test]
    fn automatique_takes_a_large_model_only_on_a_gpu_with_room_for_it() {
        let gib = 1u64 << 30;
        let models = vec![
            ("ggml-large-v3.bin".to_string(), 3_095_033_483),
            ("ggml-small.bin".to_string(), 487_601_967),
        ];
        // No GPU, or one too small for the model plus its buffers: Small.
        assert_eq!(auto_model(&models, None), Some("ggml-small.bin"));
        assert_eq!(auto_model(&models, Some(4 * gib)), Some("ggml-small.bin"));
        // An 8 GiB RTX 4060 (8187 MiB): large-v3.
        assert_eq!(
            auto_model(&models, Some(8187 << 20)),
            Some("ggml-large-v3.bin")
        );
        // Only a large model installed: it is still used, slow or not.
        assert_eq!(auto_model(&models[..1], None), Some("ggml-large-v3.bin"));
        assert_eq!(auto_model(&[], None), None);
        assert!(is_large("ggml-large-v3-turbo-q5_0.bin") && !is_large("ggml-medium.bin"));
    }

    #[test]
    fn gpu_memory_is_read_from_the_transcribers_device_line() {
        let help = "ggml_cuda_init: found 1 CUDA devices (Total VRAM: 8187 MiB):\n  Device 0: NVIDIA GeForce RTX 4060, compute capability 8.9, VMM: yes, VRAM: 8187 MiB\nload_backend: loaded CUDA backend\n";
        assert_eq!(parse_vram(help), Some(8187 << 20));
        assert_eq!(
            parse_vram("load_backend: loaded CPU backend\nusage: whisper-cli"),
            None
        );
        assert_eq!(parse_vram("  Device 0: NVIDIA X, VRAM: lots"), None);
    }

    #[test]
    fn the_saved_choice_reads_auto_a_bundled_name_or_an_absolute_path() {
        assert_eq!(ModelChoice::parse(""), ModelChoice::Auto);
        assert_eq!(ModelChoice::parse("auto\n"), ModelChoice::Auto);
        assert_eq!(
            ModelChoice::parse("ggml-small.bin"),
            ModelChoice::Installed("ggml-small.bin".into())
        );
        // A relative path is neither a bundled name nor a file: Automatique.
        assert_eq!(
            ModelChoice::parse("models/ggml-small.bin"),
            ModelChoice::Auto
        );
        let absolute = std::env::temp_dir().join("ggml-large-v3.bin");
        let absolute = absolute.display().to_string();
        assert_eq!(
            ModelChoice::parse(&absolute),
            ModelChoice::File(absolute.clone())
        );
        for choice in [
            ModelChoice::Auto,
            ModelChoice::Installed("ggml-small.bin".into()),
            ModelChoice::File(absolute),
        ] {
            assert_eq!(ModelChoice::parse(choice.code()), choice);
        }
    }

    fn bundle(models: &[&str]) -> (tempfile::TempDir, Workspace) {
        let folder = tempfile::tempdir().unwrap();
        let speech = folder.path().join("bin/speech");
        std::fs::create_dir_all(&speech).unwrap();
        for file in [&transcriber_name(), "ggml-silero-v6.2.0.bin"] {
            std::fs::write(speech.join(file), []).unwrap();
        }
        for model in models {
            std::fs::write(speech.join(model), []).unwrap();
        }
        let mut workspace = Workspace::default();
        assert!(workspace.load_bundled(folder.path()));
        (folder, workspace)
    }
    fn file(workspace: &Workspace) -> String {
        Path::new(&workspace.model)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn automatique_moves_to_a_large_model_once_the_gpu_is_known_but_never_over_a_choice() {
        let (_folder, mut workspace) = bundle(&["ggml-large-v3.bin", "ggml-small.bin"]);
        // Before the probe answers: Small.
        assert_eq!(file(&workspace), "ggml-small.bin");
        let (tx, rx) = mpsc::channel();
        workspace.probe = Some(rx);
        tx.send(Some(8 << 30)).unwrap();
        workspace.receive_probe();
        assert_eq!(file(&workspace), "ggml-large-v3.bin");
        assert_eq!(workspace.vram, Some(Some(8 << 30)));
        // A model set by hand (or by a capture) is never replaced by the probe.
        let (_folder, mut workspace) = bundle(&["ggml-large-v3.bin", "ggml-small.bin"]);
        workspace.model = "C:/models/ggml-medium.bin".into();
        let (tx, rx) = mpsc::channel();
        workspace.probe = Some(rx);
        tx.send(Some(8 << 30)).unwrap();
        workspace.receive_probe();
        assert_eq!(workspace.model, "C:/models/ggml-medium.bin");
    }

    #[test]
    fn a_saved_model_that_is_gone_falls_back_to_automatique_with_a_notice() {
        let (folder, mut workspace) = bundle(&["ggml-small.bin", "ggml-medium.bin"]);
        workspace.choice = ModelChoice::Installed("ggml-small.bin".into());
        workspace.resolve_choice();
        assert_eq!(file(&workspace), "ggml-small.bin");
        assert!(workspace.model_notice.is_none());
        workspace.choice = ModelChoice::Installed("ggml-large-v3.bin".into());
        workspace.resolve_choice();
        assert_eq!(file(&workspace), "ggml-medium.bin");
        assert_eq!(
            workspace.model_notice.as_ref().unwrap().key,
            "ui.subtitle_model_missing"
        );
        // The saved choice is kept: the file may come back.
        assert_eq!(
            workspace.choice,
            ModelChoice::Installed("ggml-large-v3.bin".into())
        );
        let elsewhere = folder.path().join("ggml-tiny.bin");
        std::fs::write(&elsewhere, []).unwrap();
        workspace.choice = ModelChoice::File(elsewhere.display().to_string());
        workspace.resolve_choice();
        assert_eq!(Path::new(&workspace.model), elsewhere);
    }

    #[test]
    fn a_large_model_that_fails_under_automatique_is_retried_once_smaller() {
        let (_folder, mut workspace) = bundle(&["ggml-large-v3.bin", "ggml-small.bin"]);
        workspace.vram = Some(Some(8 << 30));
        workspace.resolve_choice();
        assert_eq!(file(&workspace), "ggml-large-v3.bin");
        // Only a transcriber failure counts: not a bad input, not a timeout.
        assert!(!workspace.fall_back("error.subtitle_input"));
        assert!(!workspace.fall_back("error.subtitle_limit"));
        assert!(workspace.fall_back("error.subtitle_backend"));
        assert_eq!(file(&workspace), "ggml-small.bin");
        let retry = workspace.retry.clone().unwrap();
        assert_eq!(retry.key, "lyrics.retry_smaller");
        assert_eq!(retry.args, ["large-v3", "small"]);
        // Small failing too is not retried again.
        assert!(!workspace.fall_back("error.subtitle_backend"));
        // A model chosen by hand is never swapped.
        let (_folder, mut workspace) = bundle(&["ggml-large-v3.bin", "ggml-small.bin"]);
        workspace.choice = ModelChoice::Installed("ggml-large-v3.bin".into());
        workspace.resolve_choice();
        assert!(!workspace.fall_back("error.subtitle_backend"));
        assert_eq!(file(&workspace), "ggml-large-v3.bin");
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
