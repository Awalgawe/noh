//! Bounded subprocess capture for development tools and integration fixtures.
//! OS process-tree ownership is delegated to process-wrap, not shell commands.
use process_wrap::std::{ChildWrapper, CommandWrap};
use std::{
    io::{self, Read},
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const CAPTURE_LIMIT: usize = 16 * 1024 * 1024;

struct OwnedTree(Box<dyn ChildWrapper>);
impl Drop for OwnedTree {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    overflow: bool,
}

fn reader(
    mut pipe: impl Read + Send + 'static,
    capture: Arc<Mutex<Capture>>,
) -> mpsc::Receiver<io::Result<()>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| {
            let mut buffer = [0; 8192];
            loop {
                let count = pipe.read(&mut buffer)?;
                if count == 0 {
                    return Ok(());
                }
                let mut output = capture.lock().unwrap();
                let keep = count.min(CAPTURE_LIMIT.saturating_sub(output.bytes.len()));
                output.bytes.extend_from_slice(&buffer[..keep]);
                output.overflow |= keep != count;
            }
        })();
        let _ = tx.send(result);
    });
    rx
}

pub fn run(mut command: Command, timeout: Duration) -> io::Result<Output> {
    let description = format!("{command:?}");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut wrapped = CommandWrap::from(command);
    #[cfg(windows)]
    wrapped.wrap(process_wrap::std::JobObject);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessSession);
    let mut tree = OwnedTree(wrapped.spawn_with(|command| {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Apply after JobObject's hook: process-wrap 9.1.1 loses access to
            // CreationFlags during spawn. Suspension still protects attachment.
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const CREATE_SUSPENDED: u32 = 0x00000004;
            command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        }
        command.spawn()
    })?);
    let stdout = Arc::new(Mutex::new(Capture::default()));
    let stderr = Arc::new(Mutex::new(Capture::default()));
    let out_done = reader(tree.0.stdout().take().unwrap(), stdout.clone());
    let err_done = reader(tree.0.stderr().take().unwrap(), stderr.clone());
    let deadline = Instant::now() + timeout;
    let (mut out_finished, mut err_finished) = (false, false);
    let mut status = None;
    let mut failure = None;
    loop {
        for (rx, done) in [
            (&out_done, &mut out_finished),
            (&err_done, &mut err_finished),
        ] {
            if !*done {
                match rx.try_recv() {
                    Ok(result) => {
                        result?;
                        *done = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(io::Error::other("Capture thread stopped"));
                    }
                }
            }
        }
        if status.is_none() {
            status = tree.0.try_wait()?;
        }
        if status.is_some() && out_finished && err_finished {
            break;
        }
        if Instant::now() >= deadline {
            failure = Some(format!(
                "Timed out after {}s: {description}",
                timeout.as_secs_f64()
            ));
            break;
        }
        if stdout.lock().unwrap().overflow || stderr.lock().unwrap().overflow {
            failure = Some(format!(
                "Capture exceeded {CAPTURE_LIMIT} bytes: {description}"
            ));
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if let Some(reason) = failure {
        tree.0.start_kill()?;
        let cleanup_deadline = Instant::now() + Duration::from_secs(5);
        for (rx, done) in [(&out_done, out_finished), (&err_done, err_finished)] {
            if !done {
                rx.recv_timeout(cleanup_deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| {
                        io::Error::other(format!("{reason}; process pipes failed to close"))
                    })??;
            }
        }
        while tree.0.try_wait()?.is_none() && Instant::now() < cleanup_deadline {
            thread::sleep(Duration::from_millis(10));
        }
        return Err(io::Error::other(format!(
            "{reason}\n{}\n{}",
            tail(&stdout.lock().unwrap().bytes),
            tail(&stderr.lock().unwrap().bytes)
        )));
    }
    let mut out = stdout.lock().unwrap();
    let mut err = stderr.lock().unwrap();
    if out.overflow || err.overflow {
        return Err(io::Error::other("Subprocess capture exceeded its limit"));
    }
    Ok(Output {
        status: status.unwrap(),
        stdout: std::mem::take(&mut out.bytes),
        stderr: std::mem::take(&mut err.bytes),
    })
}

fn tail(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(5000)..]).into_owned()
}
