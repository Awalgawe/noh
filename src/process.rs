//! Bounded pipe readers and process ownership. Workers inherit their supervisor's
//! tree; standalone inspections receive their own process tree and deadline.
//!
//! A media helper does not outlive this process. An owner that drops its
//! `Tree` kills and reaps it; a process that ends before its owners did (the
//! window closed while a worker thread still held a decoder) is covered
//! twice: on Windows every `Tree`, and every helper given to `adopt`, belongs
//! to a job the system closes with this process, killing what is left,
//! however the process ended; everywhere, `settle` lets an ordinary exit wait
//! a bounded time for the owners of a `Tree` to finish reaping.
//!
//! Unix has the bounded wait only. No parent-death signal is asked for: on
//! Linux PR_SET_PDEATHSIG follows the thread that spawned, not the process,
//! and would kill a child when its worker thread ends. So there a process
//! that is killed, or an owner that does not stop when told, still leaves
//! its child; and a worker that spawns after `settle` saw no child left is
//! not waited for.
//!
//! What must outlive NOH on purpose (the update's installer, the relaunch
//! after an update) is started with `Command` directly, and is in no job.
use process_wrap::std::{ChildWrapper, CommandWrap};
use std::{
    io::{self, BufRead, BufReader, Read},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

/// The `Tree`s alive in this process: spawned, not yet killed and reaped.
static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Wait until every child this process started is gone, `limit` at most.
/// Called at the end of an ordinary exit, once the owners were told to stop:
/// a worker thread needs a moment to kill and reap its child, and the
/// process must not end under it. True when none is left.
pub fn settle(limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if LIVE.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Windows: one job for every child of this process, with "kill on job
/// close". This process holds its only handle and never closes it: the system
/// does when the process ends, in whatever way, and then ends every process
/// still in the job. A child's own job (`isolate`) nests inside it.
#[cfg(windows)]
mod exit_job {
    use std::{ffi::c_void, os::windows::io::AsRawHandle, sync::OnceLock};

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    /// JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }
    const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            information: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// The job's handle, as an address (a raw pointer is not `Sync`); 0 when
    /// the system gave none.
    static JOB: OnceLock<usize> = OnceLock::new();

    fn job() -> *mut c_void {
        *JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut limits = ExtendedLimits::default();
            limits.basic.limit_flags = KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                EXTENDED_LIMIT_INFORMATION,
                (&raw const limits).cast(),
                size_of::<ExtendedLimits>() as u32,
            ) == 0
            {
                // A job that would not kill is of no use here.
                CloseHandle(job);
                return 0;
            }
            job as usize
        }) as *mut c_void
    }

    /// Put a child just spawned in the job. Best effort: a child the system
    /// refuses to add keeps its owner's kill only, and the journal says so.
    pub fn adopt(child: &std::process::Child) {
        let job = job();
        if job.is_null() {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| {
                tracing::warn!("no exit job: a helper may outlive a process that is killed");
            });
        } else if unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) } == 0 {
            tracing::warn!(
                pid = child.id(),
                error = %std::io::Error::last_os_error(),
                "a helper could not join the exit job"
            );
        }
    }
}

/// A helper spawned without a `Tree` (its owner kills it itself when told to
/// stop) still ends with this process on Windows. Nothing elsewhere.
pub(crate) fn adopt(child: &std::process::Child) {
    #[cfg(windows)]
    exit_job::adopt(child);
    #[cfg(not(windows))]
    let _ = child;
}

