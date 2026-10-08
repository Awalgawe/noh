//! Shared isolated-worker controller for CLI, desktop and future protocol adapters.
//! The supervisor owns staging and publication, including forced-stop cleanup.
use crate::captions::CaptionRequest;
use crate::engine::{EngineError, Event, ExportRequest, ExportResult, Message};
use crate::inspection::{Diagnosis, InspectionRequest, InspectionResult, Snapshot};
use crate::project::ProjectRequest;
use crate::shorts::ShortRequest;
use crate::subtitles::{SubtitleRequest, SubtitleResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
struct WorkerRequest {
    version: u32,
    request: Option<ExportRequest>,
    #[serde(default)]
    inspection: Option<InspectionRequest>,
    #[serde(default)]
    subtitles: Option<SubtitleRequest>,
    #[serde(default)]
    captions: Option<CaptionRequest>,
    #[serde(default)]
    shorts: Option<ShortRequest>,
    #[serde(default)]
    project: Option<ProjectRequest>,
    #[serde(default)]
    expected: Option<Diagnosis>,
    workspace: PathBuf,
}

enum Operation {
    Export(ExportRequest, Option<Box<Diagnosis>>),
    Inspect(InspectionRequest),
    Subtitles(SubtitleRequest),
    Captions(CaptionRequest),
    Short(ShortRequest),
    Project(ProjectRequest),
}
enum Outcome {
    Export(ExportResult),
    Inspect(InspectionResult),
    Subtitles(SubtitleResult),
}

#[derive(Default)]
struct Mailbox {
    queue: VecDeque<Event>,
    closed: bool,
}
type Shared = Arc<(Mutex<Mailbox>, Condvar)>;

/// Progress is coalesced; diagnostics retain at most 64 bounded entries.
/// Plans, at most one warning per clip plus fixed adaptations, and the terminal
/// event are retained. The request bounds the number of clips to 4096.
pub struct Events(Shared);
impl Events {
    pub fn try_recv(&self) -> Result<Event, mpsc::TryRecvError> {
        let mut state = self.0.0.lock().unwrap();
        state.queue.pop_front().ok_or(if state.closed {
            mpsc::TryRecvError::Disconnected
        } else {
            mpsc::TryRecvError::Empty
        })
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, mpsc::RecvTimeoutError> {
        let state = self.0.0.lock().unwrap();
        let (mut state, _) = self
            .0
            .1
            .wait_timeout_while(state, timeout, |s| s.queue.is_empty() && !s.closed)
            .unwrap();
        state.queue.pop_front().ok_or(if state.closed {
            mpsc::RecvTimeoutError::Disconnected
        } else {
            mpsc::RecvTimeoutError::Timeout
        })
    }
}
fn deliver(events: &Shared, event: Event, repaint: &dyn Fn()) {
    let mut state = events.0.lock().unwrap();
    if state.closed {
        return;
    }
    match &event {
        Event::Progress { .. } => state.queue.retain(|e| !matches!(e, Event::Progress { .. })),
        Event::Diagnostic(_)
            if state
                .queue
                .iter()
                .filter(|e| matches!(e, Event::Diagnostic(_)))
                .count()
                >= 64 =>
        {
            if let Some(index) = state
                .queue
                .iter()
                .position(|e| matches!(e, Event::Diagnostic(_)))
            {
                state.queue.remove(index);
            }
        }
        _ => {}
    }
    state.closed = event.is_terminal();
    state.queue.push_back(event);
    drop(state);
    events.1.notify_all();
    repaint();
}

pub struct Job {
    pub events: Events,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Job {
    pub fn start(request: ExportRequest, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        // Failure to resolve the executable is reported through the same terminal event.
        Self::start_with_worker(
            request,
            std::env::current_exe().unwrap_or_default(),
            repaint,
        )
    }
    pub fn start_with_worker(
        request: ExportRequest,
        worker: PathBuf,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::start_with_cancel(request, worker, Arc::new(AtomicBool::new(false)), repaint)
    }
    pub fn start_with_cancel(
        request: ExportRequest,
        worker: PathBuf,
        cancel: Arc<AtomicBool>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::launch(Operation::Export(request, None), worker, cancel, repaint)
    }
    pub fn start_checked(
        request: ExportRequest,
        expected: Diagnosis,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::start_checked_with_worker(
            request,
            expected,
            std::env::current_exe().unwrap_or_default(),
            repaint,
        )
    }
    pub fn start_checked_with_worker(
        request: ExportRequest,
        expected: Diagnosis,
        worker: PathBuf,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::start_checked_with_cancel(
            request,
            expected,
            worker,
            Arc::new(AtomicBool::new(false)),
            repaint,
        )
    }
    /// Reuse an admission-time cancellation signal, including cancellation before launch.
    pub fn start_checked_with_cancel(
        request: ExportRequest,
        expected: Diagnosis,
        worker: PathBuf,
        cancel: Arc<AtomicBool>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::launch(
            Operation::Export(request, Some(Box::new(expected))),
            worker,
            cancel,
            repaint,
        )
    }
    pub fn inspect(request: InspectionRequest, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        Self::inspect_with_worker(
            request,
            std::env::current_exe().unwrap_or_default(),
            Arc::new(AtomicBool::new(false)),
            repaint,
        )
    }
    pub fn inspect_with_worker(
        request: InspectionRequest,
        worker: PathBuf,
        cancel: Arc<AtomicBool>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::launch(Operation::Inspect(request), worker, cancel, repaint)
    }
    pub fn transcribe(
        request: SubtitleRequest,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::transcribe_with_worker(
            request,
            std::env::current_exe().unwrap_or_default(),
            repaint,
        )
    }
    pub fn transcribe_with_worker(
        request: SubtitleRequest,
        worker: PathBuf,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::transcribe_with_cancel(request, worker, Arc::new(AtomicBool::new(false)), repaint)
    }
    pub fn transcribe_with_cancel(
        request: SubtitleRequest,
        worker: PathBuf,
        cancel: Arc<AtomicBool>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::launch(Operation::Subtitles(request), worker, cancel, repaint)
    }
    /// Render a reviewed SRT on one video; publication and cancellation are shared.
    pub fn burn(request: CaptionRequest, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        Self::burn_with_worker(
            request,
            std::env::current_exe().unwrap_or_default(),
            repaint,
        )
    }
    pub fn burn_with_worker(
        request: CaptionRequest,
        worker: PathBuf,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::burn_with_cancel(request, worker, Arc::new(AtomicBool::new(false)), repaint)
    }
    pub fn burn_with_cancel(
        request: CaptionRequest,
        worker: PathBuf,
        cancel: Arc<AtomicBool>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::launch(Operation::Captions(request), worker, cancel, repaint)
    }
    /// Extract one selected interval; staging and publication remain supervisor-owned.
    pub fn short(request: ShortRequest, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        Self::short_with_worker(
            request,
            std::env::current_exe().unwrap_or_default(),
            repaint,
        )
    }
    pub fn short_with_worker(
        request: ShortRequest,
        worker: PathBuf,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::short_with_cancel(request, worker, Arc::new(AtomicBool::new(false)), repaint)
    }
    pub fn short_with_cancel(
        request: ShortRequest,
        worker: PathBuf,
        cancel: Arc<AtomicBool>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::launch(Operation::Short(request), worker, cancel, repaint)
    }
    /// Compose original project inputs, optionally selecting a WAV interval and captions.
    pub fn project(request: ProjectRequest, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        Self::project_with_worker(
            request,
            std::env::current_exe().unwrap_or_default(),
            repaint,
        )
    }
    pub fn project_with_worker(
        request: ProjectRequest,
        worker: PathBuf,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::project_with_cancel(request, worker, Arc::new(AtomicBool::new(false)), repaint)
    }
    pub fn project_with_cancel(
        request: ProjectRequest,
        worker: PathBuf,
        cancel: Arc<AtomicBool>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::launch(Operation::Project(request), worker, cancel, repaint)
    }
    fn launch(
        operation: Operation,
        worker: PathBuf,
        cancel: Arc<AtomicBool>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let mailbox: Shared = Arc::default();
        let target = mailbox.clone();
        let cancelled = cancel.clone();
        let handle = thread::spawn(move || {
            let inspecting = matches!(operation, Operation::Inspect(_));
            let transcribing = matches!(operation, Operation::Subtitles(_));
            let result = run(&worker, &operation, &cancelled, |event| {
                deliver(&target, event, &repaint)
            });
            let terminal = match result {
                Err(error) if error.code == "status.export_cancelled" => Event::Cancelled,
                Ok(Outcome::Export(result)) => Event::Done(Ok(result)),
                Ok(Outcome::Inspect(result)) => Event::Inspected(Ok(result)),
                Ok(Outcome::Subtitles(result)) => Event::Subtitled(Ok(result)),
                Err(error) if inspecting => Event::Inspected(Err(error)),
                Err(error) if transcribing => Event::Subtitled(Err(error)),
                Err(error) => Event::Done(Err(error)),
            };
            deliver(&target, terminal, &repaint);
        });
        Self {
            events: Events(mailbox),
            cancel,
            thread: Some(handle),
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn cancelled() -> EngineError {
    EngineError::new(
        "status.export_cancelled",
        "cancel",
        None,
        "Operation cancelled.",
    )
}

enum Pipe {
    Event(Event),
    Error(EngineError),
    Closed,
}
fn read_pipe(
    reader: impl std::io::Read + Send + 'static,
    protocol: bool,
    tx: mpsc::SyncSender<Pipe>,
) {
    thread::spawn(move || {
        let result = crate::process::lines(
            reader,
            if protocol { 4 * 1024 * 1024 } else { 16384 },
            |line| {
                let event = if protocol {
                    match serde_json::from_str::<Event>(&line) {
                        Ok(event) => event,
                        Err(error) => {
                            let _ = tx.send(Pipe::Error(EngineError::new(
                                "error.protocol",
                                "worker",
                                None,
                                error,
                            )));
                            return false;
                        }
                    }
                } else {
                    Event::Diagnostic(crate::engine::bounded(&line, 4096))
                };
                tx.send(Pipe::Event(event)).is_ok()
            },
        );
        if let Err(error) = result {
            let _ = tx.send(Pipe::Error(EngineError::new(
                "error.protocol",
                "read_worker",
                None,
                error,
            )));
        }
        let _ = tx.send(Pipe::Closed);
    });
}

fn run(
    worker: &Path,
    operation: &Operation,
    cancel: &AtomicBool,
    emit: impl FnMut(Event),
) -> Result<Outcome, EngineError> {
    run_with_command(
        worker,
        operation,
        cancel,
        |request_file| {
            let mut command = crate::command(worker);
            command.arg("--engine-worker").arg(request_file);
            command
        },
        emit,
    )
}

fn run_with_command(
    worker: &Path,
    operation: &Operation,
    cancel: &AtomicBool,
    command: impl FnOnce(&Path) -> std::process::Command,
    mut emit: impl FnMut(Event),
) -> Result<Outcome, EngineError> {
    if cancel.load(Ordering::Acquire) {
        return Err(cancelled());
    }
    let (request, inspection, subtitles, captions, shorts, project, expected) = match operation {
        Operation::Export(request, expected) => (
            Some(request),
            None,
            None,
            None,
            None,
            None,
            expected.as_deref(),
        ),
        Operation::Inspect(request) => (None, Some(request), None, None, None, None, None),
        Operation::Subtitles(request) => (None, None, Some(request), None, None, None, None),
        Operation::Captions(request) => (None, None, None, Some(request), None, None, None),
        Operation::Short(request) => (None, None, None, None, Some(request), None, None),
        Operation::Project(request) => (None, None, None, None, None, Some(request), None),
    };
    if let Some(InspectionRequest::Plan { request, .. }) = inspection {
        request.validate()?;
    }
    let mut snapshot = None;
    let mut subtitle_snapshot = None;
    let output = if let Some(project) = project {
        project.validate()?;
        let output = std::path::absolute(&project.montage.output).map_err(|e| {
            EngineError::new(
                "error.output_folder",
                "validate",
                Some(&project.montage.output),
                e,
            )
        })?;
        if output.exists() {
            return Err(EngineError::new(
                "error.output_exists",
                "validate",
                Some(&output),
                "Output already exists. Choose another name.",
            ));
        }
        snapshot = Some(project.snapshot()?);
        Some(output)
    } else if let Some(request) = request {
        request.validate()?;
        if let Some(expected) = expected {
            expected.validate(request)?;
        }
        let output = std::path::absolute(&request.output).map_err(|e| {
            EngineError::new("error.output_folder", "validate", Some(&request.output), e)
        })?;
        if output.exists() {
            return Err(EngineError::new(
                "error.output_exists",
                "validate",
                Some(&output),
                format!(
                    "Output already exists: {}. Choose another name.",
                    output.display()
                ),
            ));
        }
        // Preserve existing input-specific errors before recording invalidation stamps.
        for item in &request.items {
            let path = item.path();
            if !path.is_file() {
                return Err(EngineError::new(
                    if item.is_image() {
                        "error.image"
                    } else {
                        "error.video"
                    },
                    "validate_input",
                    Some(path),
                    "Media file not found",
                ));
            }
        }
        snapshot = Some(Snapshot::project(request)?);
        Some(output)
    } else if let Some(request) = subtitles {
        request.validate()?;
        let output = std::path::absolute(&request.output).map_err(|e| {
            EngineError::new("error.output_folder", "validate", Some(&request.output), e)
        })?;
        if output.exists() {
            return Err(EngineError::new(
                "error.output_exists",
                "validate",
                Some(&output),
                format!(
                    "Output already exists: {}. Choose another name.",
                    output.display()
                ),
            ));
        }
        subtitle_snapshot = Some(request.snapshot()?);
        Some(output)
    } else if let Some(request) = captions {
        request.validate()?;
        let output = std::path::absolute(&request.output).map_err(|e| {
            EngineError::new("error.output_folder", "validate", Some(&request.output), e)
        })?;
        if output.exists() {
            return Err(EngineError::new(
                "error.output_exists",
                "validate",
                Some(&output),
                format!(
                    "Output already exists: {}. Choose another name.",
                    output.display()
                ),
            ));
        }
        snapshot = Some(request.snapshot()?);
        Some(output)
    } else if let Some(request) = shorts {
        request.validate()?;
        let output = std::path::absolute(&request.output).map_err(|e| {
            EngineError::new("error.output_folder", "validate", Some(&request.output), e)
        })?;
        if output.exists() {
            return Err(EngineError::new(
                "error.output_exists",
                "validate",
                Some(&output),
                format!(
                    "Output already exists: {}. Choose another name.",
                    output.display()
                ),
            ));
        }
        snapshot = Some(request.snapshot()?);
        Some(output)
    } else {
        None
    };
    let temp = std::env::temp_dir();
    let parent = output.as_ref().and_then(|p| p.parent()).unwrap_or(&temp);
    let workspace = tempfile::Builder::new()
        .prefix(".noh-")
        .tempdir_in(parent)
        .map_err(|e| EngineError::new("error.output_folder", "staging", Some(parent), e))?;
    let result = (|| {
        let wire = WorkerRequest {
            version: 1,
            request: request.cloned(),
            inspection: inspection.cloned(),
            subtitles: subtitles.cloned(),
            captions: captions.cloned(),
            shorts: shorts.cloned(),
            project: project.cloned(),
            expected: expected.cloned(),
            workspace: workspace.path().into(),
        };
        let request_file = workspace.path().join("request.json");
        let bytes = serde_json::to_vec(&wire)
            .map_err(|e| EngineError::new("error.request", "serialize", None, e))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(EngineError::new(
                "error.request",
                "serialize",
                None,
                "Request exceeds size limit",
            ));
        }
        std::fs::write(&request_file, bytes)
            .map_err(|e| EngineError::new("error.engine", "staging", Some(&request_file), e))?;
        let mut command = command(&request_file);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if cancel.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        let mut child = crate::process::Tree::spawn(command, true)
            .map_err(|e| EngineError::new("error.start", "spawn_worker", Some(worker), e))?;
        let (tx, rx) = mpsc::sync_channel(64);
        read_pipe(child.0.stdout().take().unwrap(), true, tx.clone());
        read_pipe(child.0.stderr().take().unwrap(), false, tx);
        let mut result = None;
        let mut status = None;
        let mut closed = 0;
        let mut exited = None;
        let mut last_progress = 0;
        let mut warnings = 0;
        let mut plans = 0;
        let started = Instant::now();
        // Backstop for a wedged worker, including blocked filesystem/protocol IO.
        let inspection_limit = match inspection {
            Some(InspectionRequest::Metadata { .. } | InspectionRequest::Image { .. }) => 30,
            Some(InspectionRequest::Plan { request, level }) => {
                let per_clip = if *level == crate::inspection::Level::Exact {
                    160
                } else {
                    20
                };
                30 + per_clip * request.items.len().min(4096) as u64
            }
            None => 0,
        };
        loop {
            if project.is_some()
                && started.elapsed() > Duration::from_secs(crate::project::TOTAL_TIMEOUT_SECS)
            {
                return Err(EngineError::new(
                    "error.timeout",
                    "project_export",
                    None,
                    "Project rendering deadline exceeded.",
                ));
            }
            if shorts.is_some()
                && started.elapsed() > Duration::from_secs(crate::shorts::TOTAL_TIMEOUT_SECS)
            {
                return Err(EngineError::new(
                    "error.timeout",
                    "make_short",
                    None,
                    "Short rendering deadline exceeded.",
                ));
            }
            if captions.is_some()
                && started.elapsed() > Duration::from_secs(crate::captions::TOTAL_TIMEOUT_SECS)
            {
                return Err(EngineError::new(
                    "error.timeout",
                    "burn_captions",
                    None,
                    "Caption rendering deadline exceeded.",
                ));
            }
            if subtitles.is_some()
                && started.elapsed() > Duration::from_secs(crate::subtitles::TOTAL_TIMEOUT_SECS)
            {
                return Err(EngineError::new(
                    "error.timeout",
                    "transcribe",
                    None,
                    "Transcription deadline exceeded.",
                ));
            }
            if inspection_limit > 0 && started.elapsed() > Duration::from_secs(inspection_limit) {
                return Err(EngineError::new(
                    "error.timeout",
                    "inspect",
                    None,
                    "Inspection deadline exceeded.",
                ));
            }
            if cancel.load(Ordering::Acquire) {
                child.0.start_kill().ok();
                return Err(cancelled());
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Pipe::Event(Event::Done(value))) => {
                    if result.is_some()
                        || (request.is_none()
                            && captions.is_none()
                            && shorts.is_none()
                            && project.is_none()
                            && value.is_ok())
                    {
                        return Err(EngineError::new(
                            "error.protocol",
                            "worker",
                            None,
                            "Duplicate worker result",
                        ));
                    }
                    result = Some(value.map(Outcome::Export));
                }
                Ok(Pipe::Event(Event::Inspected(value))) => {
                    if result.is_some() || inspection.is_none() {
                        return Err(EngineError::new(
                            "error.protocol",
                            "worker",
                            None,
                            "Unexpected inspection result",
                        ));
                    }
                    result = Some(value.map(Outcome::Inspect));
                }
                Ok(Pipe::Event(Event::Subtitled(value))) => {
                    if result.is_some() || subtitles.is_none() {
                        return Err(EngineError::new(
                            "error.protocol",
                            "worker",
                            None,
                            "Unexpected subtitle result",
                        ));
                    }
                    result = Some(value.map(Outcome::Subtitles));
                }
                Ok(Pipe::Event(Event::Progress { percent, phase })) => {
                    last_progress = last_progress.max(percent.min(99));
                    emit(Event::Progress {
                        percent: last_progress,
                        phase,
                    });
                }
                Ok(Pipe::Event(event)) => {
                    if matches!(event, Event::Warning(_)) {
                        warnings += 1;
                    }
                    if matches!(event, Event::Plan(_)) {
                        plans += 1;
                    }
                    if event.is_terminal()
                        || warnings
                            > request.map_or(
                                if let Some(project) = project {
                                    project.montage.items.len() + 10
                                } else if shorts.is_some() {
                                    10
                                } else if captions.is_some() {
                                    8
                                } else {
                                    4
                                },
                                |r| r.items.len() + 4,
                            )
                        || plans > 4
                    {
                        return Err(EngineError::new(
                            "error.protocol",
                            "worker",
                            None,
                            "Unexpected or excessive worker events",
                        ));
                    }
                    emit(event);
                }
                Ok(Pipe::Error(error)) => return Err(error),
                Ok(Pipe::Closed) => closed += 1,
                Err(_) => {}
            }
            if status.is_none() {
                status = child.0.try_wait().map_err(|e| {
                    EngineError::new("error.engine", "wait_worker", Some(worker), e)
                })?;
                if status.is_some() {
                    exited = Some(Instant::now());
                }
            }
            if status.is_some() && closed == 2 {
                break;
            }
            if exited.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
                return Err(EngineError::new(
                    "error.protocol",
                    "worker",
                    None,
                    "Worker exited with open descendant pipes",
                ));
            }
        }
        // Stop any surviving descendants before deleting supervisor-owned files.
        child.0.start_kill().ok();
        let result = result.ok_or_else(|| {
            EngineError::new(
                "error.engine",
                "worker",
                Some(worker),
                "Worker exited without a result",
            )
        })??;
        if !status.unwrap().success() {
            return Err(EngineError::new(
                "error.engine",
                "worker",
                Some(worker),
                "Worker reported success but failed",
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        if inspection.is_some() {
            return match result {
                Outcome::Inspect(value) => Ok(Outcome::Inspect(value)),
                _ => Err(EngineError::new(
                    "error.protocol",
                    "worker",
                    None,
                    "Expected inspection result",
                )),
            };
        }
        if let Outcome::Subtitles(mut result) = result {
            let output = output.as_ref().ok_or_else(|| {
                EngineError::new(
                    "error.protocol",
                    "worker",
                    None,
                    "Unexpected subtitle publication",
                )
            })?;
            if result.track.duration_ms > crate::subtitles::MAX_AUDIO_MS {
                return Err(EngineError::new(
                    "error.subtitle_limit",
                    "validate_subtitles",
                    None,
                    "Subtitle result exceeds the supported audio duration",
                ));
            }
            let srt = result.track.to_srt().map_err(|e| {
                EngineError::new("error.subtitle_result", "validate_subtitles", None, e)
            })?;
            let staged = workspace.path().join("subtitle.srt");
            let metadata = std::fs::symlink_metadata(&staged).map_err(|e| {
                EngineError::new(
                    "error.subtitle_result",
                    "validate_subtitles",
                    Some(&staged),
                    e,
                )
            })?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(EngineError::new(
                    "error.subtitle_result",
                    "validate_subtitles",
                    Some(&staged),
                    "Staged subtitles must be a regular file, not a link",
                ));
            }
            // Publish exactly the validated timed track. Never trust a worker's
            // output path or a success event without its complete staged file.
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(&staged)
                .and_then(|file| file.take(srt.len() as u64 + 1).read_to_end(&mut bytes))
                .map_err(|e| {
                    EngineError::new(
                        "error.subtitle_result",
                        "validate_subtitles",
                        Some(&staged),
                        e,
                    )
                })?;
            if bytes != srt.as_bytes() {
                return Err(EngineError::new(
                    "error.subtitle_result",
                    "validate_subtitles",
                    Some(&staged),
                    "Staged subtitles do not match the validated timed track",
                ));
            }
            result.output = output.clone();
            let event_bytes = serde_json::to_vec(&Event::Subtitled(Ok(result.clone())))
                .map_err(|e| EngineError::new("error.subtitle_result", "serialize", None, e))?;
            if event_bytes.len() > 4 * 1024 * 1024 {
                return Err(EngineError::new(
                    "error.subtitle_limit",
                    "serialize",
                    None,
                    "Subtitle result exceeds worker limit",
                ));
            }
            if let Some(snapshot) = subtitle_snapshot {
                snapshot.verify()?;
            }
            commit(&staged, output, cancel)?;
            emit(Event::Progress {
                percent: 100,
                phase: Message::parse("status.done"),
            });
            return Ok(Outcome::Subtitles(result));
        }
        let Outcome::Export(result) = result else {
            return Err(EngineError::new(
                "error.protocol",
                "worker",
                None,
                "Expected export result",
            ));
        };
        if let Some(snapshot) = snapshot {
            snapshot.verify()?;
        }
        let output = output.as_ref().unwrap();
        let extension = output
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let staged = workspace.path().join(format!("export.{extension}"));
        if captions.is_some() || shorts.is_some() || project.is_some() {
            let result_code = if shorts.is_some() {
                "error.short_result"
            } else {
                "error.caption_result"
            };
            let result_stage = if shorts.is_some() {
                "validate_short"
            } else {
                "validate_captions"
            };
            let metadata = std::fs::symlink_metadata(&staged)
                .map_err(|e| EngineError::new(result_code, result_stage, Some(&staged), e))?;
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() == 0
                || !result.duration.is_finite()
                || result.duration <= 0.0
            {
                return Err(EngineError::new(
                    result_code,
                    result_stage,
                    Some(&staged),
                    "Rendering must produce a nonempty regular video and a finite duration.",
                ));
            }
        }
        commit(&staged, output, cancel)?;
        emit(Event::Progress {
            percent: 100,
            phase: Message::parse("status.done"),
        });
        Ok(Outcome::Export(ExportResult {
            output: output.clone(),
            ..result
        }))
    })();
    // Process termination is asynchronous on Windows. Retry only our own folder
    // before sending the terminal event; never scan output siblings by PID.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match std::fs::remove_dir_all(workspace.path()) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                if result.is_ok() {
                    emit(Event::Warning(Message {
                        code: "warning.cleanup".into(),
                        args: vec![workspace.path().display().to_string()],
                    }));
                    emit(Event::Diagnostic(error.to_string()));
                    break;
                }
                return Err(EngineError::new(
                    "error.cleanup",
                    "cleanup",
                    Some(workspace.path()),
                    error,
                ));
            }
        }
    }
    result
}

fn commit(staged: &Path, output: &Path, cancel: &AtomicBool) -> Result<(), EngineError> {
    // The successful move is the commit point. Never recheck cancellation after it.
    if cancel.load(Ordering::Acquire) {
        return Err(cancelled());
    }
    crate::storage::publish(staged, output).map_err(|e| {
        EngineError::new(
            if output.exists() {
                "error.output_exists"
            } else {
                "error.publish"
            },
            "publish",
            Some(output),
            e,
        )
    })
}

/// Called only by binary entry points. JSON is restricted to this transport.
pub fn worker(path: &Path) -> i32 {
    let mut stdout = std::io::stdout().lock();
    let mut write = |event: Event| {
        let _ = serde_json::to_writer(&mut stdout, &event);
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
    };
    let mut transcribing = false;
    let result = (|| {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes))
            .map_err(|e| EngineError::new("error.request", "read_request", Some(path), e))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(EngineError::new(
                "error.request",
                "read_request",
                Some(path),
                "Request exceeds size limit",
            ));
        }
        let wire: WorkerRequest = serde_json::from_slice(&bytes)
            .map_err(|e| EngineError::new("error.request", "read_request", Some(path), e))?;
        transcribing = wire.subtitles.is_some()
            && wire.request.is_none()
            && wire.inspection.is_none()
            && wire.captions.is_none()
            && wire.shorts.is_none()
            && wire.project.is_none();
        if wire.version != 1 {
            return Err(EngineError::new(
                "error.protocol",
                "read_request",
                Some(path),
                "Unsupported worker protocol version",
            ));
        }
        match (
            wire.request,
            wire.inspection,
            wire.subtitles,
            wire.captions,
            wire.shorts,
            wire.project,
        ) {
            (Some(request), None, None, None, None, None) => crate::engine::execute_checked(
                request,
                &wire.workspace,
                wire.expected.as_ref(),
                &mut write,
            )
            .map(Outcome::Export),
            (None, Some(request), None, None, None, None) if wire.expected.is_none() => {
                crate::inspection::inspect(&request, &mut write).map(Outcome::Inspect)
            }
            (None, None, Some(request), None, None, None) if wire.expected.is_none() => {
                crate::subtitles::execute(&request, &wire.workspace, &mut write)
                    .map(Outcome::Subtitles)
            }
            (None, None, None, Some(request), None, None) if wire.expected.is_none() => {
                crate::captions::execute(&request, &wire.workspace, &mut write).map(Outcome::Export)
            }
            (None, None, None, None, Some(request), None) if wire.expected.is_none() => {
                crate::shorts::execute(&request, &wire.workspace, &mut write).map(Outcome::Export)
            }
            (None, None, None, None, None, Some(request)) if wire.expected.is_none() => {
                crate::project::execute(&request, &wire.workspace, &mut write).map(Outcome::Export)
            }
            _ => Err(EngineError::new(
                "error.request",
                "read_request",
                Some(path),
                "Expected one operation",
            )),
        }
    })();
    let code = i32::from(result.is_err());
    write(match result {
        Ok(Outcome::Export(result)) => Event::Done(Ok(result)),
        Ok(Outcome::Inspect(result)) => Event::Inspected(Ok(result)),
        Ok(Outcome::Subtitles(result)) => Event::Subtitled(Ok(result)),
        Err(error) if transcribing => Event::Subtitled(Err(error)),
        Err(error) => Event::Done(Err(error)),
    });
    code
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slow_event_readers_have_bounded_logs_and_keep_reliable_events() {
        let mailbox: Shared = Arc::default();
        for i in 0..10_000 {
            deliver(&mailbox, Event::Diagnostic(format!("line {i}")), &|| {});
            deliver(
                &mailbox,
                Event::Progress {
                    percent: i % 100,
                    phase: Message::parse("progress.copy"),
                },
                &|| {},
            );
        }
        deliver(
            &mailbox,
            Event::Warning(Message::parse("warning.partial_codec")),
            &|| {},
        );
        deliver(&mailbox, Event::Cancelled, &|| {});
        deliver(&mailbox, Event::Done(Err(cancelled())), &|| {});
        let state = mailbox.0.lock().unwrap();
        assert_eq!(state.queue.len(), 67);
        assert_eq!(state.queue.iter().filter(|e| e.is_terminal()).count(), 1);
        assert!(state.queue.iter().any(|e| matches!(e, Event::Warning(_))));
    }
}
