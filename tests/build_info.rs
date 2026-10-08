//! Build queries work without media, FFmpeg, GUI startup or an MCP session.
#[path = "../tools/process.rs"]
mod process;
use serde_json::Value;
use std::{path::PathBuf, process::Command, time::Duration};

fn binary(variable: &str, compiled: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| compiled.into())
}

#[test]
fn executable_build_queries_agree_without_starting_media_or_gui() {
    let folder = tempfile::tempdir().unwrap();
    let capture = folder.path().join("must-not-capture.ppm");
    let binaries = [
        ("noh", binary("NOH_EXE", env!("CARGO_BIN_EXE_noh"))),
        #[cfg(feature = "gui")]
        (
            "noh-app",
            binary("NOH_APP_EXE", env!("CARGO_BIN_EXE_noh-app")),
        ),
        #[cfg(feature = "mcp")]
        (
            "noh-mcp",
            binary("NOH_MCP_EXE", env!("CARGO_BIN_EXE_noh-mcp")),
        ),
    ];
    let mut expected = None;
    for (name, path) in &binaries {
        let run = |argument: &str| {
            let mut command = Command::new(path);
            command
                .arg(argument)
                .env("NOH_FFMPEG", folder.path().join("missing-ffmpeg"))
                .env("NOH_CAPTURE_UI", &capture);
            let result = process::run(command, Duration::from_secs(15)).unwrap();
            assert!(
                result.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(result.stderr.is_empty(), "{name}: unexpected stderr");
            result.stdout
        };
        let info: Value = serde_json::from_slice(&run("--build-info")).unwrap();
        assert_eq!(info["schema_version"], 1);
        assert_eq!(info["package_version"], env!("CARGO_PKG_VERSION"));
        for key in ["source_sha256", "build_fingerprint", "options_sha256"] {
            let value = info[key].as_str().unwrap();
            assert_eq!(value.len(), 64, "{name}: {key}");
            assert!(value.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        assert!(!info["target"].as_str().unwrap().is_empty());
        assert!(!info["profile"].as_str().unwrap().is_empty());
        assert!(info["features"].is_array());
        assert!(info["git_dirty"].is_null() || info["git_dirty"].is_boolean());
        if let Some(first) = &expected {
            assert_eq!(first, &info, "{name}: inconsistent identity");
        }
        let version = String::from_utf8(run("--version")).unwrap();
        assert!(version.contains(info["package_version"].as_str().unwrap()));
        assert!(version.contains(&info["build_fingerprint"].as_str().unwrap()[..12]));
        expected = Some(info);
    }
    assert!(!capture.exists(), "Build query started GUI capture");
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);

    let mut command = Command::new(&binaries[0].1);
    command
        .args(["--build-info", "--output"])
        .arg(folder.path().join("must-not-export.mp4"));
    let result = process::run(command, Duration::from_secs(15)).unwrap();
    assert!(
        !result.status.success(),
        "Build query must not accept export arguments"
    );
    assert!(result.stdout.is_empty());
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}
