//! Task 39 B4: without an OpenGL context, Classique's captions are drawn by libass
//! and the export says so; any other effect fails with a clear error. The failure
//! is forced where it really happens (`GlRenderer::headless`), in the worker this
//! test binary starts, through NOH_TEST_GL_FAIL; one test per binary keeps the
//! variable from reaching other tests.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;

use noh::{
    captions::{CaptionRequest, CaptionStyle},
    engine::{Event, ExportRequest},
    jobs::Job,
    project::{EffectChoice, ProjectRequest, ProjectShort},
    shorts::{Framing, ShortCaptions, ShortRequest},
};
use std::{path::Path, time::Duration};
use support::Fixture;

/// `effect`: "classic" (bundled Classique) or a test-only effect of tests/fixtures/fx.
fn request(f: &Fixture, effect: &str, preset: &str) -> ProjectRequest {
    ProjectRequest {
        montage: ExportRequest {
            items: vec![f.path("clip.mp4").into()],
            wav: f.path("song.wav"),
            output: f.path(&format!("{effect}.mp4")),
            ffmpeg: f.ffmpeg.clone(),
            fade_in: 0.0,
            fade_out: 0.0,
            partial_fades: false,
            preview: false,
            clip_audio: false,
            force_encode: false,
        },
        short: Some(ProjectShort {
            start_ms: 0,
            end_ms: 3_000,
            restart_loops: false,
            framing: Framing::Pad,
        }),
        effect: Some(EffectChoice {
            id: if effect == "classic" {
                "noh.classic".into()
            } else {
                format!("test.{effect}")
            },
            preset: Some(preset.into()),
            dir: if effect == "classic" {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects/classique")
            } else {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/fx")
                    .join(effect)
            },
            values: Default::default(),
        }),
        captions: Some(ShortCaptions {
            subtitles: f.path("lyrics.srt"),
            style: CaptionStyle::default(),
        }),
    }
}

fn run(f: &Fixture, request: &ProjectRequest) -> Vec<Event> {
    collect(Job::project_with_worker(
        request.clone(),
        f.exe.clone(),
        || {},
    ))
}

fn collect(job: Job) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(Duration::from_secs(120))
            .expect("deadline");
        let terminal = event.is_terminal();
        events.push(event);
        if terminal {
            break;
        }
    }
    drop(job);
    events
}

#[test]
fn without_opengl_classique_falls_back_to_libass_and_other_effects_fail() {
    // SAFETY: set before any thread or worker of this binary starts; this binary
    // holds this single test.
    unsafe { std::env::set_var("NOH_TEST_GL_FAIL", "1") };
    let f = Fixture::new();
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=0x202020:s=1280x720:r=25:d=4",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        f.path("clip.mp4")
    ]);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=stereo",
        "-t",
        "4",
        "-c:a",
        "pcm_s16le",
        f.path("song.wav")
    ]);
    std::fs::write(
        f.path("lyrics.srt"),
        "1\n00:00:00,500 --> 00:00:02,500\nFALLBACK CAPTION\n",
    )
    .unwrap();

    // Classique: done, with the fallback warning, and white captions burned in.
    let events = run(&f, &request(&f, "classic", "classique"));
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Warning(m) if m.code == "warning.effect_fallback")),
        "{events:?}"
    );
    // The maintainer's choice b): libass drew these captions of an HD (BT.709) export, so
    // an FFmpeg that blends them in BT.601 (6.1) is named; 7.1 is not.
    let old_colours = noh::color::libass_blends_bt709(&f.ffmpeg) == Some(false);
    assert_eq!(
        events.iter().any(
            |e| matches!(e, Event::Warning(m) if m.code == "warning.caption_colors_old_ffmpeg")
        ),
        old_colours,
        "{events:?}"
    );
    let frame = f
        .ff(args![
            "-ss",
            "1.5",
            "-i",
            f.path("classic.mp4"),
            "-frames:v",
            "1",
            "-vf",
            "format=gray",
            "-f",
            "rawvideo",
            "-"
        ])
        .stdout;
    assert_eq!(frame.len(), 1080 * 1920, "Short canvas");
    assert!(
        frame.iter().filter(|p| **p > 200).count() > 500,
        "no white caption pixels"
    );

    // Another effect: a clear error, nothing published.
    let events = run(&f, &request(&f, "wipe", "jaune"));
    match events.last() {
        Some(Event::Done(Err(e))) => assert_eq!(e.code, "error.effect_gl_unavailable", "{e:?}"),
        other => panic!("{other:?}"),
    }
    assert!(!f.path("wipe.mp4").exists());

    // The caption burn and the standalone Short draw Classique with libass too
    // (captions.rs, shorts.rs): each says the colour limit iff this FFmpeg blends
    // in BT.601 (Linux CI: present; Windows 7.1: absent).
    let said = |events: &[Event], code: &str| {
        events
            .iter()
            .any(|e| matches!(e, Event::Warning(m) if m.code == code))
    };
    let burn = collect(Job::burn_with_worker(
        CaptionRequest {
            scene: None,
            source: f.path("clip.mp4"),
            subtitles: f.path("lyrics.srt"),
            output: f.path("burn.mp4"),
            ffmpeg: f.ffmpeg.clone(),
            style: CaptionStyle::default(),
            preview: false,
            effect: None,
        },
        f.exe.clone(),
        || {},
    ));
    assert!(matches!(burn.last(), Some(Event::Done(Ok(_)))), "{burn:?}");
    assert!(said(&burn, "warning.effect_fallback"), "{burn:?}");
    assert_eq!(
        said(&burn, "warning.caption_colors_old_ffmpeg"),
        old_colours,
        "{burn:?}"
    );
    let short = collect(Job::short_with_worker(
        ShortRequest {
            source: f.path("clip.mp4"),
            output: f.path("short.mp4"),
            ffmpeg: f.ffmpeg.clone(),
            start_ms: 0,
            end_ms: 3_000,
            framing: Framing::Pad,
            captions: Some(ShortCaptions {
                subtitles: f.path("lyrics.srt"),
                style: CaptionStyle::default(),
            }),
            preview: false,
            effect: None,
        },
        f.exe.clone(),
        || {},
    ));
    assert!(
        matches!(short.last(), Some(Event::Done(Ok(_)))),
        "{short:?}"
    );
    assert!(said(&short, "warning.effect_fallback"), "{short:?}");
    assert_eq!(
        said(&short, "warning.caption_colors_old_ffmpeg"),
        old_colours,
        "{short:?}"
    );
}
