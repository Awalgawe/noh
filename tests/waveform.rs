//! Real FFmpeg waveform qualification, using generated PCM/float RIFF/RF64/BW64.
use noh::waveform::{MAX_BASE_BINS, WaveformReply, WaveformWorker, extract};
use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::{Duration, Instant},
};

/// Sparse fixture: silence except for an exact one-frame opposite-phase impulse.
/// The ordinary suite uses one second; the long fixture is explicitly opt-in.
fn fixture(path: &Path, container: &[u8; 4], float: bool, bits: u16, seconds: u32, amplitude: f64) {
    let rate = 48_000_u32;
    let channels = 2_u16;
    let align = channels * (bits / 8);
    let data_size = u64::from(rate) * u64::from(seconds) * u64::from(align);
    let extended = container != b"RIFF";
    let total = 44 + data_size + if extended { 36 } else { 0 };
    let mut file = File::create(path).unwrap();
    file.write_all(container).unwrap();
    file.write_all(
        &(if extended {
            u32::MAX
        } else {
            (total - 8) as u32
        })
        .to_le_bytes(),
    )
    .unwrap();
    file.write_all(b"WAVE").unwrap();
    if extended {
        file.write_all(b"ds64\x1c\0\0\0").unwrap();
        file.write_all(&(total - 8).to_le_bytes()).unwrap();
        file.write_all(&data_size.to_le_bytes()).unwrap();
        file.write_all(&(u64::from(rate) * u64::from(seconds)).to_le_bytes())
            .unwrap();
        file.write_all(&0_u32.to_le_bytes()).unwrap();
    }
    file.write_all(b"fmt \x10\0\0\0").unwrap();
    file.write_all(&(if float { 3_u16 } else { 1 }).to_le_bytes())
        .unwrap();
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&rate.to_le_bytes()).unwrap();
    file.write_all(&(rate * u32::from(align)).to_le_bytes())
        .unwrap();
    file.write_all(&align.to_le_bytes()).unwrap();
    file.write_all(&bits.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&(if extended { u32::MAX } else { data_size as u32 }).to_le_bytes())
        .unwrap();
    let payload = file.stream_position().unwrap();
    file.set_len(total).unwrap();
    file.seek(SeekFrom::Start(
        payload + u64::from(rate / 4) * u64::from(align),
    ))
    .unwrap();
    for value in [amplitude, -amplitude] {
        match (float, bits) {
            (true, 32) => file.write_all(&(value as f32).to_le_bytes()).unwrap(),
            (true, 64) => file.write_all(&value.to_le_bytes()).unwrap(),
            (false, 16) => file
                .write_all(&((value * 32768.0) as i16).to_le_bytes())
                .unwrap(),
            (false, 24) => file
                .write_all(&((value * 8_388_608.0) as i32).to_le_bytes()[..3])
                .unwrap(),
            (false, 32) => file
                .write_all(&((value * 2_147_483_648.0) as i32).to_le_bytes())
                .unwrap(),
            _ => panic!("Unsupported test format"),
        }
    }
}

fn reply(worker: &WaveformWorker, wake: &mpsc::Receiver<()>) -> WaveformReply {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(reply) = worker.try_recv() {
            return reply;
        }
        wake.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Waveform worker deadline");
    }
}

#[test]
fn pcm_float_riff_rf64_and_bw64_preserve_one_sample_opposite_phase_impulses() {
    let directory = tempfile::tempdir().unwrap();
    let ffmpeg = noh::find_ffmpeg(None).unwrap();
    for container in [b"RIFF", b"RF64", b"BW64"] {
        for (float, bits) in [
            (false, 16),
            (false, 24),
            (false, 32),
            (true, 32),
            (true, 64),
        ] {
            let path = directory.path().join(format!(
                "{}-{float}-{bits}.wav",
                String::from_utf8_lossy(container)
            ));
            fixture(&path, container, float, bits, 1, 0.75);
            let waveform = extract(&path, &ffmpeg, &AtomicBool::new(false)).unwrap();
            assert_eq!(waveform.duration, 1.0);
            assert!(waveform.peak_at(0.250, 0.251) > 0.74, "{path:?}");
            assert_eq!(waveform.peak_at(0.0, 0.249), 0.0, "{path:?}");
            assert_eq!(waveform.peak_at(0.252, 1.0), 0.0, "{path:?}");
            assert_eq!(waveform.finest_seconds(), 0.001);
            assert!(waveform.retained_bytes() < 2 * MAX_BASE_BINS * size_of::<f32>());
        }
    }
}

