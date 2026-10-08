//! Bounded, optional waveform analysis. Decoding and file checks belong off the UI thread.
//!
//! FFmpeg preserves sample rate and channels; reduction takes the largest absolute
//! channel value per frame, so opposite-phase stereo cannot disappear. Only a
//! multiresolution amplitude envelope is retained, never decoded audio. File stamps
//! are invalidation hints, with the same metadata-preserving-edit limitation as export.
use crate::inspection::{FileStamp, resolve_ffmpeg};
use std::{
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Less than 2 MiB of retained amplitudes across all pyramid levels.
pub const MAX_BASE_BINS: usize = 262_144;
/// A display request cannot allocate an unbounded number of columns.
pub const MAX_DISPLAY_COLUMNS: usize = 16_384;
const DECODE_DEADLINE: Duration = Duration::from_secs(15 * 60);
const DIAGNOSTIC_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WaveformError {
    #[error("Waveform analysis cancelled")]
    Cancelled,
    #[error("The soundtrack or FFmpeg changed during waveform analysis")]
    SourceChanged,
    #[error("Waveform unavailable: {0}")]
    Unavailable(String),
}
type Result<T> = std::result::Result<T, WaveformError>;
fn unavailable(error: impl std::fmt::Display) -> WaveformError {
    WaveformError::Unavailable(error.to_string())
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(WaveformError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub struct Waveform {
    pub source: FileStamp,
    pub decoder: FileStamp,
    pub duration: f64,
    sample_rate: u32,
    frames_per_bin: u64,
    levels: Vec<Box<[f32]>>,
}
impl Waveform {
    /// The finest available temporal precision; longer files use wider bins.
    pub fn finest_seconds(&self) -> f64 {
        self.frames_per_bin as f64 / f64::from(self.sample_rate)
    }

    /// Retained amplitude storage, excluding small metadata and allocation headers.
    pub fn retained_bytes(&self) -> usize {
        self.levels.iter().map(|level| size_of_val(&**level)).sum()
    }

    /// Maximum magnitude of bins intersecting [start, end), on the WAV clock.
    /// Amplitudes are clamped to [0, 1]. No allocation, file IO, or decoding occurs.
    /// A query narrower than the finest bin conservatively includes that whole bin.
    pub fn peak_at(&self, start: f64, end: f64) -> f32 {
        if !start.is_finite() || !end.is_finite() || start >= end || self.levels.is_empty() {
            return 0.0;
        }
        let start = start.clamp(0.0, self.duration);
        let end = end.clamp(0.0, self.duration);
        if start >= end {
            return 0.0;
        }
        let step = self.finest_seconds();
        let mut left = (start / step).floor() as usize;
        let mut right = (end / step).ceil() as usize;
        left = left.min(self.levels[0].len());
        right = right.min(self.levels[0].len());
        let mut peak = 0.0_f32;
        let mut level = 0;
        // Exact range maximum over base bins using the existing pyramid.
        while left < right {
            if left & 1 != 0 {
                peak = peak.max(self.levels[level][left]);
                left += 1;
            }
            if right & 1 != 0 {
                right -= 1;
                peak = peak.max(self.levels[level][right]);
            }
            left /= 2;
            right /= 2;
            level += 1;
        }
        peak
    }

    /// Reuses the envelope for resize, zoom and selection changes. The return
    /// length is capped at MAX_DISPLAY_COLUMNS; callers should use its actual length.
    pub fn peaks(&self, start: f64, end: f64, columns: usize) -> Vec<f32> {
        let columns = columns.min(MAX_DISPLAY_COLUMNS);
        if columns == 0 || !start.is_finite() || !end.is_finite() || start >= end {
            return Vec::new();
        }
        let width = (end - start) / columns as f64;
        (0..columns)
            .map(|column| {
                self.peak_at(
                    start + column as f64 * width,
                    start + (column + 1) as f64 * width,
                )
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct WaveformReply {
    pub revision: u64,
    pub path: PathBuf,
    pub result: Result<Arc<Waveform>>,
}
struct Request {
    revision: u64,
    path: PathBuf,
    ffmpeg: PathBuf,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
struct Mailbox {
    revision: u64,
    pending: Option<Request>,
    reply: Option<WaveformReply>,
    cancel: Arc<AtomicBool>,
    shutdown: bool,
}

/// One serial decoder, one coalesced pending request and one reply. The cache
/// retains only the current source identity. Keep the returned Arc in UI state;
/// request again only for source/decoder edits or periodic off-thread stamp checks.
/// Range, zoom and resize changes need no worker request.
pub struct WaveformWorker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
}
impl WaveformWorker {
    pub fn new(repaint: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let worker = shared.clone();
        thread::spawn(move || worker_loop(worker, repaint));
        Self { shared }
    }

    /// No file IO. A newer request cancels an active decoder and replaces pending
    /// work. Repeating an unchanged source rechecks stamps and reuses the same Arc.
    pub fn request(&self, path: PathBuf, ffmpeg: PathBuf) -> u64 {
        let (lock, wake) = &*self.shared;
        let mut mailbox = lock.lock().unwrap();
        mailbox.cancel.store(true, Ordering::Relaxed);
        mailbox.cancel = Arc::new(AtomicBool::new(false));
        mailbox.revision = mailbox.revision.wrapping_add(1);
        mailbox.reply = None;
        let revision = mailbox.revision;
        mailbox.pending = Some(Request {
            revision,
            path,
            ffmpeg,
            cancel: mailbox.cancel.clone(),
        });
        wake.notify_one();
        revision
    }

    /// Invalidates pending and completed replies immediately. Process termination
    /// and reaping happen on the worker, without blocking the caller.
    pub fn cancel(&self) {
        let mut mailbox = self.shared.0.lock().unwrap();
        mailbox.cancel.store(true, Ordering::Relaxed);
        mailbox.revision = mailbox.revision.wrapping_add(1);
        mailbox.pending = None;
        mailbox.reply = None;
    }

    /// Only a reply for the latest request can become current.
    pub fn try_recv(&self) -> Option<WaveformReply> {
        let mut mailbox = self.shared.0.lock().unwrap();
        let revision = mailbox.revision;
        mailbox
            .reply
            .take()
            .filter(|reply| reply.revision == revision)
    }
}
impl Drop for WaveformWorker {
    fn drop(&mut self) {
        self.cancel();
        self.shared.0.lock().unwrap().shutdown = true;
        self.shared.1.notify_one();
        // Deliberately do not join on the UI thread. The owned worker receives the
        // cancellation, kills/reaps its process tree and exits without further work.
    }
}

fn identity(path: &Path, ffmpeg: &Path) -> Result<(FileStamp, FileStamp)> {
    let decoder = resolve_ffmpeg(ffmpeg).map_err(unavailable)?;
    Ok((
        FileStamp::read(path).map_err(unavailable)?,
        FileStamp::read(&decoder).map_err(unavailable)?,
    ))
}

fn worker_loop(shared: Arc<(Mutex<Mailbox>, Condvar)>, repaint: impl Fn()) {
    let mut cache: Option<Arc<Waveform>> = None;
    loop {
        let request = {
            let mut mailbox = shared.0.lock().unwrap();
            while mailbox.pending.is_none() && !mailbox.shutdown {
                mailbox = shared.1.wait(mailbox).unwrap();
            }
            if mailbox.shutdown {
                return;
            }
            mailbox.pending.take().unwrap()
        };
        let result = (|| {
            check_cancel(&request.cancel)?;
            let (source, decoder) = identity(&request.path, &request.ffmpeg)?;
            if let Some(waveform) = &cache
                && waveform.source == source
                && waveform.decoder == decoder
            {
                check_cancel(&request.cancel)?;
                return Ok(waveform.clone());
            }
            cache = None;
            let waveform = Arc::new(extract_identified(source, decoder, &request.cancel)?);
            cache = Some(waveform.clone());
            Ok(waveform)
        })();
        let delivered = {
            let mut mailbox = shared.0.lock().unwrap();
            if mailbox.shutdown {
                return;
            }
            if mailbox.revision == request.revision {
                mailbox.reply = Some(WaveformReply {
                    revision: request.revision,
                    path: request.path,
                    result,
                });
                true
            } else {
                false
            }
        };
        if delivered {
            repaint();
        }
    }
}

/// Synchronous embedding/test entry point. Call off-thread; use WaveformWorker
/// in interactive callers. Cancellation also kills a blocked FFmpeg process tree.
pub fn extract(path: &Path, ffmpeg: &Path, cancel: &AtomicBool) -> Result<Waveform> {
    check_cancel(cancel)?;
    let (source, decoder) = identity(path, ffmpeg)?;
    extract_identified(source, decoder, cancel)
}

fn extract_identified(
    source: FileStamp,
    decoder: FileStamp,
    cancel: &AtomicBool,
) -> Result<Waveform> {
    check_cancel(cancel)?;
    let duration = crate::wav_duration(&source.resolved).map_err(unavailable)?;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(unavailable("The soundtrack has no audio frames"));
    }
    let mut command = crate::command(&decoder.resolved);
    command
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-threads",
            "1",
            "-i",
        ])
        .arg(&source.resolved)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-map_metadata",
            "-1",
            "-c:a",
            "pcm_f32le",
            "-threads",
            "1",
            "-f",
            "wav",
            "-rf64",
            "never",
            "pipe:1",
        ]);
    let envelope = decode(command, duration, cancel, DECODE_DEADLINE)?;
    check_cancel(cancel)?;
    if FileStamp::read(&source.path).as_ref() != Ok(&source)
        || FileStamp::read(&decoder.path).as_ref() != Ok(&decoder)
    {
        return Err(WaveformError::SourceChanged);
    }
    Ok(Waveform {
        source,
        decoder,
        duration,
        sample_rate: envelope.sample_rate,
        frames_per_bin: envelope.frames_per_bin,
        levels: pyramid(envelope.peaks),
    })
}

fn decode(
    mut command: std::process::Command,
    duration: f64,
    cancel: &AtomicBool,
    deadline: Duration,
) -> Result<Envelope> {
    check_cancel(cancel)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::process::Tree::spawn(command, true).map_err(unavailable)?;
    let stdout = child
        .0
        .stdout()
        .take()
        .ok_or_else(|| unavailable("Missing decoder output"))?;
    let stderr = child
        .0
        .stderr()
        .take()
        .ok_or_else(|| unavailable("Missing decoder diagnostics"))?;
    // Each reader has one bounded result, not a stream of queued sample buffers.
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let _ = tx.send(read_envelope(stdout, duration));
    });
    let diagnostics = thread::spawn(move || read_diagnostics(stderr));
    let started = Instant::now();
    let outcome = (|| {
        let mut decoded = None;
        let mut status = None;
        loop {
            check_cancel(cancel)?;
            if started.elapsed() >= deadline {
                return Err(unavailable("The waveform decoder exceeded its deadline"));
            }
            if decoded.is_none() {
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(result) => decoded = Some(result?),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => return Err(unavailable("The waveform reader stopped")),
                }
            } else {
                thread::sleep(Duration::from_millis(20));
            }
            if status.is_none() {
                status = child.0.try_wait().map_err(unavailable)?;
            }
            if let Some(status) = status
                && decoded.is_some()
            {
                if !status.success() {
                    return Err(unavailable(format!("FFmpeg exited with {status}")));
                }
                return Ok(decoded.take().unwrap());
            }
        }
    })();
    // Kill on any early exit; also close inherited pipes before joining readers.
    drop(child);
    let _ = reader.join();
    let log = diagnostics.join().unwrap_or_default();
    outcome.map_err(|error| match error {
        WaveformError::Unavailable(detail) if !log.is_empty() => {
            unavailable(format!("{detail}: {}", log.trim()))
        }
        other => other,
    })
}

