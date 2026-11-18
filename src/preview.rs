//! Bounded still previews decoded from original inputs, independent of export.
//!
//! Workers own their FFmpeg trees. Cursor requests replace stale work; source
//! thumbnails use a small FIFO. Identity checks and all media IO stay off-thread.
use crate::{
    engine::{EngineError, MediaItem, Reporter},
    inspection::{FileStamp, Snapshot},
    project::ProjectRequest,
};
use std::{
    collections::{HashMap, VecDeque},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub const MAX_FRAME_EDGE: u32 = 640;
pub const THUMBNAIL_WIDTH: u32 = 224;
pub const THUMBNAIL_HEIGHT: u32 = 126;
/// A first frame at monitor size (the monitor is at most 640 × 360, 2× on HiDPI).
pub const STILL_WIDTH: u32 = 1280;
pub const STILL_HEIGHT: u32 = 720;
pub const MAX_CACHE_FRAMES: usize = 24;
pub const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_QUEUED_THUMBNAILS: usize = 16;
const DEADLINE: Duration = Duration::from_secs(30);
const DIAGNOSTIC_BYTES: usize = 8192;
const MAX_WORKSPACES: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum FrameRequest {
    /// `time_ms` uses the full WAV clock, including when a short is selected.
    /// The short, if present, determines visual restart and framing; the requested
    /// clock is clamped to that selection's half-open presentation interval.
    Project {
        project: ProjectRequest,
        time_ms: u64,
    },
    /// Decode the first source frame. Neither WAV nor export settings are needed.
    Thumbnail { item: MediaItem, ffmpeg: PathBuf },
    /// The same first frame at monitor size, for the monitor while there is
    /// no montage to decode (no song, or a file still unreadable).
    Still { item: MediaItem, ffmpeg: PathBuf },
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Opaque sRGB-like FFmpeg RGBA pixels, row-major, no row padding.
    pub rgba: Vec<u8>,
}
impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba_bytes", &self.rgba.len())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PreviewError {
    #[error("Still image uses the frame renderer")]
    StillImage,
    #[error("Preview cancelled")]
    Cancelled,
    #[error("Preview inputs changed")]
    SourceChanged,
    #[error("{0}")]
    Unavailable(String),
}
type Result<T> = std::result::Result<T, PreviewError>;

#[derive(Debug)]
pub struct FrameReply {
    pub revision: u64,
    pub request: FrameRequest,
    pub result: Result<Arc<Frame>>,
}
struct Pending {
    revision: u64,
    epoch: u64,
    request: FrameRequest,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
struct Mailbox {
    revision: u64,
    epoch: u64,
    latest: Option<Pending>,
    queue: VecDeque<Pending>,
    replies: VecDeque<FrameReply>,
    active: Option<(FrameRequest, Arc<AtomicBool>)>,
    shutdown: bool,
}

/// A serial decoder with a bounded cache. Keep independent workers for the
/// cursor and thumbnails, so source-list work cannot delay scrubbing.
pub struct FrameWorker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
}
impl FrameWorker {
    pub fn new(repaint: impl Fn() + Send + Sync + 'static) -> Self {
        let worker_lease = WorkspaceLease::new();
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let worker = shared.clone();
        thread::spawn(move || {
            let _lease = worker_lease;
            worker_loop(worker, repaint);
        });
        Self { shared }
    }

