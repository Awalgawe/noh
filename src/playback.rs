//! Bounded playback of a rendered preview. The GUI only edits a coalesced
//! command mailbox and reads snapshots; files, audio and subprocesses belong to
//! the worker. Video follows the audio source position whenever sound is usable.
use crate::{inspection::FileStamp, process::Tree};
use rodio::Source;
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const EDGE: usize = 640;
const FPS: f64 = 24.0;
const TICK: Duration = Duration::from_millis(12);
const DECODE_DEADLINE: Duration = Duration::from_secs(10);
const SEEK_SETTLE: Duration = Duration::from_millis(50);
const PREFETCH_LEAD: f64 = 0.5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Empty,
    Loading,
    Ready,
    Ended,
    Error,
}

pub struct Frame {
    pub id: u64,
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    pub position: f64,
}
impl std::fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("id", &self.id)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgb_bytes", &self.rgb.len())
            .field("position", &self.position)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub generation: u64,
    pub path: Option<PathBuf>,
    pub phase: Phase,
    pub duration: f64,
    pub position: f64,
    pub playing: bool,
    pub volume: f32,
    pub audio_available: bool,
    /// A silent fallback always has an explicit reason for the UI to display.
    pub warning: Option<String>,
    pub error: Option<String>,
    pub frame: Option<Arc<Frame>>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            generation: 0,
            path: None,
            phase: Phase::Empty,
            duration: 0.0,
            position: 0.0,
            playing: false,
            volume: 1.0,
            audio_available: false,
            warning: None,
            error: None,
            frame: None,
        }
    }
}

#[derive(Clone)]
struct Input {
    path: PathBuf,
    ffmpeg: PathBuf,
    project: Option<Arc<crate::project::ProjectRequest>>,
    cancel: Arc<AtomicBool>,
}
impl Input {
    fn audio_start(&self) -> f64 {
        self.project
            .as_ref()
            .and_then(|p| p.short.as_ref())
            .map_or(0.0, |s| s.start_ms as f64 / 1000.0)
    }
}
#[derive(Clone)]
struct Desired {
    generation: u64,
    input: Option<Input>,
    seek_id: u64,
    position: f64,
    seek_at: Instant,
    playing: bool,
    volume: f32,
    shutdown: bool,
}
struct Shared {
    desired: Mutex<Desired>,
    changed: Condvar,
    snapshot: Mutex<Snapshot>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl Shared {
    fn desired(&self) -> Desired {
        self.desired.lock().unwrap().clone()
    }
    fn current(&self, generation: u64) -> bool {
        let desired = self.desired.lock().unwrap();
        !desired.shutdown && desired.generation == generation
    }
    fn publish(&self, generation: u64, update: impl FnOnce(&mut Snapshot)) {
        let mut snapshot = self.snapshot.lock().unwrap();
        if snapshot.generation != generation {
            return;
        }
        update(&mut snapshot);
        drop(snapshot);
        (self.wake)();
    }
}

/// One worker and one command slot, including during slider dragging. Dropping
/// this handle requests cleanup without waiting on the GUI thread.
pub struct Player {
    shared: Arc<Shared>,
}
impl Player {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            desired: Mutex::new(Desired {
                generation: 0,
                input: None,
                seek_id: 0,
                position: 0.0,
                seek_at: Instant::now(),
                playing: false,
                volume: 1.0,
                shutdown: false,
            }),
            changed: Condvar::new(),
            snapshot: Mutex::new(Snapshot::default()),
            wake: Arc::new(wake),
        });
        let worker = shared.clone();
        if let Err(error) = thread::Builder::new()
            .name("noh-playback".into())
            .spawn(move || run(worker))
        {
            let mut snapshot = shared.snapshot.lock().unwrap();
            snapshot.phase = Phase::Error;
            snapshot.error = Some(format!("Cannot start preview playback: {error}"));
        }
        Self { shared }
    }

    pub fn open(&self, path: PathBuf, ffmpeg: PathBuf, start_seconds: f64, playing: bool) {
        self.open_input(
            Input {
                path,
                ffmpeg,
                project: None,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            start_seconds,
            playing,
        );
    }

    fn open_input(&self, input: Input, start_seconds: f64, playing: bool) {
        let mut desired = self.shared.desired.lock().unwrap();
        if let Some(input) = &desired.input {
            input.cancel.store(true, Ordering::Release);
        }
        desired.generation = desired.generation.wrapping_add(1);
        desired.seek_id = desired.seek_id.wrapping_add(1);
        let path = input.path.clone();
        desired.input = Some(input);
        desired.position = finite_seconds(start_seconds);
        desired.seek_at = Instant::now() - SEEK_SETTLE;
        desired.playing = playing;
        *self.shared.snapshot.lock().unwrap() = Snapshot {
            generation: desired.generation,
            path: Some(path),
            phase: Phase::Loading,
            position: desired.position,
            playing,
            volume: desired.volume,
            ..Snapshot::default()
        };
        drop(desired);
        self.shared.changed.notify_one();
    }

    /// Stream the originals at the local selection clock, without rendering an
    /// intermediate montage. The helper opens only the current visual piece.
    pub fn open_project(
        &self,
        request: crate::project::ProjectRequest,
        start_seconds: f64,
        playing: bool,
    ) {
        let path = request.montage.wav.clone();
        let ffmpeg = request.montage.ffmpeg.clone();
        self.open_input(
            Input {
                path,
                ffmpeg,
                project: Some(Arc::new(request)),
                cancel: Arc::new(AtomicBool::new(false)),
            },
            start_seconds,
            playing,
        );
    }

    pub fn set_playing(&self, playing: bool) {
        let mut desired = self.shared.desired.lock().unwrap();
        let mut snapshot = self.shared.snapshot.lock().unwrap();
        if playing && snapshot.phase == Phase::Ended {
            desired.position = 0.0;
            desired.seek_id = desired.seek_id.wrapping_add(1);
            desired.seek_at = Instant::now() - SEEK_SETTLE;
        }
        desired.playing = playing;
        snapshot.playing = playing;
        drop(snapshot);
        drop(desired);
        self.shared.changed.notify_one();
    }

    pub fn seek(&self, seconds: f64) {
        let mut desired = self.shared.desired.lock().unwrap();
        let mut snapshot = self.shared.snapshot.lock().unwrap();
        desired.position = finite_seconds(seconds);
        if snapshot.duration > 0.0 {
            desired.position = desired.position.min(snapshot.duration);
        }
        desired.seek_id = desired.seek_id.wrapping_add(1);
        desired.seek_at = Instant::now();
        snapshot.position = desired.position;
        drop(snapshot);
        drop(desired);
        self.shared.changed.notify_one();
    }

    pub fn set_volume(&self, volume: f32) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.shared.desired.lock().unwrap().volume = volume;
        self.shared.snapshot.lock().unwrap().volume = volume;
        self.shared.changed.notify_one();
    }

    pub fn stop(&self) {
        let mut desired = self.shared.desired.lock().unwrap();
        desired.generation = desired.generation.wrapping_add(1);
        if let Some(input) = &desired.input {
            input.cancel.store(true, Ordering::Release);
        }
        desired.input = None;
        desired.playing = false;
        *self.shared.snapshot.lock().unwrap() = Snapshot {
            generation: desired.generation,
            volume: desired.volume,
            ..Snapshot::default()
        };
        drop(desired);
        self.shared.changed.notify_one();
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot.lock().unwrap().clone()
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let mut desired = self.shared.desired.lock().unwrap();
        desired.shutdown = true;
        if let Some(input) = &desired.input {
            input.cancel.store(true, Ordering::Release);
        }
        drop(desired);
        self.shared.changed.notify_one();
    }
}

