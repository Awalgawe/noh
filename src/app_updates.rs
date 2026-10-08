//! Settings presentation; IO and downloaded package ownership stay on the worker.
use super::*;
use noh::update::{
    State,
    desktop::{DesktopUpdater, Failure, Snapshot},
};

#[derive(Default)]
pub(super) struct Workspace {
    pub worker: Option<DesktopUpdater>,
    pub view: Snapshot,
    pub capture_fixture: bool,
    pub confirm_install: bool,
    pub install_requested: bool,
    /// Disposable native acceptance driver; independent from capture fixtures.
    pub qualification_step: u8,
}
impl NohApp {
    /// Explicit layout-only fixtures, never an alternate authenticated update path.
    pub(super) fn load_update_capture_fixture(&mut self) -> Result<(), String> {
        if std::env::var_os("NOH_CAPTURE_UI").is_none() {
            return Ok(());
        }
        let Ok(state) = std::env::var("NOH_CAPTURE_UPDATE_STATE") else {
            return Ok(());
        };
        self.updates.view.state = match state.as_str() {
            "available" => State::Available {
                version: "0.1.1".parse().unwrap(),
                notes: "Layout fixture: release notes".into(),
                size: 1_266_107_201,
            },
            "downloading" => State::Downloading {
                received: 600_000_000,
                total: 1_266_107_201,
            },
            "ready" | "safe-close" => State::Ready {
                package: PathBuf::from("unused-layout-fixture"),
                version: "0.1.1".parse().unwrap(),
            },
            "failed" => State::Failed {
                message: "Layout fixture: network error".into(),
            },
            _ => return Err("Unknown NOH_CAPTURE_UPDATE_STATE".into()),
        };
        self.updates.view.failure = Some(Failure::Network);
        if state == "failed" {
            self.updates.view.retry_at = Some(Instant::now() + Duration::from_secs(120));
        }
        self.updates.capture_fixture = true;
        self.updates.confirm_install = state == "safe-close";
        self.settings_open = true;
        Ok(())
    }
    pub(super) fn poll_updates(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        noh::update::windows_ready::notify_first_frame();
        if let Some(worker) = &self.updates.worker {
            let view = worker.snapshot();
            let busy = worker.busy();
            self.receive_update_snapshot(view, busy);
        }
        #[cfg(windows)]
        self.drive_local_update_qualification(ctx);
        #[cfg(windows)]
        if !self.updates.capture_fixture && !self.engine_locked() {
            if self.updates.view.shutdown_committed {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.updates.view.awaiting_shutdown
                && let Some(worker) = &self.updates.worker
            {
                worker.commit_for_shutdown();
            }
        }
    }
    #[cfg(windows)]
    fn drive_local_update_qualification(&mut self, ctx: &egui::Context) {
        let offline_archive = noh::update::trust::offline_install_archive();
        if !noh::update::trust::qualification_enabled()
            || self.updates.capture_fixture
            || (offline_archive.is_none()
                && (std::env::var("NOH_UPDATE_QUALIFICATION_ACTION").as_deref()
                    != Ok("import-install")
                    || std::env::var("NOH_UPDATE_QUALIFICATION_FROM_VERSION").as_deref()
                        != Ok(noh::build_info::current().package_version)))
            || self.engine_locked()
        {
            return;
        }
        match self.updates.qualification_step {
            0 => {
                let Some(archive) = offline_archive.or_else(|| {
                    std::env::var_os("NOH_UPDATE_QUALIFICATION_ARCHIVE").map(Into::into)
                }) else {
                    return;
                };
                if self.updates.worker.is_some() {
                    return;
                }
                let context = ctx.clone();
                let worker = DesktopUpdater::from_build(move || context.request_repaint());
                if worker.import_qualification_archive(archive) {
                    self.updates.worker = Some(worker);
                    self.updates.qualification_step = 1;
                    self.settings_open = true;
                    self.updates.view.state = State::Verifying;
                }
            }
            1 if self.update_install_available() => {
                // Explicit harness consent: render the same safe-close screen
                // for one frame before invoking the shared acceptance method.
                self.updates.confirm_install = true;
                self.updates.qualification_step = 2;
                ctx.request_repaint();
            }
            2 => {
                if self.request_update_install() {
                    self.updates.qualification_step = 3;
                }
            }
            _ => (),
        }
    }
    #[cfg(windows)]
    fn request_update_install(&mut self) -> bool {
        if !self.update_install_available() || !self.updates.confirm_install {
            return false;
        }
        let Some(worker) = &self.updates.worker else {
            return false;
        };
        let accepted = if self.updates.view.previous_available {
            worker.prepare_windows_auto()
        } else {
            std::env::var_os("NOH_UPDATE_PREVIOUS_ARCHIVE")
                .is_some_and(|previous| worker.prepare_windows(previous.into()))
        };
        if accepted {
            self.updates.confirm_install = false;
            self.updates.install_requested = true;
            self.updates.view.state = State::Applying;
        }
        accepted
    }
    fn receive_update_snapshot(&mut self, view: Snapshot, busy: bool) {
        self.updates.view = view;
        // Ready can be an old snapshot read just before the worker publishes
        // Applying and becomes inactive. Only actual terminal states release
        // the consent latch; a second activity read cannot reopen the session.
        if !busy && matches!(self.updates.view.state, State::Idle | State::Failed { .. }) {
            self.updates.install_requested = false;
        }
    }
    pub(super) fn update_blocks_session(&self) -> bool {
        !self.updates.capture_fixture
            && (self.updates.confirm_install
                || self.updates.install_requested
                || matches!(self.updates.view.state, State::Applying))
    }
    pub(super) fn update_install_available(&self) -> bool {
        cfg!(windows)
            && (noh::update::trust::setup_enabled()
                || option_env!("NOH_UPDATE_QUALIFICATION_ROOT").is_some())
            && (self.updates.view.previous_available
                || std::env::var_os("NOH_UPDATE_PREVIOUS_ARCHIVE").is_some())
            && !self.updates.capture_fixture
            && !self.engine_locked()
            && self.updates.worker.as_ref().is_some_and(|worker| {
                !worker.busy()
                    && !worker.worker_finished()
                    && matches!(worker.snapshot().state, State::Ready { .. })
            })
            && matches!(self.updates.view.state, State::Ready { .. })
    }
    pub(super) fn update_shutdown_controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let language = self.locale.language;
        if self.updates.confirm_install {
            ui.heading(language.text("updates.install_restart"));
            ui.label(language.text("updates.safe_close"));
            if app_style::button(ui, language.text("updates.cancel"), true, false, 0.0).clicked() {
                self.updates.confirm_install = false;
            }
            if app_style::button(
                ui,
                language.text("updates.install_restart"),
                self.update_install_available(),
                true,
                0.0,
            )
            .clicked()
            {
                #[cfg(windows)]
                self.request_update_install();
            }
        } else {
            self.update_controls(ui, ctx);
        }
    }
    fn check_updates(&mut self, ctx: &egui::Context) {
        if self
            .updates
            .worker
            .as_ref()
            .is_none_or(DesktopUpdater::worker_finished)
        {
            let context = ctx.clone();
            self.updates.worker = Some(DesktopUpdater::from_build(move || {
                context.request_repaint()
            }));
        }
        if self.updates.worker.as_ref().unwrap().check() {
            self.updates.view.state = State::Checking;
            self.updates.view.failure = None;
        }
    }
    pub(super) fn update_controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let language = self.locale.language;
        if !DesktopUpdater::configured()
            && self.updates.worker.is_none()
            && matches!(self.updates.view.state, State::Idle)
            && self.updates.view.failure.is_none()
        {
            ui.label(language.text("updates.unconfigured"));
            return;
        }
        let view = self.updates.view.clone();
        let busy = self
            .updates
            .worker
            .as_ref()
            .is_some_and(DesktopUpdater::busy);
        let cancel_button = |ui: &mut egui::Ui| {
            app_style::button(ui, language.text("updates.cancel"), true, false, 0.0).clicked()
        };
        match &view.state {
            State::Idle => {
                ui.label(language.text(if view.checked {
                    "updates.current"
                } else {
                    "updates.check_hint"
                }));
                if app_style::button(ui, language.text("updates.check"), !busy, false, 0.0)
                    .clicked()
                {
                    self.check_updates(ctx);
                }
            }
            State::Checking => {
                ui.label(language.text("updates.checking"));
                if cancel_button(ui)
                    && let Some(worker) = &self.updates.worker
                {
                    worker.cancel();
                }
            }
            State::Available {
                version,
                notes,
                size,
            } => {
                ui.label(language.format(
                    "updates.available",
                    &[
                        version.to_string(),
                        format!("{:.1}", *size as f64 / 1_000_000.0),
                    ],
                ));
                if !notes.is_empty() {
                    egui::CollapsingHeader::new(language.text("updates.notes"))
                        .id_salt("update-notes")
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(notes).wrap());
                        });
                }
                ui.label(language.text("updates.download_hint"));
                if app_style::button(ui, language.text("updates.download"), !busy, true, 0.0)
                    .clicked()
                    && let Some(worker) = &self.updates.worker
                {
                    worker.download();
                }
            }
            State::Downloading { received, total } => {
                ui.label(language.text("updates.downloading"));
                ui.add(
                    egui::ProgressBar::new((*received as f64 / (*total).max(1) as f64) as f32)
                        .show_percentage(),
                );
                if cancel_button(ui)
                    && let Some(worker) = &self.updates.worker
                {
                    worker.cancel();
                }
            }
            State::Verifying => {
                ui.label(language.text("updates.verifying"));
                if cancel_button(ui)
                    && let Some(worker) = &self.updates.worker
                {
                    worker.cancel();
                }
            }
            State::Ready { version, .. } => {
                ui.label(language.format("updates.ready", &[version.to_string()]));
                let available = self.update_install_available();
                if !available {
                    ui.label(language.text("updates.install_unqualified"));
                }
                let button = app_style::button(
                    ui,
                    language.text("updates.install_restart"),
                    available,
                    true,
                    0.0,
                );
                if button.clicked() {
                    self.updates.confirm_install = true;
                }
                #[cfg(test)]
                self.options
                    .rects
                    .push(("update-install", button.rect, button.enabled()));
                #[cfg(not(test))]
                let _ = button;
                if cancel_button(ui)
                    && let Some(worker) = &self.updates.worker
                {
                    worker.cancel();
                }
            }
            State::Applying => {
                ui.label(language.text("updates.applying"));
                if !self.updates.view.awaiting_shutdown
                    && !self.updates.view.shutdown_committed
                    && cancel_button(ui)
                    && let Some(worker) = &self.updates.worker
                {
                    worker.cancel();
                }
            }
            State::Failed { .. } => {
                ui.label(language.text(view.failure.unwrap_or(Failure::Other).message_key()));
                let delay = view.retry_after();
                if !delay.is_zero() {
                    ui.label(language.format(
                        "updates.retry_wait",
                        &[delay.as_millis().div_ceil(1000).to_string()],
                    ));
                    ctx.request_repaint_after(Duration::from_secs(1));
                }
                if app_style::button(
                    ui,
                    language.text("updates.retry"),
                    !busy && delay.is_zero(),
                    false,
                    0.0,
                )
                .clicked()
                {
                    self.check_updates(ctx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_close_is_localized_and_cannot_prepare_a_layout_only_package() {
        for language in Language::ALL {
            for width in [280.0, 410.0] {
                let context = egui::Context::default();
                app_locale::install_fonts(&context);
                let mut app = NohApp::default();
                app.locale.language = language;
                app.updates.confirm_install = true;
                app.updates.view.state = State::Ready {
                    package: "unused-fixture".into(),
                    version: "2.0.0".parse().unwrap(),
                };
                assert!(app.update_blocks_session());
                app.updates.confirm_install = false;
                app.updates.install_requested = true;
                assert!(
                    app.update_blocks_session(),
                    "accepted preparation locks even a stale Ready snapshot"
                );
                app.receive_update_snapshot(app.updates.view.clone(), false);
                assert!(
                    app.update_blocks_session(),
                    "stale Ready plus new inactive flag cannot release consent latch"
                );
                app.updates.confirm_install = true;
                assert!(!app.update_install_available());
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width + 16.0, 600.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        ui.set_width(width);
                        app.update_shutdown_controls(ui, &context);
                    },
                );
                output.textures_delta.clear();
                for shape in output.shapes {
                    if let egui::Shape::Text(text) = shape.shape {
                        assert!(!text.galley.text().contains("[missing translation]"));
                        assert!(text.pos.x + text.galley.rect.right() <= width + 16.5);
                    }
                }
                assert!(app.updates.worker.is_none());
                app.updates.capture_fixture = true;
                assert!(!app.update_blocks_session());
                assert!(!app.update_install_available());
            }
        }
    }

    /// Presentation fixtures do not simulate an installation or perform network IO.
    #[test]
    fn localized_update_states_fit_and_never_unlock_installation() {
        for language in Language::ALL {
            for width in [280.0, 410.0] {
                for state in [
                    State::Available {
                        version: "2.0.0".parse().unwrap(),
                        notes: "Authenticated fixture notes".into(),
                        size: 1_266_107_201,
                    },
                    State::Downloading {
                        received: 600_000_000,
                        total: 1_266_107_201,
                    },
                    State::Ready {
                        package: "unused-fixture.nupkg".into(),
                        version: "2.0.0".parse().unwrap(),
                    },
                    State::Failed {
                        message: "sanitized diagnostic".into(),
                    },
                ] {
                    let context = egui::Context::default();
                    app_locale::install_fonts(&context);
                    let mut app = NohApp::default();
                    app.locale.language = language;
                    app.updates.view.state = state.clone();
                    app.updates.view.failure = Some(Failure::Network);
                    app.updates.view.retry_at = Some(Instant::now() + Duration::from_secs(60));
                    let mut result = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width + 16.0, 600.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            ui.set_width(width);
                            app.update_controls(ui, &context);
                        },
                    );
                    result.textures_delta.clear();
                    assert!(app.updates.worker.is_none(), "rendering starts no worker");
                    assert!(
                        !app.engine_locked(),
                        "update rendering never locks media work"
                    );
                    for clipped in result.shapes {
                        if let egui::Shape::Text(text) = clipped.shape {
                            assert!(!text.galley.text().contains("[missing translation]"));
                            assert!(
                                text.pos.x + text.galley.rect.right() <= width + 16.5,
                                "{} width {width}: {}",
                                language.code(),
                                text.galley.text()
                            );
                        }
                    }
                    if matches!(state, State::Ready { .. }) {
                        assert!(
                            !app.options
                                .rects
                                .iter()
                                .find(|(name, _, _)| *name == "update-install")
                                .unwrap()
                                .2
                        );
                    }
                }
            }
        }
    }
}
