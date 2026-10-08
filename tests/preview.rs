//! Original-source cursor frames and bounded optional source thumbnails.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;

use noh::{
    captions::CaptionStyle,
    engine::{ExportRequest, MediaItem},
    preview::{self, Frame, FrameRequest, FrameWorker, PreviewError},
    project::{ProjectRequest, ProjectShort},
    shorts::{Framing, ShortCaptions},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use support::Fixture;

fn fixture() -> (Fixture, ProjectRequest) {
    let f = Fixture::new();
    for (name, color, duration) in [("a.mp4", "0x333333", 4), ("b.mp4", "0xcccccc", 6)] {
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            format!("color=c={color}:s=256x144:r=25:d={duration}"),
            "-c:v",
            "libx264",
            "-bf",
            "2",
            "-pix_fmt",
            "yuv420p",
            f.path(name)
        ]);
    }
    let request = ProjectRequest {
        montage: ExportRequest {
            items: vec![f.path("a.mp4").into(), f.path("b.mp4").into()],
            wav: f.wav("clock.wav", 20.0),
            output: f.path("placeholder.mp4"),
            ffmpeg: f.ffmpeg.clone(),
            fade_in: 0.0,
            fade_out: 0.0,
            partial_fades: true,
            preview: true,
            clip_audio: false,
            force_encode: false,
        },
        short: None,
        captions: None,
    };
    (f, request)
}
fn frame(project: &ProjectRequest, time_ms: u64) -> Frame {
    preview::extract(
        &FrameRequest::Project {
            project: project.clone(),
            time_ms,
        },
        &AtomicBool::new(false),
    )
    .unwrap_or_else(|error| panic!("Cursor at {time_ms} ms, short {:?}: {error}", project.short))
}
fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
    frame.rgba[((y * frame.width + x) * 4) as usize..][..4]
        .try_into()
        .unwrap()
}
fn center(frame: &Frame) -> u8 {
    pixel(frame, frame.width / 2, frame.height / 2)[0]
}
fn caption_pixels(frame: &Frame) -> bool {
    // The lower short padding is black; any bright pixel there is burned text.
    let start = (frame.height - 100) * frame.width * 4;
    frame.rgba[start as usize..]
        .chunks_exact(4)
        .any(|p| p[0] > 24)
}

#[test]
fn cursor_uses_project_clock_and_short_restart_only_in_short_mode() {
    let (_f, mut project) = fixture();
    assert!(center(&frame(&project, 13_000)) < 80);
    assert!(center(&frame(&project, 14_000)) > 170);
    for restart in [false, true] {
        project.short = Some(ProjectShort {
            start_ms: 13_000,
            end_ms: 18_000,
            restart_loops: restart,
            framing: Framing::Pad,
        });
        let initial = frame(&project, 13_000);
        assert_eq!((initial.width, initial.height), (360, 640));
        assert!(center(&initial) < 80);
        assert_eq!(pixel(&initial, 180, 10), [0, 0, 0, 255]);
        assert_eq!(center(&frame(&project, 14_000)) > 170, !restart);
        assert!(center(&frame(&project, 17_000)) > 170);
        project.short.as_mut().unwrap().framing = Framing::Crop;
        assert!(pixel(&frame(&project, 13_000), 180, 10)[0] > 30);
    }
    project.short = None;
    assert!(center(&frame(&project, 14_000)) > 170);
    project.short = Some(ProjectShort {
        start_ms: 13_015,
        end_ms: 18_000,
        restart_loops: false,
        framing: Framing::Pad,
    });
    assert!(center(&frame(&project, 13_999)) < 80);
    assert!(center(&frame(&project, 14_000)) > 170);
}

#[test]
fn caption_clock_is_trimmed_and_fades_retain_full_wav_clock() {
    let (f, mut project) = fixture();
    std::fs::write(
        f.path("reviewed.srt"),
        "1\n00:00:14,000 --> 00:00:16,000\nAT FOURTEEN\n",
    )
    .unwrap();
    project.short = Some(ProjectShort {
        start_ms: 13_000,
        end_ms: 18_000,
        restart_loops: true,
        framing: Framing::Pad,
    });
    project.captions = Some(ShortCaptions {
        subtitles: f.path("reviewed.srt"),
        style: CaptionStyle::default(),
    });
    for (time, visible) in [
        (13_000, false),
        (14_000, true),
        (15_960, true),
        (16_000, false),
    ] {
        assert_eq!(
            caption_pixels(&frame(&project, time)),
            visible,
            "Caption at {time} ms"
        );
    }
    project.captions = None;
    project.montage.fade_in = 15.0;
    project.montage.fade_out = 5.0;
    let at_start = center(&frame(&project, 13_000));
    assert!(
        at_start > 30 && at_start < 55,
        "interior short must retain fade phase: {at_start}"
    );
    project.short.as_mut().unwrap().restart_loops = false;
    let fade_out = center(&frame(&project, 17_000));
    assert!(
        fade_out > 100 && fade_out < 150,
        "fade should follow project t=17: {fade_out}"
    );
}