fn read_diagnostics(mut reader: impl Read) -> String {
    let mut tail = Vec::with_capacity(DIAGNOSTIC_BYTES);
    let mut buffer = [0; 4096];
    while let Ok(count) = reader.read(&mut buffer) {
        if count == 0 {
            break;
        }
        let remove = (tail.len() + count).saturating_sub(DIAGNOSTIC_BYTES);
        tail.drain(..remove);
        tail.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8_lossy(&tail).into_owned()
}

fn pyramid(peaks: Vec<f32>) -> Vec<Box<[f32]>> {
    let mut levels = vec![peaks.into_boxed_slice()];
    while levels.last().unwrap().len() > 1 {
        let next = levels
            .last()
            .unwrap()
            .chunks(2)
            .map(|pair| pair[0].max(pair.get(1).copied().unwrap_or(0.0)))
            .collect::<Vec<_>>();
        levels.push(next.into_boxed_slice());
    }
    levels
}

#[derive(Debug)]
struct Envelope {
    sample_rate: u32,
    frames_per_bin: u64,
    peaks: Vec<f32>,
}

/// Parse only our decoder's float-WAV pipe header. Source validation remains in
/// wav.rs; there is no second source WAV parser. Header reads/skips total <=64 KiB.
fn output_format(reader: &mut impl Read) -> Result<(u32, u16)> {
    let mut header = [0; 12];
    reader.read_exact(&mut header).map_err(unavailable)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err(unavailable("Expected a float-WAV decoder stream"));
    }
    let mut remaining = 65_536_usize - 12;
    let mut format = None;
    loop {
        if remaining < 8 {
            return Err(unavailable("Decoder header exceeds its size limit"));
        }
        let mut chunk = [0; 8];
        reader.read_exact(&mut chunk).map_err(unavailable)?;
        remaining -= 8;
        if &chunk[..4] == b"data" {
            return format.ok_or_else(|| unavailable("Decoder stream has no format"));
        }
        let size = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as usize;
        let padded = size
            .checked_add(size & 1)
            .ok_or_else(|| unavailable("Invalid decoder chunk"))?;
        if padded > remaining {
            return Err(unavailable("Decoder header exceeds its size limit"));
        }
        remaining -= padded;
        if &chunk[..4] == b"fmt " {
            let mut fmt = [0; 40];
            let read = size.min(fmt.len());
            reader.read_exact(&mut fmt[..read]).map_err(unavailable)?;
            if size < 16 {
                return Err(unavailable("Incomplete decoder format"));
            }
            let codec = u16::from_le_bytes(fmt[..2].try_into().unwrap());
            let channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
            let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
            let align = u16::from_le_bytes(fmt[12..14].try_into().unwrap());
            let bits = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
            let float = codec == 3
                || (codec == 0xfffe
                    && size >= 40
                    && fmt[24..40] == [3, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]);
            if !float
                || channels == 0
                || rate == 0
                || bits != 32
                || u32::from(align) != u32::from(channels) * 4
            {
                return Err(unavailable(
                    "Decoder must preserve channels as 32-bit float samples",
                ));
            }
            format = Some((rate, channels));
            skip(reader, padded - read)?;
        } else {
            skip(reader, padded)?;
        }
    }
}
fn skip(reader: &mut impl Read, mut bytes: usize) -> Result<()> {
    let mut buffer = [0; 4096];
    while bytes != 0 {
        let count = bytes.min(buffer.len());
        reader
            .read_exact(&mut buffer[..count])
            .map_err(unavailable)?;
        bytes -= count;
    }
    Ok(())
}

