//! Session-local job ownership and bounded, event-driven status retrieval.
//! Public methods are blocking adapter calls; invoke them outside Tokio threads.
use super::types::*;
use crate::{
    engine::{Event, ExportRequest, ExportResult, Message},
    inspection::{Diagnosis, InspectionRequest, InspectionResult, Level},
    jobs::Job,
    media::MediaInfo,
    subtitles::{SubtitleRequest, SubtitleResult},
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    io::{self, Write},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_TERMINAL: usize = 16;
const MAX_RETAINED: usize = 32 * 1024 * 1024;
const MAX_SUBTITLE_RESULT: usize = 4 * 1024 * 1024;
const MAX_REQUEST: usize = 4 * 1024 * 1024;
const MAX_WARNINGS: usize = 4096 + 8;
const MAX_DIAGNOSTICS: usize = 64;
const MAX_WARNING_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct Session(Arc<Owner>);
struct Owner {
    shared: Arc<Shared>,
    default_ffmpeg: PathBuf,
    // Start/teardown serialization never holds the status mutex while joining.
    launch: Mutex<()>,
    collector: Mutex<Option<JoinHandle<()>>>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
struct State {
    prefix: String,
    next_id: u64,
    closed: bool,
    active: Option<Active>,
    terminal: VecDeque<Record>,
    retained_bytes: usize,
}
struct Active {
    record: Record,
    cancel: Arc<AtomicBool>,
    job: Option<Arc<Job>>,
}
struct Record {
    id: String,
    state: JobState,
    revision: u64,
    progress: Option<Progress>,
    warnings: VecDeque<Stored<Phase>>,
    diagnostics: VecDeque<Stored<String>>,
    warning_bytes: usize,
    diagnostic_bytes: usize,
    progress_bytes: usize,
    result: Option<Arc<RetainedResult>>,
    error: Option<SessionError>,
    bytes: usize,
}
struct Stored<T> {
    value: T,
    bytes: usize,
}
#[derive(Serialize)]
enum RetainedResult {
    Metadata(MediaInfo),
    Diagnosis(Box<Diagnosis>),
    Export(ExportResult),
    Subtitles(SubtitleResult),
}
enum Operation {
    Inspect(InspectionRequest),
    Export(ExportRequest, Option<Box<Diagnosis>>),
    Transcribe(SubtitleRequest),
    Captions(crate::captions::CaptionRequest),
    Short(crate::shorts::ShortRequest),
    Project(crate::project::ProjectRequest),
}

impl Session {
    pub fn new(default_ffmpeg: PathBuf) -> Result<Self, SessionError> {
        // tempfile already provides OS-seeded random names; no new RNG dependency.
        let nonce = tempfile::Builder::new()
            .prefix("noh-mcp-")
            .rand_bytes(32)
            .tempdir()
            .map_err(|e| SessionError::new("mcp.start_failed", &e.to_string()))?;
        let prefix = format!("{}-", nonce.path().file_name().unwrap().to_string_lossy());
        drop(nonce);
        Ok(Self::with_prefix(default_ffmpeg, prefix))
    }

    fn with_prefix(default_ffmpeg: PathBuf, prefix: String) -> Self {
        Self(Arc::new(Owner {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    prefix,
                    next_id: 1,
                    closed: false,
                    active: None,
                    terminal: VecDeque::new(),
                    retained_bytes: 0,
                }),
                changed: Condvar::new(),
            }),
            default_ffmpeg,
            launch: Mutex::new(()),
            collector: Mutex::new(None),
        }))
    }

    pub fn inspect(&self, input: InspectInput) -> Result<StartReply, SessionError> {
        check_request_size(&input)?;
        let path = absolute(&input.path)?;
        let ffmpeg = Some(
            input
                .ffmpeg
                .as_deref()
                .map(absolute)
                .transpose()?
                .unwrap_or_else(|| self.0.default_ffmpeg.clone()),
        );
        let request = match input.kind {
            MediaKind::Image => InspectionRequest::Image { path, ffmpeg },
            MediaKind::Video | MediaKind::Wav => InspectionRequest::Metadata {
                path,
                ffmpeg,
                wav: matches!(input.kind, MediaKind::Wav),
            },
        };
        self.start(Operation::Inspect(request))
    }

    pub fn diagnose(&self, input: DiagnoseInput) -> Result<StartReply, SessionError> {
        check_request_size(&input)?;
        let request = input.request.into_engine(&self.0.default_ffmpeg, false)?;
        let level = match input.level {
            DiagnosisLevel::Quick => Level::Quick,
            DiagnosisLevel::Exact => Level::Exact,
        };
        self.start(Operation::Inspect(InspectionRequest::Plan {
            request,
            level,
        }))
    }

    pub fn export(&self, input: ExportInput) -> Result<StartReply, SessionError> {
        check_request_size(&input)?;
        let request = input.request.into_engine(&self.0.default_ffmpeg, false)?;
        let result = {
            let state = self.0.shared.state.lock().unwrap();
            let record = state.lookup(&input.diagnosis_job_id)?;
            if record.state != JobState::Succeeded {
                return Err(SessionError::new(
                    "mcp.exact_required",
                    "Wait for a successful exact diagnosis before exporting.",
                ));
            }
            record.result.clone()
        };
        let Some(result) = result else {
            return Err(SessionError::new(
                "mcp.exact_required",
                "This job does not contain an exact diagnosis.",
            ));
        };
        let RetainedResult::Diagnosis(diagnosis) = result.as_ref() else {
            return Err(SessionError::new(
                "mcp.exact_required",
                "This job does not contain an exact diagnosis.",
            ));
        };
        if diagnosis.level != Level::Exact {
            return Err(SessionError::new(
                "mcp.exact_required",
                "Quick metadata cannot approve an export. Run exact diagnosis.",
            ));
        }
        // Filesystem stamp validation and diagnosis cloning occur outside the mutex.
        // The controller also reinspects and verifies before its publication point.
        diagnosis.validate(&request).map_err(SessionError::from)?;
        self.start(Operation::Export(request, Some(diagnosis.clone())))
    }

    pub fn preview(&self, input: PreviewInput) -> Result<StartReply, SessionError> {
        check_request_size(&input)?;
        let request = input.request.into_engine(&self.0.default_ffmpeg, true)?;
        self.start(Operation::Export(request, None))
    }

    pub fn transcribe(&self, input: TranscribeInput) -> Result<StartReply, SessionError> {
        check_request_size(&input)?;
        let request = input.into_request(&self.0.default_ffmpeg)?;
        self.start(Operation::Transcribe(request))
    }

    pub fn captions(&self, input: CaptionsInput) -> Result<StartReply, SessionError> {
        check_request_size(&input)?;
        let request = input.into_request(&self.0.default_ffmpeg)?;
        self.start(Operation::Captions(request))
    }

    pub fn short(&self, input: ShortInput) -> Result<StartReply, SessionError> {
        check_request_size(&input)?;
        let request = input.into_request(&self.0.default_ffmpeg)?;
        self.start(Operation::Short(request))
    }

    pub fn project(&self, input: ProjectInput) -> Result<StartReply, SessionError> {
        check_request_size(&input)?;
        let request = input.into_request(&self.0.default_ffmpeg)?;
        self.start(Operation::Project(request))
    }

    fn start(&self, operation: Operation) -> Result<StartReply, SessionError> {
        let _launch = self.0.launch.lock().unwrap();
        let (id, cancel) = {
            let mut state = self.0.shared.state.lock().unwrap();
            if state.closed {
                return Err(SessionError::new(
                    "mcp.closed",
                    "The client session is shutting down.",
                ));
            }
            if state.active.is_some() {
                return Err(SessionError::new(
                    "mcp.busy",
                    "One job is already active. Wait for it or cancel it before starting another.",
                ));
            }
            let id = format!("{}{:016x}", state.prefix, state.next_id);
            state.next_id = state
                .next_id
                .checked_add(1)
                .ok_or_else(|| SessionError::new("mcp.closed", "Session ID space exhausted."))?;
            let cancel = Arc::new(AtomicBool::new(false));
            state.active = Some(Active {
                record: Record::running(id.clone()),
                cancel: cancel.clone(),
                job: None,
            });
            (id, cancel)
        };
        self.0.shared.changed.notify_all();
        // Reap the prior finished collector before launching another. It owns no
        // status lock here; active remains reserved throughout this operation.
        let previous = self.0.collector.lock().unwrap().take();
        if let Some(handle) = previous {
            let _ = handle.join();
        }
        let worker = std::env::current_exe().unwrap_or_default();
        let job = Arc::new(match operation {
            Operation::Inspect(request) => Job::inspect_with_worker(request, worker, cancel, || {}),
            Operation::Export(request, Some(expected)) => {
                Job::start_checked_with_cancel(request, *expected, worker, cancel, || {})
            }
            Operation::Export(request, None) => {
                Job::start_with_cancel(request, worker, cancel, || {})
            }
            Operation::Transcribe(request) => {
                Job::transcribe_with_cancel(request, worker, cancel, || {})
            }
            Operation::Captions(request) => Job::burn_with_cancel(request, worker, cancel, || {}),
            Operation::Short(request) => Job::short_with_cancel(request, worker, cancel, || {}),
            Operation::Project(request) => Job::project_with_cancel(request, worker, cancel, || {}),
        });
        {
            let mut state = self.0.shared.state.lock().unwrap();
            state.active.as_mut().unwrap().job = Some(job.clone());
        }
        let shared = self.0.shared.clone();
        let collector_id = id.clone();
        let collector = thread::Builder::new()
            .name("noh-mcp-collector".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    collect(&shared, &collector_id, &job)
                }));
                // The last owned Job is dropped and joined before active is released.
                let owned = {
                    let mut state = shared.state.lock().unwrap();
                    state.active.as_mut().and_then(|a| a.job.take())
                };
                drop(job);
                drop(owned);
                match result {
                    Ok(terminal) => finish(&shared, &collector_id, terminal),
                    Err(_) => finish(
                        &shared,
                        &collector_id,
                        Err(SessionError::new(
                            "mcp.collector_failed",
                            "The job collector stopped unexpectedly.",
                        )),
                    ),
                }
            });
        match collector {
            Ok(handle) => *self.0.collector.lock().unwrap() = Some(handle),
            Err(error) => {
                let owned = {
                    let mut state = self.0.shared.state.lock().unwrap();
                    state.active.as_mut().and_then(|a| a.job.take())
                };
                if let Some(job) = owned {
                    job.cancel();
                    drop(job);
                }
                let error = SessionError::new("mcp.start_failed", &error.to_string());
                finish(&self.0.shared, &id, Err(error.clone()));
                return Err(error);
            }
        }
        Ok(StartReply {
            version: 1,
            job_id: id,
            state: JobState::Running,
            revision: 1,
        })
    }

    pub fn job(&self, input: JobInput) -> Result<JobReply, SessionError> {
        if input.wait_ms > 30000 {
            return Err(SessionError::new(
                "mcp.invalid_request",
                "wait_ms must be between 0 and 30000.",
            ));
        }
        let deadline = Instant::now() + Duration::from_millis(input.wait_ms.into());
        let (mut reply, result) = {
            let mut state = self.0.shared.state.lock().unwrap();
            loop {
                let record = state.lookup(&input.job_id)?;
                if input
                    .after_revision
                    .is_some_and(|revision| revision > record.revision)
                {
                    return Err(SessionError::new(
                        "mcp.invalid_revision",
                        "after_revision is newer than this job's current revision.",
                    ));
                }
                let changed = input.after_revision != Some(record.revision);
                if changed
                    || record.state.is_terminal()
                    || state.closed
                    || Instant::now() >= deadline
                {
                    break record.reply(changed, input.include_result);
                }
                let duration = deadline.saturating_duration_since(Instant::now());
                state = self
                    .0
                    .shared
                    .changed
                    .wait_timeout(state, duration)
                    .unwrap()
                    .0;
            }
        };
        // JSON conversion may be large; immutable data is shared outside the lock.
        reply.result = result.map(|result| result.to_reply()).transpose()?;
        Ok(reply)
    }

    pub fn cancel(&self, input: CancelInput) -> Result<JobReply, SessionError> {
        {
            let mut state = self.0.shared.state.lock().unwrap();
            state.lookup(&input.job_id)?;
            if let Some(active) = state
                .active
                .as_mut()
                .filter(|a| a.record.id == input.job_id)
            {
                if active.record.state == JobState::Running {
                    active.record.state = JobState::Cancelling;
                    active.record.bump();
                }
                // Signal exists before Job creation, and is the controller's signal.
                active.cancel.store(true, Ordering::Release);
            }
        }
        self.0.shared.changed.notify_all();
        self.job(JobInput {
            job_id: input.job_id,
            after_revision: None,
            wait_ms: 0,
            include_result: false,
        })
    }

    /// Cheap EOF/error hook: stop admission and signal cancellation without joins.
    pub fn stop_admission_and_cancel(&self) {
        self.0.shared.stop();
    }

    /// Await controller/workspace cleanup. Invoke from a blocking adapter task.
    pub fn join_and_drop_jobs(&self) {
        self.stop_admission_and_cancel();
        let _launch = self.0.launch.lock().unwrap();
        let collector = self.0.collector.lock().unwrap().take();
        if let Some(handle) = collector {
            let _ = handle.join();
        }
        let records = {
            let mut state = self.0.shared.state.lock().unwrap();
            state.retained_bytes = 0;
            std::mem::take(&mut state.terminal)
        };
        drop(records);
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(handle) = self.collector.get_mut().unwrap().take() {
            let _ = handle.join();
        }
    }
}
impl Shared {
    fn stop(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            if let Some(active) = state.active.as_mut() {
                if active.record.state == JobState::Running {
                    active.record.state = JobState::Cancelling;
                    active.record.bump();
                }
                active.cancel.store(true, Ordering::Release);
            }
        }
        self.changed.notify_all();
    }
}
impl State {
    fn lookup(&self, id: &str) -> Result<&Record, SessionError> {
        if let Some(active) = &self.active
            && active.record.id == id
        {
            return Ok(&active.record);
        }
        if let Some(record) = self.terminal.iter().find(|record| record.id == id) {
            return Ok(record);
        }
        let known = id
            .strip_prefix(&self.prefix)
            .filter(|suffix| suffix.len() == 16)
            .and_then(|suffix| u64::from_str_radix(suffix, 16).ok())
            .is_some_and(|serial| serial > 0 && serial < self.next_id);
        if known {
            Err(SessionError::new(
                "mcp.expired_job",
                "This job was evicted. Run the operation again.",
            ))
        } else {
            Err(SessionError::new(
                "mcp.unknown_job",
                "Unknown job ID. IDs belong only to the session that created them.",
            ))
        }
    }
    fn retain(&mut self, record: Record) -> Vec<Record> {
        let mut evicted = Vec::new();
        while self.terminal.len() >= MAX_TERMINAL
            || self.retained_bytes.saturating_add(record.bytes) > MAX_RETAINED
        {
            let Some(oldest) = self.terminal.pop_front() else {
                break;
            };
            self.retained_bytes -= oldest.bytes;
            evicted.push(oldest);
        }
        self.retained_bytes += record.bytes;
        self.terminal.push_back(record);
        evicted
    }
}
impl Record {
    fn running(id: String) -> Self {
        Self {
            id,
            state: JobState::Running,
            revision: 1,
            progress: None,
            warnings: VecDeque::new(),
            diagnostics: VecDeque::new(),
            warning_bytes: 0,
            diagnostic_bytes: 0,
            progress_bytes: 0,
            result: None,
            error: None,
            bytes: 0,
        }
    }
    fn bump(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
    fn reply(
        &self,
        changed: bool,
        include_result: bool,
    ) -> (JobReply, Option<Arc<RetainedResult>>) {
        (
            JobReply {
                version: 1,
                job_id: self.id.clone(),
                state: self.state,
                revision: self.revision,
                changed,
                progress: changed.then(|| self.progress.clone()).flatten(),
                warnings: changed.then(|| {
                    self.warnings
                        .iter()
                        .map(|entry| entry.value.clone())
                        .collect()
                }),
                diagnostics: changed.then(|| {
                    self.diagnostics
                        .iter()
                        .map(|entry| entry.value.clone())
                        .collect()
                }),
                result: None,
                error: changed.then(|| self.error.clone()).flatten(),
            },
            (changed && include_result)
                .then(|| self.result.clone())
                .flatten(),
        )
    }
}
impl RetainedResult {
    fn to_reply(&self) -> Result<JobResult, SessionError> {
        match self {
            Self::Metadata(info) => Ok(JobResult::Metadata {
                info: serde_json::to_value(info).map_err(serialization_error)?,
            }),
            Self::Diagnosis(diagnosis) => Ok(JobResult::Diagnosis {
                diagnosis: serde_json::to_value(diagnosis).map_err(serialization_error)?,
            }),
            Self::Export(result) => Ok(JobResult::Export {
                output: result.output.to_string_lossy().into_owned(),
                duration: result.duration,
            }),
            Self::Subtitles(result) => Ok(JobResult::Subtitles {
                output: result.output.to_string_lossy().into_owned(),
                track: serde_json::to_value(&result.track).map_err(serialization_error)?,
            }),
        }
    }
}

type Terminal = Result<Option<RetainedResult>, SessionError>;
fn collect(shared: &Shared, id: &str, job: &Job) -> Terminal {
    loop {
        let event = match job.events.recv_timeout(Duration::from_secs(30)) {
            Ok(event) => event,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(SessionError::new(
                    "mcp.worker_closed",
                    "Worker stopped without a terminal result.",
                ));
            }
        };
        match event {
            Event::Done(result) => {
                return result
                    .map(|r| Some(RetainedResult::Export(r)))
                    .map_err(SessionError::from);
            }
            Event::Subtitled(result) => {
                return match result {
                    Ok(result) => {
                        let retained = RetainedResult::Subtitles(result);
                        serialized_len(&retained, MAX_SUBTITLE_RESULT).map_err(|_| {
                            SessionError::new(
                                "mcp.result_too_large",
                                "Subtitle track exceeds the 4 MiB adapter result limit.",
                            )
                        })?;
                        Ok(Some(retained))
                    }
                    Err(error) => Err(SessionError::from(error)),
                };
            }
            Event::Inspected(result) => {
                return result
                    .map(|r| {
                        Some(match r {
                            InspectionResult::Metadata { info, .. } => {
                                RetainedResult::Metadata(info)
                            }
                            InspectionResult::Plan(diagnosis) => {
                                RetainedResult::Diagnosis(diagnosis)
                            }
                        })
                    })
                    .map_err(SessionError::from);
            }
            Event::Cancelled => return Ok(None),
            Event::Plan(_) => continue, // Diagnosis contains the authoritative plan.
            event => {
                // Bound and measure event data before taking the status mutex.
                enum Update {
                    Progress(Progress, usize),
                    Warning(Stored<Phase>),
                    Diagnostic(Stored<String>),
                }
                let update = match event {
                    Event::Progress { percent, phase } => {
                        let progress = Progress {
                            percent: percent.min(100),
                            phase: bounded_phase(phase),
                        };
                        let bytes = serialized_len(&progress, MAX_RETAINED).unwrap();
                        Update::Progress(progress, bytes)
                    }
                    Event::Warning(message) => {
                        let value = bounded_phase(message);
                        let bytes = serialized_len(&value, MAX_RETAINED).unwrap() + 1;
                        Update::Warning(Stored { value, bytes })
                    }
                    Event::Diagnostic(mut value) => {
                        truncate(&mut value, 4096);
                        let bytes = serialized_len(&value, MAX_RETAINED).unwrap() + 1;
                        Update::Diagnostic(Stored { value, bytes })
                    }
                    _ => unreachable!(),
                };
                let mut state = shared.state.lock().unwrap();
                let Some(active) = state
                    .active
                    .as_mut()
                    .filter(|active| active.record.id == id)
                else {
                    continue;
                };
                let record = &mut active.record;
                match update {
                    Update::Progress(progress, bytes) => {
                        record.progress = Some(progress);
                        record.progress_bytes = bytes;
                    }
                    Update::Warning(warning) => {
                        while record.warnings.len() >= MAX_WARNINGS
                            || record.warning_bytes + warning.bytes > MAX_WARNING_BYTES
                        {
                            if let Some(oldest) = record.warnings.pop_front() {
                                record.warning_bytes -= oldest.bytes;
                            } else {
                                break;
                            }
                        }
                        record.warning_bytes += warning.bytes;
                        record.warnings.push_back(warning);
                    }
                    Update::Diagnostic(diagnostic) => {
                        if record.diagnostics.len() == MAX_DIAGNOSTICS {
                            record.diagnostic_bytes -=
                                record.diagnostics.pop_front().unwrap().bytes;
                        }
                        record.diagnostic_bytes += diagnostic.bytes;
                        record.diagnostics.push_back(diagnostic);
                    }
                }
                record.bump();
                drop(state);
                shared.changed.notify_all();
            }
        }
    }
}

