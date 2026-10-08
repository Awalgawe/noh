//! Direct project ranges: visual phase, WAV clock, captions and controller ownership.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;

use noh::{
    captions::CaptionStyle,
    engine::{Event, ExportRequest, MediaItem},
    jobs::Job,
    project::{ProjectRequest, ProjectShort},
    shorts::{Framing, ShortCaptions},
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use support::Fixture;

fn fixture() -> (Fixture, ProjectRequest) {
    let f = Fixture::new();
    // Clip A has delayed original audio. Clip B deliberately uses another codec.
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=0x333333:s=256x144:r=25:d=4",
        "-itsoffset",
        "0.5",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=220:sample_rate=48000:duration=3.5",
        "-c:v",
        "libx264",
        "-bf",
        "2",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        f.path("a.mp4")
    ]);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=0xcccccc:s=256x144:r=25:d=6",
        "-c:v",
        "mpeg4",
        f.path("b.mp4")
    ]);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "aevalsrc=if(between(t\\,14\\,14.08)\\,0.6*sin(2*PI*1000*t)\\,0):s=48000:d=20",
        "-c:a",
        "pcm_s24le",
        f.path("clock.wav")
    ]);
    std::fs::write(f.path("reviewed.srt"), "1\n00:00:12,500 --> 00:00:13,500\nBOUNDARY\n\n2\n00:00:14,000 --> 00:00:16,000\nAT FOURTEEN\n\n3\n00:00:19,000 --> 00:00:19,500\nOUTSIDE\n").unwrap();
    let request = ProjectRequest {
        montage: ExportRequest {
            items: vec![f.path("a.mp4").into(), f.path("b.mp4").into()],
            wav: f.path("clock.wav"),
            output: f.path("project.mp4"),
            ffmpeg: f.ffmpeg.clone(),
            fade_in: 0.0,
            fade_out: 0.0,
            partial_fades: true,
            preview: true,
            clip_audio: false,
            force_encode: false,
        },
        short: Some(ProjectShort {
            start_ms: 13_000,
            end_ms: 18_000,
            restart_loops: false,
            framing: Framing::Pad,
        }),
        captions: Some(ShortCaptions {
            subtitles: f.path("reviewed.srt"),
            style: CaptionStyle::default(),
        }),
    };
    (f, request)
}

fn run(f: &Fixture, request: &ProjectRequest) -> Vec<Event> {
    let job = Job::project_with_worker(request.clone(), f.exe.clone(), || {});
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(Duration::from_secs(55))
            .expect("Project test deadline");
        let terminal = event.is_terminal();
        events.push(event);
        if terminal {
            break;
        }
    }
    drop(job);
    f.clean();
    events
}

fn success(events: &[Event]) {
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
    let progress: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Progress { percent, .. } => Some(*percent),
            _ => None,
        })
        .collect();
    assert!(progress.windows(2).all(|p| p[0] <= p[1]));
    assert_eq!(progress.last(), Some(&100));
}

fn shades(f: &Fixture, output: &Path, region: &str) -> Vec<u8> {
    f.ff(args![
        "-i",
        output,
        "-map",
        "0:v:0",
        "-vf",
        format!("{region},scale=1:1,format=gray"),
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-"
    ])
    .stdout
}

fn audio(f: &Fixture, output: &Path) -> Vec<f32> {
    f.ff(args![
        "-i", output, "-map", "0:a:0", "-ar", "48000", "-ac", "1", "-f", "f32le", "-"
    ])
    .stdout
    .as_chunks::<4>()
    .0
    .iter()
    .map(|c| f32::from_le_bytes(*c))
    .collect()
}

fn caption_visibility(f: &Fixture, output: &Path) -> Vec<bool> {
    // Inspect the pixels themselves: a bicubic reduction to one pixel can miss
    // small text near the edge of a mostly black subtitle margin.
    f.ff(args![
        "-i",
        output,
        "-map",
        "0:v:0",
        "-vf",
        "crop=320:110:20:530,format=gray",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-"
    ])
    .stdout
    .as_chunks::<{ 320 * 110 }>()
    .0
    .iter()
    .map(|frame| frame.iter().any(|p| *p > 24))
    .collect()
}

