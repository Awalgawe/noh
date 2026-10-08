//! Controller contracts run without `gui` and use the real CLI worker boundary.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
use noh::{
    engine::{Event, ExportRequest},
    jobs::Job,
};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use support::Fixture;

fn request(f: &Fixture) -> ExportRequest {
    ExportRequest {
        items: vec![f.path("source.mp4").into()],
        wav: f.path("audio.wav"),
        output: f.path("result.mp4"),
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
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Controller deadline");
        let done = event.is_terminal();
        events.push(event);
        if done {
            break;
        }
    }
    assert!(matches!(
        job.events.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
    events
}
#[test]
fn cancelled_before_start_does_not_spawn_or_touch_files() {
    let f = Fixture::new();
    let job = Job::start_with_cancel(
        request(&f),
        f.path("worker-does-not-exist"),
        Arc::new(AtomicBool::new(true)),
        || {},
    );
    assert!(matches!(terminal(&job).as_slice(), [Event::Cancelled]));
    drop(job);
    assert_eq!(std::fs::read_dir(f.folder.path()).unwrap().count(), 0);
}

#[test]
fn worker_short_requires_exactly_one_operation_and_no_montage_diagnosis() {
    let f = Fixture::new();
    let short = noh::shorts::ShortRequest {
        source: f.path("missing.mp4"),
        output: f.path("short.mp4"),
        ffmpeg: f.ffmpeg.clone(),
        start_ms: 0,
        end_ms: 1000,
        framing: Default::default(),
        captions: None,
        preview: false,
    };
    let export = request(&f);
    let inspection = noh::inspection::InspectionRequest::Metadata {
        path: f.path("missing.mp4"),
        wav: false,
        ffmpeg: Some(f.ffmpeg.clone()),
    };
    let caption = noh::captions::CaptionRequest {
        source: f.path("missing.mp4"),
        subtitles: f.path("reviewed.srt"),
        output: f.path("captioned.mp4"),
        ffmpeg: f.ffmpeg.clone(),
        style: Default::default(),
        preview: false,
    };
    let subtitle = noh::subtitles::SubtitleRequest {
        source: f.path("speech.wav"),
        output: f.path("captions.srt"),
        ffmpeg: f.ffmpeg.clone(),
        transcriber: f.path("whisper"),
        model: f.path("model.bin"),
        vad_model: f.path("vad.bin"),
        language: "auto".into(),
    };
    let diagnosis = noh::inspection::Diagnosis {
        version: 1,
        level: noh::inspection::Level::Exact,
        request: export.clone(),
        snapshot: noh::inspection::Snapshot(Vec::new()),
        duration: 1.0,
        container: "mp4".into(),
        audio: "aac_320".into(),
        plan: None,
        quick_media: Vec::new(),
        notes: Vec::new(),
    };
    for (key, value) in [
        ("request", serde_json::to_value(export).unwrap()),
        ("inspection", serde_json::to_value(inspection).unwrap()),
        ("captions", serde_json::to_value(caption).unwrap()),
        ("subtitles", serde_json::to_value(subtitle).unwrap()),
        ("expected", serde_json::to_value(diagnosis).unwrap()),
    ] {
        let mut wire = serde_json::json!({"version":1,"request":null,"shorts":short,"workspace":f.folder.path()});
        wire[key] = value;
        let path = f.path("wire.json");
        std::fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
        let output = f.run(&f.exe, args!["--engine-worker", &path]);
        assert!(!output.status.success(), "{key}");
        let event: Event = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            matches!(event,Event::Done(Err(ref error)) if error.code=="error.request"),
            "{key}: {event:?}"
        );
        assert!(!short.output.exists());
        assert!(!f.path("export.mp4").exists());
    }
    f.clean();
}
#[test]
fn typed_failures_do_not_terminate_the_caller_or_publish() {
    let f = Fixture::new();
    let job = Job::start_with_worker(request(&f), f.exe.clone(), || {});
    let events = terminal(&job);
    let Some(Event::Done(Err(error))) = events.last() else {
        panic!("{events:?}");
    };
    assert_eq!(error.code, "error.video");
    assert_eq!(error.operation, "validate_input");
    assert_eq!(error.path.as_ref(), Some(&f.path("source.mp4")));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Progress { percent: 100, .. }))
    );
    drop(job);
    f.clean();
    assert!(!f.path("result.mp4").exists());
}
#[test]
fn slow_consumer_preserves_plan_warning_and_one_terminal_after_publication() {
    let f = Fixture::new();
    let mut req = request(&f);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "mpeg4",
        req.items[0].path()
    ]);
    req.wav = f.wav("audio.wav", 3.44);
    req.fade_in = 0.2;
    req.fade_out = 0.2;
    let (tx, rx) = std::sync::mpsc::channel();
    let output = req.output.clone();
    let job = Job::start_with_worker(req, f.exe.clone(), move || {
        if output.exists() {
            let _ = tx.send(());
        }
    });
    // Do not receive events until publication. This exercises coalescing.
    rx.recv_timeout(Duration::from_secs(20))
        .expect("No publication");
    let events = terminal(&job);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Warning(m) if m.code == "warning.convert_clip"))
    );
    assert!(events.iter().any(|e| matches!(e, Event::Plan(p) if p.clips.iter().any(|c| c.reasons.iter().any(|r| r == "warning.partial_codec")))));
    assert!(matches!(events.last(), Some(Event::Done(Ok(_)))));
    let progress = events
        .iter()
        .filter_map(|e| {
            if let Event::Progress { percent, .. } = e {
                Some(*percent)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert!(progress.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(progress.last(), Some(&100));
    assert!(f.path("result.mp4").is_file());
    // Cancellation after the commit point cannot turn success into cancellation.
    job.cancel();
    drop(job);
    f.clean();
}
#[test]
fn cancellation_during_inspection_reaps_worker_and_cleans_only_owned_workspace() {
    let f = Fixture::new();
    let mut req = request(&f);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "libx264",
        req.items[0].path()
    ]);
    req.wav = f.wav("audio.wav", 3.44);
    std::fs::create_dir(f.path(".noh-unrelated")).unwrap();
    std::fs::write(f.path(".noh-unrelated/user.txt"), "preserve").unwrap();
    let job = Job::start_with_worker(req, f.exe.clone(), || {});
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            Event::Progress { phase, .. } if phase.code == "progress.analyze_clip" => {
                job.cancel();
                break;
            }
            e if e.is_terminal() => panic!("Unexpected terminal event: {e:?}"),
            _ => {}
        }
    }
    assert!(matches!(terminal(&job).last(), Some(Event::Cancelled)));
    drop(job);
    assert!(!f.path("result.mp4").exists());
    assert_eq!(
        std::fs::read_to_string(f.path(".noh-unrelated/user.txt")).unwrap(),
        "preserve"
    );
    assert_eq!(
        std::fs::read_dir(f.folder.path())
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".noh-"))
            .count(),
        1
    );
}

