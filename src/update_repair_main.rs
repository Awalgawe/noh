//! Authenticated Setup installation/repair and legacy qualification restoration.
use clap::{Parser, Subcommand};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Parser)]
#[command(subcommand_negates_reqs = true)]
struct Options {
    #[command(subcommand)]
    command: Option<SetupCommand>,
    #[arg(long, exclusive = true)]
    build_info: bool,
    #[arg(long, required_unless_present = "build_info")]
    root: Option<PathBuf>,
    #[arg(long, required_unless_present = "build_info")]
    retained_archive: Option<PathBuf>,
    #[arg(long, required_unless_present = "build_info")]
    version: Option<String>,
    /// Required acknowledgement: the controlled harness has stopped all owned
    /// guardians/helpers and will start no new updater during this restoration.
    #[arg(long, required_unless_present = "build_info")]
    controlled_manual_restore: bool,
}
#[derive(Subcommand)]
enum SetupCommand {
    /// Inspect a fixed installation base without creating or modifying it.
    Inspect {
        #[arg(long)]
        base: PathBuf,
        #[arg(long)]
        version: semver::Version,
        #[arg(long)]
        output: PathBuf,
    },
    /// Download one exact authenticated profile; never installs anything.
    Fetch {
        #[arg(long)]
        envelope: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        version: semver::Version,
        #[arg(long, value_enum)]
        profile: noh::update::Profile,
        /// Optional same-repository asset API URL for an authenticated private trial.
        #[arg(long)]
        source: Option<String>,
        /// The wizard may create this marker to request cooperative cancellation.
        #[arg(long)]
        cancel_file: PathBuf,
    },
    /// Install or repair through the independently authenticated full Setup.
    Setup {
        #[arg(long)]
        base: PathBuf,
        #[arg(long)]
        package: PathBuf,
        #[arg(long)]
        envelope: PathBuf,
        #[arg(long)]
        version: semver::Version,
        /// Expected content choice; never inferred from downloaded metadata.
        #[arg(long, value_enum, default_value = "complete")]
        profile: noh::update::Profile,
        /// Explicit consent for exact-version repair, including older retained versions.
        #[arg(long, required = true)]
        confirm_install: bool,
        /// Create a new installation identity; existing identities are never replaced.
        #[arg(long)]
        initialize: bool,
        #[arg(long)]
        no_downgrade: bool,
    },
}
fn main() {
    let options = Options::parse();
    if options.build_info {
        println!("{}", noh::build_info::current().json());
        return;
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    if let Err(error) = ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)) {
        eprintln!("noh-update-repair: cannot establish cancellation: {error}");
        std::process::exit(1);
    }
    #[cfg(windows)]
    let result = (|| {
        if let Some(command) = options.command {
            return match command {
                SetupCommand::Inspect {
                    base,
                    version,
                    output,
                } => {
                    use noh::update::{Error, bootstrap, trust};
                    use std::io::Write;
                    let plan = bootstrap::inspect(
                        &base,
                        &version,
                        &trust::compiled_keys()?,
                        trust::package_id(),
                    )?;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(output)?;
                    write!(
                        file,
                        "[installation]\ninitialize={}\nprofile={}\nlatest_version={}\n",
                        u8::from(plan.initialize),
                        plan.profile.map(|p| p.as_str()).unwrap_or(""),
                        plan.latest_version
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default()
                    )?;
                    file.sync_all()?;
                    serde_json::to_value(plan).map_err(|_| Error::Metadata)
                }
                SetupCommand::Fetch {
                    envelope,
                    destination,
                    version,
                    profile,
                    source,
                    cancel_file,
                } => run_fetch(
                    envelope,
                    destination,
                    version,
                    profile,
                    source,
                    cancel_file,
                    &cancelled,
                ),
                command => run_setup(command, &cancelled),
            };
        }
        if !options.controlled_manual_restore {
            return Err(noh::update::Error::InstallationUnavailable);
        }
        let version = options
            .version
            .unwrap()
            .parse()
            .map_err(|_| noh::update::Error::Metadata)?;
        // The external tool must not inherit a directory handle in the tree
        // it is about to preserve/replace. All restore inputs are absolute.
        let root = std::fs::canonicalize(options.root.unwrap())?;
        let archive = std::fs::canonicalize(options.retained_archive.unwrap())?;
        let executable = std::env::current_exe()?;
        std::env::set_current_dir(executable.parent().ok_or(noh::update::Error::Metadata)?)?;
        noh::update::windows_repair::restore(&root, &archive, &version, &cancelled)
    })();
    #[cfg(not(windows))]
    let result: noh::update::Result<serde_json::Value> =
        Err(noh::update::Error::InstallationUnavailable);
    match result {
        Ok(report) => println!("{}", serde_json::to_string_pretty(&report).unwrap()),
        Err(error) => {
            eprintln!("noh-update-repair: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(windows)]
fn run_setup(
    command: SetupCommand,
    cancelled: &AtomicBool,
) -> noh::update::Result<serde_json::Value> {
    use noh::update::{
        windows::ProtectedFile,
        windows_setup::{Action, PreparedSetup},
        *,
    };
    use std::{io::Read, time::Duration};
    let SetupCommand::Setup {
        base,
        package,
        envelope,
        version,
        profile,
        confirm_install,
        initialize,
        no_downgrade,
    } = command
    else {
        return Err(Error::Metadata);
    };
    if !confirm_install {
        return Err(Error::InstallationUnavailable);
    }
    let _base = ProtectedFile::directory(&base)?;
    let base = std::fs::canonicalize(base)?;
    trust::validate_setup_base(&base)?;
    let mut envelope = ProtectedFile::open(&envelope)?;
    let mut bytes = Vec::new();
    envelope
        .file()
        .take(MAX_ENVELOPE as u64 + 1)
        .read_to_end(&mut bytes)?;
    let keys = trust::compiled_keys()?;
    VerifiedRelease::verify(&bytes, &keys, trust::package_id())?.require_profile(profile)?;
    let prepared = PreparedSetup::prepare(
        &bytes,
        &keys,
        trust::package_id(),
        Channel::Stable,
        &base,
        &package,
        &version,
        if initialize {
            Action::Initialize
        } else if no_downgrade {
            Action::InstallOrRepair
        } else {
            Action::Repair
        },
        cancelled,
    )?;
    let attempt = tempfile::Builder::new()
        .prefix("repair-")
        .tempdir_in(prepared.anchor().coordination())?
        .keep();
    std::env::set_current_dir(&base)?;
    let result = prepared.apply(
        Duration::ZERO,
        Duration::from_secs(600),
        &attempt.join("setup.log"),
        cancelled,
    );
    let report = match &result {
        Ok(outcome) => {
            serde_json::json!({"success":true,"outcome":outcome,"method":"authenticated full Setup"})
        }
        Err(error) => {
            serde_json::json!({"success":false,"error":error.to_string(),"method":"authenticated full Setup"})
        }
    };
    let report_path = attempt.join("result.json");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(report_path)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(&report).map_err(|_| Error::Metadata)?)?;
    file.sync_all()?;
    result.map(|_| report)
}

#[cfg(windows)]
fn run_fetch(
    envelope: PathBuf,
    destination: PathBuf,
    version: semver::Version,
    profile: noh::update::Profile,
    source: Option<String>,
    cancel_file: PathBuf,
    cancelled: &Arc<AtomicBool>,
) -> noh::update::Result<serde_json::Value> {
    use noh::update::*;
    use std::{
        io::{Read, Write},
        time::{Duration, Instant},
    };
    let mut metadata = windows::ProtectedFile::open(&envelope)?;
    let mut bytes = Vec::new();
    metadata
        .file()
        .take(MAX_ENVELOPE as u64 + 1)
        .read_to_end(&mut bytes)?;
    let release = VerifiedRelease::verify(&bytes, &trust::compiled_keys()?, trust::package_id())?;
    release.require_profile(profile)?;
    // Use a fresh staging directory; an interrupted or previous attempt is never authority.
    std::fs::create_dir(&destination)?;
    let _destination = windows::ProtectedFile::directory(&destination)?;
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                match cancel_file.try_exists() {
                    Ok(false) => {}
                    _ => {
                        cancelled.store(true, Ordering::Release);
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let result = (|| {
            let transport = GithubTransport::new(std::env::var("NOH_UPDATE_TOKEN").ok())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(Error::Io)?;
            let mut last = Instant::now();
            runtime.block_on(transport.stage_setup(
                &release,
                profile,
                &version,
                source.as_deref(),
                &destination,
                cancelled,
                |received, total| {
                    if received == total || last.elapsed() >= Duration::from_millis(200) {
                        println!(
                            "NOH_PROGRESS {}",
                            received.saturating_mul(100) / total.max(1)
                        );
                        let _ = std::io::stdout().flush();
                        last = Instant::now();
                    }
                },
            ))
        })();
        done.store(true, Ordering::Release);
        result
    });
    result.map(|package| serde_json::json!({"downloaded":true,"package":package,"profile":profile}))
}
