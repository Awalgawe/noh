//! The motion section of the style panel (motion overlay plan § 5.2, M4,
//! mockup v2). A scene written beside the song (`<song>.noh-motion.rhai`) is
//! found and listed, never run on its own. One click checks it in the worker,
//! every frame of the song at the montage's frame, and uses it if it passes;
//! refused, it says why and cannot be chosen. A used scene draws the lyrics
//! itself: the styles are set aside until one is applied again (scene or
//! style, not both). Once the user has asked for it, a scene whose file or
//! lyrics change is checked again on its own.
//!
//! The window prepares a used scene only after the worker has passed it: its
//! own validation, on its script thread (`app_motion_draw`), then draws it over
//! the monitor at the export's size, on the song's clock. Nothing is kept from
//! one session to the next: the scene comes back because the file is there.
use super::NohApp;
use super::app_job::Job;
use eframe::egui;
use noh::{engine::Event, safe_area::SafeArea};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::{Duration, Instant, SystemTime},
};

/// The file is looked at this often (as the song and a followed scene).
const POLL: Duration = Duration::from_secs(2);
/// What a scene file beside the song is called after the song's stem.
pub const SUFFIX: &str = ".noh-motion.rhai";

/// A scene's identity as the poll, the check and the window compare it: the
/// scene file, then each file of its shaders folder (name, time, length),
/// sorted. None: no scene file.
pub(super) type Stamp = Option<Vec<(String, SystemTime, u64)>>;

/// The scene a song would have beside it.
pub fn beside(song: &Path) -> Option<PathBuf> {
    let stem = song.file_stem()?.to_string_lossy();
    Some(song.with_file_name(format!("{stem}{SUFFIX}")))
}

pub(super) fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    m.is_file().then_some(())?;
    let mut files = vec![(String::new(), m.modified().ok()?, m.len())];
    // The shaders beside it (`noh::fx::layer_shader::read_beside`'s folder).
    if let Ok(dir) = std::fs::read_dir(path.with_extension("shaders")) {
        let mut shaders: Vec<_> = dir
            .filter_map(|e| {
                let e = e.ok()?;
                let m = e.metadata().ok()?;
                Some((
                    e.file_name().to_string_lossy().into_owned(),
                    m.modified().ok()?,
                    m.len(),
                ))
            })
            .collect();
        shaders.sort();
        files.extend(shaders);
    }
    Some(files)
}

/// What a check was made for: any change is another check.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Inputs {
    pub stamp: Stamp,
    /// The lyrics' revision.
    pub track: u64,
    pub size: (u32, u32),
    pub fps: f64,
    pub safe: SafeArea,
    pub wav: PathBuf,
}

/// What a passed check measured (its `report.json`), for the details.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Checked {
    pub canvas: (u32, u32),
    pub fps: f64,
    pub seconds: f64,
    pub script_ms: f64,
    /// The whole frame, script, painting and GPU (None without OpenGL).
    pub frame_ms: Option<f64>,
    /// (at most, limit)
    pub ops: (u64, u64),
    pub particles: (u64, u64),
    pub passes: (u64, u64),
    pub shader_layers: u64,
    pub warnings: Vec<String>,
}

impl Checked {
    fn from_report(v: &serde_json::Value) -> Self {
        let n = |p: &str| {
            v.pointer(p)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        };
        let f = |p: &str| v.pointer(p).and_then(serde_json::Value::as_f64);
        let interval = (
            f("/interval_s/0").unwrap_or(0.0),
            f("/interval_s/1").unwrap_or(0.0),
        );
        let mut warnings: Vec<String> = Vec::new();
        for key in ["/warnings", "/layout_warnings"] {
            if let Some(list) = v.pointer(key).and_then(serde_json::Value::as_array) {
                warnings.extend(list.iter().filter_map(|w| w.as_str().map(str::to_string)));
            }
        }
        Self {
            canvas: (n("/canvas/0") as u32, n("/canvas/1") as u32),
            fps: f("/fps").unwrap_or(0.0),
            seconds: (interval.1 - interval.0).max(0.0),
            script_ms: f("/script_ms/p95").unwrap_or(0.0),
            frame_ms: f("/frame_ms/p95"),
            ops: (n("/max/ops"), n("/limits/ops")),
            particles: (n("/max/particles"), n("/limits/particles")),
            passes: (n("/max/passes"), n("/limits/passes")),
            shader_layers: n("/max/shader_layers"),
            warnings,
        }
    }
}