#[test]
fn cli_event_stream_and_signal_file_cancel_active_encoding() {
    let f = Fixture::new();
    let req = request(&f);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=640x360:r=25:d=1",
        "-c:v",
        "libx264",
        req.items[0].path()
    ]);
    let wav = f.wav("audio.wav", 120.0);
    let events_path = f.path("events.jsonl");
    let signal = f.path("cancel.signal");
    let mut command = std::process::Command::new(&f.exe);
    command.args(args![
        req.items[0].path(),
        &wav,
        "-o",
        &req.output,
        "--ffmpeg",
        &f.ffmpeg,
        "--reencode",
        "--events",
        &events_path,
        "--cancel-file",
        &signal
    ]);
    let process =
        std::thread::spawn(move || process::run(command, Duration::from_secs(25)).unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let text = std::fs::read_to_string(&events_path).unwrap_or_default();
        if text
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .any(|e| e["event"] == "progress" && e["data"]["phase"]["code"] == "progress.encode")
        {
            break;
        }
        if Instant::now() >= deadline {
            std::fs::write(&signal, "cancel").unwrap();
            panic!("Encoding did not start");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let cancelled = Instant::now();
    std::fs::write(&signal, "cancel").unwrap();
    let output = process.join().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", support::log(&output));
    assert!(cancelled.elapsed() < Duration::from_secs(5));
    let events: Vec<Event> = std::fs::read_to_string(&events_path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events.iter().filter(|e| e.is_terminal()).count(), 1);
    assert!(matches!(events.last(), Some(Event::Cancelled)));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Progress { percent: 100, .. }))
    );
    assert!(!req.output.exists());
    f.clean();
}

