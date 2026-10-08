//! End-to-end media contracts using synthetic fixtures.
use crate::support::*;
use assert_cmd::assert::OutputAssertExt;
use std::path::Path;

fn audio_hash(f: &Fixture, path: &Path) -> Vec<u8> {
    f.ff(args![
        "-i",
        path,
        "-map",
        "0:a:0",
        "-c:a",
        "pcm_s24le",
        "-f",
        "hash",
        "-"
    ])
    .stdout
}

#[test]
fn multimedia_contracts() {
    let f = Fixture::new();
    let video = f.path("video with spaces Δ.mp4");
    let wav = f.path("24-bit soundtrack.wav");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=white:s=96x64:r=25:d=1",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=110:duration=1",
        "-c:v",
        "libx264",
        "-c:a",
        "aac",
        "-shortest",
        &video
    ]);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:duration=3.44",
        "-c:a",
        "pcm_s24le",
        &wav
    ]);
    let original = f.hashes(&video, false, false, false);
    // Packet preservation, duration and refusal to overwrite.
    for ext in ["mp4", "mkv"] {
        let output = f.path(&format!("copy.{ext}"));
        f.cli(args![&video, &wav, "-o", &output]);
        let hashes = f.hashes(&output, false, false, false);
        assert!((84..=88).contains(&hashes.len()));
        assert!(
            hashes
                .iter()
                .enumerate()
                .all(|(i, h)| *h == original[i % original.len()])
        );
        near(f.duration(&output), 3.44, 0.04);
        let bytes = std::fs::read(&output).unwrap();
        f.run(&f.exe, args![&video, &wav, "-o", &output])
            .assert()
            .failure();
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
    }
    // lossless audio samples, not merely matching stream metadata.
    assert_eq!(audio_hash(&f, &wav), audio_hash(&f, &f.path("copy.mkv")));
    // fades affect only endpoints and never the WAV samples.
    let faded = f.path("fades.mkv");
    f.cli(args![
        &video,
        &wav,
        "--fade-in",
        "0,4",
        "--fade-out",
        "0.52",
        "-o",
        &faded
    ]);
    let light = f.gray(&faded);
    assert_eq!(light.len(), 86);
    assert!(light[0] < 5 && light[25] > 240 && light[50] > 240);
    assert!(*light.last().unwrap() < 30 && light[light.len() - 15] > 240);
    assert_eq!(audio_hash(&f, &wav), audio_hash(&f, &faded));
    // float/RF64, including audio shorter than the source video.
    for (codec, name, extra) in [
        ("pcm_f32le", "float.wav", args![]),
        ("pcm_s16le", "short_rf64.wav", args!["-rf64", "always"]),
    ] {
        let audio = f.path(name);
        let out = f.path(&format!("{name}.mkv"));
        let mut flags = args!["-f", "lavfi", "-i", "sine=duration=0.36", "-c:a", codec];
        flags.extend(extra);
        flags.extend(args![&audio]);
        f.ff(flags);
        f.cli(args![&video, &audio, "-o", &out]);
        near(f.duration(&out), 0.36, 0.04);
    }
    // invalid settings, corrupt input, no published or leaked output.
    for flags in [
        args!["--fade-in", "-1"],
        args!["--fade-out", "NaN"],
        args!["--fade-in", "4"],
        args!["-o", &video],
    ] {
        let mut args = args![&video, &wav];
        args.extend(flags);
        f.run(&f.exe, args).assert().failure();
    }
    let broken = f.path("corrupted.mp4");
    let failed = f.path("failed.mp4");
    std::fs::write(&broken, b"not a video").unwrap();
    f.run(&f.exe, args![&broken, &wav, "-o", &failed])
        .assert()
        .failure();
    assert!(!failed.exists());
    f.clean();
    // hybrid joins, all fade combinations, copied slices and decoded pixels.
    let adapted = f.path("adapted-bframes.mp4");
    let result = f.cli(args![
        &video,
        &wav,
        "--partial-fades",
        "--fade-in",
        ".4",
        "-o",
        &adapted
    ]);
    assert!(log(&result).contains("central loops copied"));
    assert!(!text(&result.stderr).contains("Warning:"));
    near(f.duration(&adapted), 3.44, 0.01);
    assert!(f.gray(&adapted)[0] < 5);
    let source = f.path("source main.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "libx264",
        "-profile:v",
        "main",
        "-bf",
        "0",
        &source
    ]);
    let slices = f.hashes(&source, true, false, false);
    let decoded = f.hashes(&source, false, true, false);
    for (i, (fade_in, fade_out)) in [(0.4, 0.52), (0.0, 0.52), (0.4, 0.0)]
        .into_iter()
        .enumerate()
    {
        let out = f.path(&format!("partial-{i}.mp4"));
        f.cli(args![
            &source,
            &wav,
            "--partial-fades",
            "--fade-in",
            fade_in,
            "--fade-out",
            fade_out,
            "-o",
            &out
        ]);
        assert_eq!(&f.hashes(&out, true, false, false)[25..50], slices);
        assert_eq!(&f.hashes(&out, false, true, false)[25..50], decoded);
        near(f.duration(&out), 3.44, 0.01);
        assert_eq!(
            f.hashes(&out, false, false, true),
            f.hashes(&f.path("copy.mp4"), false, false, true)
        );
    }
    // whole-duration preview, scaling, optional fades, complete audio.
    let large = f.path("large.mp4");
    let reference_audio = f.path("preview audio.m4a");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=white:s=960x540:r=25:d=1",
        "-c:v",
        "libx264",
        &large
    ]);
    f.ff(args![
        "-i",
        &wav,
        "-c:a",
        "aac",
        "-b:a",
        "160k",
        &reference_audio
    ]);
    for fades in [false, true] {
        let out = f.path(&format!("preview-{fades}.mp4"));
        let mut flags = args![&large, &wav, "--preview", "-o", &out];
        if fades {
            flags.extend(args![
                "--fade-in",
                ".4",
                "--fade-out",
                ".52",
                "--partial-fades"
            ]);
        }
        f.cli(flags);
        near(f.duration(&out), 3.44, 0.01);
        assert!(
            f.packets(&out, false, false, false)
                .contains("#dimensions 0: 640x360")
        );
        let light = f.gray(&out);
        assert_eq!(light.len(), 86);
        if fades {
            assert!(light[0] < 5 && light[85] < 30 && light[25] > 240 && light[50] > 240);
        } else {
            assert!(light.iter().all(|&v| v > 240));
        }
        assert_eq!(
            f.hashes(&out, false, false, true),
            f.hashes(&reference_audio, false, false, true)
        );
    }
    // order/reordering, repeated sequence, silent clips and optional mixing.
    let a = f.path("A.mp4");
    let b = f.path("B.mp4");
    let c = f.path("C.mp4");
    for (path, color, length, tone) in [
        (&a, "white", 1.0, Some(330)),
        (&b, "black", 0.6, None),
        (&c, "gray", 0.4, Some(550)),
    ] {
        let mut flags = args![
            "-f",
            "lavfi",
            "-i",
            format!("color=c={color}:s=96x64:r=25:d={length}")
        ];
        if let Some(tone) = tone {
            flags.extend(args![
                "-f",
                "lavfi",
                "-i",
                format!("sine=frequency={tone}:duration={length}")
            ]);
        }
        flags.extend(args![
            "-c:v",
            "libx264",
            "-profile:v",
            "main",
            "-bf",
            "0",
            "-c:a",
            "aac",
            path
        ]);
        f.ff(flags);
    }
    let long = f.wav("sequence soundtrack.wav", 5.44);
    let sequence = f.path("sequence.mp4");
    f.cli(args![
        &a, &long, "--video", &b, "--video", &c, "-o", &sequence
    ]);
    near(f.duration(&sequence), 5.44, 0.01);
    let expected: Vec<_> = [&a, &b, &c]
        .into_iter()
        .flat_map(|p| f.hashes(p, true, false, false))
        .collect();
    assert!(
        f.hashes(&sequence, true, false, false)
            .iter()
            .enumerate()
            .all(|(i, h)| *h == expected[i % expected.len()])
    );
    let gray = f.gray(&sequence);
    for i in [0, 50, 100] {
        assert!(gray[i] > 240);
    }
    for i in [25, 75, 125] {
        assert!(gray[i] < 5);
    }
    for i in [40, 90] {
        assert!(gray[i] > 110 && gray[i] < 150);
    }
    let reversed = f.path("reversed.mp4");
    f.cli(args![
        &b, &long, "--video", &a, "--video", &c, "-o", &reversed
    ]);
    let gray = f.gray(&reversed);
    assert!(gray[0] < 5 && gray[15] > 240);
    let list = f.path("sequence fades.mp4");
    f.cli(args![
        &a,
        &long,
        "--video",
        &b,
        "--video",
        &c,
        "--partial-fades",
        "--fade-in",
        ".4",
        "--fade-out",
        ".52",
        "-o",
        &list
    ]);
    assert_eq!(&f.hashes(&list, true, false, false)[50..100], expected);
    assert!(f.gray(&list)[0] < 5);
    let reference = f.path("wav only.m4a");
    f.ff(args![
        "-i", &long, "-c:a", "aac", "-b:a", "320k", &reference
    ]);
    assert_eq!(
        f.hashes(&sequence, false, false, true),
        f.hashes(&reference, false, false, true)
    );
    let mix = f.path("mixed sequence.mp4");
    f.cli(args![
        &a,
        &long,
        "--video",
        &b,
        "--video",
        &c,
        "--clip-audio",
        "-o",
        &mix
    ]);
    assert_ne!(
        f.hashes(&mix, false, false, true),
        f.hashes(&reference, false, false, true)
    );
    near(f.duration(&mix), 5.44, 0.01);
    // short WAV, mixed geometry, multi-clip preview, fractional rates, cleanup.
    let tiny = f.wav("very short.wav", 0.36);
    let out = f.path("required portion.mp4");
    f.cli(args![
        &a,
        &tiny,
        "--partial-fades",
        "--fade-in",
        ".08",
        "--fade-out",
        ".12",
        "-o",
        &out
    ]);
    near(f.duration(&out), 0.36, 0.01);
    assert!(f.gray(&out)[0] < 5);
    let out = f.path("mixed formats.mp4");
    let result = f.cli(args![
        &a,
        &wav,
        "--video",
        &large,
        "--partial-fades",
        "--fade-in",
        ".4",
        "-o",
        &out
    ]);
    assert!(text(&result.stderr).contains("Warning:"));
    near(f.duration(&out), 3.44, 0.01);
    let out = f.path("sequence preview.mp4");
    f.cli(args![
        &large,
        &long,
        "--video",
        &a,
        "--preview",
        "--fade-in",
        ".4",
        "-o",
        &out
    ]);
    near(f.duration(&out), 5.44, 0.01);
    assert!(
        f.packets(&out, false, false, false)
            .contains("#dimensions 0: 640x360")
    );
    let fractional = f.path("frame rate 23976.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=white:s=96x64:r=24000/1001:d=1",
        "-c:v",
        "libx264",
        &fractional
    ]);
    let out = f.path("preserved frame rate.mp4");
    f.cli(args![
        &fractional,
        &long,
        "--video",
        &a,
        "--reencode",
        "--fade-in",
        ".4",
        "--fade-out",
        ".52",
        "-o",
        &out
    ]);
    near(f.duration(&out), 5.44, 0.01);
    assert!(f.gray(&out)[0] < 5);
    f.clean();
}
