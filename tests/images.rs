//! Timed still images through the real worker, with decoded geometry/timing checks.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
#[path = "support/benchmark.rs"]
mod benchmark_support;
#[path = "support/images.rs"]
#[allow(dead_code)]
mod images;
use noh::{
    engine::{Event, ExportRequest, MediaItem},
    inspection::{Diagnosis, InspectionRequest, InspectionResult, Level},
    jobs::Job,
    plan::Treatment,
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use support::Fixture;

fn still(path: &std::path::Path, duration: f64) -> MediaItem {
    MediaItem::Image {
        path: path.into(),
        duration,
    }
}
fn request(f: &Fixture, items: Vec<MediaItem>, wav: PathBuf, name: &str) -> ExportRequest {
    ExportRequest {
        items,
        wav,
        output: f.path(name),
        ffmpeg: f.ffmpeg.clone(),
        fade_in: 0.0,
        fade_out: 0.0,
        partial_fades: true,
        preview: false,
        clip_audio: false,
        force_encode: false,
    }
}
fn terminal(job: &Job) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Image job deadline");
        let done = event.is_terminal();
        events.push(event);
        if done {
            return events;
        }
    }
}
fn diagnose(f: &Fixture, request: &ExportRequest) -> Diagnosis {
    let job = Job::inspect_with_worker(
        InspectionRequest::Plan {
            request: request.clone(),
            level: Level::Exact,
        },
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    );
    let mut events = terminal(&job);
    match events.pop().unwrap() {
        Event::Inspected(Ok(InspectionResult::Plan(diagnosis))) => *diagnosis,
        result => panic!("Image diagnosis failed: {result:?}; {events:?}"),
    }
}
fn export(f: &Fixture, request: &ExportRequest) -> Diagnosis {
    let diagnosis = diagnose(f, request);
    let job =
        Job::start_checked_with_worker(request.clone(), diagnosis.clone(), f.exe.clone(), || {});
    let events = terminal(&job);
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
    drop(job);
    f.clean();
    diagnosis
}

#[test]
fn mixed_order_repeated_durations_packet_copy_preview_and_fades() {
    let f = Fixture::new();
    let red = images::png(
        &f,
        "portrait %05d Δ.png",
        32,
        64,
        |_, _| [255, 0, 0, 255],
        None,
    );
    let blue_png = images::png(&f, "blue.png", 96, 32, |_, _| [0, 0, 255, 255], None);
    let blue = images::jpeg(&f, &blue_png, "landscape.jpg", None);
    let video = f.path("green.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=lime:s=96x64:r=25:d=0.4",
        "-c:v",
        "libx264",
        "-bf",
        "2",
        &video
    ]);
    let wav = f.wav("audio.wav", 2.44);
    let req = request(
        &f,
        vec![
            still(&red, 0.20),
            video.clone().into(),
            still(&red, 0.32),
            still(&blue, 0.12),
        ],
        wav,
        "mixed.mp4",
    );
    let diagnosis = export(&f, &req);
    let plan = diagnosis.plan.unwrap();
    assert_eq!((plan.target.width, plan.target.height), (96, 64));
    assert_eq!(
        plan.clips.iter().map(|c| c.treatment).collect::<Vec<_>>(),
        [
            Treatment::Convert,
            Treatment::Copy,
            Treatment::Convert,
            Treatment::Convert
        ]
    );
    for (clip, duration) in plan.clips.iter().zip([0.2, 0.4, 0.32, 0.12]) {
        support::near(clip.source_seconds, duration, 0.000001);
    }
    assert!(plan.clips[0].reasons.contains(&"image".into()));
    let frames = images::rgb(&f, &req.output);
    assert_eq!(frames.len(), 96 * 64 * 3 * 61);
    for frame in 0..61 {
        let expected = match frame % 26 {
            0..=4 | 15..=22 => [255, 0, 0],
            5..=14 => [0, 255, 0],
            _ => [0, 0, 255],
        };
        let actual = images::pixel(&frames, 96, 64, frame, 48, 32);
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, b)| a.abs_diff(b) <= 15),
            "Frame {frame}: RGB {actual:?}, expected {expected:?}"
        );
    }
    // Portrait fit leaves black side bars; the landscape JPEG leaves top/bottom bars.
    images::close(images::pixel(&frames, 96, 64, 0, 2, 32), [0, 0, 0], 8);
    images::close(images::pixel(&frames, 96, 64, 23, 48, 2), [0, 0, 0], 8);
    let source = f.hashes(&video, true, false, false);
    let copied = f.hashes(&req.output, true, false, false);
    assert_eq!(&copied[5..15], source.as_slice());
    assert_eq!(&copied[31..41], source.as_slice());
    let mut preview = req.clone();
    preview.preview = true;
    preview.fade_in = 0.12;
    preview.fade_out = 0.12;
    preview.output = f.path("preview.mp4");
    export(&f, &preview);
    let frames = images::rgb(&f, &preview.output);
    assert_eq!(frames.len(), 96 * 64 * 3 * 61);
    images::close(images::pixel(&frames, 96, 64, 0, 48, 32), [0, 0, 0], 10);
    images::close(images::pixel(&frames, 96, 64, 10, 48, 32), [0, 255, 0], 20);
    images::close(images::pixel(&frames, 96, 64, 20, 48, 32), [255, 0, 0], 20);
    let mut faded = req.clone();
    faded.fade_in = 0.12;
    faded.fade_out = 0.12;
    faded.output = f.path("partial.mp4");
    export(&f, &faded);
    let copied = f.hashes(&faded.output, true, false, false);
    assert_eq!(
        &copied[31..41],
        source.as_slice(),
        "Central video must remain copied with fades"
    );
}

