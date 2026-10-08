//! A single update worker owns the controller and its temporary packages.
//! Progress is coalesced into one snapshot; media and native UI never do network IO.
use super::*;
use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Configuration,
    Authentication,
    Network,
    Integrity,
    Target,
    Storage,
    Other,
}
impl Failure {
    pub fn message_key(self) -> &'static str {
        match self {
            Self::Configuration => "updates.error_configuration",
            Self::Authentication => "updates.error_authentication",
            Self::Network => "updates.error_network",
            Self::Integrity => "updates.error_integrity",
            Self::Target => "updates.error_target",
            Self::Storage => "updates.error_storage",
            Self::Other => "updates.error_other",
        }
    }
    fn from_error(error: &Error) -> Self {
        match error {
            Error::Http(401 | 403) => Self::Authentication,
            Error::Http(_) | Error::Network | Error::RateLimited(_) => Self::Network,
            Error::Integrity | Error::Signature | Error::Metadata | Error::Url => Self::Integrity,
            Error::Target | Error::InstallationUnavailable => Self::Target,
            Error::Io(_) => Self::Storage,
            Error::Cancelled => Self::Other,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: State,
    pub failure: Option<Failure>,
    pub retry_at: Option<std::time::Instant>,
    /// Completion of the last check, distinct from an initial idle controller.
    pub checked: bool,
    pub awaiting_shutdown: bool,
    pub shutdown_committed: bool,
    pub previous_available: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: State::Idle,
            failure: None,
            retry_at: None,
            checked: false,
            awaiting_shutdown: false,
            shutdown_committed: false,
            previous_available: false,
        }
    }
}
impl Snapshot {
    pub fn retry_after(&self) -> Duration {
        self.retry_at
            .map(|at| at.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or_default()
    }
}

/// Public build configuration and a runtime-only token. Intentionally no Debug.
pub struct Options {
    pub transport: GithubTransport,
    pub keys: Vec<TrustKey>,
    pub package_id: String,
    pub target: Target,
    pub channel: Channel,
    pub profile: Profile,
    pub current: Version,
    pub feed_url: String,
    pub cache: PathBuf,
    pub setup: bool,
    #[cfg(test)]
    pub before_publish: Option<Box<dyn Fn(&State) + Send>>,
}
impl Options {
    /// Called on the worker, not the UI thread. The token is never fingerprinted.
    pub fn compiled() -> Result<Self> {
        let feed_url = option_env!("NOH_UPDATE_FEED_URL").ok_or(Error::InstallationUnavailable)?;
        // Feed configuration is public and embedded. Query credentials belong
        // exclusively to runtime requests, never to compilation options.
        if validate_url(feed_url)?.query().is_some() {
            return Err(Error::Url);
        }
        let channel = match option_env!("NOH_UPDATE_CHANNEL").unwrap_or("stable") {
            "stable" => Channel::Stable,
            "beta" => Channel::Beta,
            _ => return Err(Error::Metadata),
        };
        let profile = installed_profile()?;
        Ok(Self {
            transport: GithubTransport::new(std::env::var("NOH_UPDATE_TOKEN").ok())?,
            keys: trust::compiled_keys()?,
            package_id: trust::package_id().into(),
            target: Target::native()?,
            channel,
            profile,
            current: trust::compiled_version()?,
            feed_url: profile.feed_url(feed_url)?,
            cache: cache_directory()?,
            setup: setup_format()?,
            #[cfg(test)]
            before_publish: None,
        })
    }
}
fn installed_profile() -> Result<Profile> {
    #[cfg(windows)]
    if setup_format()? {
        return super::windows_anchor::InstallationAnchor::for_executable(
            &std::env::current_exe()?,
            trust::package_id(),
        )?
        .map(|anchor| anchor.profile())
        .ok_or(Error::InstallationUnavailable);
    }
    Ok(Profile::Complete)
}
fn cache_directory() -> Result<PathBuf> {
    #[cfg(windows)]
    if setup_format()? {
        let anchor = super::windows_anchor::InstallationAnchor::for_executable(
            &std::env::current_exe()?,
            trust::package_id(),
        )?
        .ok_or(Error::InstallationUnavailable)?;
        return Ok(anchor.coordination().join("downloads"));
    }
    if let Some(root) = option_env!("NOH_UPDATE_QUALIFICATION_ROOT") {
        return Ok(std::fs::canonicalize(root)?.join("desktop-downloads"));
    }
    let base = crate::i18n::preferences_path().ok_or(Error::InstallationUnavailable)?;
    Ok(base.parent().ok_or(Error::Metadata)?.join("updates"))
}

fn setup_format() -> Result<bool> {
    match option_env!("NOH_UPDATE_FORMAT").unwrap_or("nupkg") {
        "nupkg" => Ok(false),
        "windows-setup" => Ok(true),
        _ => Err(Error::Metadata),
    }
}

enum Command {
    Check,
    #[cfg(windows)]
    ImportQualificationArchive(PathBuf),
    Download,
    Discard,
    #[cfg(windows)]
    PrepareWindows(Option<PathBuf>),
    #[cfg(windows)]
    CommitShutdown,
}
pub struct DesktopUpdater {
    sender: Option<mpsc::SyncSender<Command>>,
    snapshot: Arc<Mutex<Snapshot>>,
    active: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    /// Serializes accepted cancellation with terminal publication/active release.
    transition: Arc<Mutex<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
    committing: Arc<AtomicBool>,
}
impl DesktopUpdater {
    pub fn configured() -> bool {
        option_env!("NOH_UPDATE_FEED_URL").is_some()
    }
    pub fn from_build(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start(Options::compiled, wake)
    }
    pub fn with_options(options: Options, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start(move || Ok(options), wake)
    }
    fn start(
        initialize: impl FnOnce() -> Result<Options> + Send + 'static,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let active = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let transition = Arc::new(Mutex::new(()));
        let committing = Arc::new(AtomicBool::new(false));
        let commit_in_progress = committing.clone();
        let view = snapshot.clone();
        let busy = active.clone();
        let cancel = cancelled.clone();
        let operation = transition.clone();
        let worker = std::thread::spawn(move || {
            // Even client/TLS initialization takes place only after user action.
            let Ok(first) = receiver.recv() else { return };
            let options = match initialize() {
                Ok(options) => options,
                Err(_) => {
                    view.lock().unwrap().failure = Some(Failure::Configuration);
                    view.lock().unwrap().state = State::Failed {
                        message: "Update configuration is unavailable".into(),
                    };
                    busy.store(false, Ordering::Release);
                    wake();
                    return;
                }
            };
            #[cfg(windows)]
            let previous_context = (
                options.keys.clone(),
                options.package_id.clone(),
                options.target,
                options.channel,
                options.current.clone(),
                if options.setup {
                    options
                        .cache
                        .parent()
                        .unwrap_or(&options.cache)
                        .join("retained")
                } else {
                    options.cache.join("retained")
                },
            );
            let mut controller = Controller::new(
                options.transport,
                options.keys,
                options.package_id,
                options.target,
                options.channel,
                options.current,
            );
            if options.setup {
                controller = controller.with_setup().with_profile(options.profile);
            }
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => {
                    view.lock().unwrap().failure = Some(Failure::Other);
                    view.lock().unwrap().state = State::Failed {
                        message: "Update worker is unavailable".into(),
                    };
                    busy.store(false, Ordering::Release);
                    wake();
                    return;
                }
            };
            let mut command = Some(first);
            #[cfg(windows)]
            let mut handoff: Option<super::windows_handoff::WindowsHandoff> = None;
            #[cfg(windows)]
            let mut previous_archive = None;
            let mut shutdown_committed = false;
            loop {
                let command = match command.take().or_else(|| receiver.recv().ok()) {
                    Some(command) => command,
                    None => break,
                };
                let checking = matches!(command, Command::Check);
                let mut result = match command {
                    Command::Check => {
                        #[cfg(windows)]
                        {
                            handoff.take();
                            previous_archive = None;
                        }
                        view.lock().unwrap().state = State::Checking;
                        wake();
                        runtime.block_on(controller.check(&options.feed_url, &cancel))
                    }
                    #[cfg(windows)]
                    Command::ImportQualificationArchive(archive) => {
                        handoff.take();
                        previous_archive = None;
                        view.lock().unwrap().state = State::Verifying;
                        wake();
                        controller.import_qualification_archive(&archive, &options.cache, &cancel)
                    }
                    Command::Download => {
                        runtime.block_on(controller.download(&options.cache, &cancel, |state| {
                            if matches!(state, State::Ready { .. }) {
                                // Readiness is published only by the coordinated
                                // terminal transition, never by a progress callback.
                                return;
                            }
                            view.lock().unwrap().state = state.clone();
                            wake();
                        }))
                    }
                    Command::Discard => {
                        #[cfg(windows)]
                        {
                            handoff.take();
                            previous_archive = None;
                        }
                        controller.discard();
                        Ok(())
                    }
                    #[cfg(windows)]
                    Command::PrepareWindows(previous) => {
                        view.lock().unwrap().state = State::Applying;
                        wake();
                        controller.take_ready().and_then(|download| {
                            let previous = previous
                                .or_else(|| previous_archive.clone())
                                .ok_or(Error::InstallationUnavailable)?;
                            super::windows_handoff::WindowsHandoff::prepare_installed(
                                download,
                                &previous,
                                Duration::from_secs(30),
                                &cancel,
                            )
                            .map(|prepared| {
                                handoff = Some(prepared);
                            })
                        })
                    }
                    #[cfg(windows)]
                    Command::CommitShutdown => {
                        let result = handoff
                            .take()
                            .ok_or(Error::Target)
                            .and_then(|handoff| handoff.release_for_shutdown());
                        match result {
                            Ok(detached) => {
                                // The guardian owns persistent inputs from this point.
                                // Closing our process handle never cancels that owner.
                                drop(detached);
                                shutdown_committed = true;
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    }
                };
                #[cfg(windows)]
                if result.is_ok()
                    && matches!(controller.state(), State::Ready { .. })
                    && (options.setup || option_env!("NOH_UPDATE_QUALIFICATION_ROOT").is_some())
                {
                    let (keys, package_id, target, channel, current, archive) = &previous_context;
                    let storage = if options.setup {
                        super::retention::SETUP.for_profile(options.profile)
                    } else {
                        super::retention::NUPKG
                    };
                    match storage.find_candidate(
                        archive, keys, package_id, *target, *channel, current, &cancel,
                    ) {
                        Ok(candidate) => previous_archive = candidate,
                        Err(error) => result = Err(error),
                    }
                }
                #[cfg(test)]
                if let Some(before_publish) = &options.before_publish {
                    before_publish(controller.state());
                }
                // Cancellation accepted while active must win even after finish()
                // persisted the package. A canceller after this transition instead
                // queues Discard; there is no accepted-but-lost cancellation window.
                {
                    let mut transition = operation.lock().unwrap();
                    if cancel.load(Ordering::Relaxed) && !commit_in_progress.load(Ordering::Acquire)
                    {
                        // Cleanup is synchronous filesystem work: never hold the
                        // UI command gate while removing the owned directory.
                        drop(transition);
                        controller.discard();
                        #[cfg(windows)]
                        {
                            handoff.take();
                            previous_archive = None;
                        }
                        result = Err(Error::Cancelled);
                        transition = operation.lock().unwrap();
                    }
                    let mut snapshot = view.lock().unwrap();
                    snapshot.state = controller.state().clone();
                    #[cfg(windows)]
                    {
                        snapshot.awaiting_shutdown = handoff.is_some();
                        snapshot.previous_available = previous_archive.is_some();
                    }
                    snapshot.shutdown_committed = shutdown_committed;
                    if snapshot.awaiting_shutdown || shutdown_committed {
                        snapshot.state = State::Applying;
                    } else if let Err(error) = &result {
                        if !matches!(error, Error::Cancelled) {
                            snapshot.state = State::Failed {
                                message: "Update operation failed".into(),
                            };
                        }
                    }
                    snapshot.failure = result
                        .as_ref()
                        .err()
                        .filter(|error| !matches!(error, Error::Cancelled))
                        .map(Failure::from_error);
                    snapshot.retry_at = controller.retry_deadline();
                    if checking {
                        snapshot.checked = result.is_ok();
                    } else if matches!(snapshot.state, State::Idle) {
                        snapshot.checked = false;
                    }
                    busy.store(false, Ordering::Release);
                    drop(snapshot);
                    if !shutdown_committed {
                        commit_in_progress.store(false, Ordering::Release);
                    }
                    drop(transition);
                }
                wake();
            }
            // Dropping this owner discards Ready or partial staged packages.
        });
        Self {
            sender: Some(sender),
            snapshot,
            active,
            cancelled,
            transition,
            worker: Some(worker),
            committing,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.snapshot.lock().unwrap().clone();
        if self.worker_finished() && !matches!(snapshot.state, State::Failed { .. }) {
            snapshot.state = State::Failed {
                message: "Update worker stopped unexpectedly".into(),
            };
            snapshot.failure = Some(Failure::Other);
            snapshot.checked = false;
        }
        snapshot
    }
    pub fn busy(&self) -> bool {
        self.active.load(Ordering::Acquire) && !self.worker_finished()
    }
    pub fn worker_finished(&self) -> bool {
        self.worker
            .as_ref()
            .is_none_or(|worker| worker.is_finished())
    }
    fn submit(&self, command: Command) -> bool {
        let _transition = self.transition.lock().unwrap();
        self.submit_locked(command)
    }
    fn submit_locked(&self, command: Command) -> bool {
        if self.committing.load(Ordering::Acquire)
            || self.snapshot.lock().unwrap().shutdown_committed
            || self.worker_finished()
            || self
                .active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        self.cancelled.store(false, Ordering::Relaxed);
        if self.sender.as_ref().unwrap().try_send(command).is_err() {
            self.active.store(false, Ordering::Release);
            return false;
        }
        true
    }
    pub fn check(&self) -> bool {
        self.submit(Command::Check)
    }
    #[cfg(windows)]
    pub fn import_qualification_archive(&self, archive: PathBuf) -> bool {
        trust::qualification_enabled() && self.submit(Command::ImportQualificationArchive(archive))
    }
    pub fn download(&self) -> bool {
        matches!(self.snapshot().state, State::Available { .. }) && self.submit(Command::Download)
    }
    pub fn cancel(&self) -> bool {
        let _transition = self.transition.lock().unwrap();
        if self.committing.load(Ordering::Acquire) {
            return false;
        }
        if self.busy() {
            self.cancelled.store(true, Ordering::Relaxed);
            true
        } else {
            self.submit_locked(Command::Discard)
        }
    }
    #[cfg(windows)]
    pub fn prepare_windows(&self, previous_archive: PathBuf) -> bool {
        matches!(self.snapshot().state, State::Ready { .. })
            && self.submit(Command::PrepareWindows(Some(previous_archive)))
    }
    #[cfg(windows)]
    pub fn prepare_windows_auto(&self) -> bool {
        matches!(self.snapshot().state, State::Ready { .. })
            && self.submit(Command::PrepareWindows(None))
    }
    #[cfg(windows)]
    pub fn commit_for_shutdown(&self) -> bool {
        let _transition = self.transition.lock().unwrap();
        if !self.snapshot.lock().unwrap().awaiting_shutdown {
            return false;
        }
        if !self.submit_locked(Command::CommitShutdown) {
            return false;
        }
        // Serialization with terminal publication makes commitment uninterruptible.
        self.committing.store(true, Ordering::Release);
        true
    }
}
impl Drop for DesktopUpdater {
    fn drop(&mut self) {
        {
            let _transition = self.transition.lock().unwrap();
            if !self.committing.load(Ordering::Acquire) {
                self.cancelled.store(true, Ordering::Relaxed);
            }
        }
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
