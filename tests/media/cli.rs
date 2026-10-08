use crate::support::*;
use assert_cmd::assert::OutputAssertExt;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[test]
fn short_cli_structured_range_failures_and_pre_cancel_leave_output_free() {
    let f = Fixture::new();
    let output = f.path("short.mp4");
    let bad = f.run(
        &f.exe,
        args![
            "--short",
            f.path("missing.mp4"),
            "--start-ms",
            "1000",
            "--end-ms",
            "1000",
            "--output",
            &output,
            "--json"
        ],
    );
    assert!(!bad.status.success());
    let value: serde_json::Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["result"]["event"], "done");
    assert_eq!(value["result"]["data"]["Err"]["code"], "error.short_range");
    let signal = f.path("cancel");
    std::fs::write(&signal, b"").unwrap();
    let cancelled = f.run(
        &f.exe,
        args![
            "--short",
            f.path("missing.mp4"),
            "--start-ms",
            "0",
            "--end-ms",
            "1000",
            "--output",
            &output,
            "--json",
            "--cancel-file",
            &signal
        ],
    );
    assert_eq!(cancelled.status.code(), Some(130));
    let value: serde_json::Value = serde_json::from_slice(&cancelled.stdout).unwrap();
    assert_eq!(value["result"]["event"], "cancelled");
    assert!(!output.exists());
    f.clean();
}

struct Cases {
    f: Fixture,
    video: PathBuf,
    wav: PathBuf,
    anamorphic: PathBuf,
    rotated: PathBuf,
    bframes: PathBuf,
    other: PathBuf,
    large: PathBuf,
    rate: PathBuf,
}
impl Cases {
    fn new() -> Self {
        const NAMES: [&str; 8] = [
            "white.mp4",
            "music.wav",
            "anamorphic.mp4",
            "rotated.mp4",
            "bframes.mp4",
            "mpeg4.mp4",
            "large.mp4",
            "30fps.mp4",
        ];
        // Cache immutable bytes only: every test owns its files and the
        // generation directory is removed before initialization completes.
        static INPUTS: OnceLock<[Vec<u8>; 8]> = OnceLock::new();
        let inputs = INPUTS.get_or_init(|| {
            let source = Self::generate();
            NAMES.map(|name| std::fs::read(source.f.path(name)).unwrap())
        });
        let f = Fixture::new();
        let [video, wav, anamorphic, rotated, bframes, other, large, rate] =
            NAMES.map(|name| f.path(name));
        for (name, bytes) in NAMES.iter().zip(inputs) {
            std::fs::write(f.path(name), bytes).unwrap();
        }
        Self {
            f,
            video,
            wav,
            anamorphic,
            rotated,
            bframes,
            other,
            large,
            rate,
        }
    }
    fn generate() -> Self {
        let f = Fixture::new();
        let (video, anamorphic, rotated, bframes, other, large, rate) = (
            f.path("white.mp4"),
            f.path("anamorphic.mp4"),
            f.path("rotated.mp4"),
            f.path("bframes.mp4"),
            f.path("mpeg4.mp4"),
            f.path("large.mp4"),
            f.path("30fps.mp4"),
        );
        for (path, sar) in [(&video, "1"), (&anamorphic, "2")] {
            f.ff(args![
                "-f",
                "lavfi",
                "-i",
                "color=c=white:s=96x64:r=25:d=1",
                "-vf",
                format!("setsar={sar}"),
                "-c:v",
                "libx264",
                "-profile:v",
                "main",
                "-bf",
                "0",
                path
            ]);
        }
        f.ff(args![
            "-display_rotation:v:0",
            "90",
            "-i",
            &video,
            "-c",
            "copy",
            &rotated
        ]);
        assert!(
            f.packets(&rotated, false, true, false)
                .contains("#dimensions 0: 64x96")
        );
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=96x64:r=25:d=1",
            "-c:v",
            "libx264",
            "-profile:v",
            "high",
            "-bf",
            "2",
            "-x264-params",
            "b-adapt=0",
            &bframes
        ]);
        for (path, codec, size, fps) in [
            (&other, "mpeg4", "96x64", "25"),
            (&large, "libx264", "128x72", "25"),
            (&rate, "libx264", "96x64", "30"),
        ] {
            f.ff(args![
                "-f",
                "lavfi",
                "-i",
                format!("testsrc2=s={size}:r={fps}:d=1"),
                "-c:v",
                codec,
                "-bf",
                "0",
                path
            ]);
        }
        let wav = f.wav("music.wav", 3.44);
        Self {
            f,
            video,
            wav,
            anamorphic,
            rotated,
            bframes,
            other,
            large,
            rate,
        }
    }
    fn export(&self, name: &str, first: &Path, flags: Vec<OsString>) -> (PathBuf, String) {
        let out = self.f.path(&format!("{name}.mp4"));
        let mut args = args!["--worker", first, &self.wav, "-o", &out];
        args.extend(flags);
        let log = log(&self.f.cli(args));
        (out, log)
    }
    fn frame(&self, path: &Path, seconds: f64) -> ((usize, usize), (usize, usize, usize, usize)) {
        let header = self.f.packets(path, false, false, false);
        assert!(header.contains("#sar 0: 1/1"));
        let (w, h) = header
            .lines()
            .find_map(|l| l.strip_prefix("#dimensions 0: "))
            .unwrap()
            .split_once('x')
            .unwrap();
        let (w, h): (usize, usize) = (w.parse().unwrap(), h.parse().unwrap());
        let pixels = self
            .f
            .ff(args![
                "-ss",
                seconds,
                "-i",
                path,
                "-map",
                "0:v:0",
                "-frames:v",
                "1",
                "-vf",
                "format=gray",
                "-f",
                "rawvideo",
                "-"
            ])
            .stdout;
        assert_eq!(pixels.len(), w * h);
        let white: Vec<_> = pixels
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 240)
            .map(|(i, _)| (i % w, i / w))
            .collect();
        assert!(!white.is_empty());
        (
            (w, h),
            (
                white.iter().map(|p| p.0).min().unwrap(),
                white.iter().map(|p| p.1).min().unwrap(),
                white.iter().map(|p| p.0).max().unwrap() + 1,
                white.iter().map(|p| p.1).max().unwrap() + 1,
            ),
        )
    }
}

