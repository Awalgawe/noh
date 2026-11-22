//! Export with a caption effect ("E4"): FFmpeg #1 decodes and filters the video and
//! writes raw I420 frames on its stdout, with each frame's timestamp in its
//! `-stats_mux_pre` file; the OpenGL engine draws the caption layer for that
//! timestamp and mixes it into the frame in YUV on the GPU; the frames go out in
//! Matroska with the same timestamps to FFmpeg #2, which adds the sound and encodes.
//! Outside the caption layer the video bytes reach the encoder unchanged.
//!
//! Every wait is bounded by the deadline. Frames are read and written by their own
//! threads, so a stalled FFmpeg cannot block the GPU thread past it. On any failure
//! both FFmpeg processes are settled and their diagnostics read to the end before
//! the log is returned: the NVENC fallback decides on that log.
use super::gl::{GlRenderer, Matrix, YuvMix};
use super::mkv;
use super::plugin::PluginScene;
use super::shaders::Output;
use super::text::Fonts;
use crate::process::Tree;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// How long a frame may wait for its timestamp line, which FFmpeg writes before
/// the frame: past this, the line is missing, not late.
const LINE_GRACE: Duration = Duration::from_secs(5);
/// What every GL call answers once a GPU reset has lost a robust context.
const CONTEXT_LOST: u32 = 0x0507;
/// How long a failing FFmpeg may take to exit before it is killed.
const SETTLE: Duration = Duration::from_secs(5);

/// What draws the layer mixed into each frame.
pub enum Layer {
    /// A caption effect (the shipped styles and the effect plugins).
    Plugin(Box<PluginScene>),
    /// Anything else that draws an overlay on the engine's context (a motion
    /// scene): set up once on the context, then asked for each frame's time.
    Overlay(Box<dyn Overlay>),
}

/// An overlay drawn on the engine's context. `draw` returns a texture of the
/// engine's output kind (premultiplied, display-encoded RGBA8, top row first,
/// the frame's size), valid until the next call.
pub trait Overlay {
    fn setup(&mut self, gpu: &mut GlRenderer) -> Result<(), String>;
    fn draw(&mut self, gpu: &mut GlRenderer, t: f64) -> Result<glow::Texture, String>;
}

/// What a path supplies; `run` owns the processes and the GPU work.
pub struct Job {
    /// FFmpeg #1, complete except for its output: `run` appends the stats options
    /// and `-f rawvideo -pix_fmt yuv420p pipe:1`.
    pub first: Command,
    /// FFmpeg #2 built for an encoder, the video time base (`1/N` text) and the
    /// first frame's time in seconds, both read from FFmpeg #1. Its input 0 must be
    /// added by `pipe_input`; progress on stdout.
    pub second: Box<dyn Fn(&str, f64) -> Command>,
    /// Folder for the stats file.
    pub workspace: PathBuf,
    pub width: u32,
    pub height: u32,
    pub matrix: Matrix,
    pub scene: Layer,
    pub deadline: Duration,
    /// The video's known end on FFmpeg #1's clock (s): the last frame lasts until
    /// then, as in a single FFmpeg. None: FFmpeg #2 decides (the project's setts).
    pub end: Option<f64>,
}

/// FFmpeg #2's input 0, the mixed frames. FFmpeg moves an input's first timestamp
/// to zero: a first frame after zero (a source whose picture starts after its
/// sound, a seek between two frames) is put back there with `-itsoffset`, so the
/// picture keeps its place against the sound as in a single FFmpeg (Task 43).
/// At zero nothing is added: the argument list stays what it was.
pub fn pipe_input(cmd: &mut Command, first: f64) {
    if first != 0.0 {
        cmd.args(["-itsoffset", &format!("{first:.6}")]);
    }
    cmd.args(["-f", "matroska", "-i", "pipe:0"]);
}

