//! The NOH windows the agent opens and drives (`noh_app_open`,
//! `noh_app_close`): ordinary NOH applications started as children of this
//! server with `--driven`, reached only through their standard pipes
//! (`crate::driven`). Nothing listens; no window can be attached to that this
//! server did not open.
//!
//! The server never waits on a window: lines go out through a bounded queue,
//! and a window that has not read for 5 s is `lost`. A window outlives the
//! session: at the end the server says `session_end` and closes its pipes,
//! neither killing nor waiting for it, and the window becomes an ordinary NOH.
use super::{machine, types::*};
use crate::driven::{self, Outbox, ToApp, ToServer, Unsent};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

/// Lines queued for one window at most.
const QUEUE: usize = 64;
/// Closed or ended windows kept for the agent to read about.
const KEPT_ENDED: usize = 16;

/// How long the server waits, at most.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// For a new window's `ready` (note §3.3: 3 s).
    pub ready: Duration,
    /// For a window's answer to a request.
    pub answer: Duration,
    /// For a window that answered « closed » to end.
    pub exit: Duration,
    /// Without reading, after which a window is `lost` (note §2.2: 5 s).
    pub lost: Duration,
    /// For the last lines to reach the windows at the session's end.
    pub drain: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ready: Duration::from_secs(3),
            answer: Duration::from_secs(10),
            exit: Duration::from_secs(5),
            lost: Duration::from_secs(5),
            drain: Duration::from_secs(1),
        }
    }
}

/// A window's process, as the registry uses it.
pub trait Process: Send {
    fn pid(&self) -> u32;
    /// `Some(code)` once the process has ended (`code` is `None` when killed
    /// by a signal).
    fn exited(&mut self) -> Option<Option<i32>>;
    fn kill(&mut self);
    fn private_memory(&self) -> Option<u64>;
}

/// A window just started: its process and the two ends of its channel.
pub struct Launched {
    pub process: Box<dyn Process>,
    pub input: Box<dyn Write + Send>,
    pub output: Box<dyn Read + Send>,
}

/// What starts a window.
pub trait Launch: Send + Sync {
    fn launch(&self, title: &str) -> Result<Launched, AppError>;
}

/// The NOH application installed beside this server.
pub struct Beside;

impl Launch for Beside {
    fn launch(&self, title: &str) -> Result<Launched, AppError> {
        let server = std::env::current_exe()
            .map_err(|e| AppError::new("app.not_found", format!("This server's path: {e}")))?;
        let program = app_beside(&server).ok_or_else(|| {
            AppError::new(
                "app.not_found",
                "The NOH application was not found beside this server. Restore the complete NOH folder.",
            )
        })?;
        if cfg!(target_os = "linux")
            && std::env::var_os("DISPLAY").is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_none()
        {
            return Err(AppError::new(
                "app.no_display",
                "No display is available to this server: a NOH window cannot open.",
            ));
        }
        spawn(&program, title)
    }
}

/// The application beside the server at `server`: in the server's folder, or
/// in its parent when that folder is `bin` (the bundles keep the server in
/// `bin/`, `fx::catalog::bundled_dir_for`). `noh-app` first: in a development
/// build `noh.exe` is the command line; in the Windows bundle the app is
/// `noh.exe`. Linux starts `usr/lib/noh/noh-app` itself: the AppDir's
/// `AppRun` only runs it. Never PATH, never a shell.
pub fn app_beside(server: &Path) -> Option<PathBuf> {
    let folder = server.parent()?;
    let folder = if folder.file_name() == Some(std::ffi::OsStr::new("bin")) {
        folder.parent().unwrap_or(folder)
    } else {
        folder
    };
    let mut names = vec![format!("noh-app{}", std::env::consts::EXE_SUFFIX)];
    if cfg!(windows) {
        names.push("noh.exe".into());
    }
    names
        .into_iter()
        .map(|name| folder.join(name))
        .find(|path| path.is_file())
}

