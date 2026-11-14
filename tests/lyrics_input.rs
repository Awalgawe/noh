//! The CLI keeps `--srt` and its lyrics file in step before any job starts
//! (lyrics plan 10). Works without media or FFmpeg: the subtitles are resolved
//! first, and a refused difference stops before the job.
#[path = "../tools/process.rs"]
mod process;
use serde_json::Value;
use std::{path::PathBuf, process::Command, time::Duration};

fn noh() -> PathBuf {
    std::env::var_os("NOH_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_noh").into())
}

const A: &str = "1\n00:00:01,000 --> 00:00:02,000\nla la\n\n";
const B: &str = "1\n00:00:01,000 --> 00:00:02,000\nlo lo\n\n";

fn lyrics_json(text: &str) -> String {
    let start = 1000;
    let end = 2000;
    let line = text.lines().nth(2).unwrap();
    format!(r#"{{"version":2,"cues":[{{"start_ms":{start},"end_ms":{end},"text":"{line}"}}]}}"#)
}

fn burn(folder: &std::path::Path, extra: &[&str]) -> std::process::Output {
    let mut command = Command::new(noh());
    command
        .args(["--burn-subtitles"])
        .arg(folder.join("missing.mp4"))
        .arg("--srt")
        .arg(folder.join("song.srt"))
        .arg("--output")
        .arg(folder.join("out.mp4"))
        .args(extra)
        .env("NOH_FFMPEG", folder.join("missing-ffmpeg"));
    process::run(command, Duration::from_secs(20)).unwrap()
}

#[test]
fn a_difference_without_truth_stops_before_any_write_with_a_structured_error() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("song.srt"), B).unwrap();
    std::fs::write(folder.path().join("song.noh-lyrics.json"), lyrics_json(A)).unwrap();
    let result = burn(folder.path(), &["--json"]);
    assert!(!result.status.success());
    let line = String::from_utf8_lossy(&result.stdout);
    let reply: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(reply["error"]["code"], "error.lyrics_truth_required");
    assert_eq!(reply["error"]["technical"]["differing_lines"], 1);
    assert_eq!(
        reply["error"]["technical"]["accepted"],
        serde_json::json!(["lyrics", "srt"])
    );
    // Not a terminal, --json: nothing asked, nothing written.
    assert_eq!(
        std::fs::read_to_string(folder.path().join("song.srt")).unwrap(),
        B
    );
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 2);
}

#[test]
fn truth_unifies_and_names_the_kept_file_before_the_job_runs() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("song.srt"), B).unwrap();
    std::fs::write(folder.path().join("song.noh-lyrics.json"), lyrics_json(A)).unwrap();
    // The job itself fails (no source, no FFmpeg): the unification came first.
    let result = burn(folder.path(), &["--truth", "lyrics"]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    let kept = folder.path().join("song.replaced.srt");
    assert!(stderr.contains(&kept.display().to_string()), "{stderr}");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), B);
    assert!(
        std::fs::read_to_string(folder.path().join("song.srt"))
            .unwrap()
            .contains("la la")
    );
}

#[test]
fn an_srt_alone_gets_its_lyrics_file_and_says_so() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("song.srt"), A).unwrap();
    let result = burn(folder.path(), &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("song.noh-lyrics.json"), "{stderr}");
    assert!(folder.path().join("song.noh-lyrics.json").is_file());
}

/// A refused caption effect stops the command before the subtitles are touched:
/// no lyrics file is created beside an .srt given alone.
#[test]
fn a_refused_effect_stops_before_the_lyrics_file_is_created() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("song.srt"), A).unwrap();
    for extra in [
        &["--effect", "noh.nope"][..],
        &["--effect", "noh.classic", "--effect-value", "size=huge"][..],
    ] {
        let result = burn(folder.path(), extra);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{extra:?}");
        assert!(stderr.contains("error.effect"), "{extra:?}: {stderr}");
        assert!(
            !folder.path().join("song.noh-lyrics.json").exists(),
            "{extra:?}: {stderr}"
        );
    }
    let names: Vec<_> = std::fs::read_dir(folder.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["song.srt"]);
}
