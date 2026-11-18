#![cfg(feature = "gui")]
//! The interface records its start in `startup.log`, whether the window opens or not.
//!
//! Where the machine can open a window the log ends with the OpenGL that drew it; where it
//! cannot (a Windows runner without OpenGL 2.0, a Linux one without a display) the same test
//! exercises the failure: the log then says why the start ended.
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn app() -> PathBuf {
    std::env::var_os("NOH_APP_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_noh-app")))
}

#[test]
fn a_start_leaves_its_steps_and_its_outcome_in_the_profile() {
    let folder = tempfile::tempdir().unwrap();
    let profile = folder.path().join("profile");
    std::fs::create_dir(&profile).unwrap();
    // A capture run: it ends by itself and never shows a dialog.
    let mut child = Command::new(app())
        .env("NOH_CAPTURE_UI", folder.path().join("capture.ppm"))
        .env("APPDATA", &profile)
        .env("HOME", &profile)
        .env("XDG_CONFIG_HOME", &profile)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(90) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let log = [
        "noh/startup.log",
        "Library/Application Support/noh/startup.log",
    ]
    .map(|path| profile.join(path))
    .into_iter()
    .find(|path| path.is_file())
    .expect("No startup.log in the profile");
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert!(lines.len() >= 4, "{text}");
    assert!(
        lines[0].starts_with("NOH ") && lines[0].contains(", started "),
        "{text}"
    );
    assert_eq!(
        lines[1..4],
        [
            "preparing the application state",
            "loading the bundled speech resources",
            "creating the window"
        ],
        "{text}"
    );
    let status = status.unwrap_or_else(|| panic!("The interface did not end in 90 s:\n{text}"));
    let said_why = lines
        .iter()
        .any(|line| line.starts_with("failed: ") || line.starts_with("panic: "));
    let opened = lines
        .iter()
        .any(|line| line.starts_with("window opened; OpenGL: "));
    // GitHub's Windows runners have no OpenGL 2.0, and the QA run puts no software renderer
    // beside the interface: there the start must fail, and say so. If a runner gains OpenGL
    // this fails, which is the moment to revisit it.
    if cfg!(windows) && std::env::var_os("GITHUB_ACTIONS").is_some() {
        assert!(!status.success() && !opened, "{text}");
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("failed: ") && line.to_lowercase().contains("opengl")),
            "The failure must name OpenGL:\n{text}"
        );
    } else if status.success() {
        assert!(opened, "{text}");
        assert_eq!(lines.last(), Some(&"closed normally"), "{text}");
        assert!(!said_why, "{text}");
    } else {
        assert!(said_why, "A start that failed must say why:\n{text}");
    }
}