pub(super) enum Status {
    /// Listed, not checked: nothing of it has run.
    Found,
    Checking {
        job: Job,
        /// The check's folder is written in here, then read and dropped.
        out: tempfile::TempDir,
        inputs: Inputs,
        percent: u32,
        /// The version that passed before (a new version is checked): a used
        /// scene keeps drawing it until this check decides.
        previous: Option<(Inputs, Checked)>,
    },
    Usable {
        inputs: Inputs,
        checked: Checked,
    },
    Refused {
        inputs: Inputs,
        reason: String,
    },
}

impl Found {
    /// The version that last passed its check, and what it measured.
    pub fn passed(&self) -> Option<(&Inputs, &Checked)> {
        match &self.status {
            Status::Usable { inputs, checked } => Some((inputs, checked)),
            Status::Checking {
                previous: Some((inputs, checked)),
                ..
            } => Some((inputs, checked)),
            _ => None,
        }
    }
}

/// The scene beside the song.
pub(super) struct Found {
    pub path: PathBuf,
    pub stamp: Stamp,
    pub status: Status,
    /// The user asked for this scene to be checked (Q3: from then on, until
    /// NOH closes, its new versions are checked on their own).
    pub asked: bool,
}

#[derive(Default)]
pub(super) struct State {
    pub found: Option<Found>,
    /// The scene draws instead of the styles (exports and preview).
    pub used: bool,
    /// Use the scene as soon as the check under way passes.
    pub use_after: bool,
    pub details: bool,
    /// Draw the scene's shader layers in the preview (plan § 3.4, Q2 (c)).
    pub show_shaders: bool,
    song: Option<PathBuf>,
    polled: Option<Instant>,
    poll: Option<mpsc::Receiver<Stamp>>,
    pub(super) draw: super::app_motion_draw::Preview,
    /// Scroll the style panel to the section once (a scene just found, or a
    /// verdict): it lies under the styles.
    reveal: bool,
    /// Capture only: the check was clicked.
    capture_clicked: bool,
    capture_ready_at: Option<Instant>,
}

impl State {
    /// The scene in use, once its check passed (also while a new version is
    /// checked: the export validates whatever it draws).
    pub fn used_scene(&self) -> Option<&Path> {
        let f = self.found.as_ref()?;
        (self.used && f.passed().is_some()).then_some(f.path.as_path())
    }

    /// Its name as listed: the file's.
    pub fn name(&self) -> Option<String> {
        let f = self.found.as_ref()?;
        Some(f.path.file_name()?.to_string_lossy().into_owned())
    }
}

impl NohApp {
    /// What a check of the scene would be made for now: None while the song,
    /// its length, the lyrics or the montage's frame are not known yet.
    pub(super) fn motion_inputs(&self, stamp: Stamp) -> Option<Inputs> {
        let wav = self.wav.clone()?;
        self.wav_seconds?;
        if !self.has_lyrics() || !self.lyrics_ready() {
            return None;
        }
        // The montage's frame and rate from its exact analysis: the quick read
        // of the clips does not know their rate.
        let target = self
            .diagnosis
            .as_ref()
            .filter(|_| self.diagnosis_current())
            .and_then(|d| d.plan.as_ref())
            .map(|p| &p.target)?;
        let fps = target.rate_num as f64 / target.rate_den.max(1) as f64;
        if !(1.0..=240.0).contains(&fps) {
            return None;
        }
        let (w, h) = (target.width, target.height);
        Some(Inputs {
            stamp,
            track: self
                .project
                .track
                .revision
                .wrapping_add(self.project.edit_revision),
            size: (w, h),
            fps,
            safe: self.project.effective_caption_style(false).safe_area,
            wav,
        })
    }

