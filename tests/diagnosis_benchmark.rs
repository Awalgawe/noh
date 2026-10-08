//! Opt-in diagnosis scaling campaign, with no rendering or persistent cache.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
#[path = "support/benchmark.rs"]
mod benchmark_support;
use noh::{engine::Event, inspection::InspectionResult, plan::Treatment};
use std::{path::PathBuf, time::Instant};
use support::Fixture;

#[test]
#[ignore = "explicit scaling campaign; NOH_DIAG_BENCH_REPORT selects JSON output"]
fn diagnosis_scaling_campaign() {
    let f = Fixture::new();
    let source = f.path("h264.mp4");
    let hevc = f.path("hevc.mp4");
    let vp9 = f.path("vp9.webm");
    for (path, size, codec, flags) in [
        (&source, "320x180", "libx264", args!["-bf", "3"]),
        (
            &hevc,
            "160x120",
            "libx265",
            args![
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error:pools=1"
            ],
        ),
        (
            &vp9,
            "192x108",
            "libvpx-vp9",
            args!["-deadline", "realtime", "-cpu-used", "8"],
        ),
    ] {
        let mut arguments = args![
            "-f",
            "lavfi",
            "-i",
            format!("testsrc2=s={size}:r=25:d=1"),
            "-c:v",
            codec
        ];
        arguments.extend(flags);
        arguments.push(path.as_os_str().to_owned());
        f.ff(arguments);
    }
    let mixed: Vec<PathBuf> = (0..100)
        .map(|index| {
            let input = [&source, &hevc, &vp9][index % 3];
            let output = f.path(&format!(
                "clip-{index:03}.{}",
                input.extension().unwrap().to_str().unwrap()
            ));
            std::fs::copy(input, &output).unwrap();
            output
        })
        .collect();
    let wav = f.wav("soundtrack.wav", 7.44);
    let output = f.path("not-rendered.mp4");
    let mut cases = Vec::new();
    for count in [1, 10, 100] {
        for distinct in [false, true] {
            let videos = if distinct {
                mixed[..count].to_vec()
            } else {
                vec![source.clone(); count]
            };
            for level in ["quick", "exact"] {
                let mut samples = Vec::new();
                let mut response_bytes = Vec::new();
                let mut arguments = args![
                    &videos[0],
                    &wav,
                    "--output",
                    &output,
                    "--partial-fades",
                    "--fade-in",
                    "0.2",
                    "--fade-out",
                    "0.2",
                    format!("--diagnose={level}"),
                    "--json"
                ];
                for video in &videos[1..] {
                    arguments.extend(args!["--video", video]);
                }
                for _ in 0..3 {
                    let start = Instant::now();
                    let result = f.cli(arguments.clone());
                    samples.push(start.elapsed().as_secs_f64());
                    response_bytes.push(result.stdout.len());
                    // Parse/validate after the timer, independently of the measured CLI.
                    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
                    assert_eq!(json["version"], 1);
                    let Event::Inspected(Ok(InspectionResult::Plan(diagnosis))) =
                        serde_json::from_value(json["result"].clone()).unwrap()
                    else {
                        panic!("{json}")
                    };
                    assert_eq!(
                        diagnosis
                            .request
                            .items
                            .iter()
                            .map(|item| item.path())
                            .collect::<Vec<_>>(),
                        videos.iter().collect::<Vec<_>>()
                    );
                    assert_eq!(serde_json::to_value(diagnosis.level).unwrap(), level);
                    support::near(diagnosis.duration, 7.44, 1.0 / 44100.0);
                    if level == "quick" {
                        assert!(diagnosis.plan.is_none());
                        assert_eq!(diagnosis.quick_media.len(), count);
                        assert!(
                            diagnosis
                                .quick_media
                                .iter()
                                .all(|m| (m.seconds - 1.0).abs() < 0.05)
                        );
                    } else {
                        let plan = diagnosis.plan.unwrap();
                        assert!(plan.exact_inspection);
                        assert_eq!(plan.clips.len(), count);
                        assert_eq!((plan.target.width, plan.target.height), (320, 180));
                        assert_eq!(plan.target.codec, "h264");
                        for (index, clip) in plan.clips.iter().enumerate() {
                            assert_eq!(clip.path, videos[index]);
                            assert_eq!(
                                clip.treatment,
                                if distinct && index % 3 != 0 {
                                    Treatment::Convert
                                } else {
                                    Treatment::Copy
                                }
                            );
                            if clip.treatment == Treatment::Convert {
                                assert!(!clip.reasons.is_empty());
                            }
                        }
                    }
                    assert!(!output.exists());
                    f.clean();
                }
                let mut sorted = samples.clone();
                sorted.sort_by(f64::total_cmp);
                cases.push(serde_json::json!({
                    "name": if distinct {"distinct_mixed"} else {"repeated_h264"},
                    "clips": count, "unique_paths": if distinct {count} else {1}, "level": level,
                    "median_seconds": sorted[1], "min_seconds": sorted[0], "max_seconds": sorted[2],
                    "samples_seconds": samples, "response_bytes": response_bytes,
                    "command": arguments.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>()
                }));
            }
        }
    }
    let mut report = benchmark_support::context(&f);
    report["campaign"] = "diagnosis_scaling".into();
    report["fixture"] = serde_json::json!({"source_seconds":1,"soundtrack_seconds":7.44,"fps":25,"repetitions":3,
        "h264":{"width":320,"height":180,"b_frames":3},"hevc":{"width":160,"height":120},"vp9":{"width":192,"height":108},
        "mixed_order":["h264","hevc","vp9"],"counts":[1,10,100],"rendering":false});
    report["cases"] = cases.into();
    benchmark_support::save(
        &report,
        "NOH_DIAG_BENCH_REPORT",
        "target/benchmarks/diagnosis-scaling.json",
    );
}