#[test]
fn alpha_is_composited_on_black_and_fractional_durations_are_frame_bound() {
    let f = Fixture::new();
    let mut inputs = Vec::new();
    for (index, alpha) in [255, 128, 0].into_iter().enumerate() {
        let path = images::png(
            &f,
            &format!("alpha-{index}.png"),
            32,
            32,
            |_, _| [255, 0, 0, alpha],
            None,
        );
        inputs.push(still(&path, 0.10)); // nearest grid point at25fps is three frames.
    }
    let wav = f.wav("audio.wav", 0.36);
    let req = request(&f, inputs, wav, "alpha.mp4");
    let diagnosis = export(&f, &req);
    for clip in &diagnosis.plan.unwrap().clips {
        support::near(clip.source_seconds, 0.12, 1e-6);
    }
    let frames = images::rgb(&f, &req.output);
    assert_eq!(frames.len(), 32 * 32 * 3 * 9);
    for (frame, color) in [(1, [255, 0, 0]), (4, [128, 0, 0]), (7, [0, 0, 0])] {
        images::close(images::pixel(&frames, 32, 32, frame, 16, 16), color, 12);
    }
    let mut tiny = req.clone();
    tiny.items.truncate(1);
    if let MediaItem::Image { duration, .. } = &mut tiny.items[0] {
        *duration = 0.001;
    }
    tiny.output = f.path("minimum.mp4");
    let diagnosis = export(&f, &tiny);
    support::near(diagnosis.plan.unwrap().clips[0].source_seconds, 0.04, 1e-6);
    assert_eq!(images::rgb(&f, &tiny.output).len(), 32 * 32 * 3 * 9);
    // A long still needs only enough prepared frames to cover the soundtrack.
    if let MediaItem::Image { duration, .. } = &mut tiny.items[0] {
        *duration = 3600.0;
    }
    tiny.output = f.path("long.mp4");
    export(&f, &tiny);
    assert_eq!(images::rgb(&f, &tiny.output).len(), 32 * 32 * 3 * 9);
}

#[test]
fn png_and_jpeg_exif_orientations_include_mirrors_without_double_rotation() {
    let f = Fixture::new();
    let colors = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
    ];
    let pattern = |x: u32, y: u32| colors[usize::from(y >= 16) * 2 + usize::from(x >= 32)];
    let base = images::png(&f, "pattern.png", 64, 32, pattern, None);
    let wav = f.wav("audio.wav", 0.12);
    let order = [
        [0, 1, 2, 3],
        [1, 0, 3, 2],
        [3, 2, 1, 0],
        [2, 3, 0, 1],
        [0, 2, 1, 3],
        [2, 0, 3, 1],
        [3, 1, 2, 0],
        [1, 3, 0, 2],
    ];
    for format in ["png", "jpg"] {
        for orientation in 1..=8 {
            let name = format!("orientation-{orientation}.{format}");
            let path = if format == "png" {
                images::png(&f, &name, 64, 32, pattern, Some(orientation))
            } else {
                images::jpeg(&f, &base, &name, Some(orientation))
            };
            let req = request(
                &f,
                vec![still(&path, 0.12)],
                wav.clone(),
                &format!("{format}-{orientation}.mp4"),
            );
            let diagnosis = export(&f, &req);
            let (width, height) = if orientation >= 5 { (32, 64) } else { (64, 32) };
            let target = &diagnosis.plan.unwrap().target;
            assert_eq!(
                (target.width, target.height),
                (width as u32, height as u32),
                "{format}/{orientation}"
            );
            let frames = images::rgb(&f, &req.output);
            assert_eq!(frames.len(), width * height * 3 * 3);
            for (point, index) in [
                (width / 4, height / 4),
                (3 * width / 4, height / 4),
                (width / 4, 3 * height / 4),
                (3 * width / 4, 3 * height / 4),
            ]
            .into_iter()
            .zip(order[orientation as usize - 1])
            {
                let color = colors[index];
                images::close(
                    images::pixel(&frames, width, height, 1, point.0, point.1),
                    [color[0], color[1], color[2]],
                    25,
                );
            }
        }
    }
}