/// What `run` measured, for the diagnostics.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub frames: usize,
    pub setup_ms: f64,
    pub adapter: String,
    pub time_base: String,
    pub render_ms_p95: f64,
    /// Per frame, p50 / p95 / max in ms: the layer (scene and GL draw calls), the
    /// mix submit (I420 upload and mix draws), the readback wait of the previous frame.
    pub layer_ms: [f64; 3],
    pub mix_ms: [f64; 3],
    pub fetch_ms: [f64; 3],
    /// Output time of the first frame whose letters left the safe surface.
    pub overflow_at: Option<f64>,
    /// Output time of the first frame showing a word wider than its box (drawn
    /// whole, overflowing it).
    pub too_wide_at: Option<f64>,
    /// The installed family the effect's font setting names and this machine
    /// lacks (drawn in the plugin's own face instead).
    pub font_missing: Option<String>,
}

pub struct Failure {
    pub message: String,
    /// FFmpeg #1's then FFmpeg #2's diagnostics, each read to its end.
    pub log: String,
    /// No OpenGL context could be created: nothing was started.
    pub gl_unavailable: bool,
}

/// One FFmpeg process and the thread keeping the last 32 KiB of its stderr.
struct Proc {
    tree: Tree,
    log: Arc<Mutex<Vec<u8>>>,
    drain: Option<JoinHandle<()>>,
}
impl Proc {
    fn spawn(command: Command) -> Result<Self, String> {
        let mut tree =
            Tree::spawn(command, false).map_err(|e| format!("cannot start FFmpeg: {e}"))?;
        let stderr = tree.0.stderr().take().ok_or("missing FFmpeg diagnostics")?;
        let log = Arc::new(Mutex::new(Vec::new()));
        let keep = log.clone();
        let drain = thread::spawn(move || {
            let mut reader = stderr;
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut l = keep.lock().unwrap();
                l.extend_from_slice(&buf[..n]);
                if l.len() > 32768 {
                    let excess = l.len() - 32768;
                    l.drain(..excess);
                }
            }
        });
        Ok(Self {
            tree,
            log,
            drain: Some(drain),
        })
    }

    /// Wait for the exit until `until`.
    fn wait(&mut self, until: Instant) -> Result<ExitStatus, String> {
        loop {
            if let Some(s) = self.tree.0.try_wait().map_err(|e| e.to_string())? {
                return Ok(s);
            }
            if Instant::now() >= until {
                return Err("deadline exceeded".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// The whole diagnostic text: after the process has exited, the drain reaches
    /// the end of the pipe and is joined.
    fn finished_log(&mut self) -> String {
        if let Some(d) = self.drain.take() {
            let _ = d.join();
        }
        String::from_utf8_lossy(&self.log.lock().unwrap()).into_owned()
    }

    /// After a failure: give the process `SETTLE` to exit (its input may already be
    /// closed), kill it otherwise, then read its diagnostics to the end.
    fn settle(&mut self) -> String {
        if self.wait(Instant::now() + SETTLE).is_err() {
            let _ = self.tree.0.start_kill();
            let _ = self.wait(Instant::now() + SETTLE);
        }
        self.finished_log()
    }
}

/// Reads the `-stats_mux_pre` file as FFmpeg appends to it (one flushed line per
/// packet, written before the packet reaches the pipe).
struct StatsTail {
    file: BufReader<std::fs::File>,
    partial: String,
}
impl StatsTail {
    fn open(path: &Path, until: Instant) -> Result<Self, String> {
        loop {
            match std::fs::File::open(path) {
                Ok(f) => {
                    return Ok(Self {
                        file: BufReader::new(f),
                        partial: String::new(),
                    });
                }
                Err(_) if Instant::now() < until => thread::sleep(Duration::from_millis(2)),
                Err(e) => return Err(format!("frame timestamps unavailable: {e}")),
            }
        }
    }
    /// Next "pts tb" line, waiting until `until`; None if there is none by then.
    fn next(&mut self, until: Instant) -> Result<Option<(i64, String)>, String> {
        loop {
            let mut chunk = String::new();
            let n = self.file.read_line(&mut chunk).map_err(|e| e.to_string())?;
            self.partial.push_str(&chunk);
            if self.partial.ends_with('\n') {
                let line = std::mem::take(&mut self.partial);
                let mut it = line.split_whitespace();
                let pts = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| format!("bad timestamp line {line:?}"))?;
                let tb = it
                    .next()
                    .ok_or_else(|| format!("bad timestamp line {line:?}"))?
                    .to_string();
                return Ok(Some((pts, tb)));
            }
            if n == 0 {
                if Instant::now() >= until {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn seconds(pts: i64, tb: &str) -> Result<f64, String> {
    let (n, d) = tb
        .split_once('/')
        .ok_or_else(|| format!("bad time base {tb}"))?;
    let (n, d): (f64, f64) = (
        n.parse().map_err(|_| format!("bad time base {tb}"))?,
        d.parse().map_err(|_| format!("bad time base {tb}"))?,
    );
    if d <= 0.0 || n <= 0.0 {
        return Err(format!("bad time base {tb}"));
    }
    Ok(pts as f64 * n / d)
}

/// Fill `buf` with one frame; false at a clean end of stream.
fn read_frame(r: &mut impl Read, buf: &mut [u8]) -> io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        let n = r.read(&mut buf[got..])?;
        if n == 0 {
            return if got == 0 {
                Ok(false)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated frame",
                ))
            };
        }
        got += n;
    }
    Ok(true)
}

/// Frames from FFmpeg #1, read on their own thread; buffers come back through `free`.
struct Frames {
    rx: mpsc::Receiver<Result<Option<Vec<u8>>, String>>,
    free: mpsc::Sender<Vec<u8>>,
}
impl Frames {
    fn start(mut out: impl Read + Send + 'static, size: usize) -> Self {
        let (tx, rx) = mpsc::sync_channel(2);
        let (free, pool) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            loop {
                let mut buf = pool.try_recv().unwrap_or_else(|_| vec![0u8; size]);
                let msg = match read_frame(&mut out, &mut buf) {
                    Ok(true) => Ok(Some(buf)),
                    Ok(false) => Ok(None),
                    Err(e) => Err(format!("reading frames: {e}")),
                };
                let last = !matches!(msg, Ok(Some(_)));
                if tx.send(msg).is_err() || last {
                    break;
                }
            }
        });
        Self { rx, free }
    }
    /// The next frame, None at the end of the stream; fails at `until`.
    fn next(&self, until: Instant) -> Result<Option<Vec<u8>>, String> {
        match self
            .rx
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            Ok(m) => m,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err("deadline exceeded while waiting for a frame".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("frame reader stopped".into()),
        }
    }
}

/// The Matroska writer into FFmpeg #2, on its own thread.
struct Sink {
    tx: Option<mpsc::SyncSender<(u64, Option<u64>, Vec<u8>)>>,
    done: Option<JoinHandle<io::Result<()>>>,
    free: mpsc::Receiver<Vec<u8>>,
}
impl Sink {
    fn start(stdin: impl Write + Send + 'static, w: u32, h: u32) -> Self {
        let (tx, rx) = mpsc::sync_channel::<(u64, Option<u64>, Vec<u8>)>(2);
        let (give, free) = mpsc::channel();
        let done = thread::spawn(move || -> io::Result<()> {
            let mut writer = mkv::Writer::new(io::BufWriter::with_capacity(1 << 20, stdin), w, h)?;
            for (us, duration, frame) in rx {
                writer.frame(us, duration, &frame)?;
                let _ = give.send(frame);
            }
            writer.finish()
        });
        Self {
            tx: Some(tx),
            done: Some(done),
            free,
        }
    }
    /// A buffer to fill (a returned one when available).
    fn buffer(&self, size: usize) -> Vec<u8> {
        self.free.try_recv().unwrap_or_else(|_| vec![0u8; size])
    }
    /// Queue one frame; fails at `until` or when the writer stopped.
    fn send(
        &self,
        us: u64,
        duration: Option<u64>,
        frame: Vec<u8>,
        until: Instant,
    ) -> Result<(), String> {
        let tx = self.tx.as_ref().ok_or("encoder input closed")?;
        let mut item = (us, duration, frame);
        loop {
            match tx.try_send(item) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Full(back)) => {
                    if Instant::now() >= until {
                        return Err("deadline exceeded while the encoder was not reading".into());
                    }
                    item = back;
                    thread::sleep(Duration::from_millis(1));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err("the encoder stopped reading".into());
                }
            }
        }
    }
    /// Close the input (after the queued frames) and report the writer's result.
    fn close(&mut self, until: Instant) -> Result<(), String> {
        self.tx = None;
        let Some(done) = self.done.take() else {
            return Ok(());
        };
        while !done.is_finished() {
            if Instant::now() >= until {
                return Err("deadline exceeded while closing the encoder input".into());
            }
            thread::sleep(Duration::from_millis(2));
        }
        match done.join() {
            Ok(r) => r.map_err(|e| format!("encoder input: {e}")),
            Err(_) => Err("encoder input writer panicked".into()),
        }
    }
}