#[test]
fn thumbnails_preserve_non_square_pixels_rotation_stills_and_alpha() {
    let f = Fixture::new();
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=red:s=64x64:r=25:d=0.2,setsar=2/1",
        "-vf",
        "drawbox=x=0:y=0:w=32:h=64:color=white:t=fill",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        f.path("wide.mp4")
    ]);
    f.ff(args![
        "-display_rotation:v:0",
        "90",
        "-i",
        f.path("wide.mp4"),
        "-c",
        "copy",
        f.path("rotated.mp4")
    ]);
    let source = |path| FrameRequest::Thumbnail {
        item: MediaItem::Video { path },
        ffmpeg: f.ffmpeg.clone(),
    };
    let wide = preview::extract(&source(f.path("wide.mp4")), &AtomicBool::new(false)).unwrap();
    assert_eq!(
        (wide.width, wide.height),
        (
            noh::preview::THUMBNAIL_WIDTH,
            noh::preview::THUMBNAIL_HEIGHT
        )
    );
    assert!(pixel(&wide, 14, 63)[0] > 150);
    assert!(pixel(&wide, 112, 1)[0] < 5);
    // The monitor's still: the same frame at monitor size, not a scaled thumbnail.
    let still = preview::extract(
        &FrameRequest::Still {
            item: MediaItem::Video {
                path: f.path("wide.mp4"),
            },
            ffmpeg: f.ffmpeg.clone(),
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        (still.width, still.height),
        (noh::preview::STILL_WIDTH, noh::preview::STILL_HEIGHT)
    );
    assert!(pixel(&still, 80, 360)[0] > 150);
    let rotated =
        preview::extract(&source(f.path("rotated.mp4")), &AtomicBool::new(false)).unwrap();
    assert!(pixel(&rotated, 112, 1)[0] > 150);
    assert!(pixel(&rotated, 14, 63)[0] < 5);
    assert!(
        (pixel(&rotated, 112, 14)[1] > 180) ^ (pixel(&rotated, 112, 112)[1] > 180),
        "the left white stripe must rotate into exactly one horizontal end"
    );
    let mut rgba = image::RgbaImage::from_pixel(60, 30, image::Rgba([0, 255, 0, 0]));
    for y in 0..30 {
        for x in 30..60 {
            rgba.put_pixel(x, y, image::Rgba([0, 255, 0, 255]));
        }
    }
    rgba.save(f.path("transparent.png")).unwrap();
    let still = FrameRequest::Thumbnail {
        item: MediaItem::Image {
            path: f.path("transparent.png"),
            duration: 1.0,
        },
        ffmpeg: f.ffmpeg.clone(),
    };
    let thumbnail = preview::extract(&still, &AtomicBool::new(false)).unwrap();
    assert_eq!(pixel(&thumbnail, 56, 63), [0, 0, 0, 255]);
    assert!(pixel(&thumbnail, 168, 63)[1] > 200);
    assert!(thumbnail.rgba.chunks_exact(4).all(|p| p[3] == 255));
    let missing = FrameRequest::Thumbnail {
        item: f.path("missing.mp4").into(),
        ffmpeg: f.ffmpeg.clone(),
    };
    assert!(preview::extract(&missing, &AtomicBool::new(false)).is_err());
}

#[test]
fn still_occurrences_follow_the_export_frame_grid_and_composite_alpha() {
    let f = Fixture::new();
    image::RgbaImage::from_pixel(64, 32, image::Rgba([255, 0, 0, 255]))
        .save(f.path("red.png"))
        .unwrap();
    image::RgbaImage::from_pixel(64, 32, image::Rgba([0, 255, 0, 0]))
        .save(f.path("clear.png"))
        .unwrap();
    let project = ProjectRequest {
        montage: ExportRequest {
            items: vec![
                MediaItem::Image {
                    path: f.path("red.png"),
                    duration: 0.06,
                },
                MediaItem::Image {
                    path: f.path("clear.png"),
                    duration: 0.12,
                },
            ],
            wav: f.wav("clock.wav", 0.5),
            output: f.path("placeholder.mp4"),
            ffmpeg: f.ffmpeg.clone(),
            fade_in: 0.0,
            fade_out: 0.0,
            partial_fades: true,
            preview: true,
            clip_audio: false,
            force_encode: false,
        },
        short: None,
        captions: None,
    };
    // At 25 fps, the 60 ms red occurrence rounds to two frames (80 ms).
    assert!(pixel(&frame(&project, 79), 32, 16)[0] > 200);
    assert_eq!(pixel(&frame(&project, 80), 32, 16), [0, 0, 0, 255]);
    assert!(pixel(&frame(&project, 200), 32, 16)[0] > 200);
}

