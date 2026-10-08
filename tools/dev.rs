//! Repository tasks. These dependencies never enter the shipped application.
mod capture;
mod fixtures;
mod package;
mod process;
use clap::{Parser, Subcommand};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(about = "NOH development tasks")]
struct Options {
    #[command(subcommand)]
    task: Task,
}
#[derive(Subcommand)]
enum Task {
    /// Build and package a Windows portable folder or a local macOS application.
    Build {
        /// Use this source tree; explicit-root packaging currently requires Windows.
        #[arg(long)]
        source_root: Option<PathBuf>,
        #[arg(long)]
        gui: bool,
        #[arg(long)]
        mcp: bool,
        /// Include the experimental update lease, hooks and standalone guardian.
        #[arg(long)]
        updates: bool,
        #[arg(long)]
        offline: bool,
        #[arg(long)]
        no_bundle: bool,
        /// Include media libraries, Whisper and models in a relocatable macOS ZIP.
        #[arg(long, requires = "gui", conflicts_with = "no_bundle")]
        portable: bool,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
        /// Prepared speech folder (macOS default: .mcp-dev/macos-speech).
        #[arg(long)]
        speech: Option<PathBuf>,
    },
    /// Run formatting and Rust tests; --media includes the complete FFmpeg suite.
    Verify {
        #[arg(long)]
        media: bool,
        /// Include MCP protocol and real-worker scenarios (requires FFmpeg).
        #[arg(long, requires = "media")]
        mcp: bool,
        /// Include updater policy and GUI behavior in the same verification run.
        #[arg(long)]
        updates: bool,
        /// Exercise local speech recognition using explicitly configured models and fixture.
        #[arg(long, requires = "media")]
        transcription: bool,
        #[arg(long)]
        offline: bool,
    },
    /// Capture deterministic GUI states and build a browsable contact sheet.
    CaptureUi(Box<capture::Options>),
    /// Write synthetic media for GUI captures (never user media).
    Fixtures(fixtures::Options),
}

fn cargo() -> Command {
    cargo_at(Path::new(env!("CARGO_MANIFEST_DIR")))
}
fn cargo_at(root: &Path) -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(root);
    command
}
fn checked(command: Command) -> Result<std::process::Output> {
    eprintln!("Running {command:?}");
    let result = process::run(command, Duration::from_secs(1800))?;
    use std::io::Write;
    std::io::stdout().write_all(&result.stdout)?;
    std::io::stderr().write_all(&result.stderr)?;
    if !result.status.success() {
        return Err(format!("Command failed: {}", result.status).into());
    }
    Ok(result)
}

fn build(
    source_root: Option<PathBuf>,
    gui: bool,
    mcp: bool,
    updates: bool,
    offline: bool,
    no_bundle: bool,
    portable: bool,
    ffmpeg: Option<PathBuf>,
    speech: Option<PathBuf>,
) -> Result<()> {
    let explicit_root = source_root.is_some();
    let root = source_root
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        .canonicalize()?;
    if !root.join("Cargo.toml").is_file() {
        return Err("Build source root must contain Cargo.toml.".into());
    }
    if explicit_root && !no_bundle {
        if !cfg!(windows) {
            return Err("Explicit-root packaging currently requires Windows.".into());
        }
        if gui {
            package::validate_source_root(&root)?;
        }
    }
    if updates && !cfg!(windows) && !no_bundle {
        return Err("Update-enabled bundle qualification currently requires Windows; use --no-bundle for source checks.".into());
    }
    if portable && !cfg!(target_os = "macos") {
        return Err(
            "--portable is for macOS; Windows builds already include a portable folder.".into(),
        );
    }
    if !no_bundle {
        if !cfg!(any(windows, target_os = "macos")) {
            return Err("Packaging requires Windows or macOS; use --no-bundle.".into());
        }
        if cfg!(target_os = "macos") {
            if !gui {
                return Err("macOS application packaging requires --gui; use --no-bundle for CLI-only builds.".into());
            }
            if speech.is_some() && !portable {
                return Err("On macOS, --speech requires --portable.".into());
            }
        }
    }
    let mut command = cargo_at(&root);
    command.args(["build", "--release", "--locked", "--bin", "noh"]);
    if gui {
        command.args(["--features", "gui", "--bin", "noh-app"]);
    }
    if mcp {
        command.args(["--features", "mcp", "--bin", "noh-mcp"]);
    }
    if updates {
        command.args([
            "--features",
            "updates",
            "--bin",
            "noh-update-guard",
            "--bin",
            "noh-update-repair",
        ]);
    }
    if offline {
        command.arg("--offline");
    }
    checked(command)?;
    if no_bundle {
        return Ok(());
    }
    let mut metadata = cargo_at(&root);
    metadata.args(["metadata", "--format-version", "1", "--no-deps", "--locked"]);
    if offline {
        metadata.arg("--offline");
    }
    let metadata = process::run(metadata, Duration::from_secs(30))?;
    if !metadata.status.success() {
        return Err(String::from_utf8_lossy(&metadata.stderr)
            .into_owned()
            .into());
    }
    let json: serde_json::Value = serde_json::from_slice(&metadata.stdout)?;
    let mut release = PathBuf::from(
        json["target_directory"]
            .as_str()
            .ok_or("Missing Cargo target directory")?,
    );
    if let Some(target) = std::env::var_os("CARGO_BUILD_TARGET") {
        release.push(target);
    }
    release.push("release");
    let ffmpeg = noh::find_ffmpeg(ffmpeg.map(OsString::from))?;
    package::bundle(
        &root,
        &release,
        &ffmpeg,
        gui,
        mcp,
        updates,
        speech.as_deref(),
        portable,
    )?;
    Ok(())
}