fn finite_seconds(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

struct Clock {
    position: f64,
    anchor: Instant,
    playing: bool,
}
impl Clock {
    fn new(position: f64) -> Self {
        Self {
            position,
            anchor: Instant::now(),
            playing: false,
        }
    }
    fn position(&self, now: Instant) -> f64 {
        self.position
            + if self.playing {
                now.saturating_duration_since(self.anchor).as_secs_f64()
            } else {
                0.0
            }
    }
    fn set(&mut self, position: f64, playing: bool, now: Instant) {
        self.position = position;
        self.playing = playing;
        self.anchor = now;
    }
}

struct Audio {
    player: rodio::Player,
    device: rodio::MixerDeviceSink,
    failure: Arc<Mutex<Option<String>>>,
    base: f64,
    mixed: bool,
}
type AudioSource = Box<dyn Source<Item = f32> + Send>;

enum Pcm {
    Samples(Vec<f32>),
    End,
    Error(String),
}
/// At most two seconds of interleaved PCM, plus the current 43 ms chunk. The
/// physical audio callback only uses try_recv and never reads a file or waits.
struct PcmSource {
    receiver: mpsc::Receiver<Pcm>,
    buffered: VecDeque<Vec<f32>>,
    samples: std::vec::IntoIter<f32>,
    duration: Duration,
    cancel: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    ended: bool,
    producer_ended: bool,
}
impl Iterator for PcmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.ended {
            return None;
        }
        if let Some(sample) = self.samples.next() {
            return Some(sample);
        }
        let message = self
            .buffered
            .pop_front()
            .map(Pcm::Samples)
            .or_else(|| self.receiver.try_recv().ok());
        if message.is_none() && self.producer_ended {
            self.ended = true;
            return None;
        }
        match message {
            Some(Pcm::Samples(samples)) => {
                self.samples = samples.into_iter();
                self.samples.next()
            }
            Some(Pcm::End) => {
                self.ended = true;
                None
            }
            other => {
                self.ended = true;
                let error = match other {
                    Some(Pcm::Error(error)) => error,
                    _ => "Preview audio decoder fell behind".into(),
                };
                self.failure.lock().unwrap().get_or_insert(error);
                None
            }
        }
    }
}
impl Source for PcmSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.ended { Some(0) } else { None }
    }
    fn channels(&self) -> rodio::ChannelCount {
        std::num::NonZeroU16::new(2).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        std::num::NonZeroU32::new(48_000).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(self.duration)
    }
}
impl Drop for PcmSource {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn streamed_source(
    input: &Input,
    position: f64,
    remaining: f64,
    failure: Arc<Mutex<Option<String>>>,
) -> Result<AudioSource, String> {
    if remaining <= 0.0 {
        return Ok(Box::new(rodio::buffer::SamplesBuffer::new(
            std::num::NonZeroU16::new(2).unwrap(),
            std::num::NonZeroU32::new(48_000).unwrap(),
            Vec::<f32>::new(),
        )));
    }
    let input = input.clone();
    let (sender, receiver) = mpsc::sync_channel(32);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    thread::Builder::new()
        .name("noh-preview-mixed-audio".into())
        .spawn(move || {
            let result = stream_input_audio(&input, position, remaining, &worker_cancel, &sender);
            if !worker_cancel.load(Ordering::Acquire) && !input.cancel.load(Ordering::Acquire) {
                let _ = sender.send(match result {
                    Ok(()) => Pcm::End,
                    Err(error) => Pcm::Error(error),
                });
            }
        })
        .map_err(|e| format!("Cannot start preview sound: {e}"))?;
    let mut buffered = VecDeque::new();
    let mut producer_ended = false;
    let deadline = Instant::now() + DECODE_DEADLINE;
    for _ in 0..8 {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Pcm::Samples(samples)) => buffered.push_back(samples),
            Ok(Pcm::End) => {
                producer_ended = true;
                break;
            }
            Ok(Pcm::Error(error)) => {
                cancel.store(true, Ordering::Release);
                return Err(error);
            }
            Err(error) => {
                cancel.store(true, Ordering::Release);
                return Err(format!("Preview audio startup failed: {error}"));
            }
        }
    }
    if buffered.is_empty() {
        cancel.store(true, Ordering::Release);
        return Err("Mixed preview has no audio samples".into());
    }
    Ok(Box::new(PcmSource {
        receiver,
        buffered,
        samples: Vec::new().into_iter(),
        duration: Duration::from_secs_f64(remaining),
        cancel,
        failure,
        ended: false,
        producer_ended,
    }))
}

fn stream_mix(
    input: &Input,
    mut local: f64,
    remaining: f64,
    cancel: &AtomicBool,
    sender: &mpsc::SyncSender<Pcm>,
) -> Result<(), String> {
    let project = input
        .project
        .as_ref()
        .ok_or("Missing mixed preview project")?;
    let end = local + remaining;
    while local < end - 0.000_001 {
        if cancel.load(Ordering::Acquire) || input.cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let segment =
            crate::preview::playback_piece(project, local, cancel).map_err(|e| e.to_string())?;
        let span = (segment.local_end.min(end) - local).max(0.0);
        if span <= 0.0 {
            return Err("Preview audio segment has no duration".into());
        }
        let mut command = crate::command(&input.ffmpeg);
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-threads",
            "2",
        ]);
        if segment.source_audio {
            command
                .arg("-ss")
                .arg(format!(
                    "{:.9}",
                    segment.source_seconds + local - segment.local_start
                ))
                .arg("-i")
                .arg(&segment.source);
        } else {
            command.args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"]);
        }
        command
            .arg("-ss")
            .arg(format!("{:.9}", input.audio_start() + local))
            .arg("-i")
            .arg(&input.path);
        command.arg("-filter_complex").arg(format!("[0:a:0]aresample=48000:async=1:first_pts=0,apad,atrim=duration={span:.9}[clips];[1:a:0]asetpts=PTS-STARTPTS,aresample=48000,apad,atrim=duration={span:.9}[wav];[clips][wav]amix=inputs=2:duration=first:dropout_transition=0[a]"))
            .args(["-map", "[a]", "-vn", "-sn", "-dn", "-t"]).arg(format!("{span:.9}"))
            .args(["-ac", "2", "-ar", "48000", "-f", "f32le", "pipe:1"]);
        stream_pcm(command, cancel, &input.cancel, sender)?;
        local = segment.local_end;
    }
    Ok(())
}

fn stream_input_audio(
    input: &Input,
    position: f64,
    remaining: f64,
    cancel: &AtomicBool,
    sender: &mpsc::SyncSender<Pcm>,
) -> Result<(), String> {
    if input.project.is_some() {
        return stream_mix(input, position, remaining, cancel, sender);
    }
    // Rodio 0.22.2's Symphonia iterator can stop on subsequent video packets
    // in an MP4. Keep demux/decode in FFmpeg and use the same bounded PCM feed.
    let mut command = crate::command(&input.ffmpeg);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-threads",
        "2",
    ]);
    // No seek is needed at the origin. FFmpeg 9 input seeking to exactly zero
    // drops an AAC priming block a second time, shortening the decoded audio.
    if position > 0.0 {
        command.arg("-ss").arg(format!("{position:.9}"));
    }
    command
        .arg("-i")
        .arg(&input.path)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-t"])
        .arg(format!("{remaining:.9}"))
        .args(["-ac", "2", "-ar", "48000", "-f", "f32le", "pipe:1"]);
    stream_pcm(command, cancel, &input.cancel, sender)
}