struct Child(std::process::Child);

impl Process for Child {
    fn pid(&self) -> u32 {
        self.0.id()
    }
    fn exited(&mut self) -> Option<Option<i32>> {
        match self.0.try_wait() {
            Ok(Some(status)) => Some(status.code()),
            Ok(None) => None,
            // Nothing more can be learnt about it.
            Err(_) => Some(None),
        }
    }
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
    fn private_memory(&self) -> Option<u64> {
        machine::private_memory(&self.0)
    }
}

/// Start `program --driven --title <title>`: input and output piped, errors to
/// nothing (inherited, they would hold the client's error pipe open). Not in
/// this process's kill-on-close job (`process.rs`): the window outlives the
/// server on purpose. Windows: out of the client's job when that job allows
/// it, so the client closing its job leaves the window. Unix: a process
/// group of its own, so a signal to the server's group leaves it.
fn spawn(program: &Path, title: &str) -> Result<Launched, AppError> {
    let command = || {
        let mut command = Command::new(program);
        command
            .arg(driven::ARGUMENT)
            .arg(driven::TITLE_ARGUMENT)
            .arg(title)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command
    };
    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command()
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            // A job that allows no breakaway refuses the flag: stay in it.
            .or_else(|_| command().spawn())
    };
    #[cfg(not(windows))]
    let spawned = command().spawn();
    let mut child = spawned.map_err(|e| {
        let mut error = AppError::new("app.start_failed", "The NOH window could not be started.");
        error.details = Some(serde_json::json!({ "technical": e.to_string() }));
        error
    })?;
    let input = child.stdin.take().expect("piped input");
    let output = child.stdout.take().expect("piped output");
    Ok(Launched {
        process: Box::new(Child(child)),
        input: Box::new(input),
        output: Box::new(output),
    })
}

struct Window {
    id: String,
    title: String,
    pid: u32,
    process: Box<dyn Process>,
    outbox: Option<Outbox>,
    version: Option<String>,
    answers: HashMap<u64, (Option<serde_json::Value>, Option<driven::Refusal>)>,
    released: bool,
    closed_by_person: bool,
    closed_by_agent: bool,
    overflowed: bool,
    pipe_ended: bool,
    exit: Option<Option<i32>>,
}

impl Window {
    fn refresh(&mut self) {
        if self.exit.is_none() {
            self.exit = self.process.exited();
        }
    }
    fn lost(&self, timing: &Timing) -> bool {
        self.overflowed
            || self
                .outbox
                .as_ref()
                .and_then(Outbox::stalled_for)
                .is_some_and(|stalled| stalled >= timing.lost)
    }
    fn state(&self, timing: &Timing) -> AppState {
        if self.closed_by_person || self.closed_by_agent {
            AppState::Closed
        } else if self.version.is_none() {
            AppState::Opening
        } else if self.released {
            AppState::Released
        } else if self.pipe_ended || self.exit.is_some() {
            AppState::Exited
        } else if self.lost(timing) {
            AppState::Lost
        } else {
            AppState::Open
        }
    }
    fn summary(&self, timing: &Timing) -> AppSummary {
        AppSummary {
            app_id: self.id.clone(),
            title: self.title.clone(),
            state: self.state(timing),
            pid: self.pid,
        }
    }
    fn send(&mut self, message: &ToApp) -> Result<(), Unsent> {
        let result = self
            .outbox
            .as_ref()
            .map_or(Err(Unsent::Gone), |o| o.send(message));
        if result == Err(Unsent::Full) {
            self.overflowed = true;
        }
        result
    }
}

struct State {
    windows: Vec<Window>,
    next_window: u64,
    next_request: u64,
    ended: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// One open at a time: each open's cap sees the windows opened before.
    opening: Mutex<()>,
    launch: Box<dyn Launch>,
    read_machine: Box<dyn Fn() -> machine::Machine + Send + Sync>,
    timing: Timing,
}

