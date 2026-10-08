//! Opt-in external caption campaign. Fixture creation and verification are untimed.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
#[path = "support/benchmark.rs"]
mod benchmark_support;
use noh::{captions::CaptionStyle, engine::Event};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::Output,
    time::Instant,
};
use support::Fixture;

const SOURCE_SECONDS: f64 = 4.0;
const REVIEWED_SRT: &str = "1\n00:00:00,250 --> 00:00:01,750\nFrançais : Été au jardin. Grüße aus Deutschland.\n\n2\n00:00:02,000 --> 00:00:03,750\n日本語の字幕 한국어 자막 中文字幕\nLatin and CJK share this reviewed caption.\n\n";

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate(fixture: &Fixture, output: &Output, path: &Path, preview: bool) {
    // The caller stops its stopwatch before any assertion, parsing or decoding.
    assert!(output.status.success(), "{}", support::log(output));
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["version"], 1);
    let Event::Done(Ok(result)) = serde_json::from_value(reply["result"].clone()).unwrap() else {
        panic!("Expected a successful caption terminal event: {reply}");
    };
    assert_eq!(result.output, path);
    support::near(result.duration, SOURCE_SECONDS, 1.0 / 25.0);
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.file_type().is_file() && metadata.len() > 0);
    let packets = fixture.packets(path, false, true, false);
    assert!(packets.contains(if preview {
        "#dimensions 0: 960x540"
    } else {
        "#dimensions 0: 1280x720"
    }));
    assert_eq!(support::rows(&packets).len(), 100);
    // Decode every audio packet too; a valid video-only output is insufficient.
    let audio = fixture.ff(args![
        "-xerror", "-i", path, "-map", "0:a:0", "-vn", "-f", "null", "-"
    ]);
    assert!(audio.stderr.is_empty(), "{}", support::text(&audio.stderr));
    fixture.clean();
}

#[test]
#[ignore = "explicit caption campaign; NOH_CAPTION_BENCH_REPORT selects JSON output"]
fn caption_campaign() {
    let fixture = Fixture::new();
    let source = fixture.path("source.mp4");
    let subtitles = fixture.path("reviewed.srt");
    assert!(!source.exists() && !subtitles.exists());
    fixture.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=1280x720:r=25:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=4",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        "18",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        &source
    ]);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&subtitles)
        .unwrap()
        .write_all(REVIEWED_SRT.as_bytes())
        .unwrap();
    let mut cases = Vec::new();
    for (name, preview) in [("full_burn", false), ("preview", true)] {
        let mut samples = Vec::new();
        let mut warmup_seconds = 0.0;
        for sample in 0..4 {
            let output = fixture.path(&format!("{name}-{sample}.mp4"));
            assert!(
                matches!(fs::symlink_metadata(&output), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            );
            let mut arguments = args![
                "--burn-subtitles",
                &source,
                "--srt",
                &subtitles,
                "--output",
                &output,
                "--caption-size",
                "medium",
                "--caption-placement",
                "bottom",
                "--json"
            ];
            if preview {
                arguments.push("--preview".into());
            }
            let command = arguments
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let start = Instant::now();
            let reply = fixture.run(&fixture.exe, arguments);
            let seconds = start.elapsed().as_secs_f64();
            validate(&fixture, &reply, &output, preview);
            if sample == 0 {
                warmup_seconds = seconds;
            } else {
                samples.push(serde_json::json!({"seconds":seconds,"output_bytes":fs::metadata(&output).unwrap().len(),"command":command}));
            }
        }
        let mut times = samples
            .iter()
            .map(|sample| sample["seconds"].as_f64().unwrap())
            .collect::<Vec<_>>();
        times.sort_by(f64::total_cmp);
        cases.push(serde_json::json!({
            "name":name,"preview":preview,"workload_seconds":SOURCE_SECONDS,
            "warmups":1,"warmup_seconds":warmup_seconds,"repetitions":3,
            "median_seconds":times[1],"min_seconds":times[0],"max_seconds":times[2],
            "range_seconds":times[2]-times[0],"samples":samples
        }));
    }
    let mut report = benchmark_support::context(&fixture);
    report["campaign"] = "captions".into();
    report["timed_scope"] = "Sequential CLI process startup, inspection, encoding and publication; fixture generation, terminal validation and complete audio/video decoding are excluded.".into();
    report["fixture"] = serde_json::json!({
        "source_seconds":SOURCE_SECONDS,"width":1280,"height":720,"fps":25,
        "video":"testsrc2, H264 yuv420p CRF18","audio":"440Hz sine, 48kHz, AAC192kbps",
        "source_sha256":hash(&fs::read(&source).unwrap()),
        "srt_sha256":hash(&fs::read(&subtitles).unwrap()),"cues":2,
        "style":CaptionStyle::default(),
        "bundled_font":{"family":"NOH CJK","asset":"assets/NOHCJK.otf",
            "sha256":hash(include_bytes!("../assets/NOHCJK.otf")),
            "hash_source":"Repository asset embedded in this benchmark runner; tested executable identity is recorded separately."}
    });
    report["cases"] = cases.into();
    benchmark_support::save(
        &report,
        "NOH_CAPTION_BENCH_REPORT",
        "target/benchmarks/captions.json",
    );
}