#[test]
fn irregular_first_clip_uses_its_measured_rate_and_exports_valid_timestamps() {
    let f = Fixture::new();
    let mut req = request(&f);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-vf",
        "select='not(mod(n,3))'",
        "-fps_mode",
        "vfr",
        "-c:v",
        "libx264",
        "-bf",
        "0",
        "-video_track_timescale",
        "1000",
        req.items[0].path()
    ]);
    req.wav = f.wav("audio.wav", 3.0);
    req.items.push(req.items[0].clone());
    req.fade_in = 0.1;
    let plan = noh::engine::inspect_and_plan(&req, &mut |_| {}).unwrap();
    let fps = plan.target.rate_num as f64 / plan.target.rate_den as f64;
    assert!(
        (8.0..10.0).contains(&fps),
        "Irregular source must not inherit the first packet's 25 fps: {fps}"
    );
    assert!(
        plan.clips
            .iter()
            .all(|c| c.reasons.iter().any(|r| r == "warning.partial_timing"))
    );
    let output = req.output.clone();
    let job = Job::start_with_worker(req, f.exe.clone(), || {});
    let events = terminal(&job);
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
    assert!(!f.hashes(&output, false, true, false).is_empty());
    support::near(f.duration(&output), 3.0, 0.13);
    let packets = support::rows(&f.packets(&output, false, false, false));
    assert!(
        packets
            .windows(2)
            .all(|p| p[0][1].parse::<i64>().unwrap() < p[1][1].parse::<i64>().unwrap())
    );
    drop(job);
    f.clean();
}

#[test]
fn standalone_diagnosis_and_cli_json_share_the_export_plan_without_writing_output() {
    use noh::inspection::{InspectionRequest, InspectionResult, Level};
    let f = Fixture::new();
    let mut req = request(&f);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "libx264",
        "-bf",
        "3",
        req.items[0].path()
    ]);
    let other = f.path("other.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=128x72:r=30:d=1",
        "-c:v",
        "mpeg4",
        &other
    ]);
    req.items.push(other.clone().into());
    req.wav = f.wav("audio.wav", 5.44);
    req.fade_in = 0.2;
    req.fade_out = 0.2;
    let job = Job::inspect_with_worker(
        InspectionRequest::Plan {
            request: req.clone(),
            level: Level::Exact,
        },
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    );
    let events = terminal(&job);
    let Some(Event::Inspected(Ok(InspectionResult::Plan(diagnosis)))) = events.last() else {
        panic!("{events:?}")
    };
    assert!(!req.output.exists());
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::Progress { percent: 100, .. }))
    );
    let plan = diagnosis.plan.as_ref().unwrap();
    assert_eq!(plan.clips[0].treatment, noh::plan::Treatment::Copy);
    assert_eq!(plan.clips[1].treatment, noh::plan::Treatment::Convert);
    assert!(plan.clips[1].reasons.contains(&"dimensions".into()));
    let cli = f.cli(args![
        req.items[0].path(),
        &req.wav,
        "--video",
        &other,
        "--fade-in",
        "0.2",
        "--fade-out",
        "0.2",
        "--partial-fades",
        "-o",
        &req.output,
        "--diagnose",
        "--json"
    ]);
    let json: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(json["version"], 1);
    let Event::Inspected(Ok(InspectionResult::Plan(cli_diagnosis))) =
        serde_json::from_value(json["result"].clone()).unwrap()
    else {
        panic!("{json}")
    };
    assert_eq!(cli_diagnosis.plan, diagnosis.plan);
    let quick = f.cli(args![
        req.items[0].path(),
        &req.wav,
        "-o",
        &req.output,
        "--diagnose=quick",
        "--json"
    ]);
    let json: serde_json::Value = serde_json::from_slice(&quick.stdout).unwrap();
    let Event::Inspected(Ok(InspectionResult::Plan(quick))) =
        serde_json::from_value(json["result"].clone()).unwrap()
    else {
        panic!("{json}")
    };
    assert!(quick.plan.is_none());
    assert_eq!(quick.quick_media.len(), 1);
    assert!(quick.validate(&quick.request).is_err());
    drop(job);
    let export =
        Job::start_checked_with_worker(req.clone(), *diagnosis.clone(), f.exe.clone(), || {});
    let events = terminal(&export);
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Plan(actual) if actual == plan))
    );
    drop(export);
    f.clean();
    // Diagnosis is also allowed for an existing destination: it never publishes.
    let cli = f.cli(args![
        req.items[0].path(),
        &req.wav,
        "-o",
        &req.output,
        "--diagnose"
    ]);
    assert!(support::text(&cli.stdout).contains("Diagnosis: Exact"));
}