/// This session's windows.
#[derive(Clone)]
pub struct Apps(Arc<Shared>);

impl Default for Apps {
    fn default() -> Self {
        Self::new(Box::new(Beside), Box::new(machine::read), Timing::default())
    }
}

impl Apps {
    pub fn new(
        launch: Box<dyn Launch>,
        read_machine: Box<dyn Fn() -> machine::Machine + Send + Sync>,
        timing: Timing,
    ) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                windows: Vec::new(),
                next_window: 1,
                next_request: 1,
                ended: false,
            }),
            changed: Condvar::new(),
            opening: Mutex::new(()),
            launch,
            read_machine,
            timing,
        }))
    }

    /// Open a window, if the machine has room for one more (`machine::admit`).
    /// Blocking: up to `Timing::ready` for the window to say it is ready.
    pub fn open(&self, title: &str) -> Result<AppOpenReply, AppError> {
        let shared = &self.0;
        let title = driven::clean_title(title);
        let _opening = shared.opening.lock().unwrap();
        let pids = {
            let mut state = shared.state.lock().unwrap();
            if state.ended {
                return Err(ended());
            }
            state.windows.iter_mut().for_each(Window::refresh);
            state
                .windows
                .iter()
                .filter(|w| w.exit.is_none())
                .map(|w| w.pid)
                .collect::<Vec<_>>()
        };
        // Read outside the lock: tens of milliseconds.
        let machine = (shared.read_machine)();
        let usage = {
            let state = shared.state.lock().unwrap();
            state
                .windows
                .iter()
                .filter(|w| w.exit.is_none() && pids.contains(&w.pid))
                .map(|w| machine::Usage {
                    private: w.process.private_memory(),
                    vram: machine.vram_by_pid.get(&w.pid).copied(),
                })
                .collect::<Vec<_>>()
        };
        machine::admit(&machine, &usage).map_err(|refusal| AppError {
            code: "app.cap_reached".into(),
            detail: refusal.message(),
            details: serde_json::to_value(&refusal).ok(),
        })?;
        let launched = shared.launch.launch(&title)?;
        let pid = launched.process.pid();
        let id = {
            let mut state = shared.state.lock().unwrap();
            let id = format!("app-{}", state.next_window);
            state.next_window += 1;
            state.windows.push(Window {
                id: id.clone(),
                title: title.clone(),
                pid,
                process: launched.process,
                outbox: Some(Outbox::spawn("noh-app-writer", launched.input, QUEUE)),
                version: None,
                answers: HashMap::new(),
                released: false,
                closed_by_person: false,
                closed_by_agent: false,
                overflowed: false,
                pipe_ended: false,
                exit: None,
            });
            id
        };
        let (on_line, on_end) = (self.clone(), self.clone());
        let (line_id, end_id) = (id.clone(), id.clone());
        driven::spawn_reader(
            "noh-app-reader",
            launched.output,
            driven::MAX_TO_SERVER,
            move |line| on_line.received(&line_id, &line),
            move || on_end.pipe_ended(&end_id),
        );
        let deadline = Instant::now() + shared.timing.ready;
        let mut state = shared.state.lock().unwrap();
        loop {
            let window = find(&mut state.windows, &id).expect("an opening window is kept");
            window.refresh();
            if window.version.is_some() || window.pipe_ended || window.exit.is_some() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            state = shared
                .changed
                .wait_timeout(state, (deadline - now).min(Duration::from_millis(50)))
                .unwrap()
                .0;
        }
        let window = find(&mut state.windows, &id).expect("an opening window is kept");
        let version = window.version.clone();
        let failure = match &version {
            None => {
                let exit = window.exit;
                Some(AppError {
                    code: "app.no_ready".into(),
                    detail: match exit {
                        Some(_) => "The NOH window ended before it was ready.".into(),
                        None => format!(
                            "The NOH window did not say it was ready within {} s.",
                            shared.timing.ready.as_secs_f32()
                        ),
                    },
                    details: exit.map(|code| serde_json::json!({ "exit_code": code })),
                })
            }
            Some(v) if v != crate::build_info::VERSION => Some(AppError::new(
                "app.version_mismatch",
                format!(
                    "The NOH window beside this server is version {v}, the server {}.",
                    crate::build_info::VERSION
                ),
            )),
            Some(_) => None,
        };
        if let Some(failure) = failure {
            // Never a window to anyone: it is ended and forgotten.
            window.process.kill();
            if let Some(outbox) = &mut window.outbox {
                outbox.close(Duration::ZERO);
            }
            state.windows.retain(|w| w.id != id);
            return Err(failure);
        }
        let timing = shared.timing;
        let window = find(&mut state.windows, &id).expect("a ready window is kept");
        let reply_state = window.state(&timing);
        Ok(AppOpenReply {
            version: 1,
            app_id: id,
            state: reply_state,
            pid,
            app_version: version.unwrap_or_default(),
            windows: state.windows.iter().map(|w| w.summary(&timing)).collect(),
        })
    }

    /// Close a window the agent still drives. Pending lyrics are written
    /// first; nothing is ever discarded: a question open, or one the save
    /// would ask, is answered with `app.question_open` and nothing closes.
    pub fn close(&self, app_id: &str) -> Result<AppCloseReply, AppError> {
        let shared = &self.0;
        let timing = shared.timing;
        let mut state = shared.state.lock().unwrap();
        if state.ended {
            return Err(ended());
        }
        let request = state.next_request;
        state.next_request += 1;
        let window = find(&mut state.windows, app_id).ok_or_else(|| unknown(app_id))?;
        window.refresh();
        match window.state(&timing) {
            AppState::Open => {}
            other => return Err(not_driven(other, window.closed_by_person)),
        }
        match window.send(&ToApp::Close { id: request }) {
            Ok(()) => {}
            Err(Unsent::Full) => return Err(not_driven(AppState::Lost, false)),
            Err(Unsent::Gone) => return Err(not_driven(AppState::Exited, false)),
        }
        let deadline = Instant::now() + timing.answer;
        let answer = loop {
            let window = find(&mut state.windows, app_id).expect("windows are not removed");
            window.refresh();
            if let Some(answer) = window.answers.remove(&request) {
                break Some(answer);
            }
            if window.pipe_ended || window.exit.is_some() {
                return Err(not_driven(window.state(&timing), window.closed_by_person));
            }
            if window.released || window.closed_by_person || window.lost(&timing) {
                return Err(not_driven(window.state(&timing), window.closed_by_person));
            }
            let now = Instant::now();
            if now >= deadline {
                break None;
            }
            state = shared
                .changed
                .wait_timeout(state, (deadline - now).min(Duration::from_millis(50)))
                .unwrap()
                .0;
        };
        let Some((_, refusal)) = answer else {
            return Err(AppError::new(
                "app.no_answer",
                format!(
                    "The window did not answer within {} s; it may be busy. Try again.",
                    timing.answer.as_secs()
                ),
            ));
        };
        if let Some(refusal) = refusal {
            return Err(AppError {
                code: refusal.code,
                detail: refusal.message,
                details: refusal.details,
            });
        }
        let window = find(&mut state.windows, app_id).expect("windows are not removed");
        window.closed_by_agent = true;
        let deadline = Instant::now() + timing.exit;
        loop {
            let window = find(&mut state.windows, app_id).expect("windows are not removed");
            window.refresh();
            let now = Instant::now();
            if window.exit.is_some() || now >= deadline {
                break;
            }
            state = shared
                .changed
                .wait_timeout(state, (deadline - now).min(Duration::from_millis(50)))
                .unwrap()
                .0;
        }
        let window = find(&mut state.windows, app_id).expect("windows are not removed");
        let exited = window.exit.is_some();
        if let Some(outbox) = &mut window.outbox {
            // Its last line is read; the window is no longer ours to talk to.
            outbox.close(Duration::ZERO);
        }
        forget_old(&mut state.windows);
        Ok(AppCloseReply {
            version: 1,
            app_id: app_id.into(),
            state: AppState::Closed,
            exited,
        })
    }

    /// The session ends: no window opens any more; each window still reading
    /// is told, then its pipes are closed. None is killed or waited for: they
    /// stay open, as ordinary NOH windows.
    pub fn end_session(&self) {
        let shared = &self.0;
        let mut outboxes = Vec::new();
        {
            let mut state = shared.state.lock().unwrap();
            state.ended = true;
            for window in &mut state.windows {
                window.refresh();
                if !window.pipe_ended && window.exit.is_none() {
                    let _ = window.send(&ToApp::SessionEnd);
                }
                outboxes.extend(window.outbox.take());
            }
        }
        let deadline = Instant::now() + shared.timing.drain;
        for mut outbox in outboxes {
            outbox.close(deadline.saturating_duration_since(Instant::now()));
        }
    }

    /// Every window of this session.
    pub fn windows(&self) -> Vec<AppSummary> {
        let timing = self.0.timing;
        let mut state = self.0.state.lock().unwrap();
        state.windows.iter_mut().for_each(Window::refresh);
        state.windows.iter().map(|w| w.summary(&timing)).collect()
    }

    fn received(&self, id: &str, line: &[u8]) {
        let Some(message) = driven::parse_to_server(line) else {
            return;
        };
        let mut state = self.0.state.lock().unwrap();
        let Some(window) = find(&mut state.windows, id) else {
            return;
        };
        match message {
            ToServer::Ready { version, .. } => window.version = Some(version),
            ToServer::Done { id, result, error } => {
                // Bounded: only answers to requests sent are kept.
                if window.answers.len() < QUEUE {
                    window.answers.insert(id, (result, error));
                }
            }
            ToServer::Released => window.released = true,
            ToServer::Closed => window.closed_by_person = true,
        }
        self.0.changed.notify_all();
    }

    fn pipe_ended(&self, id: &str) {
        let mut state = self.0.state.lock().unwrap();
        if let Some(window) = find(&mut state.windows, id) {
            window.pipe_ended = true;
            window.refresh();
        }
        self.0.changed.notify_all();
    }
}