    /// Each frame: the file beside the song, the check under way, and a new
    /// check when what the last one was made for has changed (once asked).
    pub(super) fn motion_frame(&mut self, ctx: &egui::Context) {
        if !cfg!(feature = "motion") {
            return;
        }
        // Another song, or none: another scene, or none, and the styles again.
        if self.motion.song != self.wav {
            self.motion.song = self.wav.clone();
            self.motion_forget();
        }
        let Some(song) = self.wav.clone() else {
            return;
        };
        let path = beside(&song).unwrap_or_default();
        if let Some(rx) = &self.motion.poll {
            match rx.try_recv() {
                Ok(now) => {
                    self.motion.poll = None;
                    self.motion_seen(path.clone(), now);
                }
                Err(mpsc::TryRecvError::Disconnected) => self.motion.poll = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.motion.poll.is_none() && self.motion.polled.is_none_or(|t| t.elapsed() >= POLL) {
            self.motion.polled = Some(Instant::now());
            let (tx, rx) = mpsc::sync_channel(1);
            self.motion.poll = Some(rx);
            let (path, ctx) = (path.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(stamp(&path));
                ctx.request_repaint();
            });
        }
        ctx.request_repaint_after(POLL);
        self.motion_poll_check(ctx);
        // Asked once: a new version of the file, of the lyrics or of the frame
        // is checked on its own.
        let again = self.motion.found.as_ref().is_some_and(|f| {
            f.asked
                && match &f.status {
                    Status::Usable { inputs, .. } | Status::Refused { inputs, .. } => self
                        .motion_inputs(f.stamp.clone())
                        .is_some_and(|now| now != *inputs),
                    _ => false,
                }
        });
        if again {
            self.motion_check(ctx, false);
        }
        // The previews carry no captions while a scene draws them, as with an
        // effect: from libass to a scene (or back), they are made again.
        let used = self.motion.used_scene().is_some();
        if self.project.scene_used != used {
            let before = self.project.stamp(self.revision, false).effect;
            self.project.scene_used = used;
            if self.project.stamp(self.revision, false).effect != before {
                self.invalidated();
            }
        }
        self.motion_draw_frame(ctx);
    }

    /// The scene and everything about it are forgotten (another song).
    fn motion_forget(&mut self) {
        if let Some(Found {
            status: Status::Checking { job, .. },
            ..
        }) = &self.motion.found
        {
            job.cancel();
        }
        let was_used = self.motion.used;
        self.motion.found = None;
        self.motion.used = false;
        self.motion.use_after = false;
        self.motion.details = false;
        self.motion.draw.release();
        if was_used {
            self.outdate_exports();
        }
    }

    /// What the poll saw beside the song.
    fn motion_seen(&mut self, path: PathBuf, now: Stamp) {
        match (&mut self.motion.found, &now) {
            (_, None) => {
                if self.motion.found.is_some() {
                    self.motion_forget();
                }
            }
            (Some(f), Some(_)) if f.path == path => f.stamp = now,
            (_, Some(_)) => {
                self.motion_forget();
                self.motion.found = Some(Found {
                    path,
                    stamp: now,
                    status: Status::Found,
                    asked: false,
                });
                self.motion.reveal = true;
            }
        }
    }

    /// « Vérifier et employer » (`then_use`), « Vérifier à nouveau », or a
    /// new version checked on its own: the check starts in the worker.
    pub(super) fn motion_check(&mut self, ctx: &egui::Context, then_use: bool) {
        let Some(f) = &self.motion.found else {
            return;
        };
        let Some(inputs) = self.motion_inputs(f.stamp.clone()) else {
            return;
        };
        let Some((subtitles, _)) = self.captions_file() else {
            return;
        };
        let Ok(out) = tempfile::Builder::new()
            .prefix("noh-scene-check-")
            .tempdir()
        else {
            return;
        };
        if let Status::Checking { job, .. } = &f.status {
            job.cancel();
        }
        let previous = f.passed().map(|(i, c)| (i.clone(), c.clone()));
        let request = noh::scene_request::SceneRequest {
            scene: f.path.clone(),
            source: inputs.wav.clone(),
            subtitles,
            ffmpeg: self.ffmpeg.clone().unwrap_or_default(),
            safe_area: inputs.safe,
            from_ms: None,
            to_ms: None,
            mode: noh::scene_request::SceneMode::Check,
            vertical: false,
            bare: false,
            grid: false,
            output: out.path().join("check"),
            project: Some(noh::scene_request::SceneProject {
                width: inputs.size.0,
                height: inputs.size.1,
                fps: inputs.fps,
            }),
        };
        let repaint = ctx.clone();
        let job = Job::scene_with_cancel(
            request,
            self.worker.clone(),
            Arc::new(AtomicBool::new(false)),
            move || repaint.request_repaint(),
        );
        self.motion.use_after |= then_use;
        let f = self.motion.found.as_mut().unwrap();
        f.asked = true;
        f.status = Status::Checking {
            job,
            out,
            inputs,
            percent: 0,
            previous,
        };
    }

    /// The check under way: its progress, then its verdict.
    fn motion_poll_check(&mut self, ctx: &egui::Context) {
        let Some(f) = &mut self.motion.found else {
            return;
        };
        let Status::Checking { job, percent, .. } = &mut f.status else {
            return;
        };
        let mut done = None;
        while let Ok(event) = job.events.try_recv() {
            match event {
                Event::Progress { percent: p, .. } => *percent = p,
                Event::Done(result) => done = Some(result.map_err(|e| e.detail.to_string())),
                Event::Cancelled => done = Some(Err(String::new())),
                _ => {}
            }
        }
        let Some(result) = done else {
            return;
        };
        let Status::Checking { out, inputs, .. } = std::mem::replace(&mut f.status, Status::Found)
        else {
            unreachable!()
        };
        let report = result.and_then(|_| {
            let bytes = std::fs::read(out.path().join("check").join("report.json"))
                .map_err(|e| e.to_string())?;
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())
        });
        let was_used = self.motion.used;
        self.motion.reveal = true;
        match report {
            Ok(report) => {
                f.status = Status::Usable {
                    inputs,
                    checked: Checked::from_report(&report),
                };
                if std::mem::take(&mut self.motion.use_after) {
                    self.motion_use(true);
                } else if was_used {
                    // A new version of the scene in use: the exports were made
                    // with the previous one.
                    self.outdate_exports();
                }
            }
            // Cancelled for a newer check: that one decides.
            Err(reason) if reason.is_empty() => {}
            Err(reason) => {
                f.status = Status::Refused { inputs, reason };
                self.motion.use_after = false;
                if was_used {
                    self.motion_use(false);
                }
            }
        }
        ctx.request_repaint();
    }