pub(crate) struct Tree(pub Box<dyn ChildWrapper>);
impl Tree {
    pub fn spawn(command: Command, isolate: bool) -> io::Result<Self> {
        let mut wrapped = CommandWrap::from(command);
        if isolate {
            #[cfg(windows)]
            wrapped.wrap(process_wrap::std::JobObject);
            #[cfg(unix)]
            wrapped.wrap(process_wrap::std::ProcessSession);
        }
        let child = wrapped.spawn_with(|command| {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // process-wrap 9.1.1 removes its wrapper map during pre_spawn,
                // so JobObject cannot see CreationFlags and overwrites them.
                // Apply the final flags after every hook; retain suspension so
                // JobObject can attach the process before resuming its threads.
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                const CREATE_SUSPENDED: u32 = 0x00000004;
                command
                    .creation_flags(CREATE_NO_WINDOW | if isolate { CREATE_SUSPENDED } else { 0 });
            }
            let child = command.spawn()?;
            // Before process-wrap resumes an isolated child, so that nothing
            // it starts escapes the job.
            #[cfg(windows)]
            exit_job::adopt(&child);
            Ok(child)
        })?;
        LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Self(child))
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
        // Reap the leader without a second wait on the Windows completion port:
        // process-wrap::try_wait may already have consumed its notification.
        let _ = self.0.inner_mut().wait();
        LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Unlike BufRead::lines, even a malicious unterminated line is bounded.
pub(crate) fn lines(
    reader: impl Read,
    max: usize,
    mut line: impl FnMut(String) -> bool,
) -> io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut pending = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            if !pending.is_empty() {
                line(String::from_utf8_lossy(&pending).into_owned());
            }
            return Ok(());
        }
        let count = buffer
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buffer.len(), |p| p + 1);
        if pending.len() + count > max {
            return Err(io::Error::other("Process line exceeds its size limit"));
        }
        pending.extend_from_slice(&buffer[..count]);
        let complete = buffer[count - 1] == b'\n';
        reader.consume(count);
        if complete {
            if !line(
                String::from_utf8_lossy(&pending)
                    .trim_end_matches(['\r', '\n'])
                    .to_owned(),
            ) {
                return Ok(());
            }
            pending.clear();
        }
    }
}

#[derive(Default)]
struct Capture {
    head: Vec<u8>,
    tail: Vec<u8>,
}
impl Capture {
    fn push(&mut self, bytes: &[u8]) {
        let keep = bytes.len().min(32768 - self.head.len());
        self.head.extend_from_slice(&bytes[..keep]);
        self.tail.extend_from_slice(&bytes[keep..]);
        if self.tail.len() > 32768 {
            self.tail.drain(..self.tail.len() - 32768);
        }
    }
    fn text(&self) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&self.head),
            String::from_utf8_lossy(&self.tail)
        )
    }
}

pub(crate) fn stream(
    command: Command,
    timeout: Option<Duration>,
    isolate: bool,
    on_line: impl FnMut(&str) -> crate::Result<()>,
) -> crate::Result<(std::process::ExitStatus, String)> {
    stream_inner(command, timeout, isolate, None, false, on_line)
}

/// Whisper emits progress on stderr. Drain both pipes concurrently, retain
/// bounded diagnostics, and deliver those lines before the process completes.
pub(crate) fn stream_stderr(
    command: Command,
    timeout: Duration,
    on_line: impl FnMut(&str) -> crate::Result<()>,
) -> crate::Result<(std::process::ExitStatus, String)> {
    stream_inner(command, Some(timeout), false, None, true, on_line)
}

/// Interactive inspectors share the caller's cancellation and total deadline.
pub(crate) fn stream_interruptible(
    command: Command,
    timeout: Duration,
    cancel: &std::sync::atomic::AtomicBool,
    on_line: impl FnMut(&str) -> crate::Result<()>,
) -> crate::Result<(std::process::ExitStatus, String)> {
    stream_inner(command, Some(timeout), true, Some(cancel), false, on_line)
}

