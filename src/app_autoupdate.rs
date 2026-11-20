//! Automatic updates in the interface: the Settings section, the header badge,
//! the checks on a timer and the installation when NOH closes. The work itself
//! is in `noh::auto_update`.
use super::*;
use app_style::{self as style, space};
use noh::auto_update::{
    self as updates, Holder, Outcome, Preferences, Refusal, State, Version,
    worker::{Phase, Worker},
};
use noh::update_stop::StopRequest;

/// How often an open NOH looks again whether a check is due.
const RECHECK: Duration = Duration::from_secs(10 * 60);
/// While "Restart to update" waits for MCP servers: how often NOH looks at
/// what still runs, and renews its stop request (which lapses after
/// `update_stop::LIFETIME`).
const WAIT_LOOK: Duration = Duration::from_secs(1);
const WAIT_RENEW: Duration = Duration::from_secs(5);
/// After this long, a server that has not stopped is likely from an older
/// NOH that does not know the request: say how to stop it.
const WAIT_STUCK: Duration = Duration::from_secs(20);

/// "Restart to update" waits for the MCP servers of the copy to stop.
pub(super) struct Waiting {
    ready: updates::worker::Ready,
    since: Instant,
    next_look: Instant,
    renewed: Instant,
    /// Servers still running at the last look, and those of them running a job.
    servers: usize,
    working: usize,
    /// Servers at the first look: fewer later means some already stopped.
    first: usize,
}

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
    waiting: Option<Waiting>,
    /// The MCP servers were asked to stop and have: the installation must say
    /// to reconnect them.
    servers_asked: bool,
    /// The last installation stopped MCP servers: say to reconnect them.
    reconnect: bool,
    /// A wait ended without installing after some servers had stopped.
    dropped: bool,
}