    /// Use the scene instead of the styles, or the styles again.
    pub(super) fn motion_use(&mut self, on: bool) {
        if self.motion.used == on {
            return;
        }
        self.motion.used = on;
        if on {
            self.end_try();
        } else {
            self.motion.draw.release();
        }
        self.outdate_exports();
    }

    /// The scene a project export draws: the used one, with the captions on.
    pub(super) fn motion_request(&self) -> Option<noh::scene_request::ProjectScene> {
        self.motion
            .used_scene()
            .filter(|_| self.project.apply_subtitles)
            .map(|scene| noh::scene_request::ProjectScene {
                scene: scene.to_path_buf(),
            })
    }

    /// The section under the style cards.
    pub(super) fn motion_section(&mut self, ui: &mut egui::Ui) {
        use super::app_style::{self as style, space};
        if !cfg!(feature = "motion") {
            return;
        }
        let Some(f) = &self.motion.found else {
            return;
        };
        let l = self.locale.language;
        let t = style::tokens(ui);
        let name = f
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let ready = self.motion_inputs(f.stamp.clone()).is_some();
        ui.add_space(space::S);
        let (line, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(line, 0.0, t.border);
        let top = line.top();
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.label(style::strong(l.text("motion.title"), style::text::BODY).color(t.text));
            experimental_tag(ui, l.text("motion.experimental"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(l.text("motion.beside"))
                        .size(style::text::SMALL)
                        .color(t.text_3),
                );
            });
        });
        let mut clicked_card = false;
        let mut check = false;
        let mut details = false;
        let mut open_folder = false;
        let enabled = self.project.apply_subtitles || !self.has_lyrics();
        match &f.status {
            Status::Found | Status::Checking { previous: None, .. } => {
                let checking = match &f.status {
                    Status::Checking { percent, .. } => Some(*percent),
                    _ => None,
                };
                dashed_card(ui, |ui| {
                    file_tile(ui);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.label(style::strong(&name, style::text::BODY).color(t.text));
                        ui.label(
                            egui::RichText::new(l.text("motion.not_run"))
                                .size(style::text::SMALL)
                                .color(t.text_2),
                        );
                        let label = if checking.is_some() {
                            l.text("motion.checking")
                        } else {
                            l.text("motion.check_and_use")
                        };
                        if small_button(ui, label, checking.is_none() && ready && enabled).clicked()
                        {
                            check = true;
                        }
                        if !ready {
                            ui.label(
                                egui::RichText::new(l.text("motion.waiting"))
                                    .size(style::text::SMALL)
                                    .color(t.text_3),
                            );
                        }
                    });
                });
                if let Some(percent) = checking {
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1.0, t.border))
                        .corner_radius(style::radius::L)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                egui::RichText::new(l.text("motion.check_note"))
                                    .size(style::text::SMALL)
                                    .color(t.text_2),
                            );
                            progress(ui, percent);
                        });
                }
            }
            Status::Usable { checked, .. }
            | Status::Checking {
                previous: Some((_, checked)),
                ..
            } => {
                let used = self.motion.used;
                let response = scene_card(ui, &name, used, enabled, checked, l);
                if response.clicked() {
                    clicked_card = true;
                }
                // A new version is being checked: the one that passed stays.
                if let Status::Checking { percent, .. } = &f.status {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(l.text("motion.checking"))
                                .size(style::text::SMALL)
                                .color(t.text_2),
                        );
                        progress(ui, *percent);
                    });
                }
                ui.horizontal(|ui| {
                    let note = if used {
                        l.text("motion.back_to_styles")
                    } else {
                        l.text("motion.click_to_use")
                    };
                    ui.label(
                        egui::RichText::new(note)
                            .size(style::text::SMALL)
                            .color(t.text_3),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if small_button(ui, l.text("motion.details"), true).clicked() {
                            details = true;
                        }
                    });
                });
            }
            Status::Refused { reason, .. } => {
                egui::Frame::new()
                    .fill(t.error_soft)
                    .stroke(egui::Stroke::new(2.0, t.error))
                    .corner_radius(style::radius::L)
                    .inner_margin(7.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            let (icon, _) = ui
                                .allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                            super::app_icons::paint(
                                ui.painter(),
                                icon,
                                super::app_icons::Icon::Error,
                                t.error,
                            );
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(
                                    style::strong(
                                        l.format("motion.refused", std::slice::from_ref(&name)),
                                        style::text::BODY,
                                    )
                                    .color(t.text),
                                );
                                ui.label(
                                    egui::RichText::new(said(reason, l))
                                        .size(style::text::SMALL)
                                        .color(t.error),
                                );
                                ui.label(
                                    egui::RichText::new(l.text("motion.refused_help"))
                                        .size(style::text::SMALL)
                                        .color(t.text_2),
                                );
                            });
                        });
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if small_button(ui, l.text("motion.details"), true).clicked() {
                        details = true;
                    }
                    if small_button(ui, l.text("motion.open_folder"), true).clicked() {
                        open_folder = true;
                    }
                });
            }
        }
        if std::mem::take(&mut self.motion.reveal) {
            // The whole section in view, the least scroll needed (its top
            // first when it is taller than the list).
            let bottom = ui.min_rect().bottom();
            let section = egui::Rect::from_x_y_ranges(ui.min_rect().x_range(), top..=bottom);
            ui.scroll_to_rect(section, None);
        }
        if check {
            self.motion_check(ui.ctx(), true);
        }
        if clicked_card {
            let on = !self.motion.used;
            self.motion_use(on);
        }
        if details {
            self.motion.details = true;
        }
        if open_folder {
            self.motion_open_folder(ui.ctx());
        }
    }

    fn motion_open_folder(&mut self, ctx: &egui::Context) {
        if let Some(dir) = self
            .motion
            .found
            .as_ref()
            .and_then(|f| f.path.parent().map(Path::to_path_buf))
        {
            self.open(&dir, ctx);
        }
    }

    /// « Détails »: the file, what was checked, what it costs here, its
    /// quantities against the caps, its warnings. No editor: the folder opens.
    pub(super) fn motion_details(&mut self, ctx: &egui::Context) {
        use super::app_style::{self as style, space};
        if !self.motion.details {
            return;
        }
        let Some(f) = &self.motion.found else {
            self.motion.details = false;
            return;
        };
        let l = self.locale.language;
        let file = f
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let (checked, reason) = match &f.status {
            Status::Refused { reason, .. } => (None, Some(reason.clone())),
            _ => (f.passed().map(|(_, c)| c.clone()), None),
        };
        let mut open = true;
        let mut again = false;
        let mut folder = false;
        let shaders_hidden = self.motion.draw.hidden_shaders() > 0
            || checked.as_ref().is_some_and(|c| c.shader_layers > 0);
        let mut show_shaders = self.motion.show_shaders;
        egui::Window::new(file.clone())
            .id(egui::Id::new("motion-details"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-24.0, 132.0))
            .default_width(420.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let t = style::tokens(ui);
                ui.set_width(388.0);
                ui.spacing_mut().item_spacing.y = space::S;
                ui.horizontal(|ui| {
                    ui.label(
                        style::strong(l.text("motion.title"), style::text::HEADING).color(t.text),
                    );
                    experimental_tag(ui, l.text("motion.experimental"));
                });
                egui::Grid::new("motion-details-grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .min_col_width(110.0)
                    .show(ui, |ui| {
                        let key = |ui: &mut egui::Ui, k: &str| {
                            ui.label(egui::RichText::new(l.text(k)).color(t.text_2));
                        };
                        key(ui, "motion.d_file");
                        ui.label(file.clone());
                        ui.end_row();
                        if let Some(c) = &checked {
                            key(ui, "motion.d_checked");
                            ui.label(l.format(
                                "motion.d_checked_on",
                                &[
                                    clock(c.seconds),
                                    format!("{}", c.fps.round()),
                                    format!("{} × {}", c.canvas.0, c.canvas.1),
                                ],
                            ));
                            ui.end_row();
                            key(ui, "motion.d_measured");
                            ui.vertical(|ui| {
                                ui.label(l.format("motion.d_script", &[ms(c.script_ms)]));
                                match c.frame_ms {
                                    Some(v) => ui.label(l.format("motion.d_frame", &[ms(v)])),
                                    None => ui.label(l.text("motion.d_frame_missing")),
                                };
                                ui.label(
                                    egui::RichText::new(l.text("motion.d_info_only"))
                                        .size(style::text::SMALL)
                                        .color(t.text_3),
                                );
                            });
                            ui.end_row();
                            key(ui, "motion.d_peak");
                            ui.vertical(|ui| {
                                for (k, (v, max)) in [
                                    ("motion.d_ops", c.ops),
                                    ("motion.d_particles", c.particles),
                                    ("motion.d_passes", c.passes),
                                ] {
                                    ui.label(l.format(k, &[v.to_string(), max.to_string()]));
                                }
                            });
                            ui.end_row();
                        }
                        if let Some(r) = &reason {
                            key(ui, "motion.d_refused");
                            ui.label(egui::RichText::new(said(r, l)).color(t.error));
                            ui.end_row();
                        }
                    });
                if let Some(c) = &checked {
                    for w in c.warnings.iter().take(3) {
                        egui::Frame::new()
                            .fill(t.warn_soft)
                            .corner_radius(style::radius::M)
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    egui::RichText::new(w)
                                        .size(style::text::SMALL)
                                        .color(t.warn),
                                );
                            });
                    }
                }
                if shaders_hidden {
                    ui.checkbox(&mut show_shaders, l.text("motion.show_shaders"));
                }
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if style::button(ui, l.text("motion.check_again"), true, false, 0.0)
                            .clicked()
                        {
                            again = true;
                        }
                        if style::button(ui, l.text("motion.open_folder"), true, false, 0.0)
                            .clicked()
                        {
                            folder = true;
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(l.text("motion.d_footnote"))
                        .size(style::text::SMALL)
                        .color(t.text_3),
                );
            });
        self.motion.show_shaders = show_shaders;
        if again {
            let used = self.motion.used;
            self.motion_check(ctx, used);
        }
        if folder {
            self.motion_open_folder(ctx);
        }
        if !open {
            self.motion.details = false;
        }
    }

    /// Capture only (`motion=` tweak).
    pub(super) fn capture_motion(&self) -> Option<String> {
        super::app_capture::parse_tweaks(&super::app_capture::tweaks_text())
            .ok()?
            .motion
    }

    /// Capture only: click « Vérifier et employer » once the scene can be
    /// checked; open the details once it is used.
    pub(super) fn motion_capture_step(&mut self, ctx: &egui::Context) {
        let Some(motion) = self.capture_motion() else {
            return;
        };
        let ready = self
            .motion
            .found
            .as_ref()
            .is_some_and(|f| self.motion_inputs(f.stamp.clone()).is_some());
        if motion != "found" && !self.motion.capture_clicked && ready {
            self.motion.capture_clicked = true;
            self.motion_check(ctx, true);
        }
        if motion == "details" && self.motion.used_scene().is_some() {
            self.motion.details = true;
        }
    }

    /// Capture only: the state the tweak asks for is on screen (a used scene:
    /// drawn for a second).
    pub(super) fn motion_capture_ready(&mut self, motion: &str) -> bool {
        let Some(f) = &self.motion.found else {
            return false;
        };
        let now = match (motion, &f.status) {
            ("found", Status::Found) => true,
            ("checking", Status::Checking { percent, .. }) => *percent >= 5,
            ("refused", Status::Refused { .. }) => true,
            ("use" | "details", Status::Usable { .. }) => self.motion.draw.drawing(),
            _ => false,
        };
        if !now {
            self.motion.capture_ready_at = None;
            return false;
        }
        let at = *self
            .motion
            .capture_ready_at
            .get_or_insert_with(Instant::now);
        at.elapsed() >= Duration::from_secs(1)
    }

    /// The exports already made were made with another overlay.
    pub(super) fn outdate_exports(&mut self) {
        if let Some(a) = &mut self.export_artifact {
            a.revision = a.revision.wrapping_sub(1);
        }
        for a in [
            &mut self.project.full_export_artifact,
            &mut self.project.short_export_artifact,
        ]
        .into_iter()
        .flatten()
        {
            a.stamp.revision = a.stamp.revision.wrapping_sub(1);
        }
    }
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The instant a refusal names (`… at t=72.420: …`), and what follows it.
fn refusal_at(reason: &str) -> Option<(f64, &str)> {
    let i = reason.find("at t=")?;
    let rest = &reason[i + 5..];
    let (number, after) = rest.split_once(": ")?;
    let t: f64 = number.parse().ok()?;
    let after = after.strip_prefix("Runtime error: ").unwrap_or(after);
    Some((t, after))
}