    /// Replaces pending cursor work and invalidates old replies without file IO.
    pub fn request(&self, request: FrameRequest) -> u64 {
        let mut mailbox = self.shared.0.lock().unwrap();
        invalidate(&mut mailbox);
        mailbox.revision = mailbox.revision.wrapping_add(1);
        let revision = mailbox.revision;
        mailbox.latest = Some(Pending {
            revision,
            epoch: mailbox.epoch,
            request,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        self.shared.1.notify_one();
        revision
    }

    /// Bounded, deduplicated FIFO for thumbnails. Returns None when full; callers
    /// can retry after a reply. It never cancels the current thumbnail decoder.
    pub fn enqueue(&self, request: FrameRequest) -> Option<u64> {
        let mut mailbox = self.shared.0.lock().unwrap();
        if !matches!(
            &request,
            FrameRequest::Thumbnail { .. } | FrameRequest::Still { .. }
        ) || mailbox.queue.len() >= MAX_QUEUED_THUMBNAILS
            || mailbox.queue.iter().any(|p| p.request == request)
            || mailbox.active.as_ref().is_some_and(|p| p.0 == request)
        {
            return None;
        }
        mailbox.revision = mailbox.revision.wrapping_add(1);
        let revision = mailbox.revision;
        let epoch = mailbox.epoch;
        mailbox.queue.push_back(Pending {
            revision,
            epoch,
            request,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        self.shared.1.notify_one();
        Some(revision)
    }

    pub fn try_recv(&self) -> Option<FrameReply> {
        self.shared.0.lock().unwrap().replies.pop_front()
    }

    /// Process termination and reaping happen on the worker, never on the UI.
    pub fn cancel(&self) {
        invalidate(&mut self.shared.0.lock().unwrap());
    }
}
impl Drop for FrameWorker {
    fn drop(&mut self) {
        let mut mailbox = self.shared.0.lock().unwrap();
        invalidate(&mut mailbox);
        mailbox.shutdown = true;
        self.shared.1.notify_one();
    }
}
fn invalidate(mailbox: &mut Mailbox) {
    if let Some((_, cancel)) = &mailbox.active {
        cancel.store(true, Ordering::Relaxed);
    }
    mailbox.epoch = mailbox.epoch.wrapping_add(1);
    mailbox.latest = None;
    mailbox.queue.clear();
    mailbox.replies.clear();
}

struct CachedFrame {
    request: FrameRequest,
    snapshot: Snapshot,
    frame: Arc<Frame>,
}
#[derive(Default)]
struct Cache {
    frames: VecDeque<CachedFrame>,
    bytes: usize,
}
struct Prepared {
    snapshot: Snapshot,
    items: Vec<MediaItem>,
    info: Vec<crate::media::MediaInfo>,
    origins: HashMap<PathBuf, f64>,
    target: crate::plan::Target,
    duration: f64,
}
static PREPARED: OnceLock<Mutex<Option<Arc<Prepared>>>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkspaceKey {
    captions: Option<(FileStamp, crate::captions::CaptionStyle)>,
    source_ms: u64,
    duration_bits: u64,
    start_ms: u64,
    end_ms: u64,
    canvas: (u32, u32),
}
struct CaptionWorkspace {
    directory: Arc<tempfile::TempDir>,
    path: PathBuf,
    has_captions: bool,
}
type Workspaces = VecDeque<(WorkspaceKey, Arc<CaptionWorkspace>)>;
type WorkspaceStore = Mutex<Workspaces>;
static WORKSPACES: OnceLock<Mutex<Weak<WorkspaceStore>>> = OnceLock::new();

/// Keeps immutable caption assets cached across seeks. Cache ownership belongs
/// to live workers/players, so final teardown cleans private temporary files.
#[derive(Clone)]
pub(crate) struct WorkspaceLease {
    _store: Arc<WorkspaceStore>,
}
impl WorkspaceLease {
    pub(crate) fn new() -> Self {
        Self {
            _store: workspace_store(),
        }
    }
}
fn workspace_store() -> Arc<WorkspaceStore> {
    let mut weak = WORKSPACES
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .unwrap();
    if let Some(store) = weak.upgrade() {
        return store;
    }
    let store = Arc::new(Mutex::new(VecDeque::new()));
    *weak = Arc::downgrade(&store);
    store
}
impl Cache {
    fn get(&mut self, request: &FrameRequest, snapshot: &Snapshot) -> Option<Arc<Frame>> {
        let index = self
            .frames
            .iter()
            .position(|f| same_request(&f.request, request) && f.snapshot == *snapshot)?;
        let entry = self.frames.remove(index).unwrap();
        let frame = entry.frame.clone();
        self.frames.push_back(entry);
        Some(frame)
    }
    fn insert(&mut self, request: FrameRequest, snapshot: Snapshot, frame: Arc<Frame>) {
        let bytes = frame.rgba.len();
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        while self.frames.len() >= MAX_CACHE_FRAMES || self.bytes + bytes > MAX_CACHE_BYTES {
            self.bytes -= self.frames.pop_front().unwrap().frame.rgba.len();
        }
        self.bytes += bytes;
        self.frames.push_back(CachedFrame {
            request,
            snapshot,
            frame,
        });
    }
}
fn worker_loop(shared: Arc<(Mutex<Mailbox>, Condvar)>, repaint: impl Fn()) {
    let mut cache = Cache::default();
    loop {
        let pending = {
            let mut mailbox = shared.0.lock().unwrap();
            while mailbox.latest.is_none() && mailbox.queue.is_empty() && !mailbox.shutdown {
                mailbox = shared.1.wait(mailbox).unwrap();
            }
            if mailbox.shutdown {
                return;
            }
            let pending = mailbox
                .latest
                .take()
                .or_else(|| mailbox.queue.pop_front())
                .unwrap();
            mailbox.active = Some((pending.request.clone(), pending.cancel.clone()));
            pending
        };
        let result = extract_cached(&pending.request, &pending.cancel, &mut cache);
        let delivered = {
            let mut mailbox = shared.0.lock().unwrap();
            mailbox.active = None;
            if mailbox.shutdown {
                return;
            }
            if mailbox.epoch != pending.epoch || pending.cancel.load(Ordering::Relaxed) {
                false
            } else {
                if mailbox.replies.len() >= MAX_QUEUED_THUMBNAILS {
                    mailbox.replies.pop_front();
                }
                mailbox.replies.push_back(FrameReply {
                    revision: pending.revision,
                    request: pending.request,
                    result,
                });
                true
            }
        };
        if delivered {
            repaint();
        }
    }
}

/// Synchronous embedding/test entry point. Interactive callers use FrameWorker.
pub fn extract(request: &FrameRequest, cancel: &AtomicBool) -> Result<Frame> {
    let _lease = WorkspaceLease::new();
    let frame = extract_cached(request, cancel, &mut Cache::default())?;
    Arc::try_unwrap(frame).map_err(|_| unavailable("Preview ownership error"))
}

fn identify(request: &FrameRequest) -> Result<Snapshot> {
    match request {
        FrameRequest::Project { project, .. } => project.snapshot().map_err(engine_error),
        FrameRequest::Thumbnail { item, ffmpeg } | FrameRequest::Still { item, ffmpeg } => {
            let decoder = crate::inspection::resolve_ffmpeg(ffmpeg).map_err(engine_error)?;
            Snapshot::read([item.path(), &decoder]).map_err(engine_error)
        }
    }
}
fn extract_cached(
    request: &FrameRequest,
    cancel: &AtomicBool,
    cache: &mut Cache,
) -> Result<Arc<Frame>> {
    let started = Instant::now();
    check(cancel, started)?;
    let snapshot = identify(request)?;
    check(cancel, started)?;
    if let Some(frame) = cache.get(request, &snapshot) {
        snapshot.verify().map_err(engine_error)?;
        check(cancel, started)?;
        return Ok(frame);
    }
    let frame = match request {
        FrameRequest::Thumbnail { item, .. } => thumbnail(
            item,
            &snapshot,
            (THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT),
            cancel,
            started,
        )?,
        FrameRequest::Still { item, .. } => thumbnail(
            item,
            &snapshot,
            (STILL_WIDTH, STILL_HEIGHT),
            cancel,
            started,
        )?,
        FrameRequest::Project { project, time_ms } => {
            let prepared = prepare_shared(project, &snapshot, cancel, started)?;
            project_frame(project, *time_ms, &snapshot, &prepared, cancel, started)?
        }
    };
    snapshot.verify().map_err(engine_error)?;
    check(cancel, started)?;
    let frame = Arc::new(frame);
    cache.insert(request.clone(), snapshot, frame.clone());
    Ok(frame)
}

fn prepare_shared(
    project: &ProjectRequest,
    snapshot: &Snapshot,
    cancel: &AtomicBool,
    started: Instant,
) -> Result<Arc<Prepared>> {
    let media_snapshot = Snapshot(snapshot.0[..project.montage.items.len() + 2].to_vec());
    let lock = PREPARED.get_or_init(|| Mutex::new(None));
    // Cursor and playback reuse the same bounded inspection. Waiting for the
    // other worker remains cancellable and never acquires a UI-thread lock.
    let mut cached = loop {
        check(cancel, started)?;
        match lock.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(20)),
            Err(_) => return Err(unavailable("Preview inspection cache is unavailable")),
        }
    };
    if let Some(prepared) = cached.as_ref()
        && prepared.snapshot == media_snapshot
        && prepared.items == project.montage.items
    {
        return Ok(prepared.clone());
    }
    *cached = None;
    let prepared = Arc::new(prepare(project, media_snapshot, cancel, started)?);
    *cached = Some(prepared.clone());
    Ok(prepared)
}

fn prepare(
    project: &ProjectRequest,
    snapshot: Snapshot,
    cancel: &AtomicBool,
    started: Instant,
) -> Result<Prepared> {
    let montage = &project.montage;
    let mut items = montage.items.clone();
    for (item, stamp) in items.iter_mut().zip(&snapshot.0) {
        *item.path_mut() = stamp.resolved.clone();
    }
    let ffmpeg = &snapshot.0[items.len() + 1].resolved;
    let duration = crate::wav_duration(&snapshot.0[items.len()].resolved).map_err(unavailable)?;
    if !duration.is_finite()
        || duration <= 0.0
        || duration * 1000.0 > crate::shorts::MAX_SOURCE_MS as f64
    {
        return Err(unavailable(
            "Preview requires a positive WAV duration of at most two hours",
        ));
    }
    let mut origins = HashMap::new();
    let mut discard = |_| {};
    let mut report = Reporter::new(&mut discard);
    let inspection = crate::sequence::inspect_with(
        ffmpeg,
        &items,
        true,
        false,
        false,
        &mut report,
        (
            |decoder: &Path, path: &Path, is_image: bool| {
                check(cancel, started).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                let deadline =
                    remaining(started).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                if is_image {
                    crate::media::image_info_interruptible(decoder, path, cancel, deadline)
                } else {
                    let timed =
                        crate::media::inspect_timed_interruptible(decoder, path, cancel, deadline)?;
                    crate::captions::supported_video(&timed.video)?;
                    origins.insert(path.to_path_buf(), timed.video_start);
                    Ok(timed.video)
                }
            },
            crate::h264::inspect,
        ),
    )
    .map_err(box_error)?;
    let target = inspection.decision.public.target;
    let fps = target.rate_num as f64 / target.rate_den as f64;
    if !fps.is_finite()
        || !(0.0..=240.0).contains(&fps)
        || fps == 0.0
        || target.width > 16384
        || target.height > 16384
        || u64::from(target.width) * u64::from(target.height) > 64_000_000
    {
        return Err(unavailable(
            "Preview source geometry or frame rate exceeds its limit",
        ));
    }
    snapshot.verify().map_err(engine_error)?;
    Ok(Prepared {
        snapshot,
        items: montage.items.clone(),
        info: inspection.info,
        origins,
        target,
        duration,
    })
}

fn thumbnail(
    item: &MediaItem,
    snapshot: &Snapshot,
    (width, height): (u32, u32),
    cancel: &AtomicBool,
    started: Instant,
) -> Result<Frame> {
    let source = &snapshot.0[0].resolved;
    let mut command = base_command(&snapshot.0[1].resolved);
    let orientation = if item.is_image() {
        let codec = crate::images::codec(source).map_err(engine_error)?;
        let orientation = if codec == "png" {
            crate::images::png_orientation(source).map_err(unavailable)?
        } else {
            0
        };
        command.args(crate::images::input_args(source, 25, 1).map_err(unavailable)?);
        crate::images::orientation_filter(orientation).to_owned()
    } else {
        command.arg("-i").arg(source);
        // Read an untagged video the way players do before the RGB conversion.
        crate::color::decode_tags(&snapshot.0[1].resolved, source)
            .map(|tags| format!("{tags},"))
            .unwrap_or_default()
    };
    // Composite alpha onto black before conversion. SAR and decoder autorotation
    // contribute to DAR, so neither non-square pixels nor portraits are stretched.
    // Accurate rounding: one picture, and the chart's worst patch goes from 10 to 1.
    let filter = format!(
        "color=c=black:s={w}x{h}:r=25,format=rgba[bg];[0:v]{orientation}scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)':flags=bicubic+accurate_rnd+full_chroma_int,setsar=1,format=rgba[image];[bg][image]overlay=x=(W-w)/2:y=(H-h)/2:format=rgb:alpha=straight,format=rgba[v]",
        w = width,
        h = height
    );
    command.args(["-filter_complex", &filter, "-map", "[v]"]);
    decode_frame(
        command,
        width,
        height,
        (STILL_WIDTH, STILL_HEIGHT),
        cancel,
        started,
    )
}

fn project_frame(
    project: &ProjectRequest,
    time_ms: u64,
    snapshot: &Snapshot,
    prepared: &Prepared,
    cancel: &AtomicBool,
    started: Instant,
) -> Result<Frame> {
    let local = cursor_local(project, time_ms, prepared)?;
    let segment = project_command(project, local, snapshot, prepared, false)?;
    decode_frame(
        segment.command,
        segment.width,
        segment.height,
        (MAX_FRAME_EDGE, MAX_FRAME_EDGE),
        cancel,
        started,
    )
}

fn cursor_local(project: &ProjectRequest, time_ms: u64, prepared: &Prepared) -> Result<f64> {
    let (start, end, restart) =
        project
            .short
            .as_ref()
            .map_or((0.0, prepared.duration, false), |s| {
                (
                    s.start_ms as f64 / 1000.0,
                    s.end_ms as f64 / 1000.0,
                    s.restart_loops,
                )
            });
    let span = end - start;
    let local = (time_ms as f64 / 1000.0 - start)
        .max(0.0)
        .min((span - 1e-9).max(0.0));
    let fps = prepared.target.rate_num as f64 / prepared.target.rate_den as f64;
    let durations: Vec<_> = prepared
        .info
        .iter()
        .map(|m| (m.seconds * fps).round().max(1.0) / fps)
        .collect();
    let visual_start = if restart { 0.0 } else { start };
    let first = crate::project::preview_pieces(&durations, visual_start, span, false, 1)
        .map_err(engine_error)?[0];
    let current = crate::project::preview_pieces(&durations, visual_start + local, span, false, 1)
        .map_err(engine_error)?[0];
    // A selected start need not lie on the frame grid. Each prepared occurrence
    // starts its own grid; quantizing the global local clock can cross a seam
    // backwards (13.015 -> first A/B boundary at local 0.985).
    let occurrence_start = if local < first.duration {
        0.0
    } else {
        local - current.offset
    };
    Ok(on_grid(local, occurrence_start, fps))
}
/// The time of the frame `local` falls in, on the grid of `fps` frames a
/// second that starts at `origin`: the picture a paused playhead shows.
fn on_grid(local: f64, origin: f64, fps: f64) -> f64 {
    origin + (((local - origin) * fps + 1e-8).floor() / fps)
}

/// The montage as one virtual file libmpv can play and seek in: the sources'
/// light copies one after another, repeated over the song, and the song
/// beside them. It is refused, so that the usual player plays instead, for
/// what this does not cover: clip sound mixed in, captions drawn by libass, a
/// still image, a copy that is not ready.
#[cfg(feature = "gui")]
pub(crate) struct PlayPlan {
    pub edl: String,
    pub filter: String,
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub grid: PlayGrid,
    /// The copies stay in the store's hands while they play.
    pub copies: Vec<Arc<crate::copies::Light>>,
}
/// The frame grids of the file given to libmpv: one for each piece, as the
/// paused preview counts them (`cursor_local`).
#[cfg(feature = "gui")]
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PlayGrid {
    fps: f64,
    /// For each piece, where it starts in the file and where its grid does.
    pieces: Vec<(f64, f64)>,
}
#[cfg(feature = "gui")]
impl PlayGrid {
    /// Where to seek in the file for the picture a playhead at `seconds`
    /// shows: the time of the frame it falls in, not the time itself. libmpv
    /// shows the first picture at or after the time it is given, which between
    /// two frames is the next one; the paused preview and the export show the
    /// one before.
    pub fn frame_time(&self, seconds: f64) -> f64 {
        let Some(&(start, origin)) = self
            .pieces
            .iter()
            .rev()
            .find(|(start, _)| *start <= seconds + 1e-9)
        else {
            return seconds.max(0.0);
        };
        if self.fps <= 0.0 {
            return seconds;
        }
        on_grid(seconds, origin, self.fps).max(start)
    }
}
/// Why light copies cannot stand for these sources, by their colours. Not
/// reached today: the preview refuses a BT.2020 source at its preparation
/// (`captions::supported_video`), whoever would play. It holds the day that
/// rule is relaxed.
#[cfg(feature = "gui")]
fn play_color_refusal(inputs: &[crate::color::Input], canvas: (u32, u32)) -> Option<&'static str> {
    // A BT.2020 project keeps every clip as decoded, which a light copy is
    // not.
    if crate::color::Plan::new(inputs, canvas, None).matrix == crate::color::Matrix::Bt2020 {
        Some("the montage is in BT.2020")
    } else if inputs.iter().any(|input| input.light_copy().is_none()) {
        Some("a source is in BT.2020")
    } else {
        None
    }
}
/// Why libmpv does not play this project, as far as the request alone says.
#[cfg(feature = "gui")]
fn play_refusal(project: &ProjectRequest) -> Option<&'static str> {
    if project.montage.clip_audio {
        // One sound stream beside the pictures: the song.
        Some("the montage mixes the clips' sound")
    } else if project.captions.is_some() && project.active_effect().is_none() {
        // The effect layer draws over libmpv's picture; libass draws in it.
        Some("the captions are drawn by libass")
    } else if project.montage.items.iter().any(|item| item.is_image()) {
        Some("the montage has a still image")
    } else {
        None
    }
}
/// The parts of the sources to play in turn, as (item, offset, length) in
/// seconds: the montage's pieces over the selection, the last one cut where
/// the selection ends.
#[cfg(feature = "gui")]
fn play_pieces(
    durations: &[f64],
    start: f64,
    span: f64,
    restart: bool,
) -> Result<Vec<(usize, f64, f64)>> {
    let mut parts = Vec::new();
    let mut at = 0.0;
    for piece in crate::project::pieces(durations, start, span, restart).map_err(engine_error)? {
        let length = piece.duration.min(span - at);
        if length <= 1e-6 {
            break;
        }
        parts.push((piece.item, piece.offset, length));
        at += length;
    }
    Ok(parts)
}
/// mpv's EDL for those parts, each a (file, offset, length), with the song
/// from `start` as a second stream. A name is given with its length in bytes,
/// so that it may hold any character.
#[cfg(feature = "gui")]
fn play_edl(parts: &[(String, f64, f64)], song: &str, start: f64, span: f64) -> String {
    let segments: Vec<_> = parts
        .iter()
        .map(|(file, offset, length)| {
            format!(
                "%{}%{file},start={offset:.6},length={length:.6}",
                file.len()
            )
        })
        .collect();
    format!(
        "edl://{};!new_stream;%{}%{song},start={start:.6},length={span:.6}",
        segments.join(";"),
        song.len()
    )
}
/// The filter over that file: every copy brought to the montage's reduced
/// frame, the fades, then the short's framing. The file's clock is the
/// selection's and the fades are on the project's, `start` seconds later.
#[cfg(feature = "gui")]
fn play_filter(
    frame: (u32, u32),
    fade_in: f64,
    fade_out: f64,
    start: f64,
    project_seconds: f64,
    framing: Option<String>,
) -> String {
    let mut filter = vec![format!(
        "scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)',setsar=1,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,format=yuv420p",
        w = frame.0,
        h = frame.1,
    )];
    let fades = fade_in > 0.0 || fade_out > 0.0;
    if fades {
        filter.push(format!("setpts=PTS+{start:.9}/TB"));
    }
    if fade_in > 0.0 {
        filter.push(format!("fade=t=in:st=0:d={fade_in:.9}"));
    }
    if fade_out > 0.0 {
        filter.push(format!(
            "fade=t=out:st={:.9}:d={fade_out:.9}",
            project_seconds - fade_out
        ));
    }
    if fades {
        filter.push(format!("setpts=PTS-{start:.9}/TB"));
    }
    filter.extend(framing);
    filter.join(",")
}
#[cfg(feature = "gui")]
pub(crate) fn play_plan(project: &ProjectRequest, cancel: &AtomicBool) -> Result<PlayPlan> {
    if let Some(reason) = play_refusal(project) {
        return Err(unavailable(reason));
    }
    let started = Instant::now();
    let snapshot = project.snapshot().map_err(engine_error)?;
    let prepared = prepare_shared(project, &snapshot, cancel, started)?;
    let montage = &project.montage;
    let target = &prepared.target;
    // Every item is a video here: a still image is refused above.
    let inputs: Vec<_> = prepared
        .info
        .iter()
        .map(crate::color::Input::video)
        .collect();
    if let Some(reason) = play_color_refusal(&inputs, (target.width, target.height)) {
        return Err(unavailable(reason));
    }
    let (start, end, restart) =
        project
            .short
            .as_ref()
            .map_or((0.0, prepared.duration, false), |s| {
                (
                    s.start_ms as f64 / 1000.0,
                    s.end_ms as f64 / 1000.0,
                    s.restart_loops,
                )
            });
    let span = end - start;
    let fps = target.rate_num as f64 / target.rate_den as f64;
    let durations: Vec<_> = prepared
        .info
        .iter()
        .map(|m| (m.seconds * fps).round().max(1.0) / fps)
        .collect();
    let mut parts = Vec::new();
    let mut copies = Vec::new();
    let mut pieces = Vec::new();
    let mut at = 0.0;
    for (item, offset, length) in play_pieces(&durations, start, span, restart)? {
        // The first piece counts its frames from the selection's start, the
        // others from their source's (`cursor_local`).
        pieces.push((at, if pieces.is_empty() { 0.0 } else { at - offset }));
        at += length;
        let source = &prepared.snapshot.0[item];
        let origin = prepared
            .origins
            .get(&source.resolved)
            .copied()
            .unwrap_or(0.0);
        let copy = crate::copies::ready(source, origin)
            .ok_or_else(|| unavailable("a light copy is not ready"))?;
        parts.push((copy.path.to_string_lossy().into_owned(), offset, length));
        copies.push(copy);
    }
    if parts.is_empty() {
        return Err(unavailable("the montage has no picture to play"));
    }
    let song = snapshot.0[montage.items.len()]
        .resolved
        .to_string_lossy()
        .into_owned();
    let (canvas_w, canvas_h) = if project.short.is_some() {
        (1080, 1920)
    } else {
        (target.width, target.height)
    };
    let (width, height) = reduced_dimensions(canvas_w, canvas_h);
    let framing = match &project.short {
        Some(short) => Some(
            crate::shorts::Geometry::new(
                f64::from(target.width),
                f64::from(target.height),
                short.framing,
            )
            .map_err(engine_error)?
            .monitor_filter(width, height),
        ),
        None => None,
    };
    let filter = play_filter(
        reduced_dimensions(target.width, target.height),
        montage.fade_in,
        montage.fade_out,
        start,
        prepared.duration,
        framing,
    );
    snapshot.verify().map_err(engine_error)?;
    crate::profile!(
        "mpv player: {} pieces over {span:.3} s, filter {filter}",
        parts.len()
    );
    Ok(PlayPlan {
        edl: play_edl(&parts, &song, start, span),
        filter,
        width,
        height,
        duration: span,
        grid: PlayGrid { fps, pieces },
        copies,
    })
}

