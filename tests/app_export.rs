#![cfg(feature = "gui")]
#[allow(dead_code)]
#[path = "../src/app_job.rs"]
mod app_job;
#[allow(dead_code)]
#[path = "../src/app_media.rs"]
mod app_media;
use app_job::{Event, Job, Request};
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
#[path = "support/images.rs"]
#[allow(dead_code)]
mod images;
use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::Fixture;

fn app_worker() -> PathBuf {
    std::env::var_os("NOH_APP_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_noh-app")))
}

#[test]
fn image_metadata_and_timed_export_use_the_desktop_worker() {
    use noh::engine::MediaItem;
    let f = Fixture::new();
    let path = images::png(&f, "portrait Δ.png", 32, 64, |_, _| [255, 0, 0, 128], None);
    let worker = app_worker();
    let metadata = app_media::Metadata::with_worker(worker.clone());
    let ctx = eframe::egui::Context::default();
    metadata.request_media(
        1,
        MediaItem::Image {
            path: path.clone(),
            duration: 0.20,
        },
        Some(f.ffmpeg.clone()),
        &ctx,
    );
    metadata.retain([1].into_iter());
    let reply = metadata
        .events
        .recv_timeout(Duration::from_secs(15))
        .expect("Image metadata deadline");
    assert!(!reply.wav);
    let info = reply
        .result
        .unwrap_or_else(|e| panic!("{}: {}", e.key, e.detail));
    assert_eq!((info.width, info.height), (32, 64));
    assert_eq!(
        info.seconds, 0.0,
        "Intrinsic metadata must not cache a row duration"
    );
    drop(metadata);
    let request = Request {
        items: vec![MediaItem::Image {
            path,
            duration: 0.20,
        }],
        wav: f.wav("audio.wav", 0.36),
        output: f.path("desktop image.mp4"),
        ffmpeg: f.ffmpeg.clone(),
        clip_audio: false,
        fade_in: 0.0,
        fade_out: 0.0,
        partial_fades: true,
        force_encode: false,
        preview: false,
    };
    let job = Job::start_with_worker(request.clone(), worker, || {});
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Image export deadline")
        {
            Event::Done(result) => {
                assert_eq!(result.unwrap().output, request.output);
                break;
            }
            Event::Cancelled => panic!("Unexpected image export cancellation"),
            _ => {}
        }
    }
    drop(job);
    let frames = images::rgb(&f, &request.output);
    assert_eq!(frames.len(), 32 * 64 * 3 * 9);
    images::close(images::pixel(&frames, 32, 64, 7, 16, 32), [128, 0, 0], 12);
    f.clean();
}