/// A refusal as the panel says it: the instant in the user's language (to the
/// tenth), the engine's reason after it (English, as the agents read it).
fn said(reason: &str, l: noh::i18n::Language) -> String {
    match refusal_at(reason) {
        Some((t, after)) => {
            let tenths = (t.max(0.0) * 10.0).round() as u64;
            let at = format!("{}:{:02},{}", tenths / 600, tenths / 10 % 60, tenths % 10);
            l.format("motion.refused_at", &[at, after.to_string()])
        }
        None => reason.to_string(),
    }
}

fn ms(v: f64) -> String {
    format!("{v:.1}")
}

fn experimental_tag(ui: &mut egui::Ui, text: &str) {
    let t = super::app_style::tokens(ui);
    let galley = ui.painter().layout_no_wrap(
        text.into(),
        super::app_style::semibold(super::app_style::text::SMALL),
        t.warn,
    );
    let (rect, _) =
        ui.allocate_exact_size(galley.size() + egui::vec2(12.0, 2.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, t.warn_soft);
    ui.painter()
        .galley(rect.min + egui::vec2(6.0, 1.0), galley, t.warn);
}

/// The check's bar. The worker reports no step while it validates the frames
/// (5 % to the end): a segment then runs along the bar, so the check does not
/// look stuck.
fn progress(ui: &mut egui::Ui, percent: u32) {
    let t = super::app_style::tokens(ui);
    let (bar, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, 2.0, t.surface_3);
    let mut done = bar;
    if percent <= 5 {
        let phase = (ui.input(|i| i.time) * 0.6).fract() as f32;
        let w = bar.width() * 0.25;
        let x = bar.left() + (bar.width() + w) * phase - w;
        done = egui::Rect::from_x_y_ranges(
            x.max(bar.left())..=(x + w).min(bar.right()),
            bar.y_range(),
        );
        ui.ctx().request_repaint();
    } else {
        done.set_width(bar.width() * percent.min(100) as f32 / 100.0);
    }
    ui.painter().rect_filled(done, 2.0, t.accent);
}

fn small_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    super::app_style::button_kind(
        ui,
        label,
        enabled,
        super::app_style::Kind::Secondary,
        0.0,
        28.0,
    )
}