#[test]
fn image_intervals_have_silence_when_clip_audio_is_enabled() {
    let f = Fixture::new();
    let image = images::png(&f, "red.png", 96, 64, |_, _| [255, 0, 0, 255], None);
    let video = f.path("tone.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=c=lime:s=96x64:r=25:d=0.4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=1000:sample_rate=48000:duration=0.4",
        "-c:v",
        "libx264",
        "-bf",
        "2",
        "-c:a",
        "aac",
        "-shortest",
        &video
    ]);
    let wav = f.path("silence.wav");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=stereo",
        "-t",
        "1.6",
        "-c:a",
        "pcm_s24le",
        &wav
    ]);
    let mut req = request(
        &f,
        vec![still(&image, 0.2), video.into(), still(&image, 0.2)],
        wav,
        "audio.mp4",
    );
    req.clip_audio = true;
    export(&f, &req);
    let decoded = f
        .ff(args![
            "-i",
            &req.output,
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            "48000",
            "-f",
            "f32le",
            "-"
        ])
        .stdout;
    let samples = decoded
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v) as f64)
        .collect::<Vec<_>>();
    let rms = |start: f64, end: f64| {
        let values = &samples[(start * 48000.0) as usize..(end * 48000.0) as usize];
        (values.iter().map(|s| s * s).sum::<f64>() / values.len() as f64).sqrt()
    };
    for (start, end) in [(0.04, 0.16), (0.66, 0.74), (0.84, 0.96)] {
        assert!(
            rms(start, end) < 0.002,
            "Image has audible clip sound at{start}"
        );
    }
    for (start, end) in [(0.28, 0.52), (1.08, 1.32)] {
        assert!(rms(start, end) > 0.01, "Video audio missing at{start}");
    }
}