#[test]
fn exports_reports_failures_and_cancels_the_process_group() {
    let root = std::env::temp_dir();
    let ffmpeg = noh::find_ffmpeg(None).unwrap();
    let folder = root.join(format!(
        ".app-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&folder).unwrap();
    let video = folder.join("video test Δ.mp4");
    let wav = folder.join("musique test.wav");
    let long_wav = folder.join("long.wav");
    let worker = app_worker();
    let generate = |args: &[&str], output: &PathBuf| {
        let result = noh::command(&ffmpeg)
            .args(["-v", "error", "-nostdin", "-n"])
            .args(args)
            .arg(output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    generate(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=96x64:r=25:d=1",
            "-c:v",
            "libx264",
            "-profile:v",
            "main",
            "-bf",
            "0",
        ],
        &video,
    );
    generate(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=duration=3.44",
            "-c:a",
            "pcm_s16le",
        ],
        &wav,
    );
    generate(
        &["-f", "lavfi", "-i", "sine=duration=60", "-c:a", "pcm_s16le"],
        &long_wav,
    );
    // Exercise the desktop scheduler with real workers. Successful Job teardown
    // must not discard replies by mistaking its cleanup flag for obsolescence.
    let metadata = app_media::Metadata::with_worker(worker.clone());
    let ctx = eframe::egui::Context::default();
    metadata.request(1, video.clone(), false, Some(ffmpeg.clone()), &ctx);
    metadata.request(2, wav.clone(), true, Some(ffmpeg.clone()), &ctx);
    metadata.retain([1, 2].into_iter());
    for expected in [1, 2] {
        let reply = metadata
            .events
            .recv_timeout(Duration::from_secs(10))
            .expect("Metadata reply lost after worker completion");
        assert_eq!(reply.id, expected);
        assert!(
            reply.result.is_ok(),
            "{}",
            reply.result.err().map(|e| e.detail).unwrap_or_default()
        );
    }
    drop(metadata);
    let request = |wav: PathBuf, output: PathBuf, partial: bool| Request {
        items: vec![video.clone().into()],
        clip_audio: false,
        wav,
        output,
        fade_in: 0.4,
        fade_out: 0.52,
        partial_fades: partial,
        force_encode: !partial,
        preview: false,
        ffmpeg: ffmpeg.clone(),
    };
    let output = folder.join("result Δ.mp4");
    let job = Job::start_with_worker(
        request(wav.clone(), output.clone(), true),
        worker.clone(),
        || {},
    );
    let mut got_progress = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Export bloque")
        {
            Event::Progress { .. } => got_progress = true,
            Event::Done(result) => {
                assert!(result.is_ok(), "{result:?}");
                break;
            }
            Event::Cancelled => panic!("Annulation inattendue"),
            _ => {}
        }
    }
    drop(job);
    assert!(got_progress && output.is_file());
    // Heterogeneous clips warn through the real GUI worker and still complete.
    let other = folder.join("autre codec.mp4");
    generate(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=128x72:r=30:d=1",
            "-c:v",
            "mpeg4",
        ],
        &other,
    );
    let adapted = folder.join("adapted Δ.mp4");
    let mut mixed = request(wav.clone(), adapted.clone(), true);
    mixed.items.push(other.into());
    let job = Job::start_with_worker(mixed, worker.clone(), || {});
    let mut warnings = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            Event::Warning(reason) => warnings.push(reason.to_wire()),
            Event::Done(result) => {
                assert_eq!(result.unwrap().output, adapted);
                break;
            }
            Event::Cancelled => panic!("Unexpected cancellation"),
            _ => {}
        }
    }
    assert_eq!(
        warnings,
        [
            "warning.harmonize|96|64|25.000",
            "warning.convert_clip|2|2|mpeg4"
        ]
    );
    assert!(adapted.is_file());
    drop(job);
    // Preview uses the same controller but remains separate from the final export.
    let final_bytes = std::fs::read(&output).unwrap();
    let preview = app_job::PreviewFile::in_folder(&folder).unwrap();
    let preview_path = preview.path.clone();
    let preview_folder = preview_path.parent().unwrap().to_path_buf();
    let mut preview_request = request(wav.clone(), preview_path.clone(), true);
    preview_request.preview = true;
    let job = Job::start_with_worker(preview_request, worker.clone(), || {});
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Event::Done(result) = job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            assert_eq!(result.unwrap().output, preview_path);
            break;
        }
    }
    drop(job);
    assert!(preview_path.is_file());
    assert_eq!(std::fs::read(&output).unwrap(), final_bytes);
    drop(preview);
    assert!(!preview_folder.exists());

    let failed_output = folder.join("echec.mp4");
    let job = Job::start_with_worker(
        request(folder.join("absent.wav"), failed_output.clone(), false),
        worker.clone(),
        || {},
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Event::Done(result) = job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            assert!(result.is_err());
            break;
        }
    }
    drop(job);
    assert!(!failed_output.exists());

    let cancelled_output = folder.join("annule.mp4");
    let job = Job::start_with_worker(
        request(long_wav, cancelled_output.clone(), false),
        worker,
        || {},
    );
    let mut requested = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match job
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Annulation bloquee")
        {
            Event::Progress { .. } if !requested => {
                job.cancel();
                requested = true;
            }
            Event::Cancelled => break,
            Event::Done(result) => {
                panic!("The job finished instead of being cancelled: {result:?}")
            }
            _ => {}
        }
    }
    drop(job);
    assert!(!cancelled_output.exists());
    assert!(
        !std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(".noh-"))
    );
    // This test generated all these files; no recursive cleanup.
    for entry in std::fs::read_dir(&folder).unwrap().flatten() {
        std::fs::remove_file(entry.path()).unwrap();
    }
    std::fs::remove_dir(folder).unwrap();
}
