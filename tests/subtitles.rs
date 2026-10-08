//! Subtitle jobs through the actual worker. Real recognition is explicitly opt-in.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
#[path = "support/benchmark.rs"]
mod benchmark_support;
use noh::{
    engine::Event,
    jobs::Job,
    subtitles::{SubtitleRequest, SubtitleResult},
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use support::Fixture;

fn terminal(job: &Job) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Subtitle job deadline");
        let done = event.is_terminal();
        events.push(event);
        if done {
            return events;
        }
    }
}
fn required(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("Set {name} for this opt-in recognition test"))
}
fn request(f: &Fixture) -> SubtitleRequest {
    SubtitleRequest {
        source: f.path("input.wav"),
        output: f.path("captions.srt"),
        ffmpeg: f.ffmpeg.clone(),
        transcriber: f.path("missing-whisper"),
        model: f.path("missing-model"),
        vad_model: f.path("missing-vad"),
        language: "auto".into(),
    }
}
fn runtime(f: &Fixture) -> SubtitleRequest {
    SubtitleRequest {
        transcriber: required("NOH_WHISPER"),
        model: required("NOH_WHISPER_MODEL"),
        vad_model: required("NOH_WHISPER_VAD"),
        ..request(f)
    }
}
fn success(events: &[Event]) -> &SubtitleResult {
    let Some(Event::Subtitled(Ok(result))) = events.last() else {
        panic!("Subtitle job failed: {events:?}");
    };
    assert!(matches!(
        events.get(events.len() - 2),
        Some(Event::Progress { percent: 100, .. })
    ));
    assert_eq!(
        std::fs::read_to_string(&result.output).unwrap(),
        result.track.to_srt().unwrap()
    );
    result
}

#[test]
fn pre_cancelled_subtitles_do_not_spawn_or_touch_inputs() {
    let f = Fixture::new();
    let job = Job::transcribe_with_cancel(
        request(&f),
        f.path("no-worker"),
        Arc::new(AtomicBool::new(true)),
        || {},
    );
    assert!(matches!(terminal(&job).as_slice(), [Event::Cancelled]));
    drop(job);
    assert_eq!(std::fs::read_dir(f.folder.path()).unwrap().count(), 0);
}

#[test]
fn subtitle_validation_errors_are_typed_and_do_not_publish() {
    let f = Fixture::new();
    let mut req = request(&f);
    req.output = f.path("incorrect.mp4");
    let job = Job::transcribe_with_worker(req, f.exe.clone(), || {});
    let events = terminal(&job);
    assert!(
        matches!(events.last(), Some(Event::Subtitled(Err(_)))),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Progress { percent: 100, .. }))
    );
    drop(job);
    f.clean();
    assert!(!f.path("incorrect.mp4").exists());
}

#[test]
#[ignore = "requires NOH_WHISPER, NOH_WHISPER_MODEL, NOH_WHISPER_VAD and FFmpeg"]
fn real_silence_noise_missing_audio_and_backend_failures() {
    let f = Fixture::new();
    for (name, source) in [
        ("silence", "anullsrc=r=16000:cl=mono"),
        ("noise", "anoisesrc=r=16000:a=0.01:s=42"),
    ] {
        let mut req = runtime(&f);
        req.source = f.path(&format!("{name}.wav"));
        req.output = f.path(&format!("{name}.srt"));
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            source,
            "-t",
            "3",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            &req.source
        ]);
        let job = Job::transcribe_with_worker(req.clone(), f.exe.clone(), || {});
        let events = terminal(&job);
        let result = success(&events);
        assert!(
            result.track.cues.is_empty(),
            "Silence/noise must not invent speech"
        );
        assert!(
            result.track.language.is_none(),
            "No detected language for an empty track"
        );
        assert_eq!(result.track.duration_ms, 3000);
        drop(job);
        let again = Job::transcribe_with_worker(req, f.exe.clone(), || {});
        assert!(
            matches!(terminal(&again).last(),Some(Event::Subtitled(Err(e))) if e.code=="error.output_exists")
        );
    }
    let mut req = runtime(&f);
    req.source = f.path("silent-video.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=black:s=64x64:r=10:d=1",
        "-an",
        "-c:v",
        "libx264",
        &req.source
    ]);
    let job = Job::transcribe_with_worker(req, f.exe.clone(), || {});
    let events = terminal(&job);
    assert!(
        matches!(events.last(), Some(Event::Subtitled(Err(e))) if e.code=="error.subtitle_input"),
        "{events:?}"
    );
    drop(job);
    for bad in ["language", "model"] {
        let mut req = runtime(&f);
        req.source = f.path("silence.wav");
        if bad == "language" {
            req.language = "zz".into();
        } else {
            req.model = f.path("bad-model.bin");
            std::fs::write(&req.model, b"invalid model").unwrap();
        }
        let job = Job::transcribe_with_worker(req, f.exe.clone(), || {});
        let events = terminal(&job);
        assert!(
            matches!(events.last(), Some(Event::Subtitled(Err(_)))),
            "{bad}: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Progress { percent: 100, .. }))
        );
        drop(job);
    }
    f.clean();
    assert!(!f.path("captions.srt").exists());
}