/// Run the chain on this thread (the hidden GL context is bound to it). `progress`
/// receives FFmpeg #2's stdout lines (`-progress pipe:1`). Returns FFmpeg #2's
/// status and log; any failure of #1, of the GPU work or of the frame count fails.
pub fn run(
    job: Job,
    mut progress: impl FnMut(&str),
) -> Result<(ExitStatus, String, Stats), Failure> {
    let until = Instant::now() + job.deadline;
    let stats_path = job.workspace.join("e4-frames.txt");
    let _ = std::fs::remove_file(&stats_path);
    let mut first = job.first;
    first
        .args(["-stats_mux_pre"])
        .arg(&stats_path)
        .args([
            "-stats_mux_pre_fmt",
            "{pts} {tb}",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The OpenGL context comes first, on this thread (it is bound to it): without
    // one, nothing has started and the caller may draw the captions otherwise.
    let setup = Instant::now();
    let gpu = GlRenderer::headless(
        job.width,
        job.height,
        job.width as f32,
        job.height as f32,
        Output::PremulGamma,
    )
    .map_err(|e| Failure {
        message: format!("OpenGL: {e}"),
        log: String::new(),
        gl_unavailable: true,
    })?;
    let mut one = Proc::spawn(first).map_err(|message| Failure {
        message,
        log: String::new(),
        gl_unavailable: false,
    })?;
    let mut two: Option<Proc> = None;
    let mut sink: Option<Sink> = None;
    let result = chain(
        &job.second,
        gpu,
        setup,
        job.width,
        job.height,
        job.matrix,
        job.scene,
        job.end,
        &stats_path,
        until,
        &mut one,
        &mut two,
        &mut sink,
        &mut progress,
    );
    let _ = std::fs::remove_file(&stats_path);
    match result {
        Ok((status, stats)) => {
            let log = two.as_mut().map(Proc::finished_log).unwrap_or_default();
            Ok((status, log, stats))
        }
        Err(message) => {
            // Close the encoder's input so a waiting FFmpeg #2 can finish its own
            // error message, then settle both and read their logs to the end.
            if let Some(s) = sink.as_mut() {
                let _ = s.close(Instant::now() + SETTLE);
            }
            let mut log = one.settle();
            if let Some(t) = two.as_mut() {
                log.push('\n');
                log.push_str(&t.settle());
            }
            Err(Failure {
                message,
                log,
                gl_unavailable: false,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn chain(
    second: &dyn Fn(&str, f64) -> Command,
    mut gpu: GlRenderer,
    setup: Instant,
    w: u32,
    h: u32,
    matrix: Matrix,
    mut scene: Layer,
    end: Option<f64>,
    stats_path: &Path,
    until: Instant,
    one: &mut Proc,
    two: &mut Option<Proc>,
    sink: &mut Option<Sink>,
    progress: &mut dyn FnMut(&str),
) -> Result<(ExitStatus, Stats), String> {
    let out = one.tree.0.stdout().take().ok_or("missing FFmpeg output")?;
    if let Layer::Plugin(scene) = &scene
        && let Some(src) = scene.effect.shader.as_deref()
    {
        gpu.set_hooks(Some(src), scene.hook_user())?;
    }
    let mut mix = YuvMix::new(&gpu, matrix).map_err(|e| format!("OpenGL: {e}"))?;
    let mut fonts = Fonts::load()?;
    // An installed font is read before the first frame: an export never draws
    // with fonts « not read yet », only with the chosen one or, if it is missing,
    // the plugin's own face and a warning.
    match &mut scene {
        Layer::Plugin(scene) => {
            if scene.wants_installed_fonts() {
                fonts.set_installed(std::sync::Arc::new(super::text::system_fonts(None)));
            }
        }
        Layer::Overlay(overlay) => overlay.setup(&mut gpu)?,
    }
    let setup_ms = setup.elapsed().as_secs_f64() * 1000.0;
    let size = mix.frame_bytes();
    let frames = Frames::start(out, size);
    let mut tail = StatsTail::open(stats_path, until)?;
    let line_until = || (Instant::now() + LINE_GRACE).min(until);

    // The first frame gives the time base FFmpeg #2 must encode with.
    let Some(mut buf) = frames.next(until)? else {
        return Err("FFmpeg produced no frame".into());
    };
    let (mut pts, time_base) = tail.next(line_until())?.ok_or("frame 0 has no timestamp")?;

    let first = seconds(pts, &time_base)?;
    let mut command = second(&time_base, first);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = two.insert(Proc::spawn(command)?);
    let stdin = started
        .tree
        .0
        .stdin()
        .take()
        .ok_or("missing encoder input")?;
    let stdout = started
        .tree
        .0
        .stdout()
        .take()
        .ok_or("missing encoder output")?;
    let (tx, rx) = mpsc::channel::<String>();
    let reader = thread::spawn(move || {
        let _ = crate::process::lines(stdout, 1024 * 1024, |line| tx.send(line).is_ok());
    });
    let out_sink = sink.insert(Sink::start(stdin, w, h));

    let mut render = Vec::new();
    let (mut layer_ms, mut mix_ms, mut fetch_ms) = (Vec::new(), Vec::new(), Vec::new());
    let mut prev: Option<i64> = None;
    let mut n = 0usize;
    let mut overflow_at = None;
    let mut too_wide_at = None;
    let to_us = |p: i64| seconds(p, &time_base).map(|s| (s * 1e6).round() as u64);
    loop {
        let t0 = Instant::now();
        let t = seconds(pts, &time_base)?;
        let layer = match &mut scene {
            Layer::Plugin(scene) => {
                let (glow_i, gain, main, _) = scene.frame(&mut fonts, &gpu, t);
                if scene.overflowed() && overflow_at.is_none() {
                    overflow_at = Some(t);
                }
                if scene.too_wide() && too_wide_at.is_none() {
                    too_wide_at = Some(t);
                }
                gpu.frame(&glow_i, gain, &main, false)
                    .map_err(|e| format!("OpenGL: {e}"))?;
                gpu.output_texture()
            }
            Layer::Overlay(overlay) => overlay.draw(&mut gpu, t)?,
        };
        let t1 = Instant::now();
        let slot = n % 2;
        mix.submit(&buf, layer, slot)
            .map_err(|e| format!("OpenGL: {e}"))?;
        // A GPU reset loses a robust context (`gl.rs`): every call then answers
        // GL_CONTEXT_LOST, and the frames read back would be empty. Fail, so
        // that nothing is published.
        let e = unsafe { glow::HasContext::get_error(&**gpu.context()) };
        if e == CONTEXT_LOST {
            return Err("the graphics driver was reset during the export".into());
        } else if e != glow::NO_ERROR {
            return Err(format!("OpenGL: error 0x{e:x} after the mix"));
        }
        layer_ms.push((t1 - t0).as_secs_f64() * 1000.0);
        mix_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
        let _ = frames.free.send(buf);
        render.push(t0.elapsed().as_secs_f64() * 1000.0);
        // Frame n-1 is done on the GPU by now (or nearly): copy it out and send it.
        if let Some(p) = prev.take() {
            let mut mixed = out_sink.buffer(size);
            let t2 = Instant::now();
            mix.fetch(1 - slot, &mut mixed)
                .map_err(|e| format!("OpenGL: {e}"))?;
            fetch_ms.push(t2.elapsed().as_secs_f64() * 1000.0);
            out_sink.send(to_us(p)?, None, mixed, until)?;
        }
        prev = Some(pts);
        n += 1;
        while let Ok(line) = rx.try_recv() {
            progress(&line);
        }
        match frames.next(until)? {
            Some(next) => buf = next,
            None => break,
        }
        let (p, tb) = tail
            .next(line_until())?
            .ok_or_else(|| format!("frame {n} has no timestamp"))?;
        if tb != time_base {
            return Err(format!("time base changed from {time_base} to {tb}"));
        }
        pts = p;
    }
    if let Some(p) = prev.take() {
        let mut mixed = out_sink.buffer(size);
        mix.fetch((n - 1) % 2, &mut mixed)
            .map_err(|e| format!("OpenGL: {e}"))?;
        let us = to_us(p)?;
        // FFmpeg 7.1 (the qualified family NOH bundles; docs/BUILDING.md) gives the
        // last frame its BlockDuration minus the pipe input's
        // -itsoffset (`pipe_input`, added when the first frame is not at zero),
        // and an estimate when that is not positive (measured, Task 43: blocks of
        // 15, 40 and 100 ms with -itsoffset 0.025 end 38.4 (estimate), 15 and
        // 75 ms after). The offset is added back so the frame lasts until `end`.
        // tests/captions.rs (delayed video) fails if a runtime behaves otherwise.
        let offset = if first != 0.0 {
            (first * 1e6).round() as i64
        } else {
            0
        };
        let duration = end
            .map(|e| (e * 1e6).round() as i64 - us as i64)
            .filter(|d| *d > 0)
            .map(|d| (d + offset).max(1) as u64);
        out_sink.send(us, duration, mixed, until)?;
    }
    out_sink.close(until)?;
    let status_one = one.wait(until)?;
    if !status_one.success() {
        return Err("FFmpeg could not decode or filter the project video".into());
    }
    // Every frame has exactly one timestamp line, and the reverse.
    if tail.next(Instant::now())?.is_some() {
        return Err(format!("more timestamps than the {n} frames read"));
    }
    let status_two = two.as_mut().unwrap().wait(until)?;
    let _ = reader.join();
    while let Ok(line) = rx.try_recv() {
        progress(&line);
    }
    let adapter = gpu.adapter.clone();
    drop(mix);
    drop(gpu);
    render.sort_by(f64::total_cmp);
    let p95 = render
        .get(((render.len() as f64 * 0.95).ceil() as usize).saturating_sub(1))
        .copied()
        .unwrap_or(0.0);
    let spread = |mut v: Vec<f64>| -> [f64; 3] {
        v.sort_by(f64::total_cmp);
        let at = |q: f64| {
            v.get(((v.len() as f64 * q).ceil() as usize).saturating_sub(1))
                .copied()
                .unwrap_or(0.0)
        };
        [at(0.5), at(0.95), v.last().copied().unwrap_or(0.0)]
    };
    Ok((
        status_two,
        Stats {
            frames: n,
            setup_ms,
            adapter,
            time_base,
            render_ms_p95: p95,
            layer_ms: spread(layer_ms),
            mix_ms: spread(mix_ms),
            fetch_ms: spread(fetch_ms),
            overflow_at,
            too_wide_at,
            font_missing: match &scene {
                Layer::Plugin(scene) => scene.font_missing().map(str::to_string),
                Layer::Overlay(_) => None,
            },
        },
    ))
}
