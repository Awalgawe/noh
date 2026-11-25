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

/// An empty layer whose shader paints the whole frame red, half transparent.
const SHADED: &str = "// noh-motion 0
fn frame(t, ctx) { draw(layer().shader(\"fill\").resolution(\"full\").param(0, 0.5)); }
";
const FILL: &str =
    "fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(i.k0.x, 0.0, 0.0, i.k0.x); }";

/// A framing that zooms and slides over time: the picture moves under a still
/// overlay-free scene.
const FRAMED: &str = "// noh-motion 0
fn frame(t, ctx) { draw(video().scale(1.2 + 0.1 * t).focus(0.3 + 0.1 * t, 0.5).rot(0.05)); }
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
    std::fs::write(f.path("shaded.noh-motion.rhai"), SHADED).unwrap();
    std::fs::write(f.path("framed.noh-motion.rhai"), FRAMED).unwrap();
    std::fs::create_dir(f.path("shaded.noh-motion.shaders")).unwrap();
    std::fs::write(f.path("shaded.noh-motion.shaders").join("fill.wgsl"), FILL).unwrap();
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

fn request(f: &Fixture, mode: SceneMode, out: &str, scene: &str) -> SceneRequest {
    SceneRequest {
        project: None,
        scene: f.path(scene),
        source: f.path("source.mp4"),
        subtitles: f.path("reviewed.srt"),
        ffmpeg: f.ffmpeg.clone(),
        safe_area: SafeArea::None,
        from_ms: None,
        to_ms: None,
        mode,
        vertical: false,
        bare: false,
        grid: true,
        output: f.path(out),
    }
}

fn scene_job(f: &Fixture, mode: SceneMode, out: &str, scene: &str) -> Vec<Event> {
    let job = Job::scene_with_cancel(
        request(f, mode, out, scene),
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
            vertical: false,
        }),
        promote: None,
    };
    let job = Job::burn_with_worker(request, f.exe.clone(), || {});
    wait(&job)
}

/// A short that follows its scene: the burn of `range_ms` on a vertical frame.
fn short(f: &Fixture, scene: &str, range_ms: [u64; 2], out: &str) -> Vec<Event> {
    let request = CaptionRequest {
        source: f.path("source.mp4"),
        subtitles: f.path("reviewed.srt"),
        output: f.path(out),
        ffmpeg: f.ffmpeg.clone(),
        style: CaptionStyle {
            safe_area: SafeArea::YoutubeShorts,
            ..Default::default()
        },
        preview: false,
        effect: None,
        scene: Some(SceneChoice {
            scene: f.path(scene),
            range_ms: Some(range_ms),
            vertical: true,
        }),
        promote: None,
    };
    wait(&Job::burn_with_worker(request, f.exe.clone(), || {}))
}

/// One frame of `video` at `at` seconds, RGB.
fn frame_at(f: &Fixture, video: &Path, at: f64) -> Vec<u8> {
    f.ff(args![
        "-ss",
        format!("{at}"),
        "-i",
        video,
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-"
    ])
    .stdout
}

fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(x.abs_diff(*y)))
        .sum::<f64>()
        / a.len() as f64
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
    // The window asks with no FFmpeg unless the person picked one: the job
    // finds it, as the other jobs do.
    let job = Job::scene_with_cancel(
        SceneRequest {
            ffmpeg: std::path::PathBuf::new(),
            ..request(
                &f,
                SceneMode::Check,
                "check-no-ffmpeg",
                "song.noh-motion.rhai",
            )
        },
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    );
    ok(&wait(&job));
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path("check").join("report.json")).unwrap())
            .unwrap();
    assert_eq!(report["canvas"], serde_json::json!([640, 360]));
    assert!(report["frames"].as_u64().unwrap() >= 70, "{report}");
    assert!(report["max"]["paths"].as_u64().unwrap() >= 1, "{report}");
    assert!(report["frame_ms"]["p95"].as_f64().is_some(), "{report}");
    assert_eq!(report["safe_area"]["name"], "none", "{report}");
    assert_eq!(
        report["safe_area"]["x0_y0_x1_y1"],
        serde_json::json!([0.0, 0.0, 1.0, 1.0])
    );

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

    // A layer shader: checked, compiled and timed in the worker, then drawn in
    // a still (bare: over the video alone, but drawn) and in a burn.
    ok(&scene_job(
        &f,
        SceneMode::Check,
        "shaded-check",
        "shaded.noh-motion.rhai",
    ));
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path("shaded-check").join("report.json")).unwrap())
            .unwrap();
    assert_eq!(report["shaders"][0]["name"], "fill", "{report}");
    assert!(
        report["shaders"][0]["full_frame_ms"].as_f64().is_some(),
        "{report}"
    );
    assert_eq!(report["max"]["shader_layers"], 1, "{report}");
    ok(&scene_job(
        &f,
        SceneMode::Stills { at_ms: vec![500] },
        "shaded-still",
        "shaded.noh-motion.rhai",
    ));
    let bare = SceneRequest {
        bare: true,
        grid: false,
        ..request(
            &f,
            SceneMode::Stills { at_ms: vec![500] },
            "shaded-bare",
            "shaded.noh-motion.rhai",
        )
    };
    ok(&wait(&Job::scene_with_cancel(
        bare,
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    )));
    let open = |dir: &str| {
        image::open(f.path(dir).join("frame-01-500ms.png"))
            .unwrap()
            .to_rgb8()
    };
    let (shaded, video) = (open("shaded-still"), open("shaded-bare"));
    // Half-transparent red over the video: 127 + v / 2 in red, v / 2 elsewhere
    // (away from the grid's lines, every tenth of the frame).
    for (x, y) in [(10, 10), (330, 190), (630, 350)] {
        let (s, v) = (shaded.get_pixel(x, y).0, video.get_pixel(x, y).0);
        let want = [127 + v[0] / 2, v[1] / 2, v[2] / 2];
        for c in 0..3 {
            assert!(s[c].abs_diff(want[c]) <= 2, "{s:?} over {v:?} at {x},{y}");
        }
    }
    ok(&burn(
        &f,
        "shaded.noh-motion.rhai",
        Some([0, 1000]),
        "shaded.mp4",
    ));

    // Reframing: a look jumps to 1.6 s, the export plays up to it; the frame
    // the export encoded there is the look's still, within the encoder's loss.
    let still = SceneRequest {
        grid: false,
        ..request(
            &f,
            SceneMode::Stills { at_ms: vec![1600] },
            "framed-still",
            "framed.noh-motion.rhai",
        )
    };
    ok(&wait(&Job::scene_with_cancel(
        still,
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    )));
    ok(&burn(&f, "framed.noh-motion.rhai", None, "framed.mp4"));
    let looked = image::open(f.path("framed-still").join("frame-01-1600ms.png"))
        .unwrap()
        .to_rgb8()
        .into_raw();
    let played = f
        .ff(args![
            "-ss",
            "1.6",
            "-i",
            f.path("framed.mp4"),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-"
        ])
        .stdout;
    let source = f
        .ff(args![
            "-ss",
            "1.6",
            "-i",
            f.path("source.mp4"),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-"
        ])
        .stdout;
    let mean = |a: &[u8], b: &[u8]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from(x.abs_diff(*y)))
            .sum::<f64>()
            / a.len() as f64
    };
    let (same, moved) = (mean(&looked, &played), mean(&played, &source));
    eprintln!("framed frame at 1.6 s: look vs export {same:.2}, export vs source {moved:.2}");
    assert!(
        same < 6.0 && moved > 3.0 * same,
        "look vs export {same}, export vs source {moved}"
    );

    // A vertical frame made from the wide video. Without a safe area, refused
    // before any work; a look draws 1080 x 1920 frames, covering and centred
    // without a `video()` (as FFmpeg's cover crop does), as the scene frames
    // it with one; a short of the interval encodes the look's frame there.
    let vertical = |out: &str, scene: &str, at: u64, bare: bool, safe: SafeArea| SceneRequest {
        vertical: true,
        bare,
        grid: false,
        safe_area: safe,
        ..request(&f, SceneMode::Stills { at_ms: vec![at] }, out, scene)
    };
    let look = |r: SceneRequest| {
        wait(&Job::scene_with_cancel(
            r,
            f.exe.clone(),
            Arc::new(AtomicBool::new(false)),
            || {},
        ))
    };
    match look(vertical(
        "vertical-unsafe",
        "framed.noh-motion.rhai",
        1600,
        false,
        SafeArea::None,
    ))
    .last()
    {
        Some(Event::Done(Err(e))) => assert_eq!(e.code, "error.safe_area_required"),
        other => panic!("a vertical look without a safe area went on: {other:?}"),
    }
    assert!(!f.path("vertical-unsafe").exists());
    let shorts = SafeArea::YoutubeShorts;
    ok(&look(vertical(
        "vertical-cover",
        "song.noh-motion.rhai",
        1600,
        true,
        shorts,
    )));
    let covered = image::open(f.path("vertical-cover").join("frame-01-1600ms.png"))
        .unwrap()
        .to_rgb8();
    assert_eq!(covered.dimensions(), (1080, 1920));
    let cover_crop = f
        .ff(args![
            "-ss",
            "1.6",
            "-i",
            f.path("source.mp4"),
            "-frames:v",
            "1",
            "-vf",
            "scale=-2:1920:flags=bilinear,crop=1080:1920",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-"
        ])
        .stdout;
    let cover = mean_difference(covered.as_raw(), &cover_crop);
    eprintln!("vertical cover at 1.6 s vs FFmpeg's cover crop: {cover:.2}");
    assert!(cover < 8.0, "{cover}");
    ok(&look(vertical(
        "vertical-framed",
        "framed.noh-motion.rhai",
        1600,
        false,
        shorts,
    )));
    let looked = image::open(f.path("vertical-framed").join("frame-01-1600ms.png"))
        .unwrap()
        .to_rgb8();
    assert_eq!(looked.dimensions(), (1080, 1920));
    ok(&short(
        &f,
        "framed.noh-motion.rhai",
        [1000, 2400],
        "vertical.mp4",
    ));
    let played = frame_at(&f, &f.path("vertical.mp4"), 0.6);
    assert_eq!(
        played.len(),
        1080 * 1920 * 3,
        "the short is not 1080 x 1920"
    );
    let (same, cover_vs) = (
        mean_difference(looked.as_raw(), &played),
        mean_difference(&cover_crop, &played),
    );
    eprintln!("vertical short at 1.6 s: look vs export {same:.2}, cover vs export {cover_vs:.2}");
    assert!(same < 6.0 && cover_vs > 2.0 * same, "{same} {cover_vs}");

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

