//! Automatic updates in the interface: the Settings section, the header badge,
//! the checks on a timer and the installation when NOH closes. The work itself
//! is in `noh::auto_update`.
use super::*;
use app_style::{self as style, Kind, space};
use noh::auto_update::{
    self as updates, Outcome, Preferences, Refusal, State, Version,
    worker::{Phase, Worker},
};

/// How often an open NOH looks again whether a daily check is due.
const RECHECK: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub(super) struct AutoUpdate {
    preferences: Preferences,
    preferences_path: Option<PathBuf>,
    /// `%LOCALAPPDATA%\noh\updates`.
    root: Option<PathBuf>,
    worker: Worker,
    /// Set once the first frame has read the previous run's outcome.
    started: bool,
    next_look: Option<Instant>,
    /// What the last installation attempt became, shown until dismissed.
    outcome: Option<Outcome>,
    /// Why "Redémarrer" did not go ahead.
    refusal: Option<Refusal>,
    blocked: bool,
    /// The user asked for a restart: install at exit and open NOH again.
    restart: bool,
    /// The version whose last installation failed: closing NOH does not try it
    /// again by itself, only the button does.
    failed: Option<Version>,
    /// Why the installer did not start when NOH last closed.
    not_started: Option<String>,
}

/// The updates folder holds the download folder of a version newer than this one.
fn pending_download(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(Version::parse)
                .is_some_and(|v| v > Version::current())
        })
    })
}

impl AutoUpdate {
    /// No network, no timer in captures and tests.
    fn enabled() -> bool {
        std::env::var_os("NOH_CAPTURE_UI").is_none() && !cfg!(test)
    }
}

impl NohApp {
    /// Called every frame: reads the last attempt once, then starts a check
    /// when one is due.
    pub(super) fn autoupdate_frame(&mut self, ctx: &egui::Context) {
        if !AutoUpdate::enabled() {
            return;
        }
        let u = &mut self.autoupdate;
        if !u.started {
            u.started = true;
            u.preferences_path = Preferences::path();
            u.preferences = u
                .preferences_path
                .as_deref()
                .map(Preferences::read)
                .unwrap_or_default();
            u.root = State::folder();
            if let Some(root) = &u.root {
                let mut state = State::read(root);
                if let Some(reason) = state.not_started.take() {
                    u.not_started = Some(reason);
                    let _ = state.save(root);
                }
                if let Some(outcome) = state.outcome(Version::current()) {
                    match outcome {
                        Outcome::Updated(version) | Outcome::Superseded(version) => {
                            let _ = std::fs::remove_dir_all(updates::version_folder(root, version));
                            state.attempt = None;
                            let _ = state.save(root);
                        }
                        Outcome::Failed(version) => u.failed = Some(version),
                    }
                    if !matches!(outcome, Outcome::Superseded(_)) {
                        u.outcome = Some(outcome);
                    }
                }
                // A newer version downloaded by an earlier run: check now rather
                // than in up to 24 hours, so it is offered again at once (its
                // files are reused once their digests match).
                if u.preferences.check && pending_download(root) {
                    let root = root.clone();
                    u.next_look = Some(Instant::now() + RECHECK);
                    Self::start_check(u, root, ctx);
                    return;
                }
            }
        }
        let now = Instant::now();
        if u.next_look.is_some_and(|at| now < at) {
            return;
        }
        u.next_look = Some(now + RECHECK);
        let Some(root) = u.root.clone() else { return };
        if u.preferences.check && State::read(&root).check_due(updates::worker::now()) {
            Self::start_check(u, root, ctx);
        }
    }

    fn start_check(u: &mut AutoUpdate, root: PathBuf, ctx: &egui::Context) {
        let repaint = ctx.clone();
        u.refusal = None;
        u.worker.start(root, move || repaint.request_repaint());
    }

    /// The header badge: a downloaded update to install, or a version to fetch
    /// by hand. Opens Settings.
    pub(super) fn autoupdate_badge(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let label = match self.autoupdate.worker.phase() {
            Phase::Ready(_) | Phase::Available(_) => l.text("autoupdate.badge"),
            _ => return,
        };
        let badge = style::button_kind(ui, label, true, Kind::Quiet, 0.0, style::CONTROL_H);
        if badge.clicked() {
            self.settings_open = true;
        }
    }

    /// Anything that would make closing NOH ask a question first.
    fn closing_blocked(&self) -> bool {
        self.project.busy()
            || self.job.is_some()
            || self.cancelling
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some()
            || self.has_unsaved_lyrics()
    }