fn verify(media: bool, mcp: bool, updates: bool, transcription: bool, offline: bool) -> Result<()> {
    if transcription {
        for name in [
            "NOH_WHISPER",
            "NOH_WHISPER_MODEL",
            "NOH_WHISPER_VAD",
            "NOH_SUBTITLE_SPEECH_FR",
        ] {
            if std::env::var_os(name).is_none() {
                return Err(
                    format!("Set {name} for --transcription; see docs/DEVELOPMENT.md#media-and-transcription-checks").into(),
                );
            }
        }
    }
    let mut fmt = cargo();
    fmt.args(["fmt", "--check"]);
    checked(fmt)?;
    let mut tests = cargo();
    let features = match (mcp, updates) {
        (true, true) => "gui,mcp,updates",
        (true, false) => "gui,mcp",
        (false, true) => "gui,updates",
        (false, false) => "gui",
    };
    tests.args([
        "test",
        "--locked",
        "--features",
        features,
        "--lib",
        "--bins",
        "--example",
        "dev",
        "--test",
        "process_runner",
        "--test",
        "build_info",
        "--test",
        "subtitles",
    ]);
    if media {
        // GUI tests that need FFmpeg skip in the quick suite; here they must run.
        tests.env("NOH_MEDIA_TESTS", "1");
        tests.args([
            "--test",
            "media",
            "--test",
            "app_export",
            "--test",
            "controller",
            "--test",
            "images",
            "--test",
            "captions",
            "--test",
            "caption_geometry",
            "--test",
            "caption_benchmark",
            "--test",
            "shorts",
            "--test",
            "short_geometry",
            "--test",
            "short_benchmark",
            "--test",
            "project",
            "--test",
            "waveform",
            "--test",
            "preview",
        ]);
    }
    if offline {
        tests.arg("--offline");
    }
    if mcp {
        tests.args(["--test", "mcp"]);
    }
    // Keep FFmpeg-heavy cases sequential to avoid oversubscribing encoder threads.
    tests.args(["--", "--test-threads=1"]);
    checked(tests)?;
    if transcription {
        let mut recognition = cargo();
        recognition.args([
            "test",
            "--locked",
            "--features",
            features,
            "--test",
            "subtitles",
        ]);
        if offline {
            recognition.arg("--offline");
        }
        recognition.args(["real_", "--", "--ignored", "--test-threads=1"]);
        checked(recognition)?;
    }
    Ok(())
}

fn main() {
    let result = match Options::parse().task {
        Task::Build {
            source_root,
            gui,
            mcp,
            updates,
            offline,
            no_bundle,
            portable,
            ffmpeg,
            speech,
        } => build(
            source_root,
            gui,
            mcp,
            updates,
            offline,
            no_bundle,
            portable,
            ffmpeg,
            speech,
        ),
        Task::Verify {
            media,
            mcp,
            updates,
            transcription,
            offline,
        } => verify(media, mcp, updates, transcription, offline),
        Task::CaptureUi(options) => capture::run(*options),
        Task::Fixtures(options) => fixtures::run(options),
    };
    if let Err(error) = result {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