fn stream_pcm(
    mut command: std::process::Command,
    cancel: &AtomicBool,
    input_cancel: &AtomicBool,
    sender: &mpsc::SyncSender<Pcm>,
) -> Result<(), String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = Tree::spawn(command, true)
        .map_err(|e| format!("Cannot start preview audio decoder: {e}"))?;
    let mut stdout = tree
        .0
        .stdout()
        .take()
        .ok_or("Missing preview audio output")?;
    let mut stderr = tree
        .0
        .stderr()
        .take()
        .ok_or("Missing preview audio diagnostics")?;
    let diagnostic = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured = diagnostic.clone();
    let errors = thread::spawn(move || {
        let mut bytes = [0u8; 2048];
        while let Ok(count) = stderr.read(&mut bytes) {
            if count == 0 {
                break;
            }
            let mut log = captured.lock().unwrap();
            log.extend_from_slice(&bytes[..count]);
            if log.len() > 8192 {
                let excess = log.len() - 8192;
                log.drain(..excess);
            }
        }
    });
    let progress = Arc::new(AtomicU64::new(0));
    let blocked = Arc::new(AtomicBool::new(false));
    let read_progress = progress.clone();
    let read_blocked = blocked.clone();
    let chunks = sender.clone();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let output = thread::spawn(move || {
        let result = (|| -> io::Result<()> {
            loop {
                let mut bytes = [0u8; 4096 * 4];
                let mut used = 0;
                while used < bytes.len() {
                    let count = stdout.read(&mut bytes[used..])?;
                    if count == 0 {
                        break;
                    }
                    used += count;
                    read_progress.fetch_add(count as u64, Ordering::Release);
                }
                if used == 0 {
                    return Ok(());
                }
                if used % 8 != 0 {
                    return Err(io::Error::other("Incomplete preview stereo sample"));
                }
                let samples = bytes[..used]
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                read_blocked.store(true, Ordering::Release);
                let sent = chunks.send(Pcm::Samples(samples));
                read_blocked.store(false, Ordering::Release);
                if sent.is_err() {
                    return Ok(());
                }
            }
        })();
        let _ = done_tx.send(result);
    });
    let mut previous = 0;
    let mut progress_at = Instant::now();
    let result = loop {
        if cancel.load(Ordering::Acquire) || input_cancel.load(Ordering::Acquire) {
            break Ok(());
        }
        let bytes = progress.load(Ordering::Acquire);
        if bytes != previous || blocked.load(Ordering::Acquire) {
            previous = bytes;
            progress_at = Instant::now();
        }
        if progress_at.elapsed() > DECODE_DEADLINE {
            break Err("Preview audio decoder deadline exceeded".into());
        }
        match done_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => break result.map_err(|e| e.to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => break Err(format!("Preview audio reader stopped: {error}")),
        }
    };
    let status = tree.0.try_wait().map_err(|e| e.to_string())?;
    drop(tree);
    // Receiver disposal on Source::drop releases a queue-full sender.
    if cancel.load(Ordering::Acquire) || input_cancel.load(Ordering::Acquire) {
        return result;
    }
    let _ = output.join();
    let _ = errors.join();
    result?;
    if status.is_some_and(|s| !s.success()) {
        return Err(format!(
            "Preview audio failed: {}",
            String::from_utf8_lossy(&diagnostic.lock().unwrap())
        ));
    }
    Ok(())
}

fn source_for(
    input: &Input,
    position: f64,
    remaining: f64,
    failure: Arc<Mutex<Option<String>>>,
) -> Result<AudioSource, String> {
    if input.project.as_ref().is_none_or(|p| p.montage.clip_audio) {
        streamed_source(input, position, remaining, failure)
    } else {
        Ok(Box::new(
            audio_source(&input.path, input.audio_start() + position)?
                .take_duration(Duration::from_secs_f64(remaining.max(0.0))),
        ))
    }
}
fn audio_source(path: &Path, position: f64) -> Result<rodio::Decoder<BufReader<File>>, String> {
    let file = File::open(path).map_err(|e| format!("Cannot open preview sound: {e}"))?;
    let mut decoder =
        rodio::Decoder::try_from(file).map_err(|e| format!("Cannot decode preview sound: {e}"))?;
    if position > 0.0 {
        decoder
            .try_seek(Duration::from_secs_f64(position))
            .map_err(|e| format!("Cannot seek preview sound: {e}"))?;
    }
    Ok(decoder)
}
impl Audio {
    fn open(input: &Input, position: f64, remaining: f64, volume: f32) -> Result<Self, String> {
        let failure = Arc::new(Mutex::new(None));
        let source = source_for(input, position, remaining, failure.clone())?;
        let errors = failure.clone();
        let mut device = rodio::DeviceSinkBuilder::from_default_device()
            .map_err(|e| format!("Audio output unavailable: {e}"))?
            .with_error_callback(move |error| {
                errors
                    .lock()
                    .unwrap()
                    .get_or_insert_with(|| format!("Audio output stopped: {error}"));
            })
            .open_sink_or_fallback()
            .map_err(|e| format!("Audio output unavailable: {e}"))?;
        device.log_on_drop(false);
        let player = rodio::Player::connect_new(device.mixer());
        player.pause();
        player.set_volume(volume);
        player.append(source);
        Ok(Self {
            player,
            device,
            failure,
            base: position,
            mixed: input.project.as_ref().is_some_and(|p| p.montage.clip_audio),
        })
    }
    fn seek(
        &mut self,
        input: &Input,
        position: f64,
        remaining: f64,
        volume: f32,
    ) -> Result<(), String> {
        // Seek the decoder before connecting it: Player::try_seek waits for the
        // device callback and can wait indefinitely after an output-device loss.
        self.player.stop();
        let source = source_for(input, position, remaining, self.failure.clone())?;
        self.player = rodio::Player::connect_new(self.device.mixer());
        self.player.pause();
        self.player.set_volume(volume);
        self.player.append(source);
        self.base = position;
        Ok(())
    }
    fn position(&self) -> f64 {
        self.base + self.player.get_pos().as_secs_f64()
    }
}