fn stream_inner(
    mut command: Command,
    timeout: Option<Duration>,
    isolate: bool,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    stderr_lines: bool,
    mut on_line: impl FnMut(&str) -> crate::Result<()>,
) -> crate::Result<(std::process::ExitStatus, String)> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = Tree::spawn(command, isolate).map_err(crate::ffmpeg_error)?;
    let stdout = child.0.stdout().take().ok_or("Missing stdout")?;
    let stderr = child.0.stderr().take().ok_or("Missing stderr")?;
    let (stream, mut diagnostics): (Box<dyn Read + Send>, Box<dyn Read + Send>) = if stderr_lines {
        (Box::new(stderr), Box::new(stdout))
    } else {
        (Box::new(stdout), Box::new(stderr))
    };
    let (tx, rx) = mpsc::sync_channel(64);
    thread::spawn(move || {
        let result = lines(stream, 1024 * 1024, |line| tx.send(Ok(Some(line))).is_ok());
        let _ = tx.send(result.map(|()| None));
    });
    let capture = Arc::new(Mutex::new(Capture::default()));
    let log = capture.clone();
    let (err_tx, err_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| -> io::Result<()> {
            let mut buffer = [0; 8192];
            loop {
                let count = diagnostics.read(&mut buffer)?;
                if count == 0 {
                    return Ok(());
                }
                log.lock().unwrap().push(&buffer[..count]);
            }
        })();
        let _ = err_tx.send(result);
    });
    let start = Instant::now();
    let mut status = None;
    let (mut out_done, mut err_done) = (false, false);
    loop {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(Box::new(crate::engine::EngineError::new(
                "error.cancelled",
                "inspect",
                None,
                "Media inspection cancelled.",
            )));
        }
        if timeout.is_some_and(|t| start.elapsed() >= t) {
            return Err(Box::new(crate::engine::EngineError::new(
                "error.timeout",
                "inspect",
                None,
                "Media inspection deadline exceeded.",
            )));
        }
        if !out_done {
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(Some(line))) => {
                    if stderr_lines {
                        let mut log = capture.lock().unwrap();
                        log.push(line.as_bytes());
                        log.push(b"\n");
                    }
                    on_line(&line)?;
                }
                Ok(Ok(None)) => out_done = true,
                Ok(Err(e)) => return Err(e.into()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("Output reader stopped".into()),
            }
        } else {
            thread::sleep(Duration::from_millis(10));
        }
        if !err_done {
            match err_rx.try_recv() {
                Ok(result) => {
                    result?;
                    err_done = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(_) => return Err("Diagnostic reader stopped".into()),
            }
        }
        if status.is_none() {
            status = child.0.try_wait()?;
        }
        if let Some(status) = status
            && out_done
            && err_done
        {
            return Ok((status, capture.lock().unwrap().text()));
        }
    }
}