fn find<'a>(windows: &'a mut [Window], id: &str) -> Option<&'a mut Window> {
    windows.iter_mut().find(|w| w.id == id)
}

/// Keep the most recent ended windows only; a window still running is kept.
fn forget_old(windows: &mut Vec<Window>) {
    let ended = windows.iter().filter(|w| w.exit.is_some()).count();
    let mut extra = ended.saturating_sub(KEPT_ENDED);
    windows.retain(|w| {
        if extra > 0 && w.exit.is_some() {
            extra -= 1;
            false
        } else {
            true
        }
    });
}

fn ended() -> AppError {
    AppError::new("app.session_ended", "This session has ended.")
}

fn unknown(app_id: &str) -> AppError {
    AppError::new(
        "app.unknown",
        format!("No window {app_id} was opened in this session."),
    )
}

fn not_driven(state: AppState, by_person: bool) -> AppError {
    match state {
        AppState::Closed if by_person => AppError::new(
            "app.closed",
            "The person closed this window. Do not reopen it unless asked.",
        ),
        AppState::Closed => AppError::new("app.closed", "This window is already closed."),
        AppState::Released => AppError::new(
            "app.released_by_user",
            "The person stopped the piloting: this window is theirs now, as an ordinary NOH.",
        ),
        AppState::Lost => AppError::new(
            "app.lost",
            "This window has not read for 5 s. Its work may still be running; try again later.",
        ),
        AppState::Exited => AppError::new("app.exited", "This window ended."),
        AppState::Opening | AppState::Open => {
            AppError::new("app.no_answer", "This window did not answer.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };

    #[derive(Clone, Default)]
    struct FakeProcess {
        exit: Arc<Mutex<Option<Option<i32>>>>,
        killed: Arc<Mutex<bool>>,
    }
    impl Process for FakeProcess {
        fn pid(&self) -> u32 {
            4242
        }
        fn exited(&mut self) -> Option<Option<i32>> {
            *self.exit.lock().unwrap()
        }
        fn kill(&mut self) {
            *self.killed.lock().unwrap() = true;
            *self.exit.lock().unwrap() = Some(None);
        }
        fn private_memory(&self) -> Option<u64> {
            Some(127 * machine::MIB)
        }
    }

    /// What a fake window does with its channel.
    type Script =
        Arc<dyn Fn(Box<dyn BufRead + Send>, Box<dyn Write + Send>, FakeProcess) + Send + Sync>;

    struct FakeLaunch {
        script: Script,
        launched: Arc<AtomicUsize>,
        processes: Arc<Mutex<Vec<FakeProcess>>>,
        /// The input end the server writes to, when given (a stalled reader).
        input: Mutex<Option<Box<dyn Write + Send>>>,
    }
    impl Launch for FakeLaunch {
        fn launch(&self, _: &str) -> Result<Launched, AppError> {
            self.launched.fetch_add(1, Ordering::SeqCst);
            let (to_app_read, to_app_write) = std::io::pipe().unwrap();
            let (to_server_read, to_server_write) = std::io::pipe().unwrap();
            let process = FakeProcess::default();
            self.processes.lock().unwrap().push(process.clone());
            let script = self.script.clone();
            let p = process.clone();
            thread::spawn(move || {
                script(
                    Box::new(BufReader::new(to_app_read)),
                    Box::new(to_server_write),
                    p,
                )
            });
            let input = self
                .input
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Box::new(to_app_write));
            Ok(Launched {
                process: Box::new(process),
                input,
                output: Box::new(to_server_read),
            })
        }
    }

    fn send(out: &mut dyn Write, message: &ToServer) {
        out.write_all(&driven::line(message)).unwrap();
        out.flush().unwrap();
    }
    fn ready(out: &mut dyn Write) {
        send(
            out,
            &ToServer::Ready {
                pid: 4242,
                version: crate::build_info::VERSION.into(),
            },
        );
    }
    fn next(input: &mut dyn BufRead) -> Option<driven::Inbound> {
        let mut line = String::new();
        (input.read_line(&mut line).ok()? > 0)
            .then(|| driven::parse_to_app(line.trim().as_bytes()))?
    }

    fn timing() -> Timing {
        Timing {
            ready: Duration::from_millis(500),
            answer: Duration::from_secs(2),
            exit: Duration::from_millis(500),
            lost: Duration::from_millis(150),
            drain: Duration::from_millis(500),
        }
    }

    fn roomy() -> machine::Machine {
        machine::Machine {
            memory: Some(machine::Memory {
                available: 20 << 30,
                total: 32 << 30,
            }),
            vram_free: Some(6 << 30),
            vram_by_pid: HashMap::new(),
        }
    }

    struct Harness {
        apps: Apps,
        launched: Arc<AtomicUsize>,
        processes: Arc<Mutex<Vec<FakeProcess>>>,
    }
    fn harness(
        script: impl Fn(Box<dyn BufRead + Send>, Box<dyn Write + Send>, FakeProcess)
        + Send
        + Sync
        + 'static,
        machine: machine::Machine,
        input: Option<Box<dyn Write + Send>>,
    ) -> Harness {
        let launched = Arc::new(AtomicUsize::new(0));
        let processes = Arc::new(Mutex::new(Vec::new()));
        let apps = Apps::new(
            Box::new(FakeLaunch {
                script: Arc::new(script),
                launched: launched.clone(),
                processes: processes.clone(),
                input: Mutex::new(input),
            }),
            Box::new(move || machine.clone()),
            timing(),
        );
        Harness {
            apps,
            launched,
            processes,
        }
    }

    /// A window that is ready, closes when asked, and reports the rest.
    fn obliging(
        seen: Arc<Mutex<Vec<driven::Inbound>>>,
    ) -> impl Fn(Box<dyn BufRead + Send>, Box<dyn Write + Send>, FakeProcess) + Send + Sync {
        move |mut input, mut output, process| {
            ready(&mut output);
            while let Some(message) = next(&mut input) {
                seen.lock().unwrap().push(message.clone());
                if let driven::Inbound::Message(ToApp::Close { id }) = message {
                    send(
                        &mut output,
                        &ToServer::Done {
                            id,
                            result: Some(serde_json::json!({ "state": "closed" })),
                            error: None,
                        },
                    );
                    *process.exit.lock().unwrap() = Some(Some(0));
                    return;
                }
            }
            seen.lock()
                .unwrap()
                .push(driven::Inbound::Unsupported { id: None }); // EOF mark
        }
    }

    #[test]
    fn a_window_opens_when_ready_and_closes_when_asked() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let h = harness(obliging(seen.clone()), roomy(), None);
        let opened = h.apps.open("Short of the day").unwrap();
        assert_eq!(opened.app_id, "app-1");
        assert_eq!(opened.state, AppState::Open);
        assert_eq!(opened.app_version, crate::build_info::VERSION);
        assert_eq!(opened.windows.len(), 1);
        assert_eq!(opened.windows[0].title, "Short of the day");
        let closed = h.apps.close("app-1").unwrap();
        assert_eq!(closed.state, AppState::Closed);
        assert!(closed.exited);
        assert_eq!(
            seen.lock().unwrap()[0],
            driven::Inbound::Message(ToApp::Close { id: 1 })
        );
        // Closed now: a second close says so, an unknown id too.
        assert_eq!(h.apps.close("app-1").unwrap_err().code, "app.closed");
        assert_eq!(h.apps.close("app-9").unwrap_err().code, "app.unknown");
    }

    #[test]
    fn a_window_that_never_says_ready_is_ended_and_forgotten() {
        let h = harness(
            |input, _output, _| {
                // Holds its pipes, says nothing.
                let _ = input.bytes().count();
            },
            roomy(),
            None,
        );
        let start = Instant::now();
        let error = h.apps.open("Silent").unwrap_err();
        assert_eq!(error.code, "app.no_ready");
        assert!(start.elapsed() >= timing().ready);
        assert!(*h.processes.lock().unwrap()[0].killed.lock().unwrap());
        assert!(h.apps.windows().is_empty());
    }

    #[test]
    fn the_cap_refuses_before_anything_starts() {
        let small = machine::Machine {
            memory: Some(machine::Memory {
                available: 2 << 30,
                total: 8 << 30,
            }),
            vram_free: None,
            vram_by_pid: HashMap::new(),
        };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let h = harness(obliging(seen), small, None);
        let error = h.apps.open("One too many").unwrap_err();
        assert_eq!(error.code, "app.cap_reached");
        assert!(error.detail.contains("GiB free"), "{}", error.detail);
        assert_eq!(error.details.unwrap()["resource"], "memory");
        assert_eq!(h.launched.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_refused_close_passes_the_window_reason_and_question_through() {
        let h = harness(
            |mut input, mut output, _| {
                ready(&mut output);
                while let Some(message) = next(&mut input) {
                    if let driven::Inbound::Message(ToApp::Close { id }) = message {
                        let mut refusal =
                            driven::Refusal::new("app.question_open", "A question is open.");
                        refusal.details = Some(serde_json::json!({ "question": "first_edit" }));
                        send(
                            &mut output,
                            &ToServer::Done {
                                id,
                                result: None,
                                error: Some(refusal),
                            },
                        );
                    }
                }
            },
            roomy(),
            None,
        );
        h.apps.open("Asks").unwrap();
        let error = h.apps.close("app-1").unwrap_err();
        assert_eq!(error.code, "app.question_open");
        assert_eq!(error.details.unwrap()["question"], "first_edit");
        assert_eq!(h.apps.windows()[0].state, AppState::Open);
    }

    #[test]
    fn released_and_closed_by_the_person_are_reflected() {
        let h = harness(
            |mut input, mut output, process| {
                ready(&mut output);
                send(&mut output, &ToServer::Released);
                // The person then closes it.
                thread::sleep(Duration::from_millis(100));
                send(&mut output, &ToServer::Closed);
                *process.exit.lock().unwrap() = Some(Some(0));
                let _ = next(&mut input);
            },
            roomy(),
            None,
        );
        h.apps.open("Taken back").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while h.apps.windows()[0].state != AppState::Released && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            h.apps.close("app-1").unwrap_err().code,
            "app.released_by_user"
        );
        while h.apps.windows()[0].state != AppState::Closed && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let error = h.apps.close("app-1").unwrap_err();
        assert_eq!(error.code, "app.closed");
        assert!(error.detail.contains("person closed"));
    }

    /// An input end that never takes a byte: a window that stopped reading.
    struct Stalled;
    impl Write for Stalled {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            thread::sleep(Duration::from_secs(3600));
            Ok(0)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_window_that_stops_reading_is_lost_and_never_blocks_the_server() {
        let h = harness(
            |_input, mut output, _| {
                ready(&mut output);
                // Alive, its output open, never reading.
                thread::sleep(Duration::from_secs(3600));
            },
            roomy(),
            Some(Box::new(Stalled)),
        );
        h.apps.open("Stuck").unwrap();
        let start = Instant::now();
        let error = h.apps.close("app-1").unwrap_err();
        assert_eq!(error.code, "app.lost");
        assert!(start.elapsed() < timing().answer);
        assert_eq!(h.apps.windows()[0].state, AppState::Lost);
        // The session still ends promptly.
        let start = Instant::now();
        h.apps.end_session();
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn the_session_end_is_said_and_the_window_left_running() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let h = harness(obliging(seen.clone()), roomy(), None);
        h.apps.open("Stays").unwrap();
        h.apps.end_session();
        let deadline = Instant::now() + Duration::from_secs(2);
        while seen.lock().unwrap().len() < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                driven::Inbound::Message(ToApp::SessionEnd),
                // Then the end of its input: the pipe is closed.
                driven::Inbound::Unsupported { id: None },
            ]
        );
        assert!(!*h.processes.lock().unwrap()[0].killed.lock().unwrap());
        assert_eq!(h.apps.open("After").unwrap_err().code, "app.session_ended");
        assert_eq!(h.apps.close("app-1").unwrap_err().code, "app.session_ended");
    }

    #[test]
    fn the_app_is_found_beside_the_server_never_elsewhere() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let server = bin.join(format!("noh-mcp{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&server, b"").unwrap();
        assert_eq!(app_beside(&server), None);
        let app = root
            .path()
            .join(format!("noh-app{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&app, b"").unwrap();
        assert_eq!(app_beside(&server), Some(app.clone()));
        // A development build: both in one folder, noh.exe is the command line.
        let dev = root
            .path()
            .join(format!("noh-mcp{}", std::env::consts::EXE_SUFFIX));
        if cfg!(windows) {
            std::fs::write(root.path().join("noh.exe"), b"").unwrap();
        }
        assert_eq!(app_beside(&dev), Some(app.clone()));
        if cfg!(windows) {
            std::fs::remove_file(&app).unwrap();
            assert_eq!(app_beside(&server), Some(root.path().join("noh.exe")));
        }
    }
}