/// The found scene's card: a dashed outline, nothing run yet.
fn dashed_card(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    let t = super::app_style::tokens(ui);
    let frame = egui::Frame::new()
        .corner_radius(super::app_style::radius::L)
        .inner_margin(8.0);
    let response = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            body(ui)
        });
    });
    dash(ui.painter(), response.response.rect, t.text_3);
}

fn dash(p: &egui::Painter, r: egui::Rect, color: egui::Color32) {
    let r = r.shrink(0.5);
    let stroke = egui::Stroke::new(1.0, color);
    for (a, b) in [
        (r.left_top(), r.right_top()),
        (r.right_top(), r.right_bottom()),
        (r.right_bottom(), r.left_bottom()),
        (r.left_bottom(), r.left_top()),
    ] {
        p.extend(egui::Shape::dashed_line(&[a, b], stroke, 4.0, 3.0));
    }
}

/// A file icon in a 72 × 40 tile.
fn file_tile(ui: &mut egui::Ui) {
    let t = super::app_style::tokens(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(72.0, 40.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, t.surface_2);
    let c = rect.center();
    let stroke = egui::Stroke::new(1.5, t.text_3);
    let pts = [
        c + egui::vec2(-5.0, -9.0),
        c + egui::vec2(2.0, -9.0),
        c + egui::vec2(7.0, -4.0),
        c + egui::vec2(7.0, 9.0),
        c + egui::vec2(-5.0, 9.0),
        c + egui::vec2(-5.0, -9.0),
    ];
    ui.painter().add(egui::Shape::line(pts.to_vec(), stroke));
    ui.painter().add(egui::Shape::line(
        vec![
            c + egui::vec2(2.0, -9.0),
            c + egui::vec2(2.0, -4.0),
            c + egui::vec2(7.0, -4.0),
        ],
        stroke,
    ));
}

/// The usable scene's card: a click uses it, or gives the styles back.
fn scene_card(
    ui: &mut egui::Ui,
    name: &str,
    used: bool,
    enabled: bool,
    _checked: &Checked,
    l: noh::i18n::Language,
) -> egui::Response {
    let t = super::app_style::tokens(ui);
    let width = ui.available_width();
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 66.0), sense);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, enabled, used, name)
    });
    let p = ui.painter();
    let stroke = if used {
        egui::Stroke::new(2.0, t.text)
    } else if response.hovered() {
        egui::Stroke::new(1.0, t.border_strong)
    } else {
        egui::Stroke::new(1.0, t.border)
    };
    p.rect_stroke(
        rect,
        super::app_style::radius::L,
        stroke,
        egui::StrokeKind::Inside,
    );
    let tile = egui::Rect::from_min_size(rect.min + egui::vec2(8.0, 13.0), egui::vec2(72.0, 40.0));
    p.rect_filled(tile, 4.0, egui::Color32::from_rgb(0x1B, 0x1E, 0x24));
    let label = egui::pos2(tile.right() + 10.0, rect.top() + 9.0);
    let mut job = egui::text::LayoutJob::simple(
        name.to_string(),
        super::app_style::semibold(super::app_style::text::BODY),
        t.text,
        (rect.right() - label.x - 70.0).max(20.0),
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    p.galley(label, p.layout_job(job), t.text);
    if used {
        let g = p.layout_no_wrap(
            l.text("style.used").into(),
            egui::FontId::proportional(super::app_style::text::SMALL),
            t.ok,
        );
        p.galley(
            egui::pos2(rect.right() - 10.0 - g.size().x, rect.top() + 11.0),
            g,
            t.ok,
        );
    }
    let mut job = egui::text::LayoutJob::simple(
        l.text("motion.draws_lyrics").to_string(),
        egui::FontId::proportional(super::app_style::text::SMALL),
        t.text_2,
        (rect.right() - label.x - 10.0).max(20.0),
    );
    job.wrap.max_rows = 2;
    p.galley(
        egui::pos2(label.x, rect.top() + 29.0),
        p.layout_job(job),
        t.text_2,
    );
    super::app_style::focus(ui, &response);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scene_is_named_after_the_song() {
        assert_eq!(
            beside(Path::new("C:/a/song.wav")),
            Some(PathBuf::from("C:/a/song.noh-motion.rhai"))
        );
    }

    #[test]
    fn a_check_report_gives_the_details() {
        let report = serde_json::json!({
            "canvas": [1920, 1080], "fps": 24.0, "interval_s": [0.0, 225.0],
            "max": {"ops": 1781, "particles": 485, "passes": 3, "shader_layers": 0},
            "limits": {"ops": 6000, "particles": 4000, "passes": 8},
            "script_ms": {"p95": 0.4}, "frame_ms": {"p95": 5.4},
            "warnings": ["a"], "layout_warnings": ["b"],
        });
        let c = Checked::from_report(&report);
        assert_eq!(c.canvas, (1920, 1080));
        assert_eq!(c.ops, (1781, 6000));
        assert_eq!(c.particles, (485, 4000));
        assert_eq!(c.passes, (3, 8));
        assert_eq!(c.frame_ms, Some(5.4));
        assert_eq!(c.seconds, 225.0);
        assert_eq!(c.warnings, ["a", "b"]);
        assert_eq!(clock(c.seconds), "3:45");
    }

    #[test]
    fn a_refusal_names_its_instant() {
        let why = "script at t=72.420: Runtime error: more than 256 draws in one frame (line 5, position 21)";
        assert_eq!(
            refusal_at(why),
            Some((
                72.42,
                "more than 256 draws in one frame (line 5, position 21)"
            ))
        );
        assert_eq!(
            said(why, noh::i18n::Language::Fr),
            "À 1:12,4 : more than 256 draws in one frame (line 5, position 21)"
        );
        assert_eq!(refusal_at("the scene is not UTF-8 text"), None);
    }
}