#[test]
#[ignore = "requires runtime/model/VAD variables plus NOH_SUBTITLE_SPEECH_FR: documented synthetic French fixture"]
fn real_speech_unicode_video_audio_cli_and_gui_workers() {
    let f = Fixture::new();
    let folder = f.path("échantillon 日本語");
    std::fs::create_dir(&folder).unwrap();
    let source = folder.join("entrée.wav");
    std::fs::copy(required("NOH_SUBTITLE_SPEECH_FR"), &source).unwrap();
    let original = std::fs::read(&source).unwrap();
    let mut req = runtime(&f);
    req.source = source.clone();
    req.language = "fr".into();
    for (from, to) in [
        (&req.model, folder.join("modèle.bin")),
        (&req.vad_model, folder.join("détecteur.bin")),
    ] {
        std::fs::hard_link(from, &to)
            .or_else(|_| std::fs::copy(from, &to).map(|_| ()))
            .unwrap();
    }
    req.model = folder.join("modèle.bin");
    req.vad_model = folder.join("détecteur.bin");
    let workers = vec![("cli", f.exe.clone())];
    #[cfg(feature = "gui")]
    let workers = workers
        .into_iter()
        .chain(std::iter::once((
            "gui",
            std::env::var_os("NOH_APP_EXE")
                .map(PathBuf::from)
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_noh-app").into()),
        )))
        .collect::<Vec<_>>();
    for (name, worker) in workers {
        req.output = folder.join(format!("sous-titres-{name}.srt"));
        let job = Job::transcribe_with_worker(req.clone(), worker, || {});
        let events = terminal(&job);
        let result = success(&events);
        let text = result
            .track
            .cues
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert!(
            text.contains("jardin") && text.contains("français"),
            "{text}"
        );
        assert_eq!(result.track.language.as_deref(), Some("fr"));
        assert!(result.track.cues[0].start_ms < 1000);
        let converted = folder.join(format!("{name}.vtt"));
        f.ff(args!["-i", &req.output, "-c:s", "webvtt", &converted]);
        assert!(
            std::fs::read_to_string(converted)
                .unwrap()
                .contains("français")
        );
        drop(job);
    }
    for delay in [0, 3] {
        let video = folder.join(format!("vidéo-{delay}.mp4"));
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            "color=black:s=64x64:r=10:d=16",
            "-itsoffset",
            delay,
            "-i",
            &source,
            "-shortest",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            &video
        ]);
        req.output = folder.join(format!("vidéo-{delay}.srt"));
        let output = f.cli(args![
            "--transcribe",
            &video,
            "--output",
            &req.output,
            "--ffmpeg",
            &req.ffmpeg,
            "--transcriber",
            &req.transcriber,
            "--model",
            &req.model,
            "--vad-model",
            &req.vad_model,
            "--language",
            "fr",
            "--json"
        ]);
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["result"]["event"], "subtitled");
        let cues = value["result"]["data"]["Ok"]["track"]["cues"]
            .as_array()
            .unwrap();
        assert!(!cues.is_empty());
        let first = cues[0]["start_ms"].as_u64().unwrap();
        if delay == 3 {
            assert!(
                (2850..=3500).contains(&first),
                "Audio offset was lost: first cue {first} ms"
            );
        } else {
            assert!(first < 1000);
        }
    }
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert!(!std::fs::read_dir(&folder).unwrap().any(|p| {
        p.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".noh-")
    }));
    f.clean();
}