fn finish(shared: &Shared, id: &str, terminal: Terminal) {
    // Serialize/count potentially large results before acquiring the status lock.
    let (terminal_state, result, error, result_bytes) = prepare_terminal(terminal, MAX_RETAINED);
    let error_bytes = error
        .as_ref()
        .map(|error| serialized_len(error, MAX_RETAINED).unwrap_or(16384))
        .unwrap_or(0);
    let mut discarded = None;
    let evicted = {
        let mut state = shared.state.lock().unwrap();
        if !state
            .active
            .as_ref()
            .is_some_and(|active| active.record.id == id)
        {
            return;
        }
        let mut record = state.active.take().unwrap().record;
        record.state = terminal_state;
        record.result = result;
        record.error = error;
        record.bump();
        // Header allowance plus exact serialized event/payload sizes. This is a
        // serialized-data retention budget, independent of transport wire limits.
        let base_bytes =
            1024 + record.progress_bytes + record.warning_bytes + record.diagnostic_bytes;
        record.bytes = base_bytes
            .saturating_add(result_bytes)
            .saturating_add(error_bytes);
        if record.bytes > MAX_RETAINED {
            discarded = record.result.take();
            record.state = JobState::Failed;
            record.error = Some(SessionError::new(
                "mcp.result_too_large",
                "Result and diagnostics exceed the session retention limit. Reduce the request.",
            ));
            record.bytes = base_bytes + 1024;
        }
        state.retain(record)
    };
    // Large evicted diagnoses are dropped outside the status mutex.
    drop(evicted);
    drop(discarded);
    shared.changed.notify_all();
}