/// A project export (the montage, then its short) of `scene`, with the song's
/// WAV and the reviewed lyrics.
fn project(f: &Fixture, scene: &str, short: Option<[u64; 2]>, out: &str) -> Vec<Event> {
    use noh::{
        engine::ExportRequest,
        project::{ProjectRequest, ProjectShort},
        scene_request::ProjectScene,
        shorts::{Framing, ShortCaptions},
    };
    let request = ProjectRequest {
        montage: ExportRequest {
            items: vec![f.path("source.mp4").into()],
            wav: f.path("song.wav"),
            output: f.path(out),
            ffmpeg: f.ffmpeg.clone(),
            fade_in: 0.0,
            fade_out: 0.0,
            partial_fades: false,
            preview: false,
            clip_audio: false,
            force_encode: false,
        },
        short: short.map(|[start_ms, end_ms]| ProjectShort {
            start_ms,
            end_ms,
            restart_loops: false,
            framing: Framing::Pad,
        }),
        effect: None,
        motion: Some(ProjectScene {
            scene: f.path(scene),
        }),
        captions: Some(ShortCaptions {
            subtitles: f.path("reviewed.srt"),
            style: CaptionStyle {
                safe_area: if short.is_some() {
                    SafeArea::YoutubeShorts
                } else {
                    SafeArea::None
                },
                ..Default::default()
            },
        }),
        promote: None,
    };
    wait(&Job::project_with_worker(request, f.exe.clone(), || {}))
}