#[test]
fn exact_diagnosis_reuses_renamed_and_moved_outputs_without_overwriting() {
    use noh::inspection::{InspectionRequest, InspectionResult, Level};
    let f = Fixture::new();
    let mut req = request(&f);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "libx264",
        req.items[0].path()
    ]);
    req.wav = f.wav("audio.wav", 1.2);
    let InspectionResult::Plan(diagnosis) = noh::inspection::inspect(
        &InspectionRequest::Plan {
            request: req.clone(),
            level: Level::Exact,
        },
        &mut |_| {},
    )
    .unwrap() else {
        unreachable!()
    };
    std::fs::create_dir(f.path("destination")).unwrap();
    for output in [f.path("renamed.mp4"), f.path("destination/moved.MP4")] {
        let mut renamed = req.clone();
        renamed.output = output.clone();
        assert_ne!(renamed, diagnosis.request);
        assert!(diagnosis.request.same_render_settings(&renamed));
        diagnosis.validate(&renamed).unwrap();
        let job = Job::start_checked_with_worker(
            renamed.clone(),
            *diagnosis.clone(),
            f.exe.clone(),
            || {},
        );
        let events = terminal(&job);
        assert!(
            matches!(events.last(), Some(Event::Done(Ok(result))) if result.output == output),
            "{events:?}"
        );
        assert!(events.iter().any(
            |event| matches!(event, Event::Plan(actual) if Some(actual) == diagnosis.plan.as_ref())
        ));
        assert!(!f.hashes(&output, false, true, false).is_empty());
        support::near(f.duration(&output), 1.2, 0.13);
        drop(job);
        let published = std::fs::read(&output).unwrap();
        let retry =
            Job::start_checked_with_worker(renamed, *diagnosis.clone(), f.exe.clone(), || {});
        assert!(
            matches!(terminal(&retry).last(), Some(Event::Done(Err(error))) if error.code == "error.output_exists")
        );
        drop(retry);
        assert_eq!(std::fs::read(&output).unwrap(), published);
    }
    // Reuse must not rewrite the diagnosis's original request to fit a destination.
    assert_eq!(diagnosis.request, req);
    assert!(!req.output.exists());
    f.clean();
}