#[test]
#[ignore = "requires speech runtime and synthetic French fixture; checks music and late audio windows"]
fn real_speech_with_music_keeps_late_words_on_the_source_clock() {
    let f = Fixture::new();
    let mut req = runtime(&f);
    req.language = "fr".into();
    // A second utterance well past Whisper's first 30-second window, with a
    // continuous instrumental bed. Fixtures stay synthetic and repository-free.
    f.ff(args![
        "-i", required("NOH_SUBTITLE_SPEECH_FR"),
        "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=16000:duration=58",
        "-filter_complex",
        "[0:a]asplit=2[a][b];[a]adelay=3000:all=1[a0];[b]adelay=45000:all=1[b0];[1:a]volume=0.12[m];[a0][b0][m]amix=inputs=3:normalize=0:duration=longest[out]",
        "-map", "[out]", "-ac", "1", "-c:a", "pcm_s16le", &req.source
    ]);
    let job = Job::transcribe_with_worker(req, f.exe.clone(), || {});
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        let done = event.is_terminal();
        events.push(event);
        if done {
            break;
        }
    }
    let result = success(&events);
    let late = result
        .track
        .cues
        .iter()
        .filter(|cue| cue.start_ms >= 42000)
        .map(|cue| cue.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        late.contains("jardin") && late.contains("français"),
        "Late speech was lost: {late}"
    );
    assert!(result.track.cues.last().unwrap().end_ms >= 54000);
    assert_eq!(result.track.duration_ms, 58000);
    let measured: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::Progress { percent, phase }
                if phase.code.starts_with("subtitle.transcribing") && *percent > 0 =>
            {
                Some(*percent)
            }
            _ => None,
        })
        .collect();
    assert!(
        measured.iter().any(|p| *p < 99),
        "No intermediate Whisper progress: {measured:?}"
    );
    assert!(measured.windows(2).all(|p| p[0] <= p[1]));
    assert!(measured.iter().all(|p| *p < 100));
    drop(job);
    f.clean();
}

#[test]
#[ignore = "opt-in external transcription benchmark; runtime/model/VAD and synthetic French fixture required"]
fn transcription_benchmark() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new();
    let mut req = runtime(&f);
    req.source = required("NOH_SUBTITLE_SPEECH_FR");
    let mut report = benchmark_support::context(&f);
    let hash = |p: &std::path::Path| format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()));
    report["fixture_sha256"] = hash(&req.source).into();
    report["model_sha256"] = hash(&req.model).into();
    report["vad_sha256"] = hash(&req.vad_model).into();
    report["transcriber_sha256"] = hash(&req.transcriber).into();
    let mut libraries = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(req.transcriber.parent().unwrap()).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if entry.file_type().unwrap().is_file()
            && (lower.ends_with(".dll") || lower.contains(".so") || lower.ends_with(".dylib"))
        {
            libraries.insert(name, hash(&entry.path()));
        }
    }
    report["runtime_library_sha256"] = serde_json::json!(libraries);
    report["language"] = "fr".into();
    report["threads"] = std::thread::available_parallelism()
        .unwrap()
        .get()
        .min(4)
        .into();
    req.language = "fr".into();
    let mut times = Vec::new();
    for index in 0..4 {
        req.output = f.path(&format!("take-{index}.srt"));
        let start = Instant::now();
        let job = Job::transcribe_with_worker(req.clone(), f.exe.clone(), || {});
        let events = terminal(&job);
        let elapsed = start.elapsed().as_secs_f64();
        let result = success(&events);
        assert!(!result.track.cues.is_empty());
        report["duration_ms"] = result.track.duration_ms.into();
        if index > 0 {
            times.push(elapsed);
        }
        drop(job);
    }
    let mut ordered = times.clone();
    ordered.sort_by(f64::total_cmp);
    report["timings_seconds"] = serde_json::json!(times);
    report["median_seconds"] = ordered[1].into();
    report["description"]="One warmup, three sequential complete jobs; synthetic French speech, local CPU, VAD, staging and no-overwrite publication included. No concurrent build/capture.".into();
    benchmark_support::save(
        &report,
        "NOH_SUBTITLE_BENCHMARK_REPORT",
        "target/benchmarks/subtitles.json",
    );
}