fn rms(samples: &[f32], start: f64, end: f64) -> f64 {
    let samples = &samples[(start * 48000.0) as usize..(end * 48000.0) as usize];
    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn restart_changes_only_visual_phase_while_wav_and_trimmed_captions_stay_fixed() {
    let (f, mut request) = fixture();
    let mut caption_pixels = Vec::new();
    for restart in [false, true] {
        request.short.as_mut().unwrap().restart_loops = restart;
        request.montage.output = f.path(if restart {
            "restart.mp4"
        } else {
            "continuous.mp4"
        });
        let started = Instant::now();
        success(&run(&f, &request));
        eprintln!(
            "Selected 13..18s, restart={restart}: {:?}",
            started.elapsed()
        );
        let pixels = shades(&f, &request.montage.output, "crop=8:8:(iw-8)/2:(ih-8)/2");
        assert_eq!(pixels.len(), 125);
        let boundary = if restart { 100 } else { 25 };
        assert!(pixels[..boundary].iter().all(|v| *v < 80), "{pixels:?}");
        assert!(pixels[boundary..].iter().all(|v| *v > 170), "{pixels:?}");
        let samples = audio(&f, &request.montage.output);
        assert!(rms(&samples, 1.02, 1.06) > 0.2);
        assert!(rms(&samples, 0.7, 0.8) < 0.001);
        assert!(rms(&samples, 2.0, 2.1) < 0.001);
        let captions = caption_visibility(&f, &request.montage.output);
        assert!(captions[2], "Clipped boundary cue must begin at local zero");
        assert!(!captions[20]);
        assert!(captions[40]);
        assert!(!captions[90]);
        caption_pixels.push(captions);
    }
    assert_eq!(caption_pixels[0], caption_pixels[1]);
}

#[test]
fn original_audio_preserves_delay_and_follows_visual_restart() {
    let (f, mut request) = fixture();
    request.montage.clip_audio = true;
    request.captions = None;
    for restart in [false, true] {
        request.short.as_mut().unwrap().restart_loops = restart;
        request.montage.output = f.path(if restart {
            "mix-restart.mp4"
        } else {
            "mix-phase.mp4"
        });
        success(&run(&f, &request));
        let samples = audio(&f, &request.montage.output);
        assert!(
            rms(&samples, 1.02, 1.06) > 0.10,
            "WAV pulse must retain its selected position"
        );
        if restart {
            assert!(
                rms(&samples, 0.1, 0.3) < 0.002,
                "Original audio delay must not disappear"
            );
            assert!(rms(&samples, 0.7, 0.8) > 0.02);
            assert!(rms(&samples, 2.0, 2.1) > 0.02);
            assert!(rms(&samples, 4.3, 4.5) < 0.002);
        } else {
            assert!(rms(&samples, 0.1, 0.3) > 0.02);
            assert!(rms(&samples, 2.0, 2.1) < 0.002);
        }
    }
}

#[test]
fn between_frame_selection_keeps_wav_position_and_bounded_video_end() {
    let (f, mut request) = fixture();
    request.captions = None;
    let short = request.short.as_mut().unwrap();
    short.start_ms = 13_015;
    short.end_ms = 18_017;
    success(&run(&f, &request));
    let pixels = shades(&f, &request.montage.output, "crop=8:8:(iw-8)/2:(ih-8)/2");
    assert!((125..=126).contains(&pixels.len()), "{pixels:?}");
    assert!(pixels[..24].iter().all(|v| *v < 80));
    assert!(pixels[26..].iter().all(|v| *v > 170));
    let samples = audio(&f, &request.montage.output);
    assert!(rms(&samples, 1.0, 1.03) > 0.2);
    assert!(rms(&samples, 0.8, 0.9) < 0.001);
    assert!((f.duration(&request.montage.output) - 5.002).abs() < 0.05);
}

#[test]
fn project_h264_level_uses_picture_rate_instead_of_nut_timestamp_ticks() {
    let f = Fixture::new();
    let wav = f.wav("clock.wav", 1.0);
    for (index, rate, preview, max_level) in [
        (0, "24", false, 40),
        (1, "30000/1001", false, 40),
        (2, "30000/1001", true, 30),
    ] {
        let source = f.path(&format!("source-{index}.mp4"));
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            format!("testsrc2=s=160x90:r={rate}:d=0.5"),
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            &source
        ]);
        let output = f.path(&format!("short-{index}.mp4"));
        let request = ProjectRequest {
            montage: ExportRequest {
                items: vec![source.into()],
                wav: wav.clone(),
                output: output.clone(),
                ffmpeg: f.ffmpeg.clone(),
                fade_in: 0.0,
                fade_out: 0.0,
                partial_fades: false,
                preview,
                clip_audio: false,
                force_encode: false,
            },
            short: Some(ProjectShort {
                start_ms: 0,
                end_ms: 400,
                framing: Framing::Crop,
                restart_loops: false,
            }),
            captions: None,
        };
        success(&run(&f, &request));
        let headers = f.ff(args![
            "-loglevel",
            "info",
            "-i",
            &output,
            "-map",
            "0:v:0",
            "-c",
            "copy",
            "-bsf:v",
            "trace_headers",
            "-frames:v",
            "1",
            "-f",
            "null",
            "-"
        ]);
        let log = support::text(&headers.stderr);
        let level: u32 = log
            .lines()
            .find(|line| line.contains("level_idc"))
            .and_then(|line| line.rsplit_once(" = "))
            .map(|(_, value)| value.parse().unwrap())
            .expect("The output must contain an H.264 sequence header");
        assert!(
            level <= max_level,
            "{rate} fps, preview={preview}: unexpectedly high H.264 level {level}; \
             the encoder must not interpret NUT timestamp ticks as picture rate"
        );
    }
}