    /// The Settings section.
    pub(super) fn autoupdate_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let current = Version::current().to_string();
        let mut preferences = self.autoupdate.preferences;
        for (on, key) in [
            (&mut preferences.check, "autoupdate.check_auto"),
            (
                &mut preferences.install_on_exit,
                "autoupdate.install_on_exit",
            ),
        ] {
            ui.horizontal(|ui| {
                let label = l.text(key);
                style::switch(ui, on, label);
                ui.add(egui::Label::new(label).wrap());
            });
        }
        if preferences != self.autoupdate.preferences {
            self.autoupdate.preferences = preferences;
            if let Some(path) = &self.autoupdate.preferences_path {
                let _ = preferences.save(path);
            }
        }
        if let Some(outcome) = self.autoupdate.outcome {
            match outcome {
                Outcome::Updated(version) => {
                    ui.add(
                        egui::Label::new(l.format("autoupdate.updated", &[version.to_string()]))
                            .wrap(),
                    );
                }
                Outcome::Failed(version) => {
                    ui.add(
                        egui::Label::new(l.format("autoupdate.failed", &[version.to_string()]))
                            .wrap(),
                    );
                    if let Some(root) = &self.autoupdate.root {
                        let log = updates::version_folder(root, version).join("install.log");
                        if log.is_file()
                            && style::button(ui, l.text("autoupdate.open_log"), true, false, 0.0)
                                .clicked()
                        {
                            let _ = open::that(&log);
                        }
                    }
                }
                Outcome::Superseded(_) => {}
            }
        }
        if let Some(reason) = &self.autoupdate.not_started {
            let text = l.format("autoupdate.not_started", std::slice::from_ref(reason));
            ui.add(egui::Label::new(text).wrap());
        }
        let phase = self.autoupdate.worker.phase();
        let line = match &phase {
            Phase::Idle => l.format("autoupdate.idle", std::slice::from_ref(&current)),
            Phase::Checking => l.text("autoupdate.checking").into(),
            Phase::UpToDate => l.format("autoupdate.current", std::slice::from_ref(&current)),
            Phase::Available(release) => {
                l.format("autoupdate.available", &[release.version.to_string()])
            }
            Phase::Preparing(version) => l.format("autoupdate.preparing", &[version.to_string()]),
            Phase::Downloading {
                version,
                done,
                total,
            } => l.format(
                "autoupdate.downloading",
                &[
                    version.to_string(),
                    (done / 1_000_000).to_string(),
                    total.div_ceil(1_000_000).to_string(),
                ],
            ),
            Phase::Ready(ready) => l.format("autoupdate.ready", &[ready.version.to_string()]),
            Phase::Failed(error) => l
                .text(match error {
                    updates::Error::Network(_) => "autoupdate.error_network",
                    updates::Error::Integrity(_) => "autoupdate.error_integrity",
                    updates::Error::Storage(_) => "autoupdate.error_storage",
                    updates::Error::Unsupported(_) => "autoupdate.error_unsupported",
                })
                .into(),
        };
        ui.add(egui::Label::new(egui::RichText::new(line).color(t.text_2)).wrap());
        if let Phase::Failed(error) = &phase {
            egui::CollapsingHeader::new(l.text("resources.details"))
                .id_salt("autoupdate-error")
                .show(ui, |ui| {
                    ui.add(egui::Label::new(error.to_string()).wrap());
                });
        }
        if let Phase::Available(release) = &phase {
            ui.add(egui::Label::new(l.text("autoupdate.available_manual")).wrap());
            if style::button(ui, l.text("autoupdate.open_release"), true, false, 0.0).clicked() {
                let _ = open::that(&release.page);
            }
        }
        match &self.autoupdate.refusal {
            Some(Refusal::Busy(what)) => {
                ui.add(egui::Label::new(l.format("autoupdate.busy", &[what.join(", ")])).wrap());
            }
            Some(Refusal::Failed(error)) => {
                ui.add(egui::Label::new(error.to_string()).wrap());
            }
            None => {}
        }
        if self.autoupdate.blocked {
            ui.add(egui::Label::new(l.text("autoupdate.close_blocked")).wrap());
        }
        ui.add_space(space::XS);
        ui.horizontal_wrapped(|ui| {
            let busy = self.autoupdate.worker.busy();
            if let Phase::Ready(ready) = &phase
                && style::button(ui, l.text("autoupdate.restart"), !busy, true, 0.0).clicked()
            {
                self.autoupdate_restart(ready.clone(), ctx);
            }
            if style::button(ui, l.text("autoupdate.check_now"), !busy, false, 0.0).clicked()
                && let Some(root) = self.autoupdate.root.clone()
            {
                Self::start_check(&mut self.autoupdate, root, ctx);
            }
        });
    }

    /// "Redémarrer pour mettre à jour": refused here when something runs from
    /// the installed folder or closing would first ask a question; otherwise
    /// NOH closes and the installation starts in `on_exit`.
    fn autoupdate_restart(&mut self, ready: updates::worker::Ready, ctx: &egui::Context) {
        self.autoupdate.refusal = None;
        self.autoupdate.blocked = false;
        if self.closing_blocked() {
            self.autoupdate.blocked = true;
            return;
        }
        #[cfg(target_os = "windows")]
        match updates::windows::running(&ready.installation.folder, std::process::id()) {
            Ok(busy) if !busy.is_empty() => {
                self.autoupdate.refusal =
                    Some(Refusal::Busy(busy.iter().map(|r| r.to_string()).collect()));
                return;
            }
            Err(error) => {
                self.autoupdate.refusal = Some(Refusal::Failed(error));
                return;
            }
            Ok(_) => {}
        }
        let _ = ready;
        self.autoupdate.restart = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// At exit: install a ready update when the user asked for a restart, or
    /// when "install at exit" is on and Windows is not ending the session.
    pub(super) fn autoupdate_exit(&mut self) {
        if !AutoUpdate::enabled() {
            return;
        }
        let u = &mut self.autoupdate;
        let Phase::Ready(ready) = u.worker.phase() else {
            return;
        };
        #[cfg(target_os = "windows")]
        {
            let automatic = u.preferences.install_on_exit
                && u.failed != Some(ready.version)
                && !updates::windows::session_ending();
            let Some(root) = &u.root else { return };
            if !(u.restart || automatic) {
                return;
            }
            if let Err(refusal) = updates::apply(&ready, root, u.restart) {
                // Something started meanwhile, space ran out, the installer
                // changed: say so at the next start, and keep the promise of a
                // restart by opening this version again.
                let reason = match refusal {
                    Refusal::Busy(what) => what.join(", "),
                    Refusal::Failed(error) => error.to_string(),
                };
                let mut state = State::read(root);
                state.not_started = Some(reason);
                let _ = state.save(root);
                if u.restart
                    && let Ok(exe) = std::env::current_exe()
                {
                    let _ = std::process::Command::new(exe).spawn();
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = ready;
    }
}
