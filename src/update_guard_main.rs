//! Standalone experimental guardian. Production execution stays disabled.
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Options {
    #[arg(long, exclusive = true)]
    build_info: bool,
    #[arg(long, required_unless_present = "build_info")]
    root: Option<PathBuf>,
    #[arg(long, required_unless_present = "build_info")]
    package: Option<PathBuf>,
    #[arg(long, required_unless_present = "build_info")]
    envelope: Option<PathBuf>,
    #[arg(long, required_unless_present_any = ["build_info", "setup_base"])]
    helper: Option<PathBuf>,
    #[arg(long)]
    setup_base: Option<PathBuf>,
    #[arg(long)]
    from_version: Option<semver::Version>,
    #[arg(long, required_unless_present = "build_info")]
    transaction: Option<PathBuf>,
    #[arg(long,default_value="stable",value_parser=["stable","beta"])]
    channel: String,
    /// Relaunch the fixed installed GUI only after authenticated installation.
    #[arg(long)]
    restart_gui: bool,
}
fn main() {
    let options = Options::parse();
    if options.build_info {
        println!("{}", noh::build_info::current().json());
        return;
    }
    #[cfg(windows)]
    let result = run(options);
    #[cfg(not(windows))]
    let result: noh::update::Result<()> = Err(noh::update::Error::InstallationUnavailable);
    if let Err(error) = result {
        eprintln!("noh-update-guard: {error}");
        std::process::exit(1);
    }
}
#[cfg(windows)]
fn run(options: Options) -> noh::update::Result<()> {
    if options.setup_base.is_some() {
        return run_setup(options);
    }
    use noh::update::{windows_guard::GuardedUpdate, *};
    use std::{
        io::{Read, Write},
        time::Duration,
    };
    let allowed =
        option_env!("NOH_UPDATE_QUALIFICATION_ROOT").ok_or(Error::InstallationUnavailable)?;
    let allowed = std::fs::canonicalize(allowed)?;
    let root = options.root.unwrap();
    let resolved_root = std::fs::canonicalize(&root)?;
    if !resolved_root.starts_with(&allowed) || resolved_root == allowed {
        return Err(Error::InstallationUnavailable);
    }
    let transaction = options.transaction.unwrap();
    let parent = std::fs::canonicalize(transaction.parent().ok_or(Error::Metadata)?)?;
    let leaf = transaction.file_name().ok_or(Error::Metadata)?;
    let transaction = parent.join(leaf);
    if !parent.starts_with(&allowed)
        || transaction.starts_with(&resolved_root)
        || resolved_root.starts_with(&transaction)
    {
        return Err(Error::InstallationUnavailable);
    }
    let keys = trust::compiled_keys()?;
    // A fresh transaction prevents output aliases, stale markers and collisions.
    // This directory must remain outside the installation that the helper replaces.
    std::fs::create_dir(&transaction)?;
    let anchor = transaction.join("anchor");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&anchor)?
        .sync_all()?;
    let _protection = windows::ProtectedFile::open(&anchor)?;
    use std::os::windows::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .share_mode(1)
        .open(transaction.join("result.json"))?;
    let log = transaction.join("helper.log");
    // Permit the helper to append, while refusing deletion or pathname replacement.
    let _log_handle = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .share_mode(3)
        .open(&log)?;
    let mut bytes = Vec::new();
    std::fs::File::open(options.envelope.unwrap())?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let channel = if options.channel == "stable" {
        Channel::Stable
    } else {
        Channel::Beta
    };
    let outcome = (|| {
        let guard = GuardedUpdate::prepare(
            &bytes,
            &keys,
            trust::package_id(),
            channel,
            &options.package.unwrap(),
            &options.helper.unwrap(),
            &root,
        )?;
        // Ready is emitted only after independent authentication and held protections.
        let mut ready_file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(transaction.join("ready"))?;
        ready_file.write_all(b"authenticated-and-protected")?;
        ready_file.sync_all()?;
        // Process disappearance releases leases but is not installation consent.
        // The client persists inputs before emitting this distinct commitment.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            match std::fs::File::open(transaction.join("commit")) {
                Ok(file) => {
                    let mut marker = Vec::new();
                    file.take(64).read_to_end(&mut marker)?;
                    if marker == b"authorize-shutdown" {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if options.restart_gui {
            guard
                .apply_with_restart(Duration::from_secs(30), Duration::from_secs(600), &log)
                .map(|(installed, restart)| (installed, Some(restart)))
        } else {
            guard
                .apply_measured(Duration::from_secs(30), Duration::from_secs(600), &log)
                .map(|installed| (installed, None))
        }
    })();
    let report = match &outcome {
        Ok((outcome, restart)) => {
            let restart = match restart {
                None => serde_json::json!({"requested": false}),
                Some(Ok(pid)) => {
                    serde_json::json!({"requested":true,"launched":true,"pid":pid,"readiness":"unverified"})
                }
                Some(Err(error)) => {
                    serde_json::json!({"requested":true,"launched":false,"error":error.to_string()})
                }
            };
            serde_json::json!({"success":true,"version":outcome.version.to_string(),"metrics":outcome,"restart":restart,"qualification":"experimental portable Windows"})
        }
        Err(error) => {
            serde_json::json!({"success":false,"error":error.to_string(),"qualification":"experimental portable Windows"})
        }
    };
    file.write_all(&serde_json::to_vec_pretty(&report).map_err(|_| Error::Metadata)?)?;
    file.sync_all()?;
    // Only this final marker plus process exit makes the result conclusive.
    // Ready alone never establishes liveness or installation success.
    let mut completed = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(transaction.join("completed"))?;
    completed.write_all(b"result-flushed")?;
    completed.sync_all()?;
    outcome.map(|_| ())
}

#[cfg(windows)]
fn run_setup(options: Options) -> noh::update::Result<()> {
    use noh::update::{
        windows::{ProtectedFile, RuntimeLease, SupervisedProcess},
        windows_anchor::InstallationAnchor,
        windows_setup::{Action, PreparedSetup},
        *,
    };
    use std::{
        ffi::OsStr,
        io::{Read, Write},
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };
    let base = options.setup_base.unwrap();
    let _base = ProtectedFile::directory(&base)?;
    let base = std::fs::canonicalize(base)?;
    trust::validate_setup_base(&base)?;
    let root = base.join("application");
    if std::fs::canonicalize(options.root.unwrap())? != root {
        return Err(Error::InstallationUnavailable);
    }
    let transaction = options.transaction.unwrap();
    let parent = std::fs::canonicalize(transaction.parent().ok_or(Error::Metadata)?)?;
    if !parent.starts_with(base.join(".noh-update/downloads")) {
        return Err(Error::InstallationUnavailable);
    }
    let transaction = parent.join(transaction.file_name().ok_or(Error::Metadata)?);
    std::fs::create_dir(&transaction)?;
    let _transaction = ProtectedFile::directory(&transaction)?;
    use std::os::windows::fs::OpenOptionsExt;
    let mut report_file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .share_mode(1)
        .open(transaction.join("result.json"))?;
    let mut metadata = ProtectedFile::open(&options.envelope.unwrap())?;
    let mut envelope = Vec::new();
    metadata
        .file()
        .take(MAX_ENVELOPE as u64 + 1)
        .read_to_end(&mut envelope)?;
    let keys = trust::compiled_keys()?;
    let channel = if options.channel == "stable" {
        Channel::Stable
    } else {
        Channel::Beta
    };
    let release = VerifiedRelease::verify(&envelope, &keys, trust::package_id())?;
    let cancelled = AtomicBool::new(false);
    let outcome = (|| -> Result<serde_json::Value> {
        let prepared = PreparedSetup::prepare(
            &envelope,
            &keys,
            trust::package_id(),
            channel,
            &base,
            &options.package.unwrap(),
            &release.release().version,
            Action::Update {
                from: options.from_version.ok_or(Error::Metadata)?,
            },
            &cancelled,
        )?;
        let mut ready = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(transaction.join("ready"))?;
        ready.write_all(b"authenticated-and-protected")?;
        ready.sync_all()?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match std::fs::File::open(transaction.join("commit")) {
                Ok(file) => {
                    let mut marker = Vec::new();
                    file.take(64).read_to_end(&mut marker)?;
                    if marker == b"authorize-shutdown" {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if Instant::now() >= deadline {
                return Err(Error::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let installed = prepared.apply(
            Duration::from_secs(30),
            Duration::from_secs(600),
            &transaction.join("setup.log"),
            &cancelled,
        )?;
        let restart = if options.restart_gui {
            let restarted = (|| -> Result<windows_ready::Proof> {
                let anchor = InstallationAnchor::open(&base, trust::package_id(), Some(channel))?;
                let lease = RuntimeLease::acquire(&anchor.coordination())?.into_protection();
                let installed = windows_setup::protect_installed(&anchor, &release, &cancelled)?;
                let contract =
                    windows_setup::exact_setup(&release, channel, &release.release().version)?
                        .setup
                        .as_ref()
                        .ok_or(Error::Metadata)?;
                let mut ready = windows_ready::Server::new()?;
                let mut protections: Vec<_> = installed.iter().map(|(_, file)| file).collect();
                protections.extend([&lease, anchor.protection()]);
                let current = root.join("current");
                let child = SupervisedProcess::spawn_protected(
                    &current.join("noh.exe"),
                    &[
                        OsStr::new(windows_ready::Server::argument()),
                        OsStr::new(ready.name()),
                    ],
                    &current,
                    &protections,
                )?;
                let proof = ready.wait(
                    child.id(),
                    &release.release().version,
                    &contract.build_fingerprint,
                    Duration::from_secs(30),
                )?;
                if child.wait(Duration::ZERO)?.is_some() {
                    return Err(Error::Integrity);
                }
                child.detach()?;
                Ok(proof)
            })();
            match restarted {
                Ok(proof) => serde_json::json!({"requested":true,"ready":true,"proof":proof}),
                Err(error) => {
                    serde_json::json!({"requested":true,"ready":false,"error":error.to_string()})
                }
            }
        } else {
            serde_json::json!({"requested":false})
        };
        Ok(serde_json::json!({"success":true,"outcome":installed,"restart":restart}))
    })();
    let report = match &outcome {
        Ok(value) => value.clone(),
        Err(error) => serde_json::json!({"success":false,"error":error.to_string()}),
    };
    report_file.write_all(&serde_json::to_vec_pretty(&report).map_err(|_| Error::Metadata)?)?;
    report_file.sync_all()?;
    let mut completed = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(transaction.join("completed"))?;
    completed.write_all(b"result-flushed")?;
    completed.sync_all()?;
    outcome.map(|_| ())
}