#[test]
fn image_mutation_duration_edit_cancellation_and_no_overwrite_are_guarded() {
    use std::io::Write;
    let f = Fixture::new();
    let image = images::png(
        &f,
        "source.png",
        320,
        180,
        |x, y| [x as u8, y as u8, 128, 255],
        None,
    );
    let wav = f.wav("audio.wav", 4.44);
    let mut req = request(&f, vec![still(&image, 4.0)], wav, "result.mp4");
    let diagnosis = diagnose(&f, &req);
    if let MediaItem::Image { duration, .. } = &mut req.items[0] {
        *duration = 2.0;
    }
    let job = Job::start_checked_with_worker(req.clone(), diagnosis.clone(), f.exe.clone(), || {});
    assert!(
        matches!(terminal(&job).last(),Some(Event::Done(Err(e))) if e.code=="error.stale_inspection")
    );
    drop(job);
    if let MediaItem::Image { duration, .. } = &mut req.items[0] {
        *duration = 4.0;
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(&image)
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    let job = Job::start_checked_with_worker(req.clone(), diagnosis, f.exe.clone(), || {});
    assert!(
        matches!(terminal(&job).last(),Some(Event::Done(Err(e))) if e.code=="error.stale_inspection")
    );
    drop(job);
    assert!(!req.output.exists());
    let job = Job::start_with_worker(req.clone(), f.exe.clone(), || {});
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut requested = false;
    loop {
        let event = job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if matches!(&event,Event::Progress{phase,..} if phase.code=="progress.prepare_clip")
            && !requested
        {
            job.cancel();
            requested = true;
        }
        if event.is_terminal() {
            assert!(requested && matches!(event, Event::Cancelled), "{event:?}");
            break;
        }
    }
    drop(job);
    assert!(!req.output.exists());
    f.clean();
    std::fs::write(&req.output, b"existing user output").unwrap();
    let job = Job::start_with_worker(req.clone(), f.exe.clone(), || {});
    assert!(
        matches!(terminal(&job).last(),Some(Event::Done(Err(e))) if e.code=="error.output_exists")
    );
    drop(job);
    assert_eq!(std::fs::read(&req.output).unwrap(), b"existing user output");
    f.clean();
}

#[test]
fn invalid_images_and_unrepresentable_durations_report_typed_errors() {
    let f = Fixture::new();
    let wav = f.wav("audio.wav", 0.24);
    let invalid = f.path("broken.png");
    std::fs::write(&invalid, b"\x89PNG\r\n\x1a\ntruncated").unwrap();
    let req = request(&f, vec![still(&invalid, 0.2)], wav.clone(), "invalid.mp4");
    let job = Job::inspect_with_worker(
        InspectionRequest::Plan {
            request: req,
            level: Level::Exact,
        },
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    );
    assert!(
        matches!(terminal(&job).last(), Some(Event::Inspected(Err(e))) if e.code == "error.image" && e.path.as_ref() == Some(&invalid))
    );
    drop(job);
    let image = images::png(&f, "valid.png", 32, 32, |_, _| [255, 0, 0, 255], None);
    let req = request(&f, vec![still(&image, f64::MAX)], wav.clone(), "huge.mp4");
    let job = Job::inspect_with_worker(
        InspectionRequest::Plan {
            request: req,
            level: Level::Exact,
        },
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    );
    assert!(
        matches!(terminal(&job).last(), Some(Event::Inspected(Err(e))) if e.code == "error.image_duration")
    );
    drop(job);
    for value in ["0", "-1", "NaN", "inf"] {
        let result = f.run(
            &f.exe,
            args![
                "--soundtrack",
                &wav,
                "--image",
                &image,
                value,
                "--output",
                f.path("must-not-exist.mp4")
            ],
        );
        assert!(!result.status.success(), "Accepted duration {value}");
        assert!(!result.stderr.is_empty());
    }
    assert!(!f.path("must-not-exist.mp4").exists());
    assert!(!f.path("huge.mp4").exists());
    assert!(!f.path("invalid.mp4").exists());
    f.clean();
}

#[test]
#[ignore = "explicit image performance campaign; NOH_IMAGE_BENCH_REPORT selects JSON output"]
fn image_campaign() {
    let f = Fixture::new();
    let image = images::png(
        &f,
        "portrait.png",
        180,
        320,
        |x, y| [x as u8, y as u8, 180, 128],
        None,
    );
    let video = f.path("source.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=320x180:r=25:d=1",
        "-c:v",
        "libx264",
        "-bf",
        "2",
        &video
    ]);
    let wav = f.wav("soundtrack.wav", 7.44);
    let mut cases = Vec::new();
    for mixed in [false, true] {
        let mut samples = Vec::new();
        for sample in 0..3 {
            let output = f.path(&format!("bench-{mixed}-{sample}.mp4"));
            let mut command = args![
                "--soundtrack",
                &wav,
                "--image",
                &image,
                "0.64",
                "--partial-fades",
                "--fade-in",
                "0.12",
                "--fade-out",
                "0.12",
                "-o",
                &output
            ];
            if mixed {
                command.extend(args!["--video", &video]);
            }
            let started = Instant::now();
            f.cli(command);
            let seconds = started.elapsed().as_secs_f64();
            let decoded = f.packets(&output, false, true, false);
            assert_eq!(support::rows(&decoded).len(), 186);
            assert!(decoded.contains(if mixed {
                "#dimensions 0: 320x180"
            } else {
                "#dimensions 0: 180x320"
            }));
            samples.push(seconds);
            f.clean();
        }
        samples.sort_by(f64::total_cmp);
        cases.push(serde_json::json!({"name":if mixed {"image_and_video"}else{"image_only"},"samples_seconds":samples,
            "median_seconds":samples[1],"min_seconds":samples[0],"max_seconds":samples[2]}));
    }
    let mut report = benchmark_support::context(&f);
    report["fixture"] = serde_json::json!({"image":[180,320],"alpha":128,"image_duration":0.64,"video":[320,180],"fps":25,"video_duration":1,"soundtrack_duration":7.44});
    report["cases"] = cases.into();
    benchmark_support::save(
        &report,
        "NOH_IMAGE_BENCH_REPORT",
        "target/benchmarks/images.json",
    );
}
