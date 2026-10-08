//! Opt-in external benchmark. Fixture generation and validation are not timed.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
#[path = "support/benchmark.rs"]
mod benchmark_support;
use std::time::Instant;
use support::Fixture;

#[test]
#[ignore = "explicit performance campaign; NOH_BENCH_REPORT selects JSON output"]
fn export_campaign() {
    let fixture = Fixture::new();
    let video = fixture.path("source.mp4");
    fixture.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=320x180:r=25:d=2",
        "-c:v",
        "libx264",
        "-bf",
        "3",
        &video
    ]);
    let wav = fixture.wav("soundtrack.wav", 7.44);
    let mut cases = Vec::new();
    for (name, flags) in [
        ("copy", args![]),
        (
            "hybrid",
            args!["--partial-fades", "--fade-in", "0.4", "--fade-out", "0.52"],
        ),
        ("encode", args!["--reencode"]),
        ("preview", args!["--preview"]),
    ] {
        let mut samples = Vec::new();
        for sample in 0..3 {
            let output = fixture.path(&format!("{name}-{sample}.mp4"));
            let mut arguments = args![&video, &wav, "-o", &output];
            arguments.extend(flags.clone());
            let command = arguments
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let start = Instant::now();
            fixture.cli(arguments);
            let elapsed = start.elapsed().as_secs_f64();
            // Full decode, duration, dimensions and packet identity for copy.
            let decoded_packets = fixture.packets(&output, false, true, false);
            assert!(decoded_packets.contains("#dimensions 0: 320x180"));
            let decoded = support::rows(&decoded_packets);
            assert!((decoded.len() as f64 / 25.0 - 7.44).abs() <= 0.12);
            if name == "copy" {
                let source = fixture.hashes(&video, true, false, false);
                let copied = fixture.hashes(&output, true, false, false);
                assert!(
                    copied
                        .iter()
                        .enumerate()
                        .all(|(i, h)| h == &source[i % source.len()])
                );
            }
            samples.push(serde_json::json!({"seconds": elapsed, "output_bytes": std::fs::metadata(&output).unwrap().len(), "command": command}));
            fixture.clean();
        }
        let mut times = samples
            .iter()
            .map(|s| s["seconds"].as_f64().unwrap())
            .collect::<Vec<_>>();
        times.sort_by(f64::total_cmp);
        cases.push(serde_json::json!({"name": name, "median_seconds": times[1], "min_seconds": times[0], "max_seconds": times[2], "samples": samples}));
    }
    let mut diagnosis_cases = Vec::new();
    for level in ["quick", "exact"] {
        let mut samples = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            let result = fixture.cli(args![
                &video,
                &wav,
                format!("--diagnose={level}"),
                "--json",
                "--partial-fades",
                "--fade-in",
                "0.4",
                "--fade-out",
                "0.52"
            ]);
            let seconds = start.elapsed().as_secs_f64();
            let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(json["result"]["event"], "inspected");
            assert_eq!(json["result"]["data"]["Ok"]["level"], level);
            assert_eq!(
                json["result"]["data"]["Ok"]["plan"].is_null(),
                level == "quick"
            );
            samples.push(seconds);
            fixture.clean();
        }
        samples.sort_by(f64::total_cmp);
        diagnosis_cases.push(serde_json::json!({"level": level, "median_seconds": samples[1], "min_seconds": samples[0], "max_seconds": samples[2], "samples_seconds": samples}));
    }
    let mut report = benchmark_support::context(&fixture);
    report["fixture"] = serde_json::json!({"source_seconds": 2, "soundtrack_seconds": 7.44, "width": 320, "height": 180, "fps": 25, "b_frames": 3});
    report["cases"] = cases.into();
    report["diagnosis_cases"] = diagnosis_cases.into();
    benchmark_support::save(&report, "NOH_BENCH_REPORT", "target/benchmarks/latest.json");
}