#[test]
fn cached_interactions_reuse_the_arc_and_external_replacement_invalidates_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("current.wav");
    let ffmpeg = noh::find_ffmpeg(None).unwrap();
    fixture(&path, b"RIFF", false, 16, 1, 0.75);
    let (tx, wake) = mpsc::channel();
    let worker = WaveformWorker::new(move || {
        let _ = tx.send(());
    });
    let first_revision = worker.request(path.clone(), ffmpeg.clone());
    let first = reply(&worker, &wake);
    assert_eq!(first.revision, first_revision);
    let first = first.result.unwrap();
    for (start, end, columns) in [(0.0, 1.0, 640), (0.1, 0.5, 1920), (0.249, 0.252, 200)] {
        assert!(
            first
                .peaks(start, end, columns)
                .iter()
                .any(|peak| *peak > 0.74)
        );
    }
    worker.request(path.clone(), ffmpeg.clone());
    let cached = reply(&worker, &wake).result.unwrap();
    assert!(Arc::ptr_eq(&first, &cached));
    // Different length ensures identity changes even on a coarse timestamp filesystem.
    fixture(&path, b"RIFF", false, 16, 2, 0.25);
    worker.request(path.clone(), ffmpeg);
    let replaced = reply(&worker, &wake).result.unwrap();
    assert!(!Arc::ptr_eq(&first, &replaced));
    assert_ne!(first.source, replaced.source);
    assert_eq!(replaced.duration, 2.0);
    assert!((replaced.peak_at(0.25, 0.251) - 0.25).abs() < 0.001);
}

#[test]
fn pending_path_changes_and_cancellation_never_publish_the_old_source() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.wav");
    let second = directory.path().join("second.wav");
    let ffmpeg = noh::find_ffmpeg(None).unwrap();
    fixture(&first, b"RIFF", false, 16, 1, 0.75);
    fixture(&second, b"RIFF", false, 16, 2, 0.25);
    let (tx, wake) = mpsc::channel();
    let worker = WaveformWorker::new(move || {
        let _ = tx.send(());
    });
    worker.request(first, ffmpeg.clone());
    worker.cancel();
    let latest = worker.request(second.clone(), ffmpeg);
    let result = reply(&worker, &wake);
    assert_eq!(result.revision, latest);
    assert_eq!(result.path, second);
    assert_eq!(result.result.unwrap().duration, 2.0);
    assert!(worker.try_recv().is_none());
}

#[test]
#[ignore = "explicit performance campaign; creates a temporary 15-minute PCM WAV"]
fn waveform_benchmark() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("long.wav");
    let ffmpeg = noh::find_ffmpeg(None).unwrap();
    fixture(&path, b"RF64", false, 16, 900, 0.75);
    let started = Instant::now();
    let waveform = extract(&path, &ffmpeg, &AtomicBool::new(false)).unwrap();
    let cold_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let mut checksum = 0.0;
    for view in 0..1000 {
        let start = (view % 10) as f64 / 100.0;
        checksum += waveform.peaks(start, start + 1.0, 1920).iter().sum::<f32>();
    }
    let cached_interactions_seconds = started.elapsed().as_secs_f64();
    assert!(checksum > 0.0);
    assert!(waveform.retained_bytes() < 2 * MAX_BASE_BINS * size_of::<f32>());
    println!(
        "{}",
        serde_json::json!({
            "waveform_seconds": waveform.duration,
            "source_bytes": waveform.source.bytes,
            "cold_seconds": cold_seconds,
            "retained_amplitude_bytes": waveform.retained_bytes(),
            "finest_seconds": waveform.finest_seconds(),
            "cached_interactions": 1000,
            "columns": 1920,
            "cached_interactions_seconds": cached_interactions_seconds,
        })
    );
}