fn read_envelope(reader: impl Read, duration: f64) -> Result<Envelope> {
    let mut reader = BufReader::with_capacity(32_768, reader);
    let (sample_rate, channels) = output_format(&mut reader)?;
    let expected_float = duration * f64::from(sample_rate);
    if !expected_float.is_finite() || expected_float < 1.0 || expected_float > (1_u64 << 53) as f64
    {
        return Err(unavailable("Invalid decoded frame count"));
    }
    let expected = expected_float.round() as u64;
    let frames_per_bin = expected
        .div_ceil(MAX_BASE_BINS as u64)
        .max(u64::from(sample_rate).div_ceil(1000))
        .max(1);
    let bins = expected.div_ceil(frames_per_bin) as usize;
    let mut peaks = vec![0.0_f32; bins];
    let mut buffer = [0_u8; 32_768];
    let (mut pending, mut frame, mut channel) = (0, 0_u64, 0_u16);
    loop {
        let count = reader.read(&mut buffer[pending..]).map_err(unavailable)?;
        if count == 0 {
            break;
        }
        let used = pending + count;
        let complete = used / 4 * 4;
        for bytes in buffer[..complete].as_chunks::<4>().0 {
            if frame >= expected {
                return Err(unavailable(
                    "Decoded waveform exceeds the validated WAV frame count",
                ));
            }
            let sample = f32::from_le_bytes(*bytes);
            let amplitude = if sample.is_nan() {
                0.0
            } else {
                sample.abs().min(1.0)
            };
            let bin = (frame / frames_per_bin) as usize;
            peaks[bin] = peaks[bin].max(amplitude);
            channel += 1;
            if channel == channels {
                channel = 0;
                frame += 1;
            }
        }
        pending = used - complete;
        buffer.copy_within(complete..used, 0);
    }
    if pending != 0 || channel != 0 || frame != expected {
        return Err(unavailable(
            "Decoded waveform is incomplete or has a different duration",
        ));
    }
    Ok(Envelope {
        sample_rate,
        frames_per_bin,
        peaks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn float_stream(rate: u32, channels: u16, values: &[f32]) -> Vec<u8> {
        let mut bytes = b"RIFF\xff\xff\xff\xffWAVEfmt \x10\0\0\0".to_vec();
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(channels) * 4).to_le_bytes());
        bytes.extend_from_slice(&(channels * 4).to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(b"data\xff\xff\xff\xff");
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn waveform(peaks: Vec<f32>) -> Waveform {
        let stamp = FileStamp {
            path: "test.wav".into(),
            resolved: "test.wav".into(),
            bytes: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            created: None,
        };
        Waveform {
            source: stamp.clone(),
            decoder: stamp,
            duration: peaks.len() as f64,
            sample_rate: 1,
            frames_per_bin: 1,
            levels: pyramid(peaks),
        }
    }

    #[test]
    fn silence_impulse_and_opposite_channels_retain_their_time_and_magnitude() {
        let mut samples = vec![0.0; 2000];
        samples[500] = 0.75;
        samples[501] = -0.75;
        let result = read_envelope(Cursor::new(float_stream(1000, 2, &samples)), 1.0).unwrap();
        assert_eq!(result.peaks.len(), 1000);
        assert_eq!(result.peaks[250], 0.75);
        assert_eq!(result.peaks.iter().filter(|p| **p != 0.0).count(), 1);
    }

    #[test]
    fn range_queries_are_exact_over_bins_and_clamp_outside_the_source() {
        let waveform = waveform(vec![0.1, 0.4, 0.3, 0.9, 0.2, 0.6, 0.0]);
        for left in 0..7 {
            for right in left + 1..=7 {
                let expected = waveform.levels[0][left..right]
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max);
                assert_eq!(waveform.peak_at(left as f64, right as f64), expected);
            }
        }
        assert_eq!(waveform.peak_at(-5.0, 1.0), 0.1);
        assert_eq!(waveform.peak_at(7.0, 9.0), 0.0);
        assert_eq!(waveform.peak_at(f64::NAN, 9.0), 0.0);
        assert_eq!(waveform.peaks(0.0, 6.0, 3), [0.4, 0.9, 0.6]);
        assert_eq!(
            waveform.peaks(0.0, 7.0, usize::MAX).len(),
            MAX_DISPLAY_COLUMNS
        );
    }

    #[test]
    fn pyramid_storage_and_sparse_long_source_reduction_are_bounded() {
        let seconds = MAX_BASE_BINS + 17;
        let mut samples = vec![0.0; seconds];
        samples[seconds - 1] = 1.0;
        let envelope =
            read_envelope(Cursor::new(float_stream(1, 1, &samples)), seconds as f64).unwrap();
        assert!(envelope.peaks.len() <= MAX_BASE_BINS);
        assert_eq!(envelope.peaks.last(), Some(&1.0));
        let waveform = waveform(envelope.peaks);
        assert!(waveform.retained_bytes() <= 2 * MAX_BASE_BINS * size_of::<f32>() + 80);
        assert_eq!(waveform.peak_at(0.0, waveform.duration), 1.0);
    }

    #[test]
    fn malformed_headers_truncated_audio_and_nonfinite_samples_are_bounded() {
        let mut too_large = b"RIFF\xff\xff\xff\xffWAVEJUNK\xff\xff\xff\x7f".to_vec();
        assert!(output_format(&mut Cursor::new(&too_large)).is_err());
        too_large = float_stream(1, 1, &[1.0]);
        too_large.pop();
        assert!(read_envelope(Cursor::new(too_large), 1.0).is_err());
        let stream = float_stream(4, 1, &[f32::NAN, f32::INFINITY, -2.0, 0.0]);
        assert_eq!(
            read_envelope(Cursor::new(stream), 1.0).unwrap().peaks,
            [0.0, 1.0, 1.0, 0.0]
        );
    }

    #[test]
    fn already_cancelled_extraction_does_not_touch_missing_paths() {
        assert_eq!(
            extract(
                Path::new("missing.wav"),
                Path::new("missing.exe"),
                &AtomicBool::new(true)
            )
            .unwrap_err(),
            WaveformError::Cancelled,
        );
    }

    #[test]
    fn cancelled_and_obsolete_mailbox_replies_cannot_become_current() {
        let worker = WaveformWorker {
            shared: Arc::new((Mutex::new(Mailbox::default()), Condvar::new())),
        };
        let revision = worker.request("first.wav".into(), "ffmpeg".into());
        worker.request("second.wav".into(), "ffmpeg".into());
        worker.shared.0.lock().unwrap().reply = Some(WaveformReply {
            revision,
            path: "first.wav".into(),
            result: Err(WaveformError::Cancelled),
        });
        assert!(worker.try_recv().is_none());
        worker.cancel();
        assert!(worker.shared.0.lock().unwrap().pending.is_none());
        assert!(worker.try_recv().is_none());
    }

    #[cfg(any(windows, unix))]
    fn blocked_decoder(ready: &Path) -> std::process::Command {
        #[cfg(windows)]
        let mut command = {
            let mut command = crate::command("powershell.exe");
            command.args([
                "-NoProfile", "-NonInteractive", "-Command",
                "[IO.File]::WriteAllText($env:NOH_WAVEFORM_READY, 'ready'); Start-Sleep -Seconds 60",
            ]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = crate::command("/bin/sh");
            command.args(["-c", "printf ready > \"$NOH_WAVEFORM_READY\"; sleep 60"]);
            command
        };
        command.env("NOH_WAVEFORM_READY", ready);
        command
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn active_decoder_with_blocked_stdout_is_cancelled_and_reaped() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        let command = blocked_decoder(&ready);
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let task = thread::spawn(move || decode(command, 1.0, &signal, Duration::from_secs(10)));
        let started = Instant::now();
        while !ready.exists() && started.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        let confirmed_running = ready.exists();
        let cancelled = Instant::now();
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(task.join().unwrap().unwrap_err(), WaveformError::Cancelled);
        assert!(confirmed_running, "Decoder fixture never started");
        assert!(cancelled.elapsed() < Duration::from_secs(3));
        // decode joins both pipe readers after process-tree Drop. Returning here
        // proves blocked readers cannot outlive cancellation waiting on open pipes.
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn a_decoder_that_never_produces_audio_has_a_total_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = decode(
            blocked_decoder(&directory.path().join("ready")),
            1.0,
            &AtomicBool::new(false),
            Duration::from_millis(200),
        );
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
