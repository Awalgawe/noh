//! First-frame acknowledgement over a local pipe bound to the launched process.
//! Tokio owns cancellable overlapped I/O; Windows supplies the writer's real PID.
use super::*;
use ring::rand::SecureRandom;
use std::os::windows::io::AsRawHandle;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions},
};

const PREFIX: &str = r"\\.\pipe\noh-update-ready-";
const ARGUMENT: &str = "--noh-update-ready-pipe";

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    pub pid: u32,
    pub version: String,
    pub build_fingerprint: String,
    pub stage: String,
}

pub struct Server {
    pipe: NamedPipeServer,
    runtime: tokio::runtime::Runtime,
    name: String,
}
impl Server {
    pub fn new() -> Result<Self> {
        let mut random = [0u8; 16];
        ring::rand::SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| Error::Integrity)?;
        let name = format!(
            "{PREFIX}{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()?;
        let pipe = {
            let _entered = runtime.enter();
            ServerOptions::new()
                .access_inbound(true)
                .access_outbound(true)
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .pipe_mode(PipeMode::Message)
                .create(&name)?
        };
        Ok(Self {
            pipe,
            runtime,
            name,
        })
    }
    pub fn argument() -> &'static str {
        ARGUMENT
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn wait(
        &mut self,
        pid: u32,
        version: &Version,
        fingerprint: &str,
        timeout: Duration,
    ) -> Result<Proof> {
        let stage = std::cell::Cell::new("pipe connection");
        self.runtime.block_on(async {
            tokio::time::timeout(timeout, async {
                self.pipe.connect().await?;
                stage.set("writer identity");
                let mut writer = 0;
                // https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-getnamedpipeclientprocessid
                if unsafe {
                    windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId(
                        self.pipe.as_raw_handle() as _,
                        &mut writer,
                    )
                } == 0
                {
                    return Err(Error::Io(std::io::Error::last_os_error()));
                }
                if writer != pid {
                    return Err(Error::Integrity);
                }
                stage.set("readiness message");
                let mut buffer = [0u8; 1024];
                let size = self.pipe.read(&mut buffer).await?;
                let proof: Proof =
                    serde_json::from_slice(&buffer[..size]).map_err(|_| Error::Metadata)?;
                if proof.pid != writer
                    || proof.version != version.to_string()
                    || proof.build_fingerprint != fingerprint
                    || proof.stage != "first-gui-frame"
                {
                    return Err(Error::Integrity);
                }
                self.pipe.write_all(b"1").await?;
                Ok(proof)
            })
            .await
            .map_err(|_| {
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "Restarted GUI did not acknowledge its first frame: {}",
                        stage.get()
                    ),
                ))
            })?
        })
    }
}

fn send(name: &str) -> Result<()> {
    let suffix = name.strip_prefix(PREFIX).ok_or(Error::Metadata)?;
    if suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Metadata);
    }
    let build = crate::build_info::current();
    let proof = Proof {
        pid: std::process::id(),
        version: build.package_version.into(),
        build_fingerprint: build.build_fingerprint.into(),
        stage: "first-gui-frame".into(),
    };
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(name)?;
    pipe.write_all(&serde_json::to_vec(&proof).map_err(|_| Error::Metadata)?)?;
    // Keep the connection alive until the server has checked the actual writer.
    let mut acknowledged = [0];
    pipe.read_exact(&mut acknowledged)?;
    if acknowledged != *b"1" {
        return Err(Error::Integrity);
    }
    Ok(())
}

/// Called once the actual GUI reaches its first update. Pipe I/O stays off the UI thread.
pub fn notify_first_frame() {
    static SENT: AtomicBool = AtomicBool::new(false);
    if SENT.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut args = std::env::args();
    args.next();
    if args.next().as_deref() != Some(ARGUMENT) {
        return;
    }
    let Some(name) = args.next() else { return };
    if args.next().is_some() {
        return;
    }
    std::thread::spawn(move || {
        if let Err(error) = send(&name) {
            eprintln!("noh-app: restart acknowledgement failed: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Child entry point for readiness pipe qualification"]
    fn ready_child() {
        eprintln!("Readiness child started");
        send(&std::env::var("NOH_READY_TEST_PIPE").unwrap()).unwrap();
        eprintln!("Readiness child acknowledged");
        std::thread::sleep(Duration::from_secs(5));
    }
    #[test]
    fn readiness_requires_the_launched_process_and_exact_build() {
        use std::os::windows::process::CommandExt;
        for refusal in ["none", "pid", "version", "fingerprint"] {
            let mut server = Server::new().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--nocapture",
                    "--ignored",
                    "--exact",
                    "update::windows_ready::tests::ready_child",
                ])
                .env("NOH_READY_TEST_PIPE", server.name())
                .creation_flags(0x08000000)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .spawn()
                .unwrap();
            let expected = if refusal == "pid" {
                std::process::id()
            } else {
                child.id()
            };
            let build = crate::build_info::current();
            let version = if refusal == "version" {
                Version::new(999, 0, 0)
            } else {
                Version::parse(build.package_version).unwrap()
            };
            let fingerprint = if refusal == "fingerprint" {
                "wrong-generation"
            } else {
                build.build_fingerprint
            };
            let result = server.wait(expected, &version, fingerprint, Duration::from_secs(3));
            child.kill().unwrap();
            child.wait().unwrap();
            if refusal != "none" {
                assert!(matches!(result, Err(Error::Integrity)));
            } else {
                assert_eq!(result.unwrap().pid, expected);
            }
        }
    }
    #[test]
    fn missing_readiness_is_bounded_without_polling_a_report_file() {
        let mut server = Server::new().unwrap();
        let start = std::time::Instant::now();
        assert!(
            server
                .wait(1, &Version::new(1, 0, 0), "", Duration::from_millis(100))
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
