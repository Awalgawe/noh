//! Identity and report storage shared by the opt-in external campaigns.
use crate::{
    process,
    support::{Fixture, text},
};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

fn identity(program: &str, arguments: &[&str]) -> String {
    let mut command = std::process::Command::new(program);
    command.args(arguments);
    let output = process::run(command, Duration::from_secs(10)).unwrap();
    assert!(output.status.success());
    text(&output.stdout).trim().into()
}

pub fn context(fixture: &Fixture) -> serde_json::Value {
    let hash = |path: &Path| format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()));
    serde_json::json!({
        "schema": 1, "profile": if cfg!(debug_assertions) {"debug"} else {"release"},
        "revision": identity("git", &["rev-parse", "HEAD"]), "tree_status": identity("git", &["status", "--short"]),
        "rustc": identity("rustc", &["-Vv"]), "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "cpu": std::env::var("PROCESSOR_IDENTIFIER").ok(), "logical_cpus": std::thread::available_parallelism().unwrap().get(),
        "executable": fixture.exe, "executable_sha256": hash(&fixture.exe),
        "ffmpeg": text(&fixture.run(&fixture.ffmpeg, args!["-version"]).stdout), "ffmpeg_sha256": hash(&fixture.ffmpeg),
        "per_stage_seconds": null, "ffmpeg_launches": null, "peak_temporary_bytes": null, "process_tree_peak_memory": null,
        "unavailable_metrics": "External measurement; internal instrumentation and resource sampling are not installed."
    })
}

pub fn save(report: &serde_json::Value, variable: &str, default: &str) {
    let path = std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| default.into());
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, serde_json::to_vec_pretty(report).unwrap()).unwrap();
    println!("Benchmark report: {}", path.display());
}
