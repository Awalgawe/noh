use crate::support::*;
use std::path::Path;

fn timing(f: &Fixture, path: &Path, rate: f64, duration: f64) {
    let packets = f.packets(path, false, false, false);
    let tb = packets
        .lines()
        .find_map(|l| l.strip_prefix("#tb 0: "))
        .unwrap();
    let (a, b) = tb.split_once('/').unwrap();
    let tb = a.parse::<f64>().unwrap() / b.parse::<f64>().unwrap();
    let rows = rows(&packets);
    let dts: Vec<i64> = rows.iter().map(|r| r[1].parse().unwrap()).collect();
    assert!(dts.windows(2).all(|v| v[0] < v[1]));
    let mut pts: Vec<f64> = rows
        .iter()
        .map(|r| r[2].parse::<f64>().unwrap() * tb)
        .collect();
    pts.sort_by(f64::total_cmp);
    for (index, time) in pts.iter().enumerate() {
        near(*time, index as f64 / rate, 1e-4);
    }
    assert!(*pts.last().unwrap() < duration);
    assert!(*pts.last().unwrap() + 1.0 / rate >= duration - 1e-4);
    // Edit lists trim a fractional last frame without changing packet duration.
    near(f.duration(path), duration, 0.001);
}

#[test]
fn bframe_fades_keep_central_loops_bit_exact() {
    let f = Fixture::new();
    let wav = f.wav("music.wav", 3.44);
    for (bframes, rate) in [
        (0, "25"),
        (1, "25"),
        (2, "25"),
        (3, "25"),
        (8, "25"),
        (3, "24000/1001"),
    ] {
        let source = f.path(&format!("source-{bframes}-{}.mp4", rate.replace('/', "-")));
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            format!("testsrc2=s=96x64:r={rate}"),
            "-frames:v",
            "25",
            "-c:v",
            "libx264",
            "-bf",
            bframes,
            "-x264-params",
            "b-adapt=0:open-gop=0",
            &source
        ]);
        let original = f.hashes(&source, true, false, false);
        let decoded = f.hashes(&source, false, true, false);
        let fps = if rate == "25" { 25.0 } else { 24000.0 / 1001.0 };
        for (index, (fade_in, fade_out)) in
            [(0.2, 0.2), (0.2, 0.0), (0.0, 0.2)].into_iter().enumerate()
        {
            let output = source.with_file_name(format!(
                "out-{}-{index}.mp4",
                source.file_stem().unwrap().to_string_lossy()
            ));
            let result = f.cli(args![
                "--worker",
                &source,
                &wav,
                "--partial-fades",
                "--fade-in",
                fade_in,
                "--fade-out",
                fade_out,
                "-o",
                &output
            ]);
            assert!(!text(&result.stderr).contains("NOH_WARNING|warning.partial_reordered"));
            assert!(!text(&result.stdout).contains("progress.encode"));
            assert!(!log(&result).contains("Non-monoton"));
            assert_eq!(&f.hashes(&output, true, false, false)[25..50], original);
            assert_eq!(&f.hashes(&output, false, true, false)[25..50], decoded);
            timing(&f, &output, fps, 3.44);
            progress(&text(&result.stdout));
        }
    }
}

#[test]
fn mixed_codecs_convert_only_incompatible_clips() {
    let f = Fixture::new();
    let wav = f.wav("long.wav", 10.44);
    let a = f.path("keep-a.mp4");
    let b = f.path("keep-b.mp4");
    for (path, bframes, timescale) in [(&a, 3, "12800"), (&b, 1, "90000")] {
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=96x64:r=25:d=1",
            "-c:v",
            "libx264",
            "-bf",
            bframes,
            "-profile:v",
            "high",
            "-x264-params",
            "b-adapt=0:open-gop=0",
            "-video_track_timescale",
            timescale,
            path
        ]);
    }
    let originals = [
        (
            &a,
            f.hashes(&a, true, false, false),
            f.hashes(&a, false, true, false),
        ),
        (
            &b,
            f.hashes(&b, true, false, false),
            f.hashes(&b, false, true, false),
        ),
    ];
    for (codec, suffix, extra) in [
        ("mpeg4", ".mp4", args![]),
        (
            "libx265",
            ".mp4",
            args!["-x265-params", "pools=1:frame-threads=1"],
        ),
        ("libvpx-vp9", ".webm", args!["-threads", "1"]),
    ] {
        let other = f.path(&format!("{codec}{suffix}"));
        let mut flags = args![
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=96x64:r=25:d=1",
            "-c:v",
            codec
        ];
        flags.extend(extra);
        flags.extend(args![&other]);
        f.ff(flags);
        for fades in [false, true] {
            let output = f.path(&format!("mixed-{codec}-{fades}.mp4"));
            let mut flags = args![
                "--worker", &a, &wav, "--video", &other, "--video", &b, "-o", &output
            ];
            if fades {
                flags.extend(args![
                    "--partial-fades",
                    "--fade-in",
                    ".2",
                    "--fade-out",
                    ".2"
                ]);
            }
            let result = f.cli(flags);
            let log = log(&result);
            let warnings: Vec<_> = log
                .lines()
                .filter_map(|l| l.strip_prefix("NOH_WARNING|warning.convert_clip|"))
                .collect();
            assert_eq!(warnings.len(), 1, "{log}");
            assert!(warnings[0].starts_with("2|3|"));
            assert!(!log.contains("Non-monoton"));
            let packets = f.hashes(&output, true, false, false);
            let decoded = f.hashes(&output, false, true, false);
            let offset = if fades { 75 } else { 0 };
            for (index, (_, expected_packets, expected_frames)) in originals.iter().enumerate() {
                let start = offset + index * 50;
                assert_eq!(&packets[start..start + 25], expected_packets);
                assert_eq!(&decoded[start..start + 25], expected_frames);
            }
            timing(&f, &output, 25.0, 10.44);
        }
    }
}

#[test]
fn first_clip_can_require_conversion_without_reencoding_the_next() {
    let f = Fixture::new();
    let wav = f.wav("long.wav", 10.44);
    let first = f.path("first-hevc.mp4");
    let keep = f.path("second-h264.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "libx265",
        "-x265-params",
        "pools=1:frame-threads=1",
        &first
    ]);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "libx264",
        "-bf",
        "3",
        "-x264-params",
        "b-adapt=0:open-gop=0",
        &keep
    ]);
    let output = f.path("first-converted.mp4");
    let result = f.cli(args![
        "--worker",
        &first,
        &wav,
        "--video",
        &keep,
        "--partial-fades",
        "--fade-in",
        ".2",
        "--fade-out",
        ".2",
        "-o",
        &output
    ]);
    assert!(log(&result).contains("NOH_WARNING|warning.convert_clip|1|2|hevc"));
    assert!(!log(&result).contains("NOH_WARNING|warning.convert_clip|2|"));
    assert_eq!(
        &f.hashes(&output, true, false, false)[75..100],
        f.hashes(&keep, true, false, false)
    );
    assert_eq!(
        &f.hashes(&output, false, true, false)[75..100],
        f.hashes(&keep, false, true, false)
    );
    timing(&f, &output, 25.0, 10.44);
}