/// Source-level working copy plus composition for a persistent silent decoder.
/// Construct off the UI thread; the workspace owns any reviewed ASS/font files.
#[cfg(feature = "gui")]
pub(crate) struct ScrubPlan {
    pub source: FileStamp,
    pub ffmpeg: PathBuf,
    pub origin: f64,
    pub seconds: f64,
    pub filter: String,
    pub width: u32,
    pub height: u32,
    pub workspace: Arc<tempfile::TempDir>,
    pub captions: Option<PathBuf>,
    pub caption_delay: f64,
}

#[cfg(feature = "gui")]
pub(crate) fn scrub_plan(
    project: &ProjectRequest,
    time_ms: u64,
    cancel: &AtomicBool,
) -> Result<ScrubPlan> {
    let started = Instant::now();
    let snapshot = project.snapshot().map_err(engine_error)?;
    let prepared = prepare_shared(project, &snapshot, cancel, started)?;
    let local = cursor_local(project, time_ms, &prepared)?;
    let mapped = map_piece(project, local, &prepared)?;
    // Still images retain the established alpha/orientation composition path.
    if project.montage.items[mapped.item].is_image() {
        return Err(PreviewError::StillImage);
    }
    let target = &prepared.target;
    let origin = prepared
        .origins
        .get(&mapped.playback.source)
        .copied()
        .unwrap_or(0.0);
    let source_seconds = (mapped.playback.source_seconds - origin).max(0.0);
    let fps = target.rate_num as f64 / target.rate_den as f64;
    let seconds = source_seconds.min((prepared.info[mapped.item].seconds - 1.0 / fps).max(0.0));
    let (canvas_w, canvas_h) = if project.short.is_some() {
        (1080, 1920)
    } else {
        (target.width, target.height)
    };
    let workspace = caption_workspace(project, &snapshot, &prepared, (canvas_w, canvas_h))?;
    let (width, height) = reduced_dimensions(canvas_w, canvas_h);
    let (base_w, base_h) = reduced_dimensions(target.width, target.height);
    let occurrence_start = mapped.clock - source_seconds;
    let fade_in = project.montage.fade_in > 0.0 && occurrence_start < project.montage.fade_in;
    let fade_out = project.montage.fade_out > 0.0
        && mapped.clock + mapped.playback.local_end - local
            > prepared.duration - project.montage.fade_out;
    let timed = fade_in || fade_out;
    let mut filter = vec![format!(
        "scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)',setsar=1,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,format=yuv420p",
        w = base_w,
        h = base_h,
    )];
    if timed {
        filter.push(format!("setpts=PTS+{occurrence_start:.9}/TB"));
    }
    if fade_in {
        filter.push(format!("fade=t=in:st=0:d={:.9}", project.montage.fade_in));
    }
    if fade_out {
        filter.push(format!(
            "fade=t=out:st={:.9}:d={:.9}",
            prepared.duration - project.montage.fade_out,
            project.montage.fade_out
        ));
    }
    let start = project
        .short
        .as_ref()
        .map_or(0.0, |s| s.start_ms as f64 / 1000.0);
    if timed {
        filter.push(format!("setpts=PTS-{start:.9}/TB"));
    }
    if let Some(short) = &project.short {
        filter.push(
            crate::shorts::Geometry::new(
                f64::from(target.width),
                f64::from(target.height),
                short.framing,
            )
            .map_err(engine_error)?
            .monitor_filter(width, height),
        );
    }
    // Restore source timestamps after composition so exact seek still compares
    // decoder and requested times. STARTPTS would reset on every seek.
    if timed {
        filter.push(format!(
            "setpts=PTS-{offset:.9}/TB",
            offset = local - source_seconds
        ));
    }
    snapshot.verify().map_err(engine_error)?;
    check(cancel, started)?;
    Ok(ScrubPlan {
        source: snapshot.0[mapped.item].clone(),
        ffmpeg: snapshot.0[project.montage.items.len() + 1].resolved.clone(),
        origin,
        seconds,
        filter: filter.join(","),
        width,
        height,
        workspace: workspace.directory.clone(),
        captions: workspace
            .has_captions
            .then(|| workspace.path.join("captions.ass")),
        caption_delay: source_seconds - local,
    })
}