fn prepare_terminal(
    terminal: Terminal,
    limit: usize,
) -> (
    JobState,
    Option<Arc<RetainedResult>>,
    Option<SessionError>,
    usize,
) {
    match terminal {
        Ok(Some(result)) => match serialized_len(&result, limit) {
            Ok(bytes) => (JobState::Succeeded, Some(Arc::new(result)), None, bytes),
            Err(_) => (
                JobState::Failed,
                None,
                Some(SessionError::new(
                    "mcp.result_too_large",
                    "Result exceeds the session retention limit. Reduce the request.",
                )),
                0,
            ),
        },
        Ok(None) => (JobState::Cancelled, None, None, 0),
        Err(error) => (JobState::Failed, None, Some(error), 0),
    }
}

fn bounded_phase(message: Message) -> Phase {
    let mut code = message.code;
    truncate(&mut code, 256);
    let mut budget = 3840;
    let args = message
        .args
        .into_iter()
        .take(16)
        .map(|mut arg| {
            truncate(&mut arg, budget.min(1024));
            budget -= arg.len();
            arg
        })
        .collect();
    Phase { code, args }
}
fn truncate(value: &mut String, max: usize) {
    if value.len() > max {
        let mut end = max;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.shrink_to_fit();
    }
}
fn serialization_error(error: serde_json::Error) -> SessionError {
    SessionError::new("mcp.serialization", &error.to_string())
}
fn check_request_size<T: Serialize>(input: &T) -> Result<(), SessionError> {
    serialized_len(input, MAX_REQUEST).map(|_| ()).map_err(|_| {
        SessionError::new(
            "mcp.request_too_large",
            "Request exceeds 4 MiB. Reduce the number or length of paths.",
        )
    })
}
fn serialized_len<T: Serialize>(value: &T, limit: usize) -> Result<usize, serde_json::Error> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes) {
                return Err(io::Error::other("Serialized data exceeds limit"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::with_prefix(PathBuf::from("missing-ffmpeg"), "test-session-".into())
    }
    fn reserve(session: &Session) -> String {
        let mut state = session.0.shared.state.lock().unwrap();
        let id = format!("{}{:016x}", state.prefix, state.next_id);
        state.next_id += 1;
        state.active = Some(Active {
            record: Record::running(id.clone()),
            cancel: Arc::new(AtomicBool::new(false)),
            job: None,
        });
        id
    }
    fn query(id: &str) -> JobInput {
        JobInput {
            job_id: id.into(),
            after_revision: None,
            wait_ms: 0,
            include_result: true,
        }
    }
    fn exported() -> RetainedResult {
        RetainedResult::Export(ExportResult {
            output: PathBuf::from("out.mp4"),
            duration: 2.0,
        })
    }

    fn assert_cancelled_before_launch(eof: bool) {
        // Preserve cancellation at admission for every media operation.
        for route in 0..6 {
            let session = session();
            let folder = tempfile::tempdir().unwrap();
            let output = folder.path().join("output.mp4");
            let request = ExportRequest {
                // Deliberately invalid: losing the pre-set signal deterministically
                // yields a validation failure rather than a cancellation result.
                items: Vec::new(),
                wav: folder.path().join("missing.wav"),
                output: output.clone(),
                ffmpeg: folder.path().join("missing-ffmpeg"),
                fade_in: 0.0,
                fade_out: 0.0,
                partial_fades: true,
                preview: false,
                clip_audio: false,
                force_encode: false,
            };
            let operation = match route {
                0 => Operation::Export(request, None),
                1 => {
                    let expected = Diagnosis {
                        version: 1,
                        level: Level::Exact,
                        request: request.clone(),
                        snapshot: crate::inspection::Snapshot(Vec::new()),
                        duration: 1.0,
                        container: "mp4".into(),
                        audio: "aac_320".into(),
                        plan: None,
                        quick_media: Vec::new(),
                        notes: Vec::new(),
                    };
                    Operation::Export(request, Some(Box::new(expected)))
                }
                2 => Operation::Inspect(InspectionRequest::Plan {
                    request,
                    level: Level::Exact,
                }),
                3 => Operation::Transcribe(SubtitleRequest {
                    source: folder.path().join("silent.wav"),
                    output: folder.path().join("captions.srt"),
                    ffmpeg: folder.path().join("missing-ffmpeg"),
                    transcriber: folder.path().join("missing-whisper"),
                    model: folder.path().join("missing-model"),
                    vad_model: folder.path().join("missing-vad"),
                    language: "auto".into(),
                }),
                4 => Operation::Captions(crate::captions::CaptionRequest {
                    source: folder.path().join("missing.mp4"),
                    subtitles: folder.path().join("missing.srt"),
                    output: output.clone(),
                    ffmpeg: folder.path().join("missing-ffmpeg"),
                    style: Default::default(),
                    preview: false,
                }),
                _ => Operation::Short(crate::shorts::ShortRequest {
                    source: folder.path().join("missing.mp4"),
                    output: output.clone(),
                    ffmpeg: folder.path().join("missing-ffmpeg"),
                    start_ms: 0,
                    end_ms: 1000,
                    framing: Default::default(),
                    captions: None,
                    preview: false,
                }),
            };
            // A prior collector's join pauses startup after reserving the new ID
            // but before creating its Job. No process fixture or timing sleep.
            let (release, paused) = std::sync::mpsc::sync_channel(1);
            *session.0.collector.lock().unwrap() = Some(thread::spawn(move || {
                // Failed assertions must not leave starter/Owner teardown hung.
                let _ = paused.recv_timeout(Duration::from_secs(5));
            }));
            let starting = session.clone();
            let starter = thread::spawn(move || starting.start(operation).unwrap());
            let (id, signal) = {
                let state = session.0.shared.state.lock().unwrap();
                let (state, _) = session
                    .0
                    .shared
                    .changed
                    .wait_timeout_while(state, Duration::from_secs(2), |state| {
                        state.active.is_none()
                    })
                    .unwrap();
                let active = state.active.as_ref().expect("startup reserved an ID");
                assert!(active.job.is_none());
                (active.record.id.clone(), active.cancel.clone())
            };
            if eof {
                session.stop_admission_and_cancel();
            } else {
                assert_eq!(
                    session
                        .cancel(CancelInput { job_id: id.clone() })
                        .unwrap()
                        .state,
                    JobState::Cancelling
                );
            }
            assert!(signal.load(Ordering::Acquire));
            release.send(()).unwrap();
            assert_eq!(starter.join().unwrap().job_id, id);
            // EOF intentionally makes status queries return immediately. Await
            // the collector outside the status mutex, retaining its final record.
            let collector = session.0.collector.lock().unwrap().take().unwrap();
            collector.join().unwrap();
            let reply = session
                .job(JobInput {
                    after_revision: Some(2),
                    wait_ms: 2000,
                    ..query(&id)
                })
                .unwrap();
            assert_eq!(
                reply.state,
                JobState::Cancelled,
                "launch route {route}, EOF={eof}"
            );
            assert!(reply.result.is_none());
            assert!(reply.error.is_none());
            assert!(!output.exists());
            assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn reserved_cancellation_reaches_all_controllers_before_launch() {
        assert_cancelled_before_launch(false);
    }

    #[test]
    fn reserved_eof_reaches_all_controllers_before_launch() {
        assert_cancelled_before_launch(true);
    }

    #[test]
    fn cancellation_is_pending_until_controller_terminal_and_late_success_wins() {
        let session = session();
        let id = reserve(&session);
        let reply = session.cancel(CancelInput { job_id: id.clone() }).unwrap();
        assert_eq!(reply.state, JobState::Cancelling);
        assert_eq!(reply.revision, 2);
        assert!(reply.result.is_none());
        finish(&session.0.shared, &id, Ok(Some(exported())));
        let reply = session.job(query(&id)).unwrap();
        assert_eq!(reply.state, JobState::Succeeded);
        assert!(matches!(
            reply.result,
            Some(JobResult::Export { duration: 2.0, .. })
        ));
        assert_eq!(
            session
                .cancel(CancelInput { job_id: id.clone() })
                .unwrap()
                .state,
            JobState::Succeeded
        );
        assert!(session.job(query(&id)).unwrap().result.is_some());

        let cancelled = reserve(&session);
        finish(&session.0.shared, &cancelled, Ok(None));
        assert_eq!(
            session.job(query(&cancelled)).unwrap().state,
            JobState::Cancelled
        );
    }

    #[test]
    fn revision_wait_wakes_on_completion_and_unchanged_payload_is_omitted() {
        let session = session();
        let id = reserve(&session);
        let waiting = session.clone();
        let waiting_id = id.clone();
        let thread = thread::spawn(move || {
            waiting
                .job(JobInput {
                    job_id: waiting_id,
                    after_revision: Some(1),
                    wait_ms: 1000,
                    include_result: true,
                })
                .unwrap()
        });
        finish(&session.0.shared, &id, Ok(Some(exported())));
        let reply = thread.join().unwrap();
        assert!(reply.changed);
        assert_eq!(reply.state, JobState::Succeeded);
        let unchanged = session
            .job(JobInput {
                after_revision: Some(reply.revision),
                ..query(&id)
            })
            .unwrap();
        assert!(!unchanged.changed);
        assert!(unchanged.result.is_none());
        assert!(unchanged.warnings.is_none());
        assert!(
            session
                .job(JobInput {
                    after_revision: Some(reply.revision + 1),
                    ..query(&id)
                })
                .is_err()
        );
        assert!(
            session
                .job(JobInput {
                    wait_ms: 30001,
                    ..query(&id)
                })
                .is_err()
        );
    }

    #[test]
    fn terminal_count_eviction_distinguishes_expired_and_foreign_ids() {
        let session = session();
        let first = reserve(&session);
        finish(&session.0.shared, &first, Ok(Some(exported())));
        let mut last = first.clone();
        for _ in 0..MAX_TERMINAL {
            last = reserve(&session);
            finish(&session.0.shared, &last, Ok(Some(exported())));
        }
        assert_eq!(
            session.0.shared.state.lock().unwrap().terminal.len(),
            MAX_TERMINAL
        );
        assert_eq!(
            session.job(query(&first)).unwrap_err().code,
            "mcp.expired_job"
        );
        assert_eq!(
            session
                .job(query("other-session-0000000000000001"))
                .unwrap_err()
                .code,
            "mcp.unknown_job"
        );
        assert_eq!(
            session
                .job(query("test-session-000000000000ffff"))
                .unwrap_err()
                .code,
            "mcp.unknown_job"
        );
        assert!(session.job(query(&last)).unwrap().result.is_some());
    }

    #[test]
    fn invalid_transcription_is_rejected_before_session_admission() {
        let session = session();
        let error = session
            .transcribe(TranscribeInput {
                source: "relative.wav".into(),
                output: "relative.srt".into(),
                transcriber: "whisper-cli".into(),
                model: "model.bin".into(),
                vad_model: "vad.bin".into(),
                language: "auto".into(),
                ffmpeg: None,
            })
            .unwrap_err();
        assert_eq!(error.code, "mcp.invalid_request");
        let state = session.0.shared.state.lock().unwrap();
        assert!(state.active.is_none());
        assert!(state.terminal.is_empty());
    }

    #[test]
    fn retained_byte_budget_evicts_oldest_without_unbounded_tombstones() {
        let session = session();
        let mut state = session.0.shared.state.lock().unwrap();
        for serial in 1..=3 {
            let mut record = Record::running(format!("test-session-{serial:016x}"));
            record.state = JobState::Succeeded;
            record.bytes = MAX_RETAINED / 2;
            state.retain(record);
        }
        assert_eq!(state.terminal.len(), 2);
        assert_eq!(state.retained_bytes, MAX_RETAINED);
        assert!(state.terminal.front().unwrap().id.ends_with('2'));
    }

    #[test]
    fn oversized_terminal_is_failed_and_never_retains_an_approval() {
        let (state, result, error, bytes) = prepare_terminal(Ok(Some(exported())), 8);
        assert_eq!(state, JobState::Failed);
        assert!(result.is_none());
        assert_eq!(bytes, 0);
        assert_eq!(error.unwrap().code, "mcp.result_too_large");
        assert_eq!(serialized_len(&"\0", 8).unwrap(), 8);
        assert!(serialized_len(&"\0", 7).is_err());
    }

    #[test]
    fn serialized_subtitle_result_is_bounded_before_retention() {
        let result = RetainedResult::Subtitles(SubtitleResult {
            output: PathBuf::from("x".repeat(MAX_SUBTITLE_RESULT)),
            track: crate::subtitle_track::SubtitleTrack {
                language: None,
                duration_ms: 1,
                cues: Vec::new(),
            },
        });
        assert!(serialized_len(&result, MAX_SUBTITLE_RESULT).is_err());
    }

    #[test]
    fn shutdown_stops_admission_and_wakes_waiters_without_claiming_cancellation() {
        let session = session();
        let id = reserve(&session);
        session.stop_admission_and_cancel();
        let reply = session.job(query(&id)).unwrap();
        assert_eq!(reply.state, JobState::Cancelling);
        assert_eq!(
            session
                .start(Operation::Export(
                    ExportRequest {
                        items: Vec::new(),
                        wav: PathBuf::new(),
                        output: PathBuf::new(),
                        ffmpeg: PathBuf::new(),
                        fade_in: 0.0,
                        fade_out: 0.0,
                        partial_fades: true,
                        preview: false,
                        clip_audio: false,
                        force_encode: false,
                    },
                    None
                ))
                .unwrap_err()
                .code,
            "mcp.closed"
        );
        finish(&session.0.shared, &id, Ok(Some(exported())));
        assert_eq!(session.job(query(&id)).unwrap().state, JobState::Succeeded);
        session.join_and_drop_jobs();
        assert_eq!(session.job(query(&id)).unwrap_err().code, "mcp.expired_job");
    }

    #[test]
    fn session_nonce_is_random_and_missing_engine_does_not_block_negotiation() {
        let a = Session::new(PathBuf::from("missing-ffmpeg")).unwrap();
        let b = Session::new(PathBuf::from("missing-ffmpeg")).unwrap();
        assert_ne!(
            a.0.shared.state.lock().unwrap().prefix,
            b.0.shared.state.lock().unwrap().prefix
        );
    }

    #[test]
    fn oversized_requests_and_event_text_are_bounded() {
        let oversized = "x".repeat(MAX_REQUEST);
        assert_eq!(
            check_request_size(&oversized).unwrap_err().code,
            "mcp.request_too_large"
        );
        let phase = bounded_phase(Message {
            code: "é".repeat(200),
            args: vec!["é".repeat(2048); 40],
        });
        assert!(phase.code.len() <= 256);
        assert!(phase.args.len() <= 16);
        assert!(phase.args.iter().map(String::len).sum::<usize>() <= 3840);
    }
}