#[test]
fn cli_uses_english_flags_and_diagnostics() {
    let c = Cases::new();
    let help = text(&c.f.cli(args!["--help"]).stdout);
    for flag in [
        "--output",
        "--partial-fades",
        "--preview",
        "--clip-audio",
        "--reencode",
    ] {
        assert!(help.contains(flag));
    }
    let out = c.f.path("english.mp4");
    let result = c.f.cli(args![
        &c.other,
        &c.wav,
        "--partial-fades",
        "--fade-in",
        ".2",
        "--output",
        &out
    ]);
    assert!(text(&result.stdout).contains("Progress:"));
    assert!(text(&result.stdout).contains("Done:"));
    assert!(text(&result.stderr).contains("Warning: Clip 1/1 (mpeg4): converted once to H.264"));
    assert!(out.is_file());
}

#[test]
fn removed_french_flags_are_rejected_without_creating_output() {
    let f = Fixture::new();
    let help = text(&f.cli(args!["--help"]).stdout);
    for flag in [
        "--sortie",
        "--fondus-partiels",
        "--apercu",
        "--son-video",
        "--reencoder",
    ] {
        let out = f.path("unexpected.mp4");
        assert!(!help.contains(flag));
        f.run(
            &f.exe,
            args!["video.mp4", "music.wav", flag, "--output", &out],
        )
        .assert()
        .failure()
        .stderr(format!("Error: Unknown option: {flag}\n"));
        assert!(!out.exists());
    }
}

#[test]
fn display_geometry_survives_harmonization_and_preview() {
    let c = Cases::new();
    for preview in [false, true] {
        let mode = if preview { "--preview" } else { "--reencode" };
        for (index, source, (w, h)) in [(0, &c.anamorphic, (192, 64)), (1, &c.rotated, (64, 96))] {
            let (out, _) = c.export(
                &format!("geometry-{preview}-{index}"),
                source,
                args!["--video", source, mode],
            );
            assert_eq!(c.frame(&out, 0.0), ((w, h), (0, 0, w, h)));
        }
        let (out, _) = c.export(
            &format!("mixed-{preview}"),
            &c.video,
            args!["--video", &c.anamorphic, mode],
        );
        assert_eq!(c.frame(&out, 1.2), ((96, 64), (0, 16, 96, 48)));
    }
    let (out, _) = c.export("single-preview", &c.anamorphic, args!["--preview"]);
    assert_eq!(c.frame(&out, 0.0), ((192, 64), (0, 0, 192, 64)));
}

