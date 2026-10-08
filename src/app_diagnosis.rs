//! One active inspection and one latest request; no filesystem work on the UI thread.
use noh::{
    engine::{EngineError, Event, ExportRequest},
    inspection::{Diagnosis, InspectionRequest, InspectionResult, Level},
    jobs::Job,
};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    generation: u64,
    request: Option<ExportRequest>,
    pending: bool,
    changed: Option<Instant>,
    cancel: Option<Arc<AtomicBool>>,
    result: Option<(u64, Result<Box<Diagnosis>, EngineError>)>,
    progress: Option<(u64, u32, noh::i18n::Message)>,
    closed: bool,
}
type Shared = Arc<(Mutex<State>, Condvar)>;
pub struct Preflight {
    shared: Shared,
    thread: Option<JoinHandle<()>>,
    ctx: eframe::egui::Context,
}
impl Preflight {
    #[cfg(test)]
    pub fn dormant(request: Option<ExportRequest>) -> Self {
        let shared: Shared = Arc::default();
        shared.0.lock().unwrap().request = request;
        Self {
            shared,
            thread: None,
            ctx: Default::default(),
        }
    }
    /// Checks run in `worker` (this executable, or the app under test).
    pub fn new(ctx: eframe::egui::Context, worker: std::path::PathBuf) -> Self {
        let shared: Shared = Arc::default();
        let state = shared.clone();
        let repaint = ctx.clone();
        let thread = std::thread::spawn(move || {
            let mut verified: Option<(u64, Box<Diagnosis>)> = None;
            loop {
                let mut s = state.0.lock().unwrap();
                if s.closed {
                    break;
                }
                if !s.pending {
                    let (next, timeout) = state.1.wait_timeout(s, Duration::from_secs(1)).unwrap();
                    s = next;
                    if s.closed {
                        break;
                    }
                    if timeout.timed_out()
                        && let Some((id, diagnosis)) = &verified
                    {
                        if *id == s.generation {
                            drop(s);
                            let validity = diagnosis.validate(&diagnosis.request);
                            s = state.0.lock().unwrap();
                            if *id == s.generation
                                && let Err(error) = validity
                            {
                                s.result = Some((*id, Err(error)));
                                verified = None;
                                drop(s);
                                repaint.request_repaint();
                                continue;
                            }
                        } else {
                            verified = None;
                        }
                    }
                    if !s.pending {
                        continue;
                    }
                }
                if let Some(changed) = s.changed {
                    let delay = Duration::from_millis(250).saturating_sub(changed.elapsed());
                    if !delay.is_zero() {
                        let _ = state.1.wait_timeout(s, delay).unwrap();
                        continue;
                    }
                }
                s.pending = false;
                let Some(request) = s.request.clone() else {
                    continue;
                };
                let id = s.generation;
                let cancel = Arc::new(AtomicBool::new(false));
                s.cancel = Some(cancel.clone());
                drop(s);
                let job = Job::inspect_with_worker(
                    InspectionRequest::Plan {
                        request,
                        level: Level::Exact,
                    },
                    worker.clone(),
                    cancel,
                    || {},
                );
                let result = loop {
                    match job.events.recv_timeout(Duration::from_secs(1)) {
                        Ok(Event::Progress { percent, phase }) => {
                            let mut s = state.0.lock().unwrap();
                            if id == s.generation {
                                s.progress = Some((id, percent, phase.to_wire().into()));
                                drop(s);
                                repaint.request_repaint();
                            }
                        }
                        Ok(Event::Inspected(Ok(InspectionResult::Plan(diagnosis)))) => {
                            break Some(Ok(diagnosis));
                        }
                        Ok(Event::Inspected(Err(error))) => break Some(Err(error)),
                        Ok(Event::Cancelled) => break None,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            break Some(Err(EngineError::new(
                                "error.engine",
                                "inspect",
                                None,
                                "Inspection disconnected",
                            )));
                        }
                        _ => {}
                    }
                };
                drop(job); // Reap before accepting the next generation, off the UI thread.
                let mut s = state.0.lock().unwrap();
                s.cancel = None;
                if id == s.generation
                    && let Some(result) = result
                {
                    verified = result.as_ref().ok().map(|d| (id, d.clone()));
                    s.result = Some((id, result));
                    drop(s);
                    repaint.request_repaint();
                }
            }
        });
        Self {
            shared,
            thread: Some(thread),
            ctx,
        }
    }
    pub fn update(&self, request: Option<ExportRequest>, force: bool) -> bool {
        let mut s = self.shared.0.lock().unwrap();
        let same = match (&s.request, &request) {
            (Some(before), Some(after)) => before.same_render_settings(after),
            (None, None) => true,
            _ => false,
        };
        if !force && same {
            return false;
        }
        s.generation += 1;
        s.request = request;
        s.pending = s.request.is_some();
        s.changed = Some(Instant::now());
        s.result = None;
        s.progress = None;
        if let Some(cancel) = &s.cancel {
            cancel.store(true, Ordering::Release);
        }
        drop(s);
        self.shared.1.notify_one();
        true
    }
    pub fn cancel(&self) {
        let mut s = self.shared.0.lock().unwrap();
        s.generation += 1;
        s.pending = false;
        s.result = None;
        s.progress = None;
        if let Some(cancel) = &s.cancel {
            cancel.store(true, Ordering::Release);
        }
        drop(s);
        self.ctx.request_repaint();
    }
    pub fn receive(&self) -> Option<Result<Box<Diagnosis>, EngineError>> {
        let mut s = self.shared.0.lock().unwrap();
        s.result
            .take()
            .and_then(|(id, result)| (id == s.generation).then_some(result))
    }
    pub fn progress(&self) -> Option<(f32, noh::i18n::Message)> {
        let s = self.shared.0.lock().unwrap();
        s.progress.as_ref().and_then(|(id, percent, phase)| {
            (*id == s.generation).then(|| (*percent as f32 / 100.0, phase.clone()))
        })
    }
}
impl Drop for Preflight {
    fn drop(&mut self) {
        self.cancel();
        self.shared.0.lock().unwrap().closed = true;
        self.shared.1.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_is_generation_bound_and_cleared_on_cancel() {
        let preflight = Preflight::dormant(None);
        preflight.shared.0.lock().unwrap().progress =
            Some((0, 46, "progress.analyze_clip|2|3".into()));
        let (fraction, phase) = preflight.progress().unwrap();
        assert_eq!(fraction, 0.46);
        assert_eq!(phase.args, ["2", "3"]);
        preflight.cancel();
        assert!(preflight.progress().is_none());
        preflight.shared.0.lock().unwrap().progress =
            Some((0, 90, "progress.analyze_clip|3|3".into()));
        assert!(
            preflight.progress().is_none(),
            "Obsolete progress cannot reappear"
        );
    }
    #[test]
    fn rapid_changes_keep_only_latest_request_and_reject_late_results() {
        let preflight = Preflight {
            shared: Arc::default(),
            thread: None,
            ctx: Default::default(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        preflight.shared.0.lock().unwrap().cancel = Some(cancel.clone());
        let mut request = ExportRequest {
            items: vec!["clip.mp4".into()],
            wav: "audio.wav".into(),
            output: "out.mp4".into(),
            ffmpeg: "ffmpeg".into(),
            fade_in: 0.0,
            fade_out: 0.0,
            partial_fades: true,
            preview: false,
            clip_audio: false,
            force_encode: false,
        };
        for index in 0..10_000 {
            request.fade_in = index as f64;
            assert!(preflight.update(Some(request.clone()), false));
        }
        assert!(cancel.load(Ordering::Acquire));
        assert!(!preflight.update(Some(request.clone()), false));
        let mut state = preflight.shared.0.lock().unwrap();
        assert_eq!(state.generation, 10_000);
        assert_eq!(state.request, Some(request.clone()));
        assert!(state.pending);
        state.result = Some((1, Err(EngineError::new("old", "inspect", None, "obsolete"))));
        drop(state);
        assert!(preflight.receive().is_none());
        preflight.cancel();
        assert!(!preflight.shared.0.lock().unwrap().pending);
        assert!(
            !preflight.update(Some(request.clone()), false),
            "Refresh cannot restart a cancelled analysis"
        );
        assert!(preflight.update(Some(request), true));
        assert!(preflight.shared.0.lock().unwrap().pending);
    }
}
