//! Motion scenes through the worker, on synthetic media: a check writes its
//! report, looks write their frames, a caption burn draws the scene (whole, or a
//! clip of the source), and a refused scene publishes nothing.
#![cfg(feature = "motion")]
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
use noh::{
    captions::{CaptionRequest, CaptionStyle},
    engine::Event,
    jobs::Job,
    safe_area::SafeArea,
    scene_request::{SceneChoice, SceneMode, SceneRequest},
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use support::Fixture;

/// A scene that draws the current line, a band behind it and a pulse on the
/// sound's energy: text, shapes and a rectangle.
const SCENE: &str = "// noh-motion 0
fn frame(t, ctx) {
    draw(rounded_rect(0.5, 0.8, 0.7, 0.14, 0.03).fill(\"#101828\").opacity(0.8));
    let i = cue_at(t);
    if i >= 0 && t < cue(i).end {
        draw(text(cue(i).text).size(0.07).at(0.5, 0.8).color(\"#F2E8D8\"));
    }
    draw(rect(0.02 + 0.2 * energy(t), 0.02).at(0.5, 0.1).color(\"#C8102E\"));
}
";

/// Over the draw cap from 1.5 s on.
const TOO_MANY: &str = "// noh-motion 0
fn frame(t, ctx) {
    let n = if t < 1.5 { 3 } else { 300 };
    for i in 0..n { draw(rect(0.01, 0.01).at(0.5, 0.5)); }
}
";

/// White over the whole frame from 1 s on, nothing before: a clip that starts
/// at 1 s shows white from its first frame only if the scene keeps the source's
/// clock.
const FROM_ONE: &str = "// noh-motion 0
fn frame(t, ctx) {
    if t >= 1.0 { draw(rect(1.0, 1.0).at(0.5, 0.5).color(\"#FFFFFF\")); }
}
";

fn media(f: &Fixture) {
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=640x360:r=25:d=3",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        f.path("source.mp4")
    ]);
    std::fs::write(
        f.path("reviewed.srt"),
        "1\n00:00:00,400 --> 00:00:02,600\nOn a gardé la lumière\n",
    )
    .unwrap();
    std::fs::write(f.path("song.noh-motion.rhai"), SCENE).unwrap();
    std::fs::write(f.path("too_many.noh-motion.rhai"), TOO_MANY).unwrap();
    std::fs::write(f.path("from_one.noh-motion.rhai"), FROM_ONE).unwrap();
}

fn wait(job: &Job) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(Duration::from_secs(300))
            .unwrap_or_else(|e| panic!("scene job silent: {e:?}; so far {events:?}"));
        let done = event.is_terminal();
        events.push(event);
        if done {
            return events;
        }
    }
}

fn scene_job(f: &Fixture, mode: SceneMode, out: &str, scene: &str) -> Vec<Event> {
    let request = SceneRequest {
        scene: f.path(scene),
        source: f.path("source.mp4"),
        subtitles: f.path("reviewed.srt"),
        ffmpeg: f.ffmpeg.clone(),
        safe_area: SafeArea::None,
        from_ms: None,
        to_ms: None,
        mode,
        bare: false,
        grid: true,
        output: f.path(out),
    };
    let job = Job::scene_with_cancel(
        request,
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    );
    wait(&job)
}

fn burn(f: &Fixture, scene: &str, range_ms: Option<[u64; 2]>, out: &str) -> Vec<Event> {
    let request = CaptionRequest {
        source: f.path("source.mp4"),
        subtitles: f.path("reviewed.srt"),
        output: f.path(out),
        ffmpeg: f.ffmpeg.clone(),
        style: CaptionStyle::default(),
        preview: range_ms.is_some(),
        effect: None,
        scene: Some(SceneChoice {
            scene: f.path(scene),
            range_ms,
        }),
    };
    let job = Job::burn_with_worker(request, f.exe.clone(), || {});
    wait(&job)
}

fn ok(events: &[Event]) {
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
}

fn refused(events: &[Event]) -> String {
    match events.last() {
        Some(Event::Done(Err(e))) => {
            assert_eq!(e.code, "error.scene_refused", "{events:?}");
            e.detail.to_string()
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn duration(f: &Fixture, path: &Path) -> f64 {
    f.duration(path)
}

#[test]
#[ignore = "needs an OpenGL 3.3 GPU and FFmpeg on PATH"]
fn scene_jobs_check_look_burn_clip_and_refuse() {
    let f = Fixture::new();
    media(&f);

    // Check: a report, with the counted maxima and the whole frame timed.
    ok(&scene_job(
        &f,
        SceneMode::Check,
        "check",
        "song.noh-motion.rhai",
    ));
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path("check").join("report.json")).unwrap())
            .unwrap();
    assert_eq!(report["canvas"], serde_json::json!([640, 360]));
    assert!(report["frames"].as_u64().unwrap() >= 70, "{report}");
    assert!(report["max"]["paths"].as_u64().unwrap() >= 1, "{report}");
    assert!(report["frame_ms"]["p95"].as_f64().is_some(), "{report}");

    // Looks: stills, a strip with its sheet, a measure.
    ok(&scene_job(
        &f,
        SceneMode::Stills {
            at_ms: vec![500, 2000],
        },
        "stills",
        "song.noh-motion.rhai",
    ));
    for name in ["frame-01-500ms.png", "frame-02-2000ms.png", "look.json"] {
        assert!(f.path("stills").join(name).is_file(), "{name}");
    }
    ok(&scene_job(
        &f,
        SceneMode::Strip { at_ms: 1000 },
        "strip",
        "song.noh-motion.rhai",
    ));
    assert!(f.path("strip").join("sheet.png").is_file());
    assert_eq!(
        std::fs::read_dir(f.path("strip"))
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("frame-"))
            .count(),
        8
    );
    ok(&scene_job(
        &f,
        SceneMode::Measure { at_ms: vec![1000] },
        "measure",
        "song.noh-motion.rhai",
    ));
    let measure: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path("measure").join("measure.json")).unwrap())
            .unwrap();
    assert_eq!(
        measure["frames"][0]["texts"][0]["text"], "On a gardé la lumière",
        "{measure}"
    );

    // A burn with the scene, then a clip of it.
    ok(&burn(&f, "song.noh-motion.rhai", None, "burned.mp4"));
    assert!((duration(&f, &f.path("burned.mp4")) - 3.0).abs() < 0.2);
    ok(&burn(
        &f,
        "song.noh-motion.rhai",
        Some([1000, 2000]),
        "clip.mp4",
    ));
    assert!((duration(&f, &f.path("clip.mp4")) - 1.0).abs() < 0.2);
    // A clip from 1 s of a scene that turns white at 1 s: white from its first
    // frame (the scene is evaluated at the source's time, not the clip's).
    ok(&burn(
        &f,
        "from_one.noh-motion.rhai",
        Some([1000, 1400]),
        "white.mp4",
    ));
    let levels = f.gray(&f.path("white.mp4"));
    assert!(
        !levels.is_empty() && levels.iter().all(|&g| g > 230),
        "{levels:?}"
    );

    // Refused at the instant it goes over the cap: nothing published.
    let why = refused(&scene_job(
        &f,
        SceneMode::Check,
        "refused",
        "too_many.noh-motion.rhai",
    ));
    assert!(why.contains("t=1.5") || why.contains("t=1.52"), "{why}");
    assert!(!f.path("refused").exists());
    refused(&burn(&f, "too_many.noh-motion.rhai", None, "refused.mp4"));
    assert!(!f.path("refused.mp4").exists());
}