#[test]
fn progress_is_monotonic_and_completes_after_publication() {
    let c = Cases::new();
    for (i, flags) in [
        args![],
        args!["--reencode"],
        args!["--clip-audio"],
        args!["--video", &c.video],
        args!["--video", &c.video, "--clip-audio"],
        args![
            "--video",
            &c.video,
            "--partial-fades",
            "--fade-in",
            ".2",
            "--fade-out",
            ".2"
        ],
        args![
            "--video",
            &c.video,
            "--preview",
            "--clip-audio",
            "--fade-in",
            ".2"
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let (out, log) = c.export(&format!("progress-{i}"), &c.video, flags);
        progress(&log);
        assert!(out.is_file());
    }
}

#[test]
fn encoding_is_never_reported_as_copy() {
    let c = Cases::new();
    for (name, flags, phase) in [
        ("copy", args![], "progress.copy"),
        ("encode", args!["--reencode"], "progress.encode"),
        ("preview", args!["--preview"], "progress.preview"),
    ] {
        let (_, log) = c.export(name, &c.video, flags);
        assert!(
            log.lines()
                .any(|l| l.starts_with("NOH_PROGRESS|") && l.ends_with(phase)),
            "{log}"
        );
        assert_eq!(log.contains("copy without re-encoding"), name == "copy");
    }
}

#[test]
fn bframes_use_partial_fades_and_no_fades_still_preserve_packets() {
    let c = Cases::new();
    let original = rows(&c.f.packets(&c.bframes, false, false, false));
    assert!(original.iter().any(|r| r[1] != r[2]));
    let (out, log) = c.export(
        "adapted",
        &c.bframes,
        args!["--partial-fades", "--fade-in", ".2", "--fade-out", ".2"],
    );
    for unexpected in ["NOH_WARNING|", "NOH_ERROR|"] {
        assert!(!log.contains(unexpected));
    }
    for expected in [
        "NOH_PROGRESS|100|",
        "progress.assemble",
        "central loops copied",
    ] {
        assert!(log.contains(expected));
    }
    c.f.clean();
    let (copied, log) = c.export("copied", &c.bframes, args!["--partial-fades"]);
    assert!(!log.contains("NOH_WARNING|"));
    assert_eq!(
        &c.f.hashes(&copied, false, false, false)[..original.len()],
        c.f.hashes(&c.bframes, false, false, false)
    );
    let (encoded, log) = c.export(
        "encoded",
        &c.bframes,
        args!["--reencode", "--fade-in", ".2", "--fade-out", ".2"],
    );
    assert!(!log.contains("NOH_WARNING|"));
    for path in [out, encoded] {
        assert!(
            c.f.ff(args![
                "-err_detect",
                "explode",
                "-i",
                &path,
                "-f",
                "null",
                "-"
            ])
            .stderr
            .is_empty()
        );
        let first =
            c.f.ff(args![
                "-i",
                &path,
                "-map",
                "0:v:0",
                "-frames:v",
                "1",
                "-vf",
                "format=gray",
                "-f",
                "rawvideo",
                "-"
            ])
            .stdout;
        assert!(!first.is_empty());
        assert!(*first.iter().max().unwrap() < 5);
    }
}

#[test]
fn different_codec_is_converted_once_before_partial_fades() {
    let c = Cases::new();
    let (out, log) = c.export(
        "adapted",
        &c.other,
        args!["--partial-fades", "--fade-in", ".2"],
    );
    assert!(log.contains("NOH_WARNING|warning.convert_clip|1|1|mpeg4"));
    assert!(
        c.f.ff(args![
            "-err_detect",
            "explode",
            "-i",
            &out,
            "-f",
            "null",
            "-"
        ])
        .stderr
        .is_empty()
    );
}

#[test]
fn heterogeneous_lists_warn_and_harmonize_automatically() {
    let c = Cases::new();
    for (i, source) in [&c.other, &c.large, &c.rate, &c.anamorphic, &c.rotated]
        .into_iter()
        .enumerate()
    {
        let (out, log) = c.export(&format!("adapted-{i}"), &c.video, args!["--video", source]);
        assert!(log.contains("NOH_WARNING|warning.harmonize|96|64|25.000"));
        let header = c.f.packets(&out, false, false, false);
        assert!(header.contains("#dimensions 0: 96x64"));
        assert!(header.contains("#codec_id 0: h264"));
        assert!(
            c.f.ff(args![
                "-err_detect",
                "explode",
                "-i",
                &out,
                "-f",
                "null",
                "-"
            ])
            .stderr
            .is_empty()
        );
    }
    let (_, log) = c.export("same", &c.video, args!["--video", &c.video]);
    assert!(!log.contains("NOH_WARNING|"));
}

#[test]
fn unreadable_video_fails_without_silent_retry_or_output() {
    let f = Fixture::new();
    let wav = f.wav("music.wav", 3.44);
    let source = f.path("broken.mp4");
    let out = f.path("failed.mp4");
    std::fs::write(&source, b"invalid video").unwrap();
    let result = f.run(
        &f.exe,
        args![
            "--worker",
            &source,
            &wav,
            "--partial-fades",
            "--fade-in",
            ".2",
            "-o",
            &out
        ],
    );
    result.clone().assert().failure();
    let log = log(&result);
    assert!(log.contains("NOH_ERROR|error.video"));
    assert!(!log.contains("NOH_WARNING|"));
    assert!(!log.contains("NOH_PROGRESS|100|"));
    assert!(!out.exists());
    f.clean();
}