#[test]
fn image_first_restart_full_captions_and_project_clock_fades() {
    let (f, mut request) = fixture();
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=0x333333:s=256x144",
        "-frames:v",
        "1",
        "-update",
        "1",
        f.path("first.png")
    ]);
    request.montage.items[0] = MediaItem::Image {
        path: f.path("first.png"),
        duration: 4.0,
    };
    request.short.as_mut().unwrap().restart_loops = true;
    request.captions = None;
    request.montage.fade_in = 3.0;
    request.montage.fade_out = 3.0;
    success(&run(&f, &request));
    let pixels = shades(&f, &request.montage.output, "crop=8:8:(iw-8)/2:(ih-8)/2");
    assert!(
        pixels[0] > 35,
        "Interior selection must not restart the project fade-in"
    );
    assert!(pixels[..100].iter().all(|v| *v < 80));
    assert!(pixels[101] > 170);
    assert!(
        pixels[124] < pixels[101],
        "Project fade-out starts at WAV time 17 s"
    );

    request.short = None;
    request.montage.output = f.path("full-captions.mp4");
    request.montage.fade_in = 0.0;
    request.montage.fade_out = 0.0;
    request.captions = Some(ShortCaptions {
        subtitles: f.path("reviewed.srt"),
        style: CaptionStyle::default(),
    });
    success(&run(&f, &request));
    assert!((f.duration(&request.montage.output) - 20.0).abs() < 0.05);
}

#[test]
fn project_validation_no_overwrite_and_cancel_leave_no_private_pieces() {
    let (f, mut request) = fixture();
    let snapshot = request.snapshot().unwrap();
    std::fs::write(&request.captions.as_ref().unwrap().subtitles, "changed").unwrap();
    assert!(snapshot.verify().is_err());
    request.captions = None;
    request.short.as_mut().unwrap().end_ms = 21_000;
    let events = run(&f, &request);
    assert!(matches!(events.last(), Some(Event::Done(Err(e))) if e.code == "error.short_range"));
    request.short.as_mut().unwrap().end_ms = 18_000;
    std::fs::write(&request.montage.output, "keep existing").unwrap();
    assert!(
        matches!(run(&f, &request).last(), Some(Event::Done(Err(e))) if e.code == "error.output_exists")
    );
    assert_eq!(
        std::fs::read(&request.montage.output).unwrap(),
        b"keep existing"
    );
    request.montage.output = f.path("cancelled.mp4");
    let job = Job::project_with_cancel(
        request.clone(),
        f.exe.clone(),
        Arc::new(AtomicBool::new(true)),
        || {},
    );
    assert!(matches!(
        job.events.recv_timeout(Duration::from_secs(10)).unwrap(),
        Event::Cancelled
    ));
    drop(job);
    let job = Job::project_with_worker(request.clone(), f.exe.clone(), || {});
    loop {
        let event = job.events.recv_timeout(Duration::from_secs(55)).unwrap();
        if matches!(event, Event::Plan(_)) {
            job.cancel();
        }
        if event.is_terminal() {
            assert!(matches!(event, Event::Cancelled));
            break;
        }
    }
    drop(job);
    assert!(!request.montage.output.exists());
    f.clean();
    request.montage.output = f.path("race.mp4");
    let job = Job::project_with_worker(request.clone(), f.exe.clone(), || {});
    loop {
        let event = job.events.recv_timeout(Duration::from_secs(55)).unwrap();
        if matches!(event, Event::Plan(_)) {
            std::fs::write(&request.montage.output, b"other publisher").unwrap();
        }
        if event.is_terminal() {
            assert!(matches!(event, Event::Done(Err(e)) if e.code == "error.output_exists"));
            break;
        }
    }
    drop(job);
    assert_eq!(
        std::fs::read(&request.montage.output).unwrap(),
        b"other publisher"
    );
    f.clean();
}

#[test]
fn additive_cli_exports_a_late_selection_without_a_full_project_video() {
    let (f, request) = fixture();
    let long = f.wav("long.wav", 1800.0);
    let started = Instant::now();
    let result = f.cli(args![
        "--project",
        "--soundtrack",
        &long,
        "--video",
        request.montage.items[0].path(),
        "--video",
        request.montage.items[1].path(),
        "--output",
        f.path("late.mp4"),
        "--start-ms",
        "1713000",
        "--end-ms",
        "1718000",
        "--restart-loops",
        "--preview",
        "--json"
    ]);
    eprintln!("5s selected late in 30min WAV: {:?}", started.elapsed());
    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(json["version"], 1);
    assert!(f.path("late.mp4").exists());
    assert!((f.duration(&f.path("late.mp4")) - 5.0).abs() < 0.05);
    f.clean();
}