/// What holds the installed files, one sentence per kind, for the user.
/// `waiting`: the servers have been asked to stop and are stopping.
fn holders_text(l: noh::i18n::Language, held: &[Holder], waiting: bool) -> Vec<String> {
    held.iter()
        .map(|holder| match holder {
            Holder::Servers { count, .. } if waiting => {
                l.plural("autoupdate.holder_servers", *count)
            }
            Holder::Servers { count, .. } => l.plural("autoupdate.holder_servers_later", *count),
            Holder::Commands(n) => l.plural("autoupdate.holder_commands", *n),
            Holder::Windows(n) => l.plural("autoupdate.holder_windows", *n),
            Holder::Own(n) => l.plural("autoupdate.holder_own", *n),
            Holder::Program { name, count } => l.format(
                "autoupdate.holder_program",
                &[name.clone(), count.to_string()],
            ),
        })
        .collect()
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
        #[cfg(target_os = "windows")]
        self.autoupdate_wait(ctx);
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
                    u.reconnect = matches!(outcome, Outcome::Updated(_))
                        && state.attempt.as_ref().is_some_and(|a| a.servers_stopped);
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
        let (label, tip) = match self.autoupdate.worker.phase() {
            // Waiting for MCP servers is visible and reachable with Settings closed.
            Phase::Ready(_) if self.autoupdate.waiting.is_some() => (
                l.text("autoupdate.badge_waiting"),
                l.text("autoupdate.waiting").to_owned(),
            ),
            Phase::Ready(ready) => (
                l.text("autoupdate.badge"),
                l.format("autoupdate.ready", &[ready.version.to_string()]),
            ),
            Phase::Available(release) => (
                l.text("autoupdate.badge_available"),
                l.format("autoupdate.available", &[release.version.to_string()]),
            ),
            _ => return,
        };
        if style::notice_button(ui, label, &tip).clicked() {
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
                    if self.autoupdate.reconnect {
                        ui.add(egui::Label::new(l.text("autoupdate.reconnect")).wrap());
                    }
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
            Some(Refusal::Busy(held)) => {
                ui.add(egui::Label::new(l.text("autoupdate.busy")).wrap());
                for line in holders_text(l, held, false) {
                    ui.add(egui::Label::new(format!("•  {line}")).wrap());
                }
            }
            Some(Refusal::Failed(error)) => {
                ui.add(egui::Label::new(error.to_string()).wrap());
            }
            None => {}
        }
        if self.autoupdate.dropped && self.autoupdate.waiting.is_none() {
            ui.add(egui::Label::new(l.text("autoupdate.reconnect_dropped")).wrap());
        }
        if let Some(waiting) = &self.autoupdate.waiting {
            ui.add(egui::Label::new(l.text("autoupdate.waiting")).wrap());
            let held = [Holder::Servers {
                count: waiting.servers.max(1),
                working: waiting.working,
            }];
            for line in holders_text(l, &held, true) {
                ui.add(egui::Label::new(format!("•  {line}")).wrap());
            }
            if waiting.working > 0 {
                ui.add(egui::Label::new(l.text("autoupdate.waiting_job")).wrap());
            }
            // Idle servers still there after a while do not know the request.
            if waiting.servers > waiting.working && waiting.since.elapsed() >= WAIT_STUCK {
                ui.add(egui::Label::new(l.text("autoupdate.waiting_stuck")).wrap());
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(l.text("autoupdate.reconnect")).color(t.text_2),
                )
                .wrap(),
            );
        }
        if self.autoupdate.blocked {
            ui.add(egui::Label::new(l.text("autoupdate.close_blocked")).wrap());
        }
        ui.add_space(space::XS);
        ui.horizontal_wrapped(|ui| {
            let waiting = self.autoupdate.waiting.is_some();
            let busy = self.autoupdate.worker.busy() || waiting;
            if waiting {
                if style::button(ui, l.text("cancel"), true, false, 0.0).clicked() {
                    self.autoupdate_stop_waiting();
                }
            } else if let Phase::Ready(ready) = &phase
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

    /// "Redémarrer pour mettre à jour": refused when closing would first ask a
    /// question or when something other than MCP servers runs from the
    /// installed folder. When only servers do, NOH asks them to stop and waits
    /// for them (`autoupdate_wait`); once nothing runs from the folder, NOH
    /// closes and the installation starts in `on_exit`. No server is asked to
    /// stop for an update that cannot install.
    fn autoupdate_restart(&mut self, ready: updates::worker::Ready, ctx: &egui::Context) {
        self.autoupdate.refusal = None;
        self.autoupdate.blocked = false;
        if self.closing_blocked() {
            self.autoupdate.blocked = true;
            return;
        }
        #[cfg(target_os = "windows")]
        {
            let Some(root) = self.autoupdate.root.clone() else {
                return;
            };
            let own = std::process::id();
            let mut first = (0, 0);
            match updates::windows::running(&ready.installation.folder, own) {
                Err(error) => {
                    self.autoupdate.refusal = Some(Refusal::Failed(error));
                    return;
                }
                Ok(processes) => {
                    let held = updates::holders(&processes, own);
                    if held.is_empty() {
                        self.autoupdate.restart = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        return;
                    }
                    if !updates::only_servers(&held) {
                        self.autoupdate.refusal = Some(Refusal::Busy(held));
                        return;
                    }
                    if let [Holder::Servers { count, working }] = held[..] {
                        first = (count, working);
                    }
                }
            }
            if let Err(error) =
                StopRequest::write(&root, &ready.installation.folder, noh::update_stop::now())
            {
                self.autoupdate.refusal = Some(Refusal::Failed(error.into()));
                return;
            }
            let now = Instant::now();
            self.autoupdate.waiting = Some(Waiting {
                ready,
                since: now,
                next_look: now,
                renewed: now,
                servers: first.0,
                working: first.1,
                first: first.0,
            });
            self.autoupdate.servers_asked = true;
            self.autoupdate.dropped = false;
            ctx.request_repaint();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = ready;
            self.autoupdate.restart = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Ends the wait and withdraws the stop request: the servers accept jobs again.
    fn autoupdate_stop_waiting(&mut self) {
        self.autoupdate.servers_asked = false;
        if let Some(waiting) = self.autoupdate.waiting.take() {
            self.autoupdate.dropped |= waiting.servers < waiting.first;
            if let Some(root) = &self.autoupdate.root {
                StopRequest::clear(root);
            }
        }
    }

    /// While "Restart to update" waits: once a second, looks at what runs from
    /// the installed folder. Nothing: close and install. Only MCP servers:
    /// keep waiting (they stop by themselves once they have no job). Anything
    /// else, or a task started in NOH meanwhile: give up and say what.
    #[cfg(target_os = "windows")]
    fn autoupdate_wait(&mut self, ctx: &egui::Context) {
        let blocked = self.closing_blocked();
        let u = &mut self.autoupdate;
        let (Some(waiting), Some(root)) = (&mut u.waiting, u.root.clone()) else {
            return;
        };
        let now = Instant::now();
        if now < waiting.next_look {
            ctx.request_repaint_after(waiting.next_look - now);
            return;
        }
        waiting.next_look = now + WAIT_LOOK;
        ctx.request_repaint_after(WAIT_LOOK);
        let folder = waiting.ready.installation.folder.clone();
        if now >= waiting.renewed + WAIT_RENEW {
            let _ = StopRequest::write(&root, &folder, noh::update_stop::now());
            waiting.renewed = now;
        }
        let own = std::process::id();
        let held = updates::windows::running(&folder, own).map(|p| updates::holders(&p, own));
        match held {
            Ok(held) if !blocked && (held.is_empty() || updates::only_servers(&held)) => {
                if let [Holder::Servers { count, working }] = held[..] {
                    waiting.servers = count;
                    waiting.working = working;
                    return;
                }
                u.waiting = None;
                u.restart = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            outcome => {
                if let Ok(held) = &outcome {
                    let left = held
                        .iter()
                        .map(|h| match h {
                            Holder::Servers { count, .. } => *count,
                            _ => 0,
                        })
                        .sum::<usize>();
                    u.dropped |= left < waiting.first;
                }
                u.waiting = None;
                u.servers_asked = false;
                StopRequest::clear(&root);
                match outcome {
                    Ok(_) if blocked => u.blocked = true,
                    Ok(held) => u.refusal = Some(Refusal::Busy(held)),
                    Err(error) => u.refusal = Some(Refusal::Failed(error)),
                }
            }
        }
    }

    /// At exit: install a ready update when the user asked for a restart, or
    /// when "install at exit" is on and Windows is not ending the session.
    pub(super) fn autoupdate_exit(&mut self) {
        if !AutoUpdate::enabled() {
            return;
        }
        // Closed while still waiting for servers: `apply` asks them again if
        // it installs. A finished wait has already left `waiting`.
        if self.autoupdate.waiting.is_some() {
            self.autoupdate_stop_waiting();
        }
        let l = self.locale.language;
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
            if let Err(refusal) = updates::apply(&ready, root, u.restart, u.servers_asked) {
                // Something started meanwhile, space ran out, the installer
                // changed: say so at the next start, and keep the promise of a
                // restart by opening this version again.
                let reason = match refusal {
                    Refusal::Busy(held) => holders_text(l, &held, false).join(" "),
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
        let _ = (ready, l);
    }

    /// Interface captures: the badge (`update-ready`), the wait for MCP
    /// servers (`update-waiting`) and a refusal (`update-busy`).
    pub(super) fn stage_autoupdate(&mut self, state: &str) {
        let version = Version(9, 9, 0);
        let ready = updates::worker::Ready {
            version,
            page: updates::release_page(version),
            setup: PathBuf::from(updates::setup_name(version)),
            setup_sha256: String::new(),
            components: Vec::new(),
            installation: updates::Installation {
                folder: PathBuf::from("NOH"),
                profile: updates::Profile::Complete,
            },
        };
        self.autoupdate.worker.stage(Phase::Ready(ready.clone()));
        if state != "update-ready" {
            // Resources unfold above Updates when components are missing, as
            // in a capture's fixture: keep Updates in view.
            self.resources.issues.clear();
            self.resources.preview = None;
        }
        match state {
            "update-waiting" => {
                let now = Instant::now();
                self.autoupdate.waiting = Some(Waiting {
                    ready,
                    since: now.checked_sub(WAIT_STUCK).unwrap_or(now),
                    next_look: now,
                    renewed: now,
                    servers: 3,
                    working: 1,
                    first: 9,
                });
                self.settings_open = true;
            }
            "update-busy" => {
                self.autoupdate.refusal = Some(Refusal::Busy(vec![
                    Holder::Servers {
                        count: 9,
                        working: 0,
                    },
                    Holder::Commands(1),
                ]));
                self.settings_open = true;
            }
            _ => {}
        }
    }
}