fn wait_reply(worker: &FrameWorker) -> preview::FrameReply {
    let started = Instant::now();
    loop {
        if let Some(reply) = worker.try_recv() {
            return reply;
        }
        assert!(
            started.elapsed() < Duration::from_secs(35),
            "frame worker deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn worker_reuses_cache_invalidates_changed_sources_and_suppresses_stale_replies() {
    let (f, project) = fixture();
    let worker = FrameWorker::new(|| {});
    let request = |time_ms| FrameRequest::Project {
        project: project.clone(),
        time_ms,
    };
    worker.request(request(13_000));
    let a = wait_reply(&worker).result.unwrap();
    worker.request(request(13_000));
    let b = wait_reply(&worker).result.unwrap();
    assert!(
        Arc::ptr_eq(&a, &b),
        "unchanged frames reuse the bounded cache"
    );
    for time in 13_000..13_100 {
        worker.request(request(time));
    }
    let latest = worker.request(request(14_000));
    let reply = wait_reply(&worker);
    assert_eq!(reply.revision, latest);
    assert!(center(&reply.result.unwrap()) > 170);
    worker.cancel();
    assert!(worker.try_recv().is_none());
    // Replace a media file without touching export/UI inputs. A cache hit must
    // still verify its off-thread identity and reflect the replacement pixels.
    std::fs::rename(f.path("a.mp4"), f.path("old-a.mp4")).unwrap();
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=0xeeeeee:s=256x144:r=25:d=4",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        f.path("a.mp4")
    ]);
    worker.request(request(13_000));
    let changed = wait_reply(&worker).result.unwrap();
    assert!(!Arc::ptr_eq(&a, &changed));
    assert!(center(&changed) > 210);
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        preview::extract(&request(13_000), &cancelled).unwrap_err(),
        PreviewError::Cancelled
    );
    cancelled.store(false, Ordering::Relaxed);
}

#[test]
#[ignore = "opt-in 1080p cold/warm cursor performance measurement"]
fn cursor_cold_and_eight_warm_seeks_on_thirty_minute_captioned_project() {
    let f = Fixture::new();
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=1920x1080:r=25:d=10",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "28",
        "-g",
        "25",
        "-pix_fmt",
        "yuv420p",
        f.path("1080p.mp4")
    ]);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=8000:cl=mono:d=1800",
        "-c:a",
        "pcm_s16le",
        f.path("thirty-minutes.wav")
    ]);
    std::fs::write(
        f.path("reviewed.srt"),
        "1\n00:00:00,000 --> 00:30:00,000\nWARM SEEK\n",
    )
    .unwrap();
    let project = ProjectRequest {
        montage: ExportRequest {
            items: vec![f.path("1080p.mp4").into()],
            wav: f.path("thirty-minutes.wav"),
            output: f.path("must-not-render.mp4"),
            ffmpeg: f.ffmpeg.clone(),
            fade_in: 0.0,
            fade_out: 0.0,
            partial_fades: true,
            preview: true,
            clip_audio: false,
            force_encode: false,
        },
        short: None,
        captions: Some(ShortCaptions {
            subtitles: f.path("reviewed.srt"),
            style: CaptionStyle::default(),
        }),
    };
    let worker = FrameWorker::new(|| {});
    let mut timings = Vec::new();
    let seeks = [
        0, 1250, 4000, 9000, 599_500, 899_000, 1_200_300, 1_500_600, 1_799_700,
    ];
    for time_ms in seeks {
        let started = Instant::now();
        let revision = worker.request(FrameRequest::Project {
            project: project.clone(),
            time_ms,
        });
        let reply = wait_reply(&worker);
        assert_eq!(reply.revision, revision);
        let frame = reply.result.unwrap();
        assert!(frame.width.max(frame.height) <= preview::MAX_FRAME_EDGE);
        assert_eq!(frame.rgba.len(), (frame.width * frame.height * 4) as usize);
        assert!(
            frame.rgba.len() <= (preview::MAX_FRAME_EDGE * preview::MAX_FRAME_EDGE * 4) as usize
        );
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
        assert!(!project.montage.output.exists());
    }
    eprintln!(
        "PREVIEW_PERF source=1920x1080 wav_seconds=1800 captions=true cold_ms={:.3} warm_eight_ms={:?} frame_cache_limit={} frame_cache_bytes_limit={}",
        timings[0],
        &timings[1..],
        preview::MAX_CACHE_FRAMES,
        preview::MAX_CACHE_BYTES
    );
    drop(worker);
    f.clean();
}