/// The montage form (M4): a scene checked on the song alone, at the project's
/// frame; the montage exported with it draws what a caption burn of the same
/// video draws; its short keeps the song's clock; a scene over a cap is
/// refused before anything is published.
#[test]
#[ignore = "needs an OpenGL 3.3 GPU and FFmpeg on PATH"]
fn a_montage_checks_and_draws_its_scene() {
    let f = Fixture::new();
    media(&f);
    f.ff(args![
        "-i",
        f.path("source.mp4"),
        "-vn",
        "-c:a",
        "pcm_s16le",
        f.path("song.wav")
    ]);
    // The check, on the song alone.
    let check = SceneRequest {
        project: Some(noh::scene_request::SceneProject {
            width: 640,
            height: 360,
            fps: 25.0,
        }),
        source: f.path("song.wav"),
        ..request(
            &f,
            SceneMode::Check,
            "montage-check",
            "song.noh-motion.rhai",
        )
    };
    let check = SceneRequest {
        grid: false,
        ..check
    };
    ok(&wait(&Job::scene_with_cancel(
        check,
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    )));
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.path("montage-check").join("report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["canvas"], serde_json::json!([640, 360]));
    assert_eq!(report["fps"], serde_json::json!(25.0));
    assert!(report["max"]["draws"].as_u64().unwrap() >= 2, "{report}");
    // The montage, against a burn of the same video.
    ok(&project(&f, "song.noh-motion.rhai", None, "montage.mp4"));
    ok(&burn(&f, "song.noh-motion.rhai", None, "burn.mp4"));
    for at in [0.5, 1.5] {
        let d = mean_difference(
            &frame_at(&f, &f.path("montage.mp4"), at),
            &frame_at(&f, &f.path("burn.mp4"), at),
        );
        assert!(d < 3.0, "montage and burn differ by {d} at {at} s");
    }
    // The short keeps the song's clock: white from its first frame.
    ok(&project(
        &f,
        "from_one.noh-motion.rhai",
        Some([1_000, 2_500]),
        "short.mp4",
    ));
    let first = frame_at(&f, &f.path("short.mp4"), 0.0);
    let mean = first.iter().map(|&v| f64::from(v)).sum::<f64>() / first.len() as f64;
    assert!(mean > 200.0, "the short's first frame is not white: {mean}");
    // Over the draw cap from 1.5 s: refused, nothing published.
    let reason = refused(&project(
        &f,
        "too_many.noh-motion.rhai",
        None,
        "refused.mp4",
    ));
    assert!(reason.contains("1.5"), "{reason}");
    assert!(!f.path("refused.mp4").exists());
}