/// One original-source visual occurrence. Keep `workspace` alive until the owned
/// decoder exits: it contains the reviewed caption script and qualified font.
pub(crate) struct PlaybackSegment {
    pub command: Command,
    pub local_start: f64,
    pub local_end: f64,
    pub width: u32,
    pub height: u32,
    pub workspace: Arc<tempfile::TempDir>,
    #[cfg(test)]
    pub source: PathBuf,
    /// Accurate-seek time in the source's default demux playback origin.
    #[cfg(test)]
    pub source_seconds: f64,
}

/// Original media mapping without a video command or caption/font workspace.
pub(crate) struct PlaybackPiece {
    pub local_start: f64,
    pub local_end: f64,
    pub source: PathBuf,
    pub source_seconds: f64,
    pub source_audio: bool,
}

pub(crate) fn playback_piece(
    project: &ProjectRequest,
    local_seconds: f64,
    cancel: &AtomicBool,
) -> Result<PlaybackPiece> {
    let started = Instant::now();
    check(cancel, started)?;
    if !local_seconds.is_finite() || local_seconds < 0.0 {
        return Err(unavailable("Playback clock must be finite and nonnegative"));
    }
    let snapshot = project.snapshot().map_err(engine_error)?;
    let prepared = prepare_shared(project, &snapshot, cancel, started)?;
    let mapped = map_piece(project, local_seconds, &prepared)?;
    snapshot.verify().map_err(engine_error)?;
    check(cancel, started)?;
    Ok(mapped.playback)
}

struct MappedPiece {
    item: usize,
    clock: f64,
    playback: PlaybackPiece,
}

fn map_piece(
    project: &ProjectRequest,
    local_seconds: f64,
    prepared: &Prepared,
) -> Result<MappedPiece> {
    let (start, end, restart) =
        project
            .short
            .as_ref()
            .map_or((0.0, prepared.duration, false), |s| {
                (
                    s.start_ms as f64 / 1000.0,
                    s.end_ms as f64 / 1000.0,
                    s.restart_loops,
                )
            });
    if end > prepared.duration + 0.000_000_1 {
        return Err(unavailable("Selected range exceeds the WAV duration"));
    }
    let span = end - start;
    if local_seconds >= span {
        return Err(unavailable(
            "Playback position has reached the selected end",
        ));
    }
    let local = local_seconds.max(0.0);
    let clock = start + local;
    let fps = prepared.target.rate_num as f64 / prepared.target.rate_den as f64;
    let durations: Vec<_> = prepared
        .info
        .iter()
        .map(|m| (m.seconds * fps).round().max(1.0) / fps)
        .collect();
    let piece = crate::project::preview_pieces(
        &durations,
        if restart { local } else { clock },
        (span - local).max(1.0 / fps),
        false,
        1,
    )
    .map_err(engine_error)?[0];
    let item = &project.montage.items[piece.item];
    let source = &prepared.snapshot.0[piece.item].resolved;
    let meta = &prepared.info[piece.item];
    let source_seconds = if item.is_image() {
        0.0
    } else {
        let origin = prepared.origins.get(source).copied().unwrap_or(0.0);
        (origin + piece.offset).max(0.0)
    };
    Ok(MappedPiece {
        item: piece.item,
        clock,
        playback: PlaybackPiece {
            local_start: local,
            local_end: local + piece.duration.min(span - local),
            source: source.clone(),
            source_seconds,
            source_audio: meta.audio && !item.is_image(),
        },
    })
}