pub(crate) fn capture(
    command: Command,
    timeout: Duration,
    isolate: bool,
) -> crate::Result<(std::process::ExitStatus, String)> {
    stream(command, Some(timeout), isolate, |_| Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_command(role: &str) -> Command {
        let mut command = crate::command(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "process::tests::process_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("NOH_PROCESS_FIXTURE", role);
        command
    }
    #[test]
    #[ignore = "owned subprocess fixture"]
    fn process_fixture() {
        use std::io::Write;
        match std::env::var("NOH_PROCESS_FIXTURE").as_deref() {
            Ok("stderr-progress") => {
                // More than a pipe buffer: the unobserved stream must drain too.
                std::io::stdout().write_all(&[b'x'; 131072]).unwrap();
                std::io::stdout().flush().unwrap();
                eprintln!("whisper_print_progress_callback: progress =  42%");
                let ack = std::env::var_os("NOH_PROGRESS_ACK").unwrap();
                let deadline = Instant::now() + Duration::from_secs(3);
                while !std::path::Path::new(&ack).is_file() {
                    assert!(
                        Instant::now() < deadline,
                        "progress was buffered until exit"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                eprintln!("whisper_print_progress_callback: progress = 100%");
            }
            Ok("parent") => {
                let mut child = fixture_command("child").spawn().unwrap();
                println!("PARENT_READY|{}", std::process::id());
                std::io::stdout().flush().unwrap();
                child.wait().unwrap();
            }
            Ok("abandon") => {
                // A process that ends while it still owns a child: the child
                // is never killed by its owner.
                let isolate = std::env::var("NOH_FIXTURE_ISOLATE").unwrap() == "true";
                let mut child = fixture_command("child");
                child.stdout(Stdio::inherit()).stderr(Stdio::null());
                std::mem::forget(Tree::spawn(child, isolate).unwrap());
                thread::sleep(Duration::from_millis(
                    std::env::var("NOH_FIXTURE_LINGER")
                        .unwrap()
                        .parse()
                        .unwrap(),
                ));
                std::process::exit(0);
            }
            Ok("child") => {
                println!("CHILD_READY|{}", std::process::id());
                std::io::stdout().flush().unwrap();
                thread::sleep(Duration::from_secs(60));
            }
            Ok("noisy") => loop {
                std::io::stdout().write_all(&[b'x'; 8192]).unwrap();
            },
            #[cfg(windows)]
            Ok("console") => {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn GetConsoleWindow() -> *mut std::ffi::c_void;
                }
                println!("CONSOLE_WINDOW|{}", unsafe { GetConsoleWindow() } as usize);
            }
            #[cfg(windows)]
            Ok("desktop-parent") => {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn FreeConsole() -> i32;
                }
                // Model a GUI launched from Explorer, with no inherited console.
                unsafe { FreeConsole() };
                let isolate = std::env::var("NOH_FIXTURE_ISOLATE").unwrap() == "true";
                let (status, diagnostic) = stream(
                    fixture_command("console"),
                    Some(Duration::from_secs(5)),
                    isolate,
                    |line| {
                        println!("{line}");
                        Ok(())
                    },
                )
                .unwrap();
                assert!(status.success(), "{diagnostic}");
            }
            _ => panic!("Only launch as an owned subprocess"),
        }
    }
    #[test]
    fn stderr_progress_arrives_while_the_child_is_running() {
        let folder = tempfile::tempdir().unwrap();
        let ack = folder.path().join("observed");
        let mut command = fixture_command("stderr-progress");
        command.env("NOH_PROGRESS_ACK", &ack);
        let mut observed = Vec::new();
        let (status, log) = stream_stderr(command, Duration::from_secs(5), |line| {
            if line.contains("progress =") {
                observed.push(line.to_owned());
                std::fs::write(&ack, "received before child exit")?;
            }
            Ok(())
        })
        .unwrap();
        assert!(status.success(), "{log}");
        assert_eq!(observed.len(), 2);
        assert!(observed[0].ends_with("42%"));
        assert!(observed[1].ends_with("100%"));
        assert!(log.contains("progress = 100%"));
        assert!(log.len() <= 65536);
    }

    #[test]
    #[cfg(windows)]
    fn owned_helpers_do_not_allocate_a_console() {
        for isolate in [false, true] {
            for _ in 0..4 {
                let mut console_window = None;
                let mut parent = fixture_command("desktop-parent");
                parent.env("NOH_FIXTURE_ISOLATE", isolate.to_string());
                let (status, diagnostic) =
                    stream(parent, Some(Duration::from_secs(5)), false, |line| {
                        if let Some(handle) = line.strip_prefix("CONSOLE_WINDOW|") {
                            console_window = Some(handle.parse::<usize>()?);
                        }
                        Ok(())
                    })
                    .unwrap();
                assert!(status.success(), "{diagnostic}");
                assert_eq!(
                    console_window,
                    Some(0),
                    "isolate={isolate}: helper acquired a console and can steal focus"
                );
            }
        }
    }

    #[cfg(windows)]
    struct Handle(*mut std::ffi::c_void);
    #[cfg(windows)]
    impl Handle {
        fn open(pid: u32) -> Self {
            unsafe extern "system" {
                fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            }
            let handle = unsafe { OpenProcess(0x00100000, 0, pid) };
            assert!(!handle.is_null());
            Self(handle)
        }
        fn wait(&self, ms: u32) -> u32 {
            unsafe extern "system" {
                fn WaitForSingleObject(handle: *mut std::ffi::c_void, ms: u32) -> u32;
            }
            unsafe { WaitForSingleObject(self.0, ms) }
        }
    }
    #[cfg(windows)]
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe extern "system" {
                fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
            }
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    /// The window closed while a worker thread still held a decoder: the
    /// process ends without its `Tree` being dropped, and the system ends
    /// the child with it.
    #[test]
    #[cfg(windows)]
    fn a_child_does_not_outlive_a_process_that_ended_without_killing_it() {
        for isolate in [false, true] {
            let mut owner = fixture_command("abandon");
            owner
                .env("NOH_FIXTURE_ISOLATE", isolate.to_string())
                .env("NOH_FIXTURE_LINGER", "300")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut owner = owner.spawn().unwrap();
            let pipe = owner.stdout.take().unwrap();
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let _ = lines(pipe, 4096, |line| tx.send(line).is_ok());
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            let child = loop {
                let line = rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                if let Some(pid) = line.strip_prefix("CHILD_READY|") {
                    break pid.parse::<u32>().unwrap();
                }
            };
            let child = Handle::open(child);
            assert_eq!(child.wait(0), 258, "the child runs while its owner does");
            assert!(owner.wait().unwrap().success());
            assert_eq!(
                child.wait(5000),
                0,
                "isolate={isolate}: the child outlived the process that started it"
            );
        }
    }

    #[test]
    fn settle_waits_for_a_child_being_reaped_and_gives_up_at_its_limit() {
        let mut command = fixture_command("child");
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let tree = Tree::spawn(command, true).unwrap();
        let started = Instant::now();
        assert!(!settle(Duration::from_millis(50)), "a child is alive");
        assert!(started.elapsed() < Duration::from_secs(2));
        // An owner told to stop reaps a moment later, on its own thread.
        let reaped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = reaped.clone();
        let owner = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            drop(tree);
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        // Other tests may own children of their own meanwhile, which can only
        // make the wait longer: when it ends before its limit, this child was
        // reaped first.
        if settle(Duration::from_secs(5)) {
            assert!(started.elapsed() >= Duration::from_millis(150));
            owner.join().unwrap();
            assert!(reaped.load(std::sync::atomic::Ordering::SeqCst));
        } else {
            owner.join().unwrap();
        }
    }

    #[test]
    fn cancellation_reaps_a_confirmed_active_descendant() {
        let mut command = fixture_command("parent");
        command.stdout(Stdio::piped()).stderr(Stdio::null());
        let mut tree = Tree::spawn(command, true).unwrap();
        let pipe = tree.0.stdout().take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = lines(pipe, 4096, |line| tx.send(line).is_ok());
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut parent, mut child) = (None, None);
        while parent.is_none() || child.is_none() {
            let line = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if let Some(pid) = line.strip_prefix("PARENT_READY|") {
                parent = Some(pid.parse::<u32>().unwrap());
            }
            if let Some(pid) = line.strip_prefix("CHILD_READY|") {
                child = Some(pid.parse::<u32>().unwrap());
            }
        }
        #[cfg(windows)]
        let handles = [Handle::open(parent.unwrap()), Handle::open(child.unwrap())];
        #[cfg(windows)]
        for handle in &handles {
            assert_eq!(handle.wait(0), 258);
        }
        drop(tree);
        #[cfg(windows)]
        for handle in &handles {
            assert_eq!(handle.wait(5000), 0, "Descendant survived cancellation");
        }
        while rx.recv_timeout(Duration::from_secs(5)).is_ok() {}
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
    #[test]
    fn interactive_inspection_cancellation_reaps_a_running_tree() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let result = stream_interruptible(
            fixture_command("parent"),
            Duration::from_secs(10),
            &cancel,
            |line| {
                if line.starts_with("CHILD_READY|") {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            },
        );
        let error = result.unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::engine::EngineError>()
                .unwrap()
                .code,
            "error.cancelled"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
    #[test]
    fn inspection_deadline_and_unterminated_noisy_line_are_bounded() {
        let started = Instant::now();
        let result = stream(
            fixture_command("parent"),
            Some(Duration::from_millis(300)),
            true,
            |_| Ok(()),
        );
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(5));
        let result = stream(
            fixture_command("noisy"),
            Some(Duration::from_secs(5)),
            true,
            |_| Ok(()),
        );
        assert!(result.unwrap_err().to_string().contains("size limit"));
    }
}