enum Decoded {
    Frame(Frame),
    End,
    Error(String),
}
struct Video {
    tree: Option<Tree>,
    receiver: Option<mpsc::Receiver<Decoded>>,
    readers: Vec<thread::JoinHandle<()>>,
    diagnostic: Arc<Mutex<Vec<u8>>>,
    pending: Option<Frame>,
    ended: bool,
    last_frame: Instant,
    local_end: f64,
    _workspace: Option<Arc<tempfile::TempDir>>,
}
impl Video {
    fn open(input: &Input, position: f64) -> Result<Self, String> {
        if let Some(project) = &input.project {
            let segment = crate::preview::playback_segment(project, position, &input.cancel)
                .map_err(|e| e.to_string())?;
            return Self::from_command(
                segment.command,
                segment.local_start,
                segment.local_end,
                Some(segment.workspace),
            );
        }
        let mut command = crate::command(&input.ffmpeg);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-threads",
                "2",
                "-ss",
            ])
            .arg(format!("{position:.6}"))
            .arg("-i")
            .arg(&input.path)
            .args([
                "-map",
                "0:v:0",
                "-an",
                "-sn",
                "-dn",
                "-vf",
                "scale=640:640:force_original_aspect_ratio=decrease,setsar=1,fps=24",
                "-threads",
                "2",
                "-c:v",
                "ppm",
                "-f",
                "image2pipe",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Self::from_command(command, position, f64::INFINITY, None)
    }
    fn from_command(
        mut command: std::process::Command,
        position: f64,
        local_end: f64,
        workspace: Option<Arc<tempfile::TempDir>>,
    ) -> Result<Self, String> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut tree =
            Tree::spawn(command, true).map_err(|e| format!("Cannot start preview video: {e}"))?;
        let stdout = tree
            .0
            .stdout()
            .take()
            .ok_or("Missing preview video output")?;
        let mut stderr = tree
            .0
            .stderr()
            .take()
            .ok_or("Missing preview video diagnostics")?;
        let (sender, receiver) = mpsc::sync_channel(2);
        let output = thread::Builder::new()
            .name("noh-preview-frames".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut index = 0u64;
                loop {
                    let result = read_frame(&mut reader, position + index as f64 / FPS);
                    let terminal = !matches!(result, Ok(Some(_)));
                    let message = match result {
                        Ok(Some(frame)) => Decoded::Frame(frame),
                        Ok(None) => Decoded::End,
                        Err(e) => Decoded::Error(format!("Cannot decode preview video: {e}")),
                    };
                    if sender.send(message).is_err() || terminal {
                        break;
                    }
                    index = index.wrapping_add(1);
                }
            })
            .map_err(|e| format!("Cannot start preview frame reader: {e}"))?;
        let diagnostic = Arc::new(Mutex::new(Vec::new()));
        let captured = diagnostic.clone();
        let errors = thread::Builder::new()
            .name("noh-preview-diagnostics".into())
            .spawn(move || {
                let mut bytes = [0u8; 2048];
                while let Ok(count) = stderr.read(&mut bytes) {
                    if count == 0 {
                        break;
                    }
                    let mut log = captured.lock().unwrap();
                    log.extend_from_slice(&bytes[..count]);
                    if log.len() > 8192 {
                        let excess = log.len() - 8192;
                        log.drain(..excess);
                    }
                }
            })
            .map_err(|e| format!("Cannot start preview diagnostic reader: {e}"))?;
        Ok(Self {
            tree: Some(tree),
            receiver: Some(receiver),
            readers: vec![output, errors],
            diagnostic,
            pending: None,
            ended: false,
            last_frame: Instant::now(),
            local_end,
            _workspace: workspace,
        })
    }
    fn next(&mut self) -> Result<(), String> {
        if self.ended {
            if let Some(status) = self
                .tree
                .as_mut()
                .unwrap()
                .0
                .try_wait()
                .map_err(|e| e.to_string())?
                && !status.success()
            {
                return Err(format!(
                    "Preview video failed: {}",
                    String::from_utf8_lossy(&self.diagnostic.lock().unwrap())
                ));
            }
            return Ok(());
        }
        if self.pending.is_some() {
            return Ok(());
        }
        match self.receiver.as_ref().unwrap().try_recv() {
            Ok(Decoded::Frame(frame)) => {
                self.last_frame = Instant::now();
                self.pending = Some(frame);
            }
            Ok(Decoded::Error(error)) => return Err(error),
            Ok(Decoded::End) | Err(mpsc::TryRecvError::Disconnected) => {
                self.ended = true;
                if let Some(status) = self
                    .tree
                    .as_mut()
                    .unwrap()
                    .0
                    .try_wait()
                    .map_err(|e| e.to_string())?
                    && !status.success()
                {
                    return Err(format!(
                        "Preview video failed: {}",
                        String::from_utf8_lossy(&self.diagnostic.lock().unwrap())
                    ));
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        Ok(())
    }
}
impl Drop for Video {
    fn drop(&mut self) {
        // Dropping the receiver releases a reader blocked on its bounded send;
        // dropping Tree closes any remaining blocked pipe reads and reaps FFmpeg.
        self.receiver.take();
        self.tree.take();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn header_line(reader: &mut impl BufRead, limit: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(line);
        }
        let length = buffer
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buffer.len(), |p| p + 1);
        if line.len() + length > limit {
            return Err(io::Error::other("Preview frame header exceeds its limit"));
        }
        line.extend_from_slice(&buffer[..length]);
        reader.consume(length);
        if line.last() == Some(&b'\n') {
            return Ok(line);
        }
    }
}
fn read_frame(reader: &mut impl BufRead, position: f64) -> io::Result<Option<Frame>> {
    let magic = header_line(reader, 8)?;
    if magic.is_empty() {
        return Ok(None);
    }
    if magic != b"P6\n" {
        return Err(io::Error::other("Invalid preview frame format"));
    }
    let geometry = header_line(reader, 48)?;
    let geometry = std::str::from_utf8(&geometry).map_err(io::Error::other)?;
    let mut dimensions = geometry.split_whitespace();
    let width = dimensions
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let height = dimensions
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    if dimensions.next().is_some() || width == 0 || height == 0 || width > EDGE || height > EDGE {
        return Err(io::Error::other(
            "Preview frame dimensions exceed their limit",
        ));
    }
    if header_line(reader, 8)? != b"255\n" {
        return Err(io::Error::other("Invalid preview RGB depth"));
    }
    let mut rgb = vec![0u8; width * height * 3];
    reader.read_exact(&mut rgb)?;
    Ok(Some(Frame {
        id: 0,
        width,
        height,
        rgb,
        position,
    }))
}

struct Session {
    input: Input,
    stamp: FileStamp,
    duration: f64,
    seek_id: u64,
    audio: Option<Audio>,
    warning: Option<String>,
    video: Option<Video>,
    /// One future occurrence, with its own two-frame pipe queue. No further
    /// occurrence is prepared until this slot becomes the current decoder.
    next_video: Option<Video>,
    clock: Clock,
    ready: bool,
    ended: bool,
    audio_progress: f64,
    audio_progress_at: Instant,
}
impl Session {
    fn open(mut input: Input, desired: &Desired) -> Result<Self, String> {
        let stamp = FileStamp::read(&input.path).map_err(|e| e.to_string())?;
        let info = if let Some(project) = &input.project {
            project.validate().map_err(|e| e.to_string())?;
            input.ffmpeg =
                crate::inspection::resolve_ffmpeg(&input.ffmpeg).map_err(|e| e.to_string())?;
            let wav_duration = crate::wav_duration(&input.path).map_err(|e| e.to_string())?;
            let (start, end) = project.short.as_ref().map_or((0.0, wav_duration), |s| {
                (s.start_ms as f64 / 1000.0, s.end_ms as f64 / 1000.0)
            });
            if end > wav_duration + 0.000_001 {
                return Err("Preview range exceeds the WAV duration".into());
            }
            crate::media::MediaInfo {
                seconds: end - start,
                audio: true,
                ..Default::default()
            }
        } else {
            // Existing rendered previews retain the shared 15-second header probe.
            crate::media::preview_info(&input.ffmpeg, &input.path).map_err(|e| e.to_string())?
        };
        if !info.seconds.is_finite() || info.seconds <= 0.0 {
            return Err("Preview duration is invalid".into());
        }
        if FileStamp::read(&input.path).as_ref() != Ok(&stamp) {
            return Err("Preview changed while opening it".into());
        }
        let position = desired.position.min(info.seconds);
        let (audio, warning) = if info.audio {
            match Audio::open(&input, position, info.seconds - position, desired.volume) {
                Ok(audio) => (Some(audio), None),
                Err(error)
                    if input.project.as_ref().is_some_and(|p| p.montage.clip_audio)
                        && !error.starts_with("Audio output") =>
                {
                    return Err(format!(
                        "Live preview could not preserve clip sound: {error}."
                    ));
                }
                Err(error) => (None, Some(format!("Silent preview: {error}"))),
            }
        } else {
            (
                None,
                Some("Silent preview: the rendered file has no audio track".into()),
            )
        };
        let video = Video::open(&input, position.min((info.seconds - 1.0 / FPS).max(0.0)))?;
        Ok(Self {
            input,
            stamp,
            duration: info.seconds,
            seek_id: desired.seek_id,
            audio,
            warning,
            video: Some(video),
            next_video: None,
            clock: Clock::new(position),
            ready: false,
            ended: position >= info.seconds,
            audio_progress: position,
            audio_progress_at: Instant::now(),
        })
    }
    fn seek(&mut self, desired: &Desired) -> Result<(), String> {
        if FileStamp::read(&self.input.path).as_ref() != Ok(&self.stamp) {
            return Err("Preview changed while playing it".into());
        }
        let position = desired.position.min(self.duration);
        if let Some(audio) = &self.audio {
            audio.player.pause();
        }
        self.video.take();
        self.next_video.take();
        if let Some(audio) = &mut self.audio {
            audio.player.pause();
            if let Err(error) = audio.seek(
                &self.input,
                position,
                self.duration - position,
                desired.volume,
            ) {
                if audio.mixed && !error.starts_with("Audio output") {
                    return Err(format!(
                        "Live preview could not preserve clip sound: {error}."
                    ));
                }
                self.warning = Some(format!("Silent preview: {error}"));
                self.audio.take();
            }
        }
        self.video = Some(Video::open(
            &self.input,
            position.min((self.duration - 1.0 / FPS).max(0.0)),
        )?);
        self.seek_id = desired.seek_id;
        self.ready = false;
        self.ended = position >= self.duration;
        self.clock.set(position, false, Instant::now());
        self.audio_progress = position;
        self.audio_progress_at = Instant::now();
        Ok(())
    }
    fn position(&self, now: Instant) -> f64 {
        self.audio
            .as_ref()
            .filter(|_| self.ready && !self.ended)
            .map_or_else(|| self.clock.position(now), Audio::position)
            .min(self.duration)
    }
    fn tick(
        &mut self,
        desired: &Desired,
        frame_id: &mut u64,
    ) -> Result<Option<Arc<Frame>>, String> {
        let now = Instant::now();
        let mut position = self.position(now);
        if let Some(audio) = &self.audio {
            let failure = audio.failure.lock().unwrap().take();
            let stalled = self.ready
                && desired.playing
                && !self.ended
                && !audio.player.empty()
                && now.duration_since(self.audio_progress_at) > Duration::from_secs(5);
            if let Some(error) =
                failure.or_else(|| stalled.then(|| "Audio output stopped advancing".into()))
            {
                if audio.mixed && !error.starts_with("Audio output") {
                    return Err(format!(
                        "Live preview could not preserve clip sound: {error}."
                    ));
                }
                self.warning = Some(format!("Silent preview: {error}"));
                self.audio.take();
                self.clock
                    .set(position, desired.playing && self.ready && !self.ended, now);
            }
        }
        if (position - self.audio_progress).abs() > 0.002 || !desired.playing || !self.ready {
            self.audio_progress = position;
            self.audio_progress_at = now;
        }
        let should_play = desired.playing && self.ready && !self.ended;
        if self.clock.playing != should_play {
            self.clock.set(position, should_play, now);
        }
        if let Some(audio) = &self.audio {
            audio.player.set_volume(desired.volume);
            if should_play {
                audio.player.play();
            } else {
                audio.player.pause();
            }
        }
        let mut latest = None;
        if let Some(boundary) = prefetch_at(
            self.video.as_ref().map(|v| v.local_end),
            position,
            self.duration,
            self.next_video.is_some(),
        ) {
            // Normal cache/spawn work runs ahead of the seam without stopping
            // sound. A late request holds transport before synchronous setup.
            if position >= boundary {
                if let Some(audio) = &self.audio {
                    audio.player.pause();
                }
                self.clock.set(position, false, Instant::now());
                self.ready = false;
            }
            self.next_video = Some(Video::open(&self.input, boundary)?);
            self.audio_progress_at = Instant::now();
            // Helper setup is off-thread but may take time; select due frames
            // against the current physical audio clock after that work.
            position = self.position(Instant::now());
        }
        if self
            .video
            .as_ref()
            .is_some_and(|video| position >= video.local_end && position < self.duration)
        {
            let mut next = match self.next_video.take() {
                Some(next) => next,
                None => Video::open(&self.input, position)?,
            };
            next.next()?;
            if next.pending.is_none() {
                // A slow first frame must not leave the old picture running
                // against advancing sound. Existing first-frame logic resumes
                // transport when this occurrence becomes available.
                if let Some(audio) = &self.audio {
                    audio.player.pause();
                }
                self.clock.set(position, false, Instant::now());
                self.ready = false;
            }
            self.video.take();
            self.video = Some(next);
            position = self.position(Instant::now());
        }
        if let Some(video) = &mut self.video {
            loop {
                video.next()?;
                if video
                    .pending
                    .as_ref()
                    .is_some_and(|f| !self.ready || f.position <= position + 0.001)
                {
                    let mut frame = video.pending.take().unwrap();
                    *frame_id = frame_id.wrapping_add(1);
                    frame.id = *frame_id;
                    latest = Some(Arc::new(frame));
                    self.ready = true;
                } else {
                    break;
                }
            }
            if !self.ready && video.ended {
                return Err("Preview contains no readable video frame".into());
            }
            if (!self.ready || should_play)
                && !video.ended
                && video.pending.is_none()
                && now.duration_since(video.last_frame) > DECODE_DEADLINE
            {
                return Err("Preview video decoder deadline exceeded".into());
            }
        }
        if self.ready
            && (position >= self.duration || self.audio.as_ref().is_some_and(|a| a.player.empty()))
        {
            self.ended = true;
            self.clock.set(self.duration, false, now);
            if let Some(audio) = &self.audio {
                audio.player.pause();
            }
            self.video.take();
            self.next_video.take();
        }
        Ok(latest)
    }
}

fn prefetch_at(end: Option<f64>, position: f64, duration: f64, occupied: bool) -> Option<f64> {
    end.filter(|end| {
        !occupied && end.is_finite() && *end < duration && position + PREFETCH_LEAD >= *end
    })
}

fn run(shared: Arc<Shared>) {
    let _workspace_lease = crate::preview::WorkspaceLease::new();
    let mut generation = 0;
    let mut session: Option<Session> = None;
    let mut frame_id = 0;
    loop {
        let desired = shared.desired();
        if desired.shutdown {
            break;
        }
        if desired.generation != generation {
            session.take();
            generation = desired.generation;
            if let Some(input) = desired.input.clone() {
                match Session::open(input, &desired) {
                    Ok(loaded) if shared.current(generation) => session = Some(loaded),
                    Ok(_) => continue,
                    Err(error) => shared.publish(generation, |snapshot| {
                        snapshot.phase = Phase::Error;
                        snapshot.playing = false;
                        snapshot.error = Some(error);
                    }),
                }
            }
        }
        // Loading can span several GUI edits. Apply the latest transport state
        // before starting sound or publishing a position from the old request.
        let desired = shared.desired();
        if desired.shutdown {
            break;
        }
        if desired.generation != generation {
            continue;
        }
        if let Some(active) = &mut session {
            let result = if active.seek_id != desired.seek_id {
                if desired.seek_at.elapsed() >= SEEK_SETTLE {
                    active.seek(&desired).map(|()| None)
                } else {
                    if let Some(audio) = &active.audio {
                        audio.player.pause();
                    }
                    active
                        .clock
                        .set(active.position(Instant::now()), false, Instant::now());
                    Ok(None)
                }
            } else {
                active.tick(&desired, &mut frame_id)
            };
            match result {
                Ok(frame) => shared.publish(generation, |snapshot| {
                    snapshot.phase = if active.ended {
                        Phase::Ended
                    } else if active.ready {
                        Phase::Ready
                    } else {
                        Phase::Loading
                    };
                    snapshot.duration = active.duration;
                    if active.seek_id == desired.seek_id {
                        snapshot.position = active.position(Instant::now());
                    }
                    snapshot.playing = desired.playing && !active.ended;
                    snapshot.audio_available = active.audio.is_some();
                    snapshot.warning = active.warning.clone();
                    if let Some(frame) = frame {
                        snapshot.frame = Some(frame);
                    }
                }),
                Err(error) => {
                    shared.publish(generation, |snapshot| {
                        snapshot.phase = Phase::Error;
                        snapshot.playing = false;
                        snapshot.error = Some(error);
                    });
                    session.take();
                }
            }
        }
        let newest = shared.desired.lock().unwrap();
        if newest.shutdown {
            break;
        }
        if newest.generation != generation
            || newest.seek_id != desired.seek_id
            || newest.playing != desired.playing
            || newest.volume != desired.volume
        {
            continue;
        }
        let timeout = if session.as_ref().is_some_and(|active| {
            !active.ready || active.seek_id != newest.seek_id || (!active.ended && newest.playing)
        }) {
            TICK
        } else {
            Duration::from_secs(60)
        };
        drop(shared.changed.wait_timeout(newest, timeout).unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn mock_video(local_end: f64) -> (mpsc::SyncSender<Decoded>, Video) {
        let (sender, receiver) = mpsc::sync_channel(2);
        (
            sender,
            Video {
                tree: None,
                receiver: Some(receiver),
                readers: Vec::new(),
                diagnostic: Arc::new(Mutex::new(Vec::new())),
                pending: None,
                ended: false,
                last_frame: Instant::now(),
                local_end,
                _workspace: None,
            },
        )
    }

    #[test]
    fn delayed_seam_holds_clock_until_frame_and_disposes_both_decoder_slots() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("identity.wav");
        std::fs::write(&path, b"identity-only fixture").unwrap();
        let input = Input {
            path: path.clone(),
            ffmpeg: "missing-ffmpeg".into(),
            project: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let desired = Desired {
            generation: 1,
            input: Some(input.clone()),
            seek_id: 0,
            position: 1.1,
            seek_at: Instant::now(),
            playing: true,
            volume: 1.0,
            shutdown: false,
        };
        let (old_sender, current) = mock_video(1.0);
        let (next_sender, next) = mock_video(2.0);
        let mut session = Session {
            input,
            stamp: FileStamp::read(&path).unwrap(),
            duration: 3.0,
            seek_id: 0,
            audio: None,
            warning: None,
            video: Some(current),
            next_video: Some(next),
            clock: Clock::new(1.1),
            ready: true,
            ended: false,
            audio_progress: 1.1,
            audio_progress_at: Instant::now(),
        };
        let mut frame_id = 0;
        assert!(session.tick(&desired, &mut frame_id).unwrap().is_none());
        assert!(!session.ready && !session.clock.playing);
        assert!((session.position(Instant::now() + Duration::from_secs(20)) - 1.1).abs() < 0.001);
        assert!(session.next_video.is_none());
        assert!(
            old_sender.send(Decoded::End).is_err(),
            "Old decoder queue survived the seam"
        );
        next_sender
            .send(Decoded::Frame(Frame {
                id: 0,
                width: 1,
                height: 1,
                rgb: vec![0, 0, 255],
                position: 1.0,
            }))
            .unwrap();
        assert!(session.tick(&desired, &mut frame_id).unwrap().is_some());
        assert!(session.ready);
        session.tick(&desired, &mut frame_id).unwrap();
        assert!(session.clock.playing);
        let (future_sender, future) = mock_video(3.0);
        session.next_video = Some(future);
        drop(session);
        assert!(next_sender.send(Decoded::End).is_err());
        assert!(future_sender.send(Decoded::End).is_err());
    }

    #[test]
    fn prefetch_stays_in_one_slot_and_never_crosses_selection_end() {
        assert_eq!(prefetch_at(Some(2.0), 1.49, 5.0, false), None);
        assert_eq!(prefetch_at(Some(2.0), 1.5, 5.0, false), Some(2.0));
        assert_eq!(prefetch_at(Some(2.0), 1.9, 5.0, true), None);
        assert_eq!(prefetch_at(Some(5.0), 4.9, 5.0, false), None);
        assert_eq!(prefetch_at(Some(f64::INFINITY), 4.9, 5.0, false), None);
    }

    fn request(
        folder: &Path,
        ffmpeg: PathBuf,
        wav: PathBuf,
        a: PathBuf,
        b: PathBuf,
        restart: bool,
        clip_audio: bool,
    ) -> crate::project::ProjectRequest {
        crate::project::ProjectRequest {
            montage: crate::engine::ExportRequest {
                items: vec![a.into(), b.into()],
                wav,
                output: folder.join("unused.mp4"),
                ffmpeg,
                fade_in: 0.0,
                fade_out: 0.0,
                partial_fades: true,
                preview: true,
                clip_audio,
                force_encode: false,
            },
            short: Some(crate::project::ProjectShort {
                start_ms: 13_000,
                end_ms: 18_000,
                restart_loops: restart,
                framing: crate::shorts::Framing::Pad,
            }),
            captions: None,
        }
    }

    fn identity_wav(path: &Path) {
        use std::io::Write;
        let rate = 48_000u32;
        let count = rate * 20;
        let bytes = count * 2;
        let mut file = File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&rate.to_le_bytes()).unwrap();
        file.write_all(&(rate * 2).to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&bytes.to_le_bytes()).unwrap();
        let pcm: Vec<u8> = (0..count)
            .flat_map(|index| (((index * 73) % 10_000) as i16).to_le_bytes())
            .collect();
        file.write_all(&pcm).unwrap();
    }

    #[test]
    fn selected_wav_phase_and_duration_survive_both_visual_restart_modes() {
        let folder = tempfile::tempdir().unwrap();
        let wav = folder.path().join("clock.wav");
        identity_wav(&wav);
        let expected: Vec<_> = audio_source(&wav, 13.125).unwrap().take(256).collect();
        for restart in [false, true] {
            let project = request(
                folder.path(),
                "unused-ffmpeg".into(),
                wav.clone(),
                "a.mp4".into(),
                "b.mp4".into(),
                restart,
                false,
            );
            let input = Input {
                path: wav.clone(),
                ffmpeg: "unused-ffmpeg".into(),
                project: Some(Arc::new(project)),
                cancel: Arc::new(AtomicBool::new(false)),
            };
            let failure = Arc::new(Mutex::new(None));
            let mut source = source_for(&input, 0.125, 1.0, failure).unwrap();
            let actual: Vec<_> = source.by_ref().take(256).collect();
            assert_eq!(actual, expected);
            assert_eq!(source.count() + 256, 48_000);
        }
    }

    #[test]
    fn mixed_callback_preserves_samples_and_reports_underrun_without_waiting() {
        let (tx, rx) = mpsc::sync_channel(48);
        tx.send(Pcm::Samples(vec![0.1, 0.2])).unwrap();
        tx.send(Pcm::End).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        let mut source = PcmSource {
            receiver: rx,
            buffered: VecDeque::new(),
            samples: Vec::new().into_iter(),
            duration: Duration::from_secs(1),
            cancel: cancel.clone(),
            failure: failure.clone(),
            ended: false,
            producer_ended: false,
        };
        assert_eq!(source.by_ref().collect::<Vec<_>>(), [0.1, 0.2]);
        assert!(failure.lock().unwrap().is_none());
        drop(source);
        assert!(cancel.load(Ordering::Acquire));
        let (_tx, rx) = mpsc::sync_channel(48);
        let failure = Arc::new(Mutex::new(None));
        let mut source = PcmSource {
            receiver: rx,
            buffered: VecDeque::new(),
            samples: Vec::new().into_iter(),
            duration: Duration::from_secs(1),
            cancel: Arc::new(AtomicBool::new(false)),
            failure: failure.clone(),
            ended: false,
            producer_ended: false,
        };
        let started = Instant::now();
        assert_eq!(source.next(), None);
        assert!(started.elapsed() < Duration::from_millis(20));
        assert!(
            failure
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains("fell behind")
        );
    }

    #[test]
    fn silent_clock_pause_resume_and_seek_do_not_drift() {
        let base = Instant::now();
        let mut clock = Clock::new(4.0);
        clock.set(4.0, true, base);
        assert_eq!(clock.position(base + Duration::from_secs(3)), 7.0);
        clock.set(7.0, false, base + Duration::from_secs(3));
        assert_eq!(clock.position(base + Duration::from_secs(30)), 7.0);
        clock.set(2.5, true, base + Duration::from_secs(30));
        assert_eq!(clock.position(base + Duration::from_secs(32)), 4.5);
    }

    #[test]
    fn rgb_reader_bounds_geometry_headers_and_truncated_pixels() {
        let frame = read_frame(
            &mut Cursor::new(b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06"),
            3.5,
        )
        .unwrap()
        .unwrap();
        assert_eq!((frame.width, frame.height, frame.position), (2, 1, 3.5));
        assert_eq!(frame.rgb, [1, 2, 3, 4, 5, 6]);
        for invalid in [
            b"P6\n641 1\n255\n".as_slice(),
            b"P6\n0 5\n255\n",
            b"P6\n2 1\n255\n\x01",
            b"P6\n2 1 8\n255\n",
            b"P6\n2 1\n65535\n",
        ] {
            assert!(read_frame(&mut Cursor::new(invalid), 0.0).is_err());
        }
        assert!(header_line(&mut Cursor::new(vec![b'x'; 4096]), 48).is_err());
        assert!(
            read_frame(&mut Cursor::new(Vec::<u8>::new()), 0.0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn commands_coalesce_and_stop_invalidates_old_updates() {
        let player = Player::new(|| {});
        player.open(
            PathBuf::from("missing-preview.mp4"),
            PathBuf::from("missing-ffmpeg"),
            0.0,
            false,
        );
        for index in 0..1000 {
            player.seek(index as f64 / 10.0);
        }
        assert_eq!(player.shared.desired().position, 99.9);
        let generation = player.snapshot().generation;
        player.set_volume(f32::NAN);
        assert_eq!(player.snapshot().volume, 1.0);
        player.stop();
        player.shared.publish(generation, |s| {
            s.phase = Phase::Ready;
        });
        assert_eq!(player.snapshot().phase, Phase::Empty);
        assert!(player.snapshot().frame.is_none());
    }

    /// Uses real H.264/AAC, real FFmpeg frames and a physical audio output. It
    /// verifies data and device clocks; a person must assess audible quality.
    #[test]
    fn real_aac_device_play_pause_seek_and_frames() {
        if std::env::var("NOH_TEST_PLAYBACK_AUDIO").as_deref() != Ok("1") {
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("preview.mp4");
        let ffmpeg = crate::inspection::resolve_ffmpeg(Path::new("")).unwrap();
        let mut fixture = crate::command(&ffmpeg);
        fixture
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=960x540:r=24:d=3",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&path);
        let (status, diagnostic) =
            crate::process::capture(fixture, Duration::from_secs(15), true).unwrap();
        assert!(status.success(), "{diagnostic}");
        let input = Input {
            path: path.clone(),
            ffmpeg: ffmpeg.clone(),
            project: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let (sender, receiver) = mpsc::sync_channel(32);
        let decoded = thread::spawn(move || {
            let result = stream_input_audio(&input, 0.0, 3.0, &AtomicBool::new(false), &sender);
            sender
                .send(match result {
                    Ok(()) => Pcm::End,
                    Err(error) => Pcm::Error(error),
                })
                .unwrap();
        });
        let (mut samples, mut nonzero) = (0usize, false);
        loop {
            match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
                Pcm::Samples(chunk) => {
                    samples += chunk.len();
                    nonzero |= chunk.iter().any(|sample| sample.abs() > 0.001);
                }
                Pcm::End => break,
                Pcm::Error(error) => panic!("{error}"),
            }
        }
        decoded.join().unwrap();
        assert_eq!(
            samples,
            3 * 48_000 * 2,
            "AAC ended before the complete fixture was decoded"
        );
        assert!(nonzero);
        let player = Player::new(|| {});
        player.open(path, ffmpeg, 0.0, true);
        let wait = |predicate: &dyn Fn(&Snapshot) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let snapshot = player.snapshot();
                assert_ne!(snapshot.phase, Phase::Error, "{:?}", snapshot.error);
                if predicate(&snapshot) {
                    return snapshot;
                }
                assert!(Instant::now() < deadline, "Timed out: {snapshot:?}");
                thread::sleep(Duration::from_millis(20));
            }
        };
        let playing = wait(&|s| s.phase == Phase::Ready && s.position > 0.3);
        assert!(playing.audio_available, "{:?}", playing.warning);
        let frame = playing.frame.unwrap();
        assert!(frame.width <= EDGE && frame.height <= EDGE);
        assert!((frame.position - playing.position).abs() < 0.2);
        player.set_playing(false);
        thread::sleep(Duration::from_millis(150));
        let paused = player.snapshot().position;
        thread::sleep(Duration::from_millis(150));
        assert!((player.snapshot().position - paused).abs() < 0.03);
        player.seek(1.5);
        let sought = wait(&|s| {
            s.phase == Phase::Ready && s.frame.as_ref().is_some_and(|f| f.position >= 1.49)
        });
        assert!((sought.position - 1.5).abs() < 0.12);
        player.set_volume(0.2);
        player.set_playing(true);
        wait(&|s| s.position > 1.8);
        let ended = wait(&|s| s.phase == Phase::Ended);
        assert!(!ended.playing);
        player.set_playing(true);
        wait(&|s| s.phase == Phase::Ready && s.position > 0.1 && s.position < 0.8);
        player.stop();
        assert_eq!(player.snapshot().phase, Phase::Empty);
    }

    #[test]
    fn real_fractional_loop_seam_keeps_audio_and_video_advancing() {
        if std::env::var("NOH_TEST_PLAYBACK_AUDIO").as_deref() != Ok("1") {
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let ffmpeg = crate::inspection::resolve_ffmpeg(Path::new("")).unwrap();
        let video = folder.path().join("fractional.mp4");
        let wav = folder.path().join("clock.wav");
        let mut command = crate::command(&ffmpeg);
        command
            .args([
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=256x144:r=24",
                "-frames:v",
                "241",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
            ])
            .arg(&video);
        let (status, log) =
            crate::process::capture(command, Duration::from_secs(15), true).unwrap();
        assert!(status.success(), "{log}");
        let mut command = crate::command(&ffmpeg);
        command
            .args([
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=220:duration=65",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&wav);
        let (status, log) =
            crate::process::capture(command, Duration::from_secs(15), true).unwrap();
        assert!(status.success(), "{log}");
        let mut project = request(
            folder.path(),
            ffmpeg,
            wav,
            video.clone(),
            video,
            false,
            false,
        );
        project.montage.items.truncate(1);
        project.short = None;
        let player = Player::new(|| {});
        // Fifth seam was rounded to a zero-duration occurrence at 50.208333 s.
        player.open_project(project, 49.8, true);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = player.snapshot();
            assert_ne!(state.phase, Phase::Error, "{state:?}");
            if state.position > 51.0 && state.frame.as_ref().is_some_and(|f| f.position > 51.0) {
                assert!(state.playing && state.audio_available, "{state:?}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Stalled at fractional seam: {state:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        player.stop();
    }

    #[test]
    fn real_project_stream_preserves_wav_clock_mix_and_visual_seams() {
        if std::env::var("NOH_TEST_PLAYBACK_AUDIO").as_deref() != Ok("1") {
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let ffmpeg = crate::inspection::resolve_ffmpeg(Path::new("")).unwrap();
        let a = folder.path().join("red.mp4");
        let b = folder.path().join("blue.mp4");
        let wav = folder.path().join("clock.wav");
        for (path, color, duration, tone) in [(&a, "red", 4, 220), (&b, "blue", 6, 660)] {
            let mut command = crate::command(&ffmpeg);
            command
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                ])
                .arg(format!("color={color}:s=256x144:r=25:d={duration}"))
                .args(["-f", "lavfi", "-i"])
                .arg(format!("sine=frequency={tone}:duration={duration}"))
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-shortest",
                ])
                .arg(path);
            let (status, log) =
                crate::process::capture(command, Duration::from_secs(15), true).unwrap();
            assert!(status.success(), "{log}");
        }
        let mut command = crate::command(&ffmpeg);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=0.15*sin(2*PI*(200*t+5*t*t)):s=48000:d=20",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&wav);
        let (status, log) =
            crate::process::capture(command, Duration::from_secs(15), true).unwrap();
        assert!(status.success(), "{log}");
        for restart in [false, true] {
            for clip_audio in [false, true] {
                let project = request(
                    folder.path(),
                    ffmpeg.clone(),
                    wav.clone(),
                    a.clone(),
                    b.clone(),
                    restart,
                    clip_audio,
                );
                let player = Player::new(|| {});
                let wait = |predicate: &dyn Fn(&Snapshot) -> bool| {
                    let deadline = Instant::now() + Duration::from_secs(15);
                    loop {
                        let snapshot = player.snapshot();
                        assert_ne!(
                            snapshot.phase,
                            Phase::Error,
                            "restart={restart} mix={clip_audio}: {:?}",
                            snapshot.error
                        );
                        if predicate(&snapshot) {
                            return snapshot;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "Timed out restart={restart} mix={clip_audio}: {snapshot:?}"
                        );
                        thread::sleep(Duration::from_millis(20));
                    }
                };
                let opened_at = Instant::now();
                player.open_project(project.clone(), 0.0, true);
                wait(&|s| s.phase == Phase::Ready && s.frame.is_some());
                let first_frame_ms = opened_at.elapsed().as_secs_f64() * 1000.0;
                let first = wait(&|s| s.phase == Phase::Ready && s.position > 0.25);
                let device_progress_ms = opened_at.elapsed().as_secs_f64() * 1000.0;
                assert!(first.audio_available, "{:?}", first.warning);
                assert!(first.warning.is_none(), "{:?}", first.warning);
                assert!((first.duration - 5.0).abs() < 0.001);
                let center = |frame: &Frame| {
                    let pixel = (frame.height / 2 * frame.width + frame.width / 2) * 3;
                    (frame.rgb[pixel], frame.rgb[pixel + 2])
                };
                let (red, blue) = center(first.frame.as_ref().unwrap());
                assert!(red > blue + 80);
                player.set_playing(false);
                thread::sleep(Duration::from_millis(120));
                let paused = player.snapshot().position;
                thread::sleep(Duration::from_millis(100));
                assert!((player.snapshot().position - paused).abs() < 0.03);
                let seam = if restart { 4.0 } else { 1.0 };
                let sought_at = Instant::now();
                player.seek(seam - 0.1);
                wait(&|s| {
                    s.phase == Phase::Ready
                        && s.frame.as_ref().is_some_and(|f| f.position >= seam - 0.11)
                });
                let seek_ms = sought_at.elapsed().as_secs_f64() * 1000.0;
                player.set_playing(true);
                let seam_max_delta_ms = std::cell::Cell::new(0.0f64);
                let after = wait(&|s| {
                    if s.phase == Phase::Ready
                        && s.playing
                        && let Some(frame) = s.frame.as_ref().filter(|f| f.position >= seam - 0.12)
                    {
                        let delta_ms = (frame.position - s.position).abs() * 1000.0;
                        seam_max_delta_ms.set(seam_max_delta_ms.get().max(delta_ms));
                    }
                    s.position > seam + 0.15 && s.frame.as_ref().is_some_and(|f| f.position >= seam)
                });
                let (red, blue) = center(after.frame.as_ref().unwrap());
                assert!(blue > red + 80, "Expected B after visual seam");
                assert!(after.audio_available && after.warning.is_none());
                assert!((after.frame.as_ref().unwrap().position - after.position).abs() < 0.2);
                player.seek(4.85);
                let ended = wait(&|s| s.phase == Phase::Ended);
                assert_eq!(ended.position, 5.0);
                assert!(!ended.playing);
                assert!(
                    !folder.path().join("unused.mp4").exists(),
                    "Direct playback rendered an intermediate output"
                );
                player.stop();
                assert_eq!(player.snapshot().phase, Phase::Empty);
                let warm_at = Instant::now();
                player.open_project(project, 0.0, false);
                let warm = wait(&|s| s.phase == Phase::Ready && s.frame.is_some());
                let warm_first_frame_ms = warm_at.elapsed().as_secs_f64() * 1000.0;
                assert!(warm.audio_available && warm.warning.is_none());
                eprintln!(
                    "PLAYBACK_TIMING {}",
                    serde_json::json!({
                        "restart": restart, "clip_audio": clip_audio,
                        "first_frame_ms": first_frame_ms, "device_progress_250ms_ms": device_progress_ms,
                        "seek_frame_ms": seek_ms, "warm_first_frame_ms": warm_first_frame_ms,
                    "seam_frame_audio_delta_ms": (after.frame.as_ref().unwrap().position - after.position).abs() * 1000.0,
                    "seam_max_frame_audio_delta_ms": seam_max_delta_ms.get(),
                    })
                );
                player.stop();
            }
        }
    }
}