fn caption_workspace(
    project: &ProjectRequest,
    snapshot: &Snapshot,
    prepared: &Prepared,
    canvas: (u32, u32),
) -> Result<Arc<CaptionWorkspace>> {
    let source_ms = (prepared.duration * 1000.0).ceil() as u64;
    let (start_ms, end_ms) = project
        .short
        .as_ref()
        .map_or((0, source_ms), |s| (s.start_ms, s.end_ms));
    // A caption effect is drawn over the preview by the GPU engine, not by libass.
    let key = if let Some(captions) = project
        .captions
        .as_ref()
        .filter(|_| project.active_effect().is_none())
    {
        WorkspaceKey {
            captions: Some((snapshot.0.last().unwrap().clone(), captions.style)),
            source_ms,
            duration_bits: prepared.duration.to_bits(),
            start_ms,
            end_ms,
            canvas,
        }
    } else {
        WorkspaceKey {
            captions: None,
            source_ms: 0,
            duration_bits: 0,
            start_ms: 0,
            end_ms: 0,
            canvas: (0, 0),
        }
    };
    let store = workspace_store();
    let mut workspaces = store.lock().unwrap();
    if let Some(index) = workspaces
        .iter()
        .position(|(candidate, _)| *candidate == key)
    {
        let entry = workspaces.remove(index).unwrap();
        let workspace = entry.1.clone();
        workspaces.push_back(entry);
        return Ok(workspace);
    }
    let directory = Arc::new(
        tempfile::Builder::new()
            .prefix("noh-frame-")
            .tempdir()
            .map_err(unavailable)?,
    );
    let path = directory.path().canonicalize().map_err(unavailable)?;
    let mut has_captions = false;
    if let Some((stamp, style)) = &key.captions {
        let text = crate::captions::read_srt(&stamp.resolved).map_err(engine_error)?;
        let track = crate::subtitle_srt::parse(&text, source_ms).map_err(unavailable)?;
        let track = if project.short.is_some() {
            track.trim(start_ms, end_ms).map_err(unavailable)?
        } else {
            track
        };
        if !track.cues.is_empty() {
            let caption_canvas =
                crate::captions::Canvas::new(f64::from(canvas.0), f64::from(canvas.1), *style)
                    .map_err(engine_error)?;
            let span = if project.short.is_some() {
                (end_ms - start_ms) as f64 / 1000.0
            } else {
                prepared.duration
            };
            crate::captions::stage(&track, &caption_canvas, 0.0, span, &path)
                .map_err(engine_error)?;
            has_captions = true;
        }
    }
    snapshot.verify().map_err(engine_error)?;
    let workspace = Arc::new(CaptionWorkspace {
        directory,
        path,
        has_captions,
    });
    while workspaces.len() >= MAX_WORKSPACES {
        workspaces.pop_front();
    }
    workspaces.push_back((key, workspace.clone()));
    Ok(workspace)
}

/// Build a bounded 24 fps PPM stream for the current visual occurrence. It does
/// no project render and shares the cursor worker's cached exact inspection.
pub(crate) fn playback_segment(
    project: &ProjectRequest,
    local_seconds: f64,
    cancel: &AtomicBool,
) -> Result<PlaybackSegment> {
    let started = Instant::now();
    check(cancel, started)?;
    if !local_seconds.is_finite() || local_seconds < 0.0 {
        return Err(unavailable("Playback clock must be finite and nonnegative"));
    }
    let snapshot = project.snapshot().map_err(engine_error)?;
    let prepared = prepare_shared(project, &snapshot, cancel, started)?;
    let mut segment = project_command(project, local_seconds, &snapshot, &prepared, true)?;
    if segment.local_end - segment.local_start < 1e-9 {
        return Err(unavailable("Preview occurrence has no decodable duration"));
    }
    segment.command.args([
        "-t",
        &format!("{:.9}", segment.local_end - segment.local_start),
        "-an",
        "-sn",
        "-dn",
        "-fps_mode",
        "passthrough",
        "-c:v",
        "ppm",
        "-threads",
        "1",
        "-f",
        "image2pipe",
        "pipe:1",
    ]);
    snapshot.verify().map_err(engine_error)?;
    check(cancel, started)?;
    Ok(segment)
}

fn project_command(
    project: &ProjectRequest,
    local_seconds: f64,
    snapshot: &Snapshot,
    prepared: &Prepared,
    stream: bool,
) -> Result<PlaybackSegment> {
    let montage = &project.montage;
    if montage.fade_in + montage.fade_out > prepared.duration {
        return Err(unavailable("Combined fades exceed the WAV duration"));
    }
    let target = &prepared.target;
    let mapped = map_piece(project, local_seconds, prepared)?;
    let local = mapped.playback.local_start;
    let clock = mapped.clock;
    let item = &montage.items[mapped.item];
    let source = &mapped.playback.source;
    let ffmpeg = &prepared.snapshot.0[montage.items.len() + 1].resolved;
    let meta = &prepared.info[mapped.item];
    let (canvas_w, canvas_h) = if project.short.is_some() {
        (1080, 1920)
    } else {
        (target.width, target.height)
    };
    let workspace = caption_workspace(project, snapshot, prepared, (canvas_w, canvas_h))?;
    let mut command = base_command(ffmpeg);
    command.current_dir(&workspace.path);
    let color = crate::color::Plan::for_items(
        &montage.items,
        &prepared.info,
        (target.width, target.height),
    );
    let source_seconds = mapped.playback.source_seconds;
    let mut graph = if item.is_image() {
        command.args(
            crate::images::input_args(source, target.rate_num, target.rate_den)
                .map_err(unavailable)?,
        );
        crate::images::filter(target, meta.image_orientation, &color)
    } else {
        // Only video needs the final-frame clamp used for export's cloned
        // padding. Audio mapping retains the exact source clock at that edge.
        let fps = target.rate_num as f64 / target.rate_den as f64;
        let origin = prepared.origins.get(source).copied().unwrap_or(0.0);
        let video_seek =
            source_seconds.min((origin + (meta.seconds - 1.0 / fps).max(0.0)).max(0.0));
        // While playing, the source's light copy is read instead once it is
        // ready: every frame of it is a keyframe, so a seek decodes nothing
        // from a distant keyframe. Its pictures start at the source's origin.
        // Without one, or until it is ready, the source is read as before.
        // The colour tail must then describe the copy, already in limited range,
        // not the source. A BT.2020 project keeps every clip as decoded, which
        // the copy of another clip is not: there the sources are read.
        #[cfg_attr(not(feature = "gui"), allow(unused_mut))]
        let mut input = (source.clone(), video_seek);
        #[cfg_attr(not(feature = "gui"), allow(unused_mut))]
        let mut pictures = crate::color::Input::video(meta);
        #[cfg(feature = "gui")]
        if stream
            && color.matrix != crate::color::Matrix::Bt2020
            && let Some(copied) = pictures.light_copy()
            && let Some(copy) = crate::copies::ready(&prepared.snapshot.0[mapped.item], origin)
        {
            input = (copy.path.clone(), copy.seek(video_seek - origin));
            pictures = copied;
        }
        if stream {
            crate::profile!(
                "player: reads {} at {:.3} s for local {:.3} s of {} (project matrix {:?})",
                if input.0 == *source {
                    "the SOURCE"
                } else {
                    "the copy"
                },
                input.1,
                local,
                source.display(),
                color.matrix
            );
        }
        command
            .args(["-ss", &format!("{:.9}", input.1), "-i"])
            .arg(&input.0);
        // A single final decoded frame can carry EOF at the same PTS inside a
        // complex graph. fps would discard it before duration-based padding.
        // Still extraction needs no cadence conversion. Streaming adds exactly
        // two look-ahead clones; repair their equal PTS when input frame rate is
        // unavailable, preserving every ordinary source presentation timestamp.
        let cadence = if stream {
            format!(
                "tpad=stop_mode=clone:stop=2,setpts='if(eq(PTS,PREV_INPTS),PREV_OUTPTS+{d}/({n}*TB),PTS)',fps={n}/{d},",
                n = target.rate_num,
                d = target.rate_den
            )
        } else {
            String::new()
        };
        format!(
            "[0:v]setpts=PTS-STARTPTS,scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)',setsar=1,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,{cadence}{tail}[v]",
            w = target.width,
            h = target.height,
            tail = color.video_tail(&pictures),
        )
    };
    let has_captions = workspace.has_captions;
    // PTS is the original project clock for fades, then the selected local
    // caption clock. Captions are composed once, after pad/crop, before reduction.
    let mut filter = vec![format!("setpts=PTS-STARTPTS+{clock:.9}/TB")];
    if montage.fade_in > 0.0 {
        filter.push(format!("fade=t=in:st=0:d={:.9}", montage.fade_in));
    }
    if montage.fade_out > 0.0 {
        filter.push(format!(
            "fade=t=out:st={:.9}:d={:.9}",
            prepared.duration - montage.fade_out,
            montage.fade_out
        ));
    }
    filter.push(format!("setpts=PTS-STARTPTS+{local:.9}/TB"));
    if let Some(short) = &project.short {
        filter.push(
            crate::shorts::Geometry::new(
                f64::from(target.width),
                f64::from(target.height),
                short.framing,
            )
            .map_err(engine_error)?
            .filter(has_captions, false),
        );
    } else if has_captions {
        filter.push(crate::captions::FILTER.into());
    }
    let (width, height) = reduced_dimensions(canvas_w, canvas_h);
    // Reset the packet origin after applying project/selection clock effects.
    // image2pipe carries no timestamps; its reader consumes this fixed grid.
    filter.push(format!(
        "scale={width}:{height},setsar=1,setpts=PTS-STARTPTS"
    ));
    if stream {
        filter.push("fps=24".into());
    }
    filter.push("format=rgba".into());
    graph.push_str(&format!(";[v]{}[out]", filter.join(",")));
    if stream {
        crate::profile!("player: graph {graph}");
    }
    command.args(["-filter_complex", &graph, "-map", "[out]"]);
    Ok(PlaybackSegment {
        command,
        local_start: local,
        local_end: mapped.playback.local_end,
        width,
        height,
        workspace: workspace.directory.clone(),
        #[cfg(test)]
        source: source.clone(),
        #[cfg(test)]
        source_seconds,
    })
}
fn reduced_dimensions(width: u32, height: u32) -> (u32, u32) {
    let ratio = (MAX_FRAME_EDGE as f64 / f64::from(width.max(height))).min(1.0);
    let even = |n: u32| ((f64::from(n) * ratio / 2.0).floor().max(1.0) * 2.0) as u32;
    (even(width), even(height))
}
fn base_command(ffmpeg: &Path) -> Command {
    let mut command = crate::command(ffmpeg);
    command.args([
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-v",
        "error",
        "-xerror",
        "-err_detect",
        "explode",
        "-threads",
        "1",
    ]);
    command
}