#[test]
fn changed_inputs_during_inspection_and_after_preflight_cannot_publish() {
    use noh::inspection::{InspectionRequest, InspectionResult, Level};
    use std::io::Write;
    let f = Fixture::new();
    let mut req = request(&f);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "libx264",
        req.items[0].path()
    ]);
    req.wav = f.wav("audio.wav", 3.44);
    let inspect = InspectionRequest::Plan {
        request: req.clone(),
        level: Level::Exact,
    };
    let mut changed = false;
    let error = noh::inspection::inspect(&inspect, &mut |event| {
        if !changed && matches!(event, Event::Progress { .. }) {
            // Synchronous event delivery is the barrier: the initial snapshot and
            // WAV read have finished, and the final snapshot has not run yet.
            std::fs::OpenOptions::new()
                .append(true)
                .open(&req.wav)
                .unwrap()
                .write_all(b"changed")
                .unwrap();
            changed = true;
        }
    })
    .unwrap_err();
    assert!(changed);
    assert_eq!(error.code, "error.stale_inspection");
    let InspectionResult::Plan(diagnosis) =
        noh::inspection::inspect(&inspect, &mut |_| {}).unwrap()
    else {
        unreachable!()
    };
    let mut mismatched = diagnosis.clone();
    mismatched.plan.as_mut().unwrap().target.width += 2;
    let job = Job::start_checked_with_worker(req.clone(), *mismatched, f.exe.clone(), || {});
    assert!(
        matches!(terminal(&job).last(), Some(Event::Done(Err(e))) if e.code == "error.stale_inspection")
    );
    assert!(!req.output.exists());
    drop(job);
    type RequestChange = (&'static str, fn(&mut ExportRequest));
    let changes: [RequestChange; 10] = [
        ("container", |r| {
            r.output.set_extension("mkv");
        }),
        ("video", |r| {
            r.items[0].path_mut().set_file_name("different.mp4")
        }),
        ("soundtrack", |r| r.wav.set_file_name("different.wav")),
        ("engine", |r| r.ffmpeg.set_file_name("different-engine")),
        ("fade in", |r| r.fade_in = 0.1),
        ("fade out", |r| r.fade_out = 0.1),
        ("partial fades", |r| r.partial_fades = false),
        ("preview", |r| r.preview = true),
        ("clip audio", |r| r.clip_audio = true),
        ("full encoding", |r| r.force_encode = true),
    ];
    for (name, change) in changes {
        let mut settings = req.clone();
        change(&mut settings);
        assert!(!diagnosis.request.same_render_settings(&settings), "{name}");
        assert_eq!(
            diagnosis.validate(&settings).unwrap_err().code,
            "error.stale_inspection",
            "{name}"
        );
    }
    let mut ordered = req.clone();
    ordered.items.push(f.path("other.mp4").into());
    let mut reordered = ordered.clone();
    reordered.items.reverse();
    assert!(!ordered.same_render_settings(&reordered));
    for invalid in [
        {
            let mut invalid = diagnosis.clone();
            invalid.version += 1;
            invalid
        },
        {
            let mut invalid = diagnosis.clone();
            invalid.level = Level::Quick;
            invalid
        },
        {
            let mut invalid = diagnosis.clone();
            invalid.plan = None;
            invalid
        },
        {
            let mut invalid = diagnosis.clone();
            // Recorded FFmpeg metadata must still match the current executable.
            invalid.snapshot.0.last_mut().unwrap().bytes += 1;
            invalid
        },
    ] {
        assert_eq!(
            invalid.validate(&req).unwrap_err().code,
            "error.stale_inspection"
        );
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(req.items[0].path())
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    let job = Job::start_checked_with_worker(req.clone(), *diagnosis, f.exe.clone(), || {});
    assert!(
        matches!(terminal(&job).last(), Some(Event::Done(Err(e))) if e.code == "error.stale_inspection")
    );
    assert!(!req.output.exists());
    drop(job);
    f.clean();
}

#[test]
fn cancelled_standalone_inspection_has_one_terminal_and_no_export() {
    use noh::inspection::{InspectionRequest, Level};
    let f = Fixture::new();
    let job = Job::inspect_with_worker(
        InspectionRequest::Plan {
            request: request(&f),
            level: Level::Exact,
        },
        f.exe.clone(),
        Arc::new(AtomicBool::new(true)),
        || {},
    );
    assert!(matches!(terminal(&job).as_slice(), [Event::Cancelled]));
    assert_eq!(std::fs::read_dir(f.folder.path()).unwrap().count(), 0);
}

#[test]
fn repeated_sources_keep_order_decisions_and_checked_export_pixels() {
    use noh::inspection::{InspectionRequest, InspectionResult, Level};
    let f = Fixture::new();
    let mut repeated = request(&f);
    let source = repeated.items[0].path().clone();
    let other = f.path("other.mp4");
    for (path, size, codec, seconds) in [
        (&source, "96x64", "libx264", "1.2"),
        (&other, "128x72", "mpeg4", "0.8"),
    ] {
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            format!("testsrc2=s={size}:r=25:d={seconds}"),
            "-c:v",
            codec,
            path
        ]);
    }
    // An alias retains its spelling in public decisions, even if inspection
    // recognizes the same resolved source as another entry.
    std::fs::create_dir(f.path("alias")).unwrap();
    repeated.items = vec![
        source.clone().into(),
        other.clone().into(),
        f.path("alias/../source.mp4").into(),
        other.into(),
        source.into(),
    ];
    repeated.wav = f.wav("audio.wav", 2.44);
    repeated.fade_in = 0.12;
    repeated.fade_out = 0.12;
    let mut distinct = repeated.clone();
    distinct.output = f.path("distinct.mp4");
    for (index, item) in distinct.items.iter_mut().enumerate() {
        let copy = f.path(&format!("distinct-{index}.mp4"));
        std::fs::copy(item.path(), &copy).unwrap();
        *item.path_mut() = copy;
    }
    let mut exact = Vec::new();
    for level in [Level::Quick, Level::Exact] {
        let mut diagnoses = Vec::new();
        for req in [&repeated, &distinct] {
            let mut analyzed = Vec::new();
            let InspectionResult::Plan(diagnosis) = noh::inspection::inspect(
                &InspectionRequest::Plan {
                    request: req.clone(),
                    level,
                },
                &mut |event| {
                    if let Event::Progress { phase, .. } = event
                        && phase.code == "progress.analyze_clip"
                    {
                        analyzed.push(phase.args);
                    }
                },
            )
            .unwrap() else {
                unreachable!()
            };
            assert_eq!(diagnosis.request, *req);
            assert_eq!(diagnosis.snapshot.0.len(), req.items.len() + 2);
            if level == Level::Exact {
                assert_eq!(
                    analyzed,
                    (1..=5)
                        .map(|i| vec![i.to_string(), "5".into()])
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    diagnosis
                        .plan
                        .as_ref()
                        .unwrap()
                        .clips
                        .iter()
                        .map(|c| &c.path)
                        .collect::<Vec<_>>(),
                    req.items.iter().map(|item| item.path()).collect::<Vec<_>>()
                );
            }
            diagnoses.push(diagnosis);
        }
        if level == Level::Quick {
            assert!(diagnoses.iter().all(|d| d.plan.is_none()));
            assert_eq!(
                serde_json::to_value(&diagnoses[0].quick_media).unwrap(),
                serde_json::to_value(&diagnoses[1].quick_media).unwrap()
            );
            let times: Vec<_> = diagnoses[0].quick_media.iter().map(|m| m.seconds).collect();
            assert_eq!(times, vec![1.2, 0.8, 1.2, 0.8, 1.2]);
        } else {
            let mut reference = diagnoses[1].plan.clone().unwrap();
            for (clip, path) in reference.clips.iter_mut().zip(&repeated.items) {
                clip.path = path.path().clone();
            }
            assert_eq!(diagnoses[0].plan.as_ref().unwrap(), &reference);
            exact = diagnoses;
        }
    }
    for (req, diagnosis) in [repeated.clone(), distinct.clone()].into_iter().zip(exact) {
        let job = Job::start_checked_with_worker(req.clone(), *diagnosis, f.exe.clone(), || {});
        let events = terminal(&job);
        assert!(
            matches!(events.last(), Some(Event::Done(Ok(_)))),
            "{events:?}"
        );
        drop(job);
        support::near(f.duration(&req.output), 2.44, 0.13);
    }
    assert_eq!(
        f.hashes(&repeated.output, false, true, false),
        f.hashes(&distinct.output, false, true, false)
    );
    assert_eq!(
        f.hashes(&repeated.output, true, false, false),
        f.hashes(&distinct.output, true, false, false)
    );
    f.clean();
}

