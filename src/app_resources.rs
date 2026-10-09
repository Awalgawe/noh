//! One coalesced availability check; filesystem/subprocess work never runs in a GUI frame.
use super::*;
use noh::resources::{Component, Inputs, Issue};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::thread::JoinHandle;

#[derive(Default)]
pub struct State {
    last: Option<Inputs>,
    desired: Option<Inputs>,
    changed: Option<Instant>,
    active: Option<(
        Inputs,
        Arc<AtomicBool>,
        Receiver<Vec<Issue>>,
        JoinHandle<()>,
    )>,
    pub issues: Vec<Issue>,
    pub preview: Option<Issue>,
    pub retry_media: bool,
    #[cfg(windows)]
    installer: Option<Receiver<std::result::Result<(), String>>>,
    #[cfg(windows)]
    installer_result: Option<std::result::Result<(), String>>,
}
impl State {
    pub fn installing(&self) -> bool {
        #[cfg(windows)]
        {
            self.installer.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    pub fn invalidate(&mut self) {
        self.last = None;
        if let Some((_, cancel, _, _)) = &self.active {
            cancel.store(true, Ordering::Release);
        }
    }
    pub fn sync(&mut self, inputs: Inputs, ctx: &egui::Context) {
        if self.desired.as_ref() != Some(&inputs) {
            self.desired = Some(inputs.clone());
            self.changed = Some(Instant::now());
            self.issues.clear();
            if let Some((_, cancel, _, _)) = &self.active {
                cancel.store(true, Ordering::Release);
            }
        }
        if let Some((checked, cancel, receiver, worker)) = &self.active
            && worker.is_finished()
        {
            match receiver.try_recv() {
                Ok(issues) => {
                    if checked == &inputs && !cancel.load(Ordering::Acquire) {
                        if self
                            .issues
                            .iter()
                            .any(|issue| issue.component == Component::Ffmpeg)
                            && !issues
                                .iter()
                                .any(|issue| issue.component == Component::Ffmpeg)
                        {
                            self.retry_media = true;
                        }
                        self.issues = issues;
                        self.last = Some(checked.clone());
                    }
                    if let Some((_, _, _, worker)) = self.active.take() {
                        let _ = worker.join();
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if checked == &inputs && !cancel.load(Ordering::Acquire) {
                        self.last = Some(inputs.clone());
                        self.issues = vec![Issue {
                            component: Component::Speech,
                            path: None,
                            detail: "Availability worker could not finish; retry the check.".into(),
                        }];
                    }
                    if let Some((_, _, _, worker)) = self.active.take() {
                        let _ = worker.join();
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.active.is_none()
            && self.last.as_ref() != Some(&inputs)
            && self
                .changed
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(250))
        {
            let cancel = Arc::new(AtomicBool::new(false));
            let stopping = cancel.clone();
            let check = inputs.clone();
            let repaint = ctx.clone();
            let (sender, receiver) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let issues = noh::resources::check(&check, &stopping);
                repaint.request_repaint();
                let _ = sender.send(issues);
            });
            self.active = Some((inputs, cancel, receiver, worker));
        }
        if self.last.as_ref() != self.desired.as_ref() || self.active.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    pub fn unavailable(&self) -> bool {
        !self.issues.is_empty() || self.preview.is_some()
    }
    pub fn ffmpeg_available(&self) -> bool {
        self.last.is_some()
            && self.last == self.desired
            && self.active.is_none()
            && !self
                .issues
                .iter()
                .any(|issue| issue.component == Component::Ffmpeg)
    }
    pub fn speech_issue(&self) -> Option<Message> {
        self.issues
            .iter()
            .find(|issue| {
                matches!(
                    issue.component,
                    Component::Speech | Component::Model | Component::Vad
                )
            })
            .map(|issue| issue.component.help_key().into())
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some((_, cancel, _, worker)) = self.active.take() {
            cancel.store(true, Ordering::Release);
            let _ = worker.join();
        }
    }
}

impl NohApp {
    pub(super) fn retry_resource_media(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        self.poll_component_installer();
        if self.resources.retry_media && self.resources.ffmpeg_available() && !self.engine_locked()
        {
            self.resources.retry_media = false;
            self.refresh_metadata(ctx);
        }
    }
    pub(super) fn resource_inputs(&self) -> Inputs {
        Inputs {
            ffmpeg: self.ffmpeg.clone(),
            speech: [
                self.subtitles.transcriber.clone().into(),
                self.subtitles.model.clone().into(),
                self.subtitles.vad_model.clone().into(),
            ],
        }
    }
    pub(super) fn resource_banner(&mut self, ui: &mut egui::Ui) {
        if !self.resources.unavailable() {
            return;
        }
        let l = self.locale.language;
        egui::Frame::new().inner_margin(12.0).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(l.text("resources.unavailable"));
                if ui.button(l.text("resources.resolve")).clicked() {
                    self.settings_open = true;
                }
            });
        });
    }
    pub(super) fn resource_settings(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        #[cfg(windows)]
        self.component_installer_controls(ui);
        if self.resources.last.as_ref() != self.resources.desired.as_ref()
            || self.resources.active.is_some()
        {
            ui.label(l.text("resources.checking"));
        }
        if !self.resources.unavailable()
            && self.resources.last.is_some()
            && self.resources.last == self.resources.desired
            && self.resources.active.is_none()
        {
            ui.label(l.text("resources.ready"));
        }
        for issue in self
            .resources
            .issues
            .iter()
            .chain(self.resources.preview.iter())
        {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(l.text(issue.component.help_key()))
                        .color(ui.visuals().warn_fg_color),
                )
                .wrap(),
            );
            if let Some(path) = &issue.path {
                ui.add(egui::Label::new(path.display().to_string()).wrap());
            }
            egui::CollapsingHeader::new(l.text("resources.details"))
                .id_salt(("resource-detail", issue.component as u8))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(&issue.detail).wrap());
                });
        }
        if ui
            .add_enabled(
                self.resources.active.is_none(),
                egui::Button::new(l.text("resources.recheck")),
            )
            .clicked()
        {
            self.resources.invalidate();
            self.resources.retry_media =
                self.clips.iter().any(|clip| clip.error.is_some()) || self.wav_error.is_some();
        }
        #[cfg(windows)]
        if self
            .resources
            .issues
            .iter()
            .any(|issue| issue.component == Component::Speech)
        {
            ui.hyperlink_to(
                l.text("resources.microsoft"),
                "https://aka.ms/vc14/vc_redist.x64.exe",
            );
        }
    }

    #[cfg(windows)]
    fn poll_component_installer(&mut self) {
        let Some(receiver) = &self.resources.installer else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Component installer worker stopped.".into())
            }
        };
        self.resources.installer = None;
        if result.is_ok() {
            if self.ffmpeg.is_none() {
                self.ffmpeg = noh::find_ffmpeg(None).ok();
            }
            if let Ok(exe) = std::env::current_exe()
                && let Some(folder) = exe.parent()
            {
                self.subtitles.load_bundled(folder);
            }
            self.resources.invalidate();
            self.resources.retry_media = true;
        }
        self.resources.installer_result = Some(result);
    }

    #[cfg(windows)]
    fn component_installer_controls(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        if self.resources.unavailable()
            && ui
                .add_enabled(
                    !self.engine_locked() && self.resources.installer.is_none(),
                    egui::Button::new(l.text("resources.install")),
                )
                .clicked()
        {
            let profile = if self.resources.speech_issue().is_some() {
                "complete"
            } else {
                "standard"
            };
            let (sender, receiver) = mpsc::channel();
            let repaint = ui.ctx().clone();
            let language = l.code();
            self.resources.installer_result = None;
            self.resources.installer = Some(receiver);
            // The same installed maintenance helper handles setup-time and later
            // additions. Its own window owns consent, progress and cancellation.
            // Do not join this worker on app close: the user may close NOH while
            // the ordinary installer finishes replacing an in-use media tool.
            std::thread::spawn(move || {
                let result = (|| -> std::result::Result<(), String> {
                    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                    let folder = exe.parent().ok_or("Executable folder unavailable")?;
                    let helper = folder.join("noh-components.exe");
                    if !helper.is_file() {
                        return Err("Component installer is missing. Repair NOH using the official installer.".into());
                    }
                    let status = std::process::Command::new(helper)
                        .arg(format!("/PROFILE={profile}"))
                        .arg(format!("/LANG={language}"))
                        .arg(format!("/DIR={}", folder.display()))
                        .status()
                        .map_err(|e| e.to_string())?;
                    if !status.success() {
                        return Err(format!("Component setup did not complete ({status})."));
                    }
                    Ok(())
                })();
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        }
        if self.resources.installer.is_some() {
            ui.label(l.text("resources.installing"));
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        if let Some(result) = &self.resources.installer_result {
            match result {
                Ok(()) => {
                    ui.add(egui::Label::new(l.text("resources.installed")).wrap());
                }
                Err(detail) => {
                    ui.add(egui::Label::new(l.text("resources.install_failed")).wrap());
                    egui::CollapsingHeader::new(l.text("resources.details"))
                        .id_salt("component-installer-error")
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(detail).wrap());
                        });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn pending_component_setup_blocks_media_and_failure_preserves_configuration() {
        let mut app = NohApp::default();
        app.ffmpeg = Some("user-selected-ffmpeg.exe".into());
        app.subtitles.transcriber = "user-selected-whisper.exe".into();
        let (sender, receiver) = mpsc::channel();
        app.resources.installer = Some(receiver);
        assert!(app.engine_locked());
        app.poll_component_installer();
        assert!(app.engine_locked());
        sender.send(Err("Cancelled test setup".into())).unwrap();
        app.poll_component_installer();
        assert!(!app.engine_locked());
        assert!(app.resources.installer_result.as_ref().unwrap().is_err());
        assert_eq!(
            app.ffmpeg.as_deref(),
            Some(std::path::Path::new("user-selected-ffmpeg.exe"))
        );
        assert_eq!(app.subtitles.transcriber, "user-selected-whisper.exe");
        assert!(!app.resources.retry_media);
    }
    fn missing() -> Issue {
        Issue {
            component: Component::Speech,
            path: Some("missing-whisper.exe".into()),
            detail: "Synthetic missing runtime".into(),
        }
    }
    #[test]
    fn obsolete_and_cancelled_reports_cannot_claim_recovery() {
        for cancelled in [false, true] {
            let inputs = Inputs::default();
            let mut old = inputs.clone();
            if !cancelled {
                old.ffmpeg = Some("obsolete.exe".into());
            }
            let (sender, receiver) = mpsc::channel();
            sender.send(Vec::new()).unwrap();
            let worker = std::thread::spawn(|| {});
            while !worker.is_finished() {
                std::thread::yield_now();
            }
            let mut state = State::default();
            state.desired = Some(inputs.clone());
            state.changed = Some(Instant::now());
            state.issues = vec![missing()];
            state.active = Some((old, Arc::new(AtomicBool::new(cancelled)), receiver, worker));
            state.sync(inputs, &egui::Context::default());
            assert!(state.last.is_none());
            assert!(state.speech_issue().is_some());
        }
    }
    #[test]
    fn optional_preview_failure_does_not_block_transcription() {
        let mut state = State::default();
        state.preview = Some(Issue {
            component: Component::Preview,
            path: None,
            detail: "Synthetic unavailable optional preview".into(),
        });
        assert!(state.unavailable());
        assert!(state.speech_issue().is_none());
    }
    #[test]
    fn known_missing_speech_resources_open_repair_without_starting_a_job() {
        let mut app = NohApp::default();
        app.resources.issues = vec![missing()];
        let ctx = egui::Context::default();
        app.start_subtitles(&ctx);
        assert!(app.job.is_none() && !app.subtitles.running);
        assert!(app.settings_open && app.subtitles.failure.is_some());
        app.settings_open = false;
        app.project_generate(&ctx);
        assert!(!app.project.busy() && !app.subtitles.running);
        assert!(app.settings_open);
    }
    #[test]
    fn recovery_help_wraps_in_all_seven_languages() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        app_style::apply(&ctx);
        for language in Language::ALL {
            for width in [420.0, 900.0] {
                let mut app = NohApp::default();
                app.locale.language = language;
                app.resources.issues = vec![missing()];
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 760.0),
                    )),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let right = ui.max_rect().right();
                        app.resource_banner(ui);
                        app.resource_settings(ui);
                        assert!(
                            ui.min_rect().right() <= right + 1.0,
                            "{} at {width}",
                            language.code()
                        );
                    });
                });
                output.textures_delta.clear();
                assert_ne!(language.text("resources.speech"), "[missing translation]");
                assert_ne!(language.text("error.dependencies"), "[missing translation]");
            }
        }
    }
    #[test]
    fn ffmpeg_recovery_requeues_failed_metadata_after_active_work() {
        let inputs = Inputs::default();
        let (sender, receiver) = mpsc::channel();
        sender.send(Vec::new()).unwrap();
        let worker = std::thread::spawn(|| {});
        while !worker.is_finished() {
            std::thread::yield_now();
        }
        let ctx = egui::Context::default();
        let mut app = NohApp::default();
        app.resources.desired = Some(inputs.clone());
        app.resources.issues = vec![Issue {
            component: Component::Ffmpeg,
            path: None,
            detail: "Synthetic missing FFmpeg".into(),
        }];
        app.resources.active = Some((
            inputs.clone(),
            Arc::new(AtomicBool::new(false)),
            receiver,
            worker,
        ));
        app.resources.sync(inputs, &ctx);
        assert!(app.resources.retry_media && app.resources.ffmpeg_available());
        let folder = tempfile::tempdir().unwrap();
        app.wav = Some(folder.path().join("unavailable-soundtrack.wav"));
        app.ffmpeg = Some(folder.path().join("unavailable-ffmpeg.exe"));
        app.wav_error = Some("error.ffmpeg".into());
        app.cancelling = true;
        app.retry_resource_media(&ctx);
        assert!(app.wav_error.is_some() && app.resources.retry_media);
        app.cancelling = false;
        let old_id = app.wav_id;
        app.retry_resource_media(&ctx);
        assert!(app.wav_error.is_none() && !app.resources.retry_media);
        assert_ne!(old_id, app.wav_id);
    }
}