fn decode_frame(
    mut command: Command,
    width: u32,
    height: u32,
    limits: (u32, u32),
    cancel: &AtomicBool,
    started: Instant,
) -> Result<Frame> {
    check(cancel, started)?;
    if width == 0 || height == 0 || width > limits.0 || height > limits.1 {
        return Err(unavailable("Preview frame exceeds its pixel limit"));
    }
    let expected = width as usize * height as usize * 4;
    command
        .args([
            "-frames:v",
            "1",
            "-an",
            "-sn",
            "-dn",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "rawvideo",
            "-threads",
            "1",
            "-pix_fmt",
            "rgba",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::process::Tree::spawn(command, true).map_err(unavailable)?;
    let stdout = child
        .0
        .stdout()
        .take()
        .ok_or_else(|| unavailable("Missing frame output"))?;
    let stderr = child
        .0
        .stderr()
        .take()
        .ok_or_else(|| unavailable("Missing frame diagnostics"))?;
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut rgba = Vec::with_capacity(expected);
        let result = stdout
            .take((expected + 1) as u64)
            .read_to_end(&mut rgba)
            .map(|_| rgba);
        let _ = tx.send(result);
    });
    let diagnostics = thread::spawn(move || diagnostic_tail(stderr));
    let outcome = (|| {
        let mut decoded = None;
        let mut status = None;
        loop {
            check(cancel, started)?;
            if decoded.is_none() {
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(result) => decoded = Some(result.map_err(unavailable)?),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => return Err(unavailable("Frame reader stopped")),
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
            if status.is_none() {
                status = child.0.try_wait().map_err(unavailable)?;
            }
            if let Some(status) = status
                && let Some(rgba) = decoded.take()
            {
                if !status.success() {
                    return Err(unavailable(format!("FFmpeg exited with {status}")));
                }
                if rgba.len() != expected {
                    return Err(unavailable(
                        "Decoder returned an incomplete or oversized frame",
                    ));
                }
                return Ok(Frame {
                    width,
                    height,
                    rgba,
                });
            }
        }
    })();
    drop(child);
    let _ = reader.join();
    let log = diagnostics.join().unwrap_or_default();
    outcome.map_err(|error| match error {
        PreviewError::Unavailable(detail) if !log.is_empty() => {
            unavailable(format!("{detail}: {}", log.trim()))
        }
        other => other,
    })
}
fn diagnostic_tail(mut reader: impl Read) -> String {
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
fn remaining(started: Instant) -> Result<Duration> {
    DEADLINE
        .checked_sub(started.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| unavailable("Preview decoder exceeded its deadline"))
}
fn check(cancel: &AtomicBool, started: Instant) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(PreviewError::Cancelled);
    }
    remaining(started).map(|_| ())
}
fn unavailable(detail: impl ToString) -> PreviewError {
    PreviewError::Unavailable(crate::engine::bounded(
        &detail.to_string(),
        DIAGNOSTIC_BYTES,
    ))
}
fn engine_error(error: EngineError) -> PreviewError {
    match error.code.as_str() {
        "error.cancelled" => PreviewError::Cancelled,
        "error.stale_inspection" => PreviewError::SourceChanged,
        _ => unavailable(error),
    }
}
fn box_error(error: Box<dyn std::error::Error>) -> PreviewError {
    if let Some(error) = error.downcast_ref::<EngineError>() {
        engine_error(error.clone())
    } else if let Some(error) = error.downcast_ref::<PreviewError>() {
        error.clone()
    } else {
        unavailable(error)
    }
}
fn same_request(a: &FrameRequest, b: &FrameRequest) -> bool {
    match (a, b) {
        (
            FrameRequest::Project {
                project: a,
                time_ms: ta,
            },
            FrameRequest::Project {
                project: b,
                time_ms: tb,
            },
        ) => ta == tb && a.same_render_settings(b),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "gui")]
    fn montage_request(items: Vec<crate::input::MediaItem>) -> ProjectRequest {
        ProjectRequest {
            montage: crate::engine::ExportRequest {
                items,
                wav: "song.wav".into(),
                output: "out.mp4".into(),
                ffmpeg: PathBuf::new(),
                fade_in: 0.0,
                fade_out: 0.0,
                partial_fades: false,
                preview: true,
                clip_audio: false,
                force_encode: true,
            },
            short: None,
            captions: None,
            effect: None,
        }
    }
    #[test]
    #[cfg(feature = "gui")]
    fn a_seek_in_the_file_given_to_libmpv_goes_to_the_frame_the_playhead_is_in() {
        // Two pieces at 24 frames a second, the second starting off the first's
        // grid, as when a selection starts inside a clip.
        let grid = PlayGrid {
            fps: 24.0,
            pieces: vec![(0.0, 0.0), (0.985, 0.985), (10.985, 10.985)],
        };
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Between two frames: the one before, where libmpv given the time
        // itself would show the one after.
        assert!(near(grid.frame_time(0.560), 13.0 / 24.0));
        assert!(near(grid.frame_time(0.5), 0.5));
        assert!(near(grid.frame_time(0.0), 0.0));
        // On a frame's time: that frame, as its time is written in milliseconds.
        assert!(near(grid.frame_time(13.0 / 24.0), 13.0 / 24.0));
        assert!(near(grid.frame_time(0.542), 13.0 / 24.0));
        assert!(near(grid.frame_time(0.541), 12.0 / 24.0));
        // In the second piece the frames are counted from its start.
        assert!(near(grid.frame_time(2.060), 0.985 + 25.0 / 24.0));
        // Each piece has its own grid: past a seam the frames are counted from
        // it, and never from before it.
        assert!(near(grid.frame_time(0.984), 23.0 / 24.0));
        assert!(near(grid.frame_time(0.990), 0.985));
        assert!(near(
            grid.frame_time(0.985 + 1.5 / 24.0),
            0.985 + 1.0 / 24.0
        ));
        assert!(near(grid.frame_time(11.0), 10.985));
        // The same rule as the paused preview's.
        assert!(near(grid.frame_time(5.03), on_grid(5.03, 0.985, 24.0)));
        // A piece cut inside its source counts from the source's start, and
        // its first frame is not left for the piece before.
        let cut = PlayGrid {
            fps: 24.0,
            pieces: vec![(0.0, 0.0), (4.0, 3.99)],
        };
        assert!(near(cut.frame_time(4.001), 4.0));
        assert!(near(cut.frame_time(4.05), 3.99 + 1.0 / 24.0));
        // Nothing known of the file: the time as asked.
        assert!(near(PlayGrid::default().frame_time(3.3), 3.3));
    }
    #[test]
    #[cfg(feature = "gui")]
    fn libmpv_is_not_given_a_montage_whose_colours_its_copies_do_not_keep() {
        let video = |line: &str, width, height| crate::color::Input::Video {
            props: crate::color::Props::parse(line),
            width,
            height,
        };
        let hd = "  Stream #0:0: Video: h264 (High), yuv420p(tv, bt709, progressive), 1920x1080";
        let full = "  Stream #0:0: Video: h264, yuvj420p(pc, bt709, progressive), 1920x1080";
        let hdr =
            "  Stream #0:0: Video: hevc, yuv420p10le(tv, bt2020nc/bt2020/smpte2084), 3840x2160";
        let plain = [video(hd, 1920, 1080), video(full, 1920, 1080)];
        assert_eq!(play_color_refusal(&plain, (1920, 1080)), None);
        let mixed = [video(hd, 1920, 1080), video(hdr, 3840, 2160)];
        assert_eq!(
            play_color_refusal(&mixed, (3840, 2160)),
            Some("the montage is in BT.2020")
        );
        assert_eq!(
            play_color_refusal(&[video(hdr, 3840, 2160)], (3840, 2160)),
            Some("the montage is in BT.2020")
        );
    }
    #[test]
    #[cfg(feature = "gui")]
    fn libmpv_is_not_asked_for_what_it_does_not_play() {
        let video = || crate::input::MediaItem::from(PathBuf::from("a.mp4"));
        let plain = montage_request(vec![video(), video()]);
        assert_eq!(play_refusal(&plain), None);

        let mut sound = plain.clone();
        sound.montage.clip_audio = true;
        assert_eq!(
            play_refusal(&sound),
            Some("the montage mixes the clips' sound")
        );

        // Captions: drawn over the picture by an effect, or in it by libass.
        let mut captions = plain.clone();
        captions.captions = Some(crate::shorts::ShortCaptions {
            subtitles: "lyrics.srt".into(),
            style: Default::default(),
        });
        assert_eq!(
            play_refusal(&captions),
            Some("the captions are drawn by libass")
        );
        captions.effect = Some(crate::project::EffectChoice {
            id: "classique".into(),
            preset: None,
            dir: "effects/classique".into(),
            values: Default::default(),
        });
        assert_eq!(play_refusal(&captions), None);
        // An effect without captions draws nothing and changes nothing.
        let mut effect_only = plain.clone();
        effect_only.effect = captions.effect.clone();
        assert_eq!(play_refusal(&effect_only), None);

        let image = montage_request(vec![
            video(),
            crate::input::MediaItem::Image {
                path: "b.png".into(),
                duration: 3.0,
            },
        ]);
        assert_eq!(play_refusal(&image), Some("the montage has a still image"));
    }
    #[test]
    #[cfg(feature = "gui")]
    fn the_montage_is_cut_for_libmpv_as_it_is_for_the_export() {
        let near = |parts: &[(usize, f64, f64)], expected: &[(usize, f64, f64)]| {
            assert_eq!(parts.len(), expected.len(), "{parts:?}");
            for (part, wanted) in parts.iter().zip(expected) {
                assert_eq!(part.0, wanted.0, "{parts:?}");
                assert!(
                    (part.1 - wanted.1).abs() < 1e-9 && (part.2 - wanted.2).abs() < 1e-9,
                    "{parts:?}"
                );
            }
        };
        // Two clips of 4 and 6 s repeated over 15 s of song: the last piece is
        // cut where the song ends.
        let whole = play_pieces(&[4.0, 6.0], 0.0, 15.0, false).unwrap();
        near(
            &whole,
            &[(0, 0.0, 4.0), (1, 0.0, 6.0), (0, 0.0, 4.0), (1, 0.0, 1.0)],
        );
        assert!((whole.iter().map(|p| p.2).sum::<f64>() - 15.0).abs() < 1e-9);
        // A short from 5 to 12 s: it starts inside the second clip.
        near(
            &play_pieces(&[4.0, 6.0], 5.0, 7.0, false).unwrap(),
            &[(1, 1.0, 5.0), (0, 0.0, 2.0)],
        );
        // The same short with its loops restarted: from the first clip's start.
        near(
            &play_pieces(&[4.0, 6.0], 5.0, 7.0, true).unwrap(),
            &[(0, 0.0, 4.0), (1, 0.0, 3.0)],
        );
    }
    #[test]
    #[cfg(feature = "gui")]
    fn the_file_given_to_libmpv_names_each_piece_and_the_song() {
        let parts = [
            ("C:\\cache\\a.mp4".to_string(), 0.0, 4.0),
            ("C:\\cache\\été,b;c.mp4".to_string(), 1.5, 2.25),
        ];
        assert_eq!(
            play_edl(&parts, "D:\\songs\\la la.wav", 13.0, 6.25),
            concat!(
                "edl://%14%C:\\cache\\a.mp4,start=0.000000,length=4.000000;",
                // The length is in bytes (two per é), so a comma or a semicolon
                // in a name ends nothing.
                "%22%C:\\cache\\été,b;c.mp4,start=1.500000,length=2.250000;",
                "!new_stream;%18%D:\\songs\\la la.wav,start=13.000000,length=6.250000"
            )
        );
    }
    #[test]
    #[cfg(feature = "gui")]
    fn the_filter_for_libmpv_fits_the_frame_fades_on_the_projects_clock_then_frames() {
        let fit = "scale=w='max(2,trunc(min(640,360*dar)/2)*2)':h='max(2,trunc(min(360,640/dar)/2)*2)',setsar=1,pad=640:360:(ow-iw)/2:(oh-ih)/2,format=yuv420p";
        assert_eq!(play_filter((640, 360), 0.0, 0.0, 0.0, 180.0, None), fit);
        // Fades are on the project's clock: the file starts at the selection,
        // 13 s into it, so its clock is moved there and back.
        assert_eq!(
            play_filter((640, 360), 2.0, 3.0, 13.0, 180.0, None),
            format!(
                "{fit},setpts=PTS+13.000000000/TB,fade=t=in:st=0:d=2.000000000,fade=t=out:st=177.000000000:d=3.000000000,setpts=PTS-13.000000000/TB"
            )
        );
        // Only a fade out, and the short's framing last.
        assert_eq!(
            play_filter((640, 360), 0.0, 3.0, 0.0, 60.0, Some("crop=360:640".into())),
            format!(
                "{fit},setpts=PTS+0.000000000/TB,fade=t=out:st=57.000000000:d=3.000000000,setpts=PTS-0.000000000/TB,crop=360:640"
            )
        );
    }
    fn request(index: u64) -> FrameRequest {
        FrameRequest::Thumbnail {
            item: PathBuf::from(format!("{index}.mp4")).into(),
            ffmpeg: "ffmpeg".into(),
        }
    }
    #[test]
    fn cache_keeps_lru_count_and_byte_limits() {
        let mut cache = Cache::default();
        for i in 0..50 {
            cache.insert(
                request(i),
                Snapshot(vec![]),
                Arc::new(Frame {
                    width: 640,
                    height: 640,
                    rgba: vec![0; 640 * 640 * 4],
                }),
            );
        }
        assert!(cache.bytes <= MAX_CACHE_BYTES);
        assert!(cache.frames.len() <= MAX_CACHE_FRAMES);
        assert!(cache.get(&request(0), &Snapshot(vec![])).is_none());
        let a = cache.get(&request(49), &Snapshot(vec![])).unwrap();
        let b = cache.get(&request(49), &Snapshot(vec![])).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let mut small = Cache::default();
        for i in 0..50 {
            small.insert(
                request(i),
                Snapshot(vec![]),
                Arc::new(Frame {
                    width: 2,
                    height: 2,
                    rgba: vec![0; 16],
                }),
            );
        }
        assert_eq!(small.frames.len(), MAX_CACHE_FRAMES);
    }
    #[test]
    fn invalidation_cancels_active_and_clears_bounded_mailbox() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut mailbox = Mailbox {
            active: Some((request(0), cancel.clone())),
            ..Default::default()
        };
        mailbox.queue.push_back(Pending {
            revision: 0,
            epoch: 0,
            request: request(1),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        invalidate(&mut mailbox);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(mailbox.latest.is_none() && mailbox.queue.is_empty() && mailbox.replies.is_empty());
        assert_eq!(mailbox.epoch, 1);
    }
    #[test]
    fn reduction_preserves_even_geometry_with_bounded_long_edge() {
        assert_eq!(reduced_dimensions(1080, 1920), (360, 640));
        assert_eq!(reduced_dimensions(1920, 1080), (640, 360));
        assert_eq!(reduced_dimensions(256, 144), (256, 144));
    }

    #[test]
    fn stream_occurrences_keep_original_phase_and_selected_clock() {
        use crate::{
            engine::ExportRequest, inspection::FileStamp, project::ProjectShort, shorts::Framing,
        };
        let folder = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a.mp4", "b.mp4", "clock.wav", "ffmpeg"]
            .into_iter()
            .map(|name| {
                let path = folder.path().join(name);
                std::fs::write(&path, b"identity-only fixture").unwrap();
                path
            })
            .collect();
        let snapshot = Snapshot(paths.iter().map(|p| FileStamp::read(p).unwrap()).collect());
        let mut project = ProjectRequest {
            montage: ExportRequest {
                items: vec![paths[0].clone().into(), paths[1].clone().into()],
                wav: paths[2].clone(),
                output: folder.path().join("unused.mp4"),
                ffmpeg: paths[3].clone(),
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
            effect: None,
            captions: None,
        };
        let prepared = Prepared {
            snapshot: snapshot.clone(),
            items: project.montage.items.clone(),
            info: [4.0, 6.0]
                .into_iter()
                .map(|seconds| crate::media::MediaInfo {
                    seconds,
                    ..Default::default()
                })
                .collect(),
            origins: HashMap::new(),
            target: crate::plan::Target {
                width: 256,
                height: 144,
                rate_num: 25,
                rate_den: 1,
                codec: "h264".into(),
                pixel_format: "yuv420p".into(),
            },
            duration: 20.0,
        };
        let first = project_command(&project, 0.0, &snapshot, &prepared, true).unwrap();
        assert_eq!(
            (first.local_start, first.local_end, first.source_seconds),
            (0.0, 1.0, 3.0)
        );
        assert_eq!(first.source, paths[0].canonicalize().unwrap());
        let second = project_command(&project, 1.0, &snapshot, &prepared, true).unwrap();
        assert_eq!(
            (second.local_start, second.local_end, second.source_seconds),
            (1.0, 5.0, 0.0)
        );
        assert_eq!(second.source, paths[1].canonicalize().unwrap());
        project.short.as_mut().unwrap().restart_loops = true;
        let restart = project_command(&project, 0.0, &snapshot, &prepared, true).unwrap();
        assert_eq!(
            (
                restart.local_start,
                restart.local_end,
                restart.source_seconds
            ),
            (0.0, 4.0, 0.0)
        );
        let restart_second = project_command(&project, 4.0, &snapshot, &prepared, true).unwrap();
        assert_eq!(
            (restart_second.local_start, restart_second.local_end),
            (4.0, 5.0)
        );
        let short = project.short.as_mut().unwrap();
        short.restart_loops = false;
        short.start_ms = 13_015;
        let seam = project_command(&project, 0.985, &snapshot, &prepared, true).unwrap();
        assert_eq!(seam.source, paths[1].canonicalize().unwrap());
        assert!((seam.local_start - 0.985).abs() < 1e-9);
        assert!((seam.local_end - 4.985).abs() < 1e-9);
        project.short = None;
        let audio_edge = map_piece(&project, 9.99, &prepared).unwrap();
        assert!(
            (audio_edge.playback.source_seconds - 5.99).abs() < 1e-9,
            "audio mapping must not jump back to the video's last frame"
        );
    }

    #[test]
    #[cfg(feature = "gui")]
    fn playback_reads_a_ready_light_copy_and_a_still_never_does() {
        use crate::{engine::ExportRequest, inspection::FileStamp};
        let folder = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a.mp4", "b.mp4", "clock.wav", "ffmpeg", "copy.mp4"]
            .into_iter()
            .map(|name| {
                let path = folder.path().join(name);
                std::fs::write(&path, b"identity-only fixture").unwrap();
                path
            })
            .collect();
        let snapshot = Snapshot(
            paths[..4]
                .iter()
                .map(|p| FileStamp::read(p).unwrap())
                .collect(),
        );
        let project = ProjectRequest {
            montage: ExportRequest {
                items: vec![paths[0].clone().into(), paths[1].clone().into()],
                wav: paths[2].clone(),
                output: folder.path().join("unused.mp4"),
                ffmpeg: paths[3].clone(),
                fade_in: 0.0,
                fade_out: 0.0,
                partial_fades: true,
                preview: true,
                clip_audio: false,
                force_encode: false,
            },
            short: None,
            effect: None,
            captions: None,
        };
        // The first clip is full range, as a phone or an action camera records.
        let info = |description: &str| crate::media::MediaInfo {
            seconds: 4.0,
            description: description.into(),
            width: 640,
            height: 360,
            ..Default::default()
        };
        let mut prepared = Prepared {
            snapshot: snapshot.clone(),
            items: project.montage.items.clone(),
            info: vec![
                info("Video: mjpeg, yuvj420p(pc), 640x360"),
                info("Video: h264, yuv420p(tv, smpte170m), 640x360"),
            ],
            origins: HashMap::from([(snapshot.0[0].resolved.clone(), 0.25)]),
            target: crate::plan::Target {
                width: 640,
                height: 360,
                rate_num: 25,
                rate_den: 1,
                codec: "h264".into(),
                pixel_format: "yuv420p".into(),
            },
            duration: 20.0,
        };
        let words = |stream: bool, prepared: &Prepared, local: f64| -> Vec<String> {
            project_command(&project, local, &snapshot, prepared, stream)
                .unwrap()
                .command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let input = |words: &[String]| {
            let at = words.iter().position(|w| w == "-i").unwrap();
            (words[at - 1].clone(), PathBuf::from(&words[at + 1]))
        };
        let graph = |words: &[String]| {
            let at = words.iter().position(|w| w == "-filter_complex").unwrap();
            words[at + 1].clone()
        };
        let source = snapshot.0[0].resolved.clone();
        // No copy yet: the source, at its own time (origin 0.25 + 1.3 s).
        let before = words(true, &prepared, 1.3);
        assert_eq!(input(&before), ("1.550000000".into(), source.clone()));
        assert!(graph(&before).contains("in_range=pc:out_range=tv"));
        // Its copy is ready: 25 pictures a second from the source's origin.
        let times: Vec<f64> = (0..100).map(|i| f64::from(i) / 25.0).collect();
        let copy = crate::copies::offer_for_test(&snapshot.0[0], 0.25, paths[4].clone(), times);
        let playing = words(true, &prepared, 1.3);
        // Asked just before the copy's first frame at or after 1.3 s: 1.32 s.
        assert_eq!(
            input(&playing),
            (format!("{:.9}", copy.seek(1.3)), paths[4].clone())
        );
        assert_eq!(input(&playing).0, "1.319500000");
        // The copy is already in limited range: no second range conversion.
        assert!(
            !graph(&playing).contains("in_range=pc"),
            "{}",
            graph(&playing)
        );
        assert!(graph(&playing).contains("range=tv"));
        // A still is always decoded from the source.
        let still = words(false, &prepared, 1.3);
        assert_eq!(input(&still), ("1.550000000".into(), source.clone()));
        assert!(graph(&still).contains("in_range=pc:out_range=tv"));
        // The other clip has no copy: its source.
        let other = words(true, &prepared, 5.0);
        assert_eq!(input(&other).1, snapshot.0[1].resolved);
        // A copy made for another origin is not this source's copy.
        prepared.origins.insert(source.clone(), 0.5);
        assert_eq!(input(&words(true, &prepared, 1.3)).1, source);
        prepared.origins.insert(source.clone(), 0.25);
        // In a BT.2020 project every clip is kept as decoded: sources only.
        prepared.info[1] = info("Video: hevc, yuv420p10le(tv, bt2020nc/bt2020/smpte2084), 640x360");
        assert_eq!(input(&words(true, &prepared, 1.3)).1, source);
    }

    #[test]
    fn caption_assets_are_immutable_reused_and_kept_alive_by_active_decoder() {
        use crate::{
            engine::ExportRequest,
            project::ProjectShort,
            shorts::{Framing, ShortCaptions},
        };
        let lease = WorkspaceLease::new();
        let folder = tempfile::tempdir().unwrap();
        let subtitles = folder.path().join("reviewed.srt");
        std::fs::write(&subtitles, "1\n00:00:14,000 --> 00:00:16,000\nCAPTION\n").unwrap();
        let snapshot = Snapshot(vec![FileStamp::read(&subtitles).unwrap()]);
        let mut project = ProjectRequest {
            montage: ExportRequest {
                items: vec![],
                wav: "unused.wav".into(),
                output: "unused.mp4".into(),
                ffmpeg: "ffmpeg".into(),
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
            effect: None,
            captions: Some(ShortCaptions {
                subtitles: subtitles.clone(),
                style: crate::captions::CaptionStyle::default(),
            }),
        };
        let prepared = Prepared {
            snapshot: snapshot.clone(),
            items: vec![],
            info: vec![],
            origins: HashMap::new(),
            target: crate::plan::Target {
                width: 256,
                height: 144,
                rate_num: 25,
                rate_den: 1,
                codec: "h264".into(),
                pixel_format: "yuv420p".into(),
            },
            duration: 20.0,
        };
        let first = caption_workspace(&project, &snapshot, &prepared, (1080, 1920)).unwrap();
        let script = first.path.join("captions.ass");
        let font = first.path.join("fonts/NOHCJK.otf");
        let timestamps = [
            std::fs::metadata(&script).unwrap().modified().unwrap(),
            std::fs::metadata(&font).unwrap().modified().unwrap(),
        ];
        project.short.as_mut().unwrap().restart_loops = true;
        project.short.as_mut().unwrap().framing = Framing::Crop;
        project.montage.output = "elsewhere.mp4".into();
        project.montage.fade_in = 2.0;
        let second = caption_workspace(&project, &snapshot, &prepared, (1080, 1920)).unwrap();
        assert!(Arc::ptr_eq(&first.directory, &second.directory));
        assert_eq!(
            timestamps,
            [
                std::fs::metadata(&script).unwrap().modified().unwrap(),
                std::fs::metadata(&font).unwrap().modified().unwrap()
            ]
        );
        let active = first.directory.clone();
        let active_path = first.path.clone();
        drop(first);
        drop(second);
        // Clear only this store's cache ownership while an active decoder still
        // holds its directory; no live process can lose its script or font.
        lease._store.lock().unwrap().clear();
        assert!(active_path.exists());
        drop(active);
        assert!(!active_path.exists());
        assert!(
            format!(
                "{:?}",
                Frame {
                    width: 640,
                    height: 640,
                    rgba: vec![255; 640 * 640 * 4]
                }
            )
            .len()
                < 100
        );
    }
}