#[test]
fn repeated_source_mutation_cannot_return_a_stale_plan_or_poison_the_next_call() {
    use std::io::Write;
    let f = Fixture::new();
    let mut req = request(&f);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "libx264",
        req.items[0].path()
    ]);
    req.wav = f.wav("audio.wav", 1.44);
    req.fade_in = 0.12;
    req.fade_out = 0.12;
    req.items = vec![req.items[0].clone(); 3];
    let mut changed = false;
    // Use the direct embedding API too: it has no outer diagnosis snapshot.
    let error = noh::engine::inspect_and_plan(&req, &mut |event| {
        if let Event::Progress { phase, .. } = event
            && phase.code == "progress.analyze_clip"
            && phase.args.first().map(String::as_str) == Some("2")
        {
            std::fs::OpenOptions::new()
                .append(true)
                .open(req.items[0].path())
                .unwrap()
                .write_all(b"changed")
                .unwrap();
            changed = true;
        }
    })
    .unwrap_err();
    assert!(changed);
    assert_eq!(error.code, "error.stale_inspection");
    // Trailing bytes leave this MP4 readable. A fresh operation must accept its
    // new stamp instead of retaining either the old success or a cached failure.
    let current = noh::engine::inspect_and_plan(&req, &mut |_| {}).unwrap();
    assert_eq!(current.clips.len(), 3);
    assert!(current.clips.iter().all(|c| c.path == *req.items[0].path()));
    assert!(!req.output.exists());
    f.clean();
}
