//! Independent selected-interval vertical workspace. Source identities and choosers stay off-frame.
use super::*;
use app_state::{Availability, ContextAction, Operation, StatusView};
use app_style::Severity;
use noh::{
    captions::{CaptionPlacement, CaptionSize, CaptionStyle},
    inspection::Snapshot,
    shorts::{Framing, ShortCaptions, ShortRequest},
};
use std::sync::mpsc::{self, Receiver};

pub struct PreviewArtifact {
    pub path: PathBuf,
    pub revision: u64,
    pub snapshot: Snapshot,
}
struct Prepared {
    request: ShortRequest,
    snapshot: Snapshot,
    owner: Option<PreviewFile>,
}
enum WorkResult {
    Prepared(u64, Result<Prepared, noh::engine::EngineError>),
    Verified(u64, PathBuf, Result<(), noh::engine::EngineError>),
}
pub struct Workspace {
    pub open: bool,
    pub source: String,
    pub subtitles: String,
    pub captions_enabled: bool,
    pub start_ms: u64,
    pub end_ms: u64,
    pub framing: Framing,
    pub output: String,
    pub size: CaptionSize,
    pub placement: CaptionPlacement,
    pub safe_area: noh::safe_area::SafeArea,
    pub revision: u64,
    pub operation_revision: u64,
    pub running: Option<bool>,
    pub preview_artifact: Option<PreviewArtifact>,
    pub export_artifact: Option<app_state::Artifact>,
    pub failure: Option<Message>,
    pub failure_revision: Option<u64>,
    pub failure_detail: Option<String>,
    pub outcome: Option<Message>,
    pub warnings: Vec<Message>,
    preview_file: Option<PreviewFile>,
    operation_preview: Option<PreviewFile>,
    operation_snapshot: Option<Snapshot>,
    work: Option<Receiver<WorkResult>>,
    prepare_cancelled: bool,
    destination: Option<app_destination::Destination>,
    pub destination_result: Option<app_destination::ResultView>,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            open: false,
            source: String::new(),
            subtitles: String::new(),
            captions_enabled: false,
            start_ms: 0,
            end_ms: 30_000,
            framing: Framing::Pad,
            output: std::env::current_dir()
                .unwrap_or_default()
                .join("short.mp4")
                .display()
                .to_string(),
            size: CaptionSize::Medium,
            placement: CaptionPlacement::Bottom,
            safe_area: noh::safe_area::SafeArea::None,
            revision: 0,
            operation_revision: 0,
            running: None,
            preview_artifact: None,
            export_artifact: None,
            failure: None,
            failure_revision: None,
            failure_detail: None,
            outcome: None,
            warnings: Vec::new(),
            preview_file: None,
            operation_preview: None,
            operation_snapshot: None,
            work: None,
            prepare_cancelled: false,
            destination: None,
            destination_result: None,
        }
    }
}
impl Workspace {
    pub fn edited(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.clear_failure();
        self.outcome = None;
        self.warnings.clear();
    }
    fn clear_failure(&mut self) {
        self.failure = None;
        self.failure_revision = None;
        self.failure_detail = None;
    }
    fn destination_edited(&mut self) {
        self.destination_result = None;
        self.clear_failure();
        self.outcome = None;
    }
    pub fn preview_current(&self) -> bool {
        self.preview_artifact
            .as_ref()
            .is_some_and(|a| a.revision == self.revision)
    }
    pub fn request(&self, ffmpeg: Option<&PathBuf>, preview: bool) -> ShortRequest {
        ShortRequest {
            source: self.source.clone().into(),
            output: if preview {
                "preview.mp4".into()
            } else {
                self.output.clone().into()
            },
            ffmpeg: ffmpeg.cloned().unwrap_or_default(),
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            framing: self.framing,
            captions: self.captions_enabled.then(|| ShortCaptions {
                subtitles: self.subtitles.clone().into(),
                style: CaptionStyle {
                    size: self.size,
                    placement: self.placement,
                    safe_area: self.safe_area,
                },
            }),
            preview,
        }
    }
    fn issue(&self, preview: bool) -> Option<Message> {
        self.request(None, preview)
            .validate()
            .err()
            .map(|e| e.message_code().into())
    }
    pub fn fail(&mut self, error: &noh::engine::EngineError, revision: u64) {
        self.failure = Some(error.message_code().into());
        self.failure_detail = Some(error.detail.to_string());
        self.failure_revision = Some(revision);
    }
    pub fn status(
        &self,
        operation: Operation,
        phase: &Message,
        notice: Option<&Message>,
    ) -> StatusView {
        let locked = operation != Operation::Idle || self.work.is_some();
        let preview_reason = if locked {
            Some("ui.locked".into())
        } else {
            self.issue(true)
        };
        let export_reason = if locked {
            Some("ui.locked".into())
        } else {
            self.issue(false)
                .or_else(|| {
                    self.destination_result
                        .as_ref()
                        .and_then(|r| r.issue.as_ref())
                        .map(|issue| issue.key().into())
                })
                .or_else(|| {
                    self.destination_result
                        .is_none()
                        .then(|| "ui.destination_checking".into())
                })
        };
        let failed = self.failure_revision == Some(self.revision) && self.failure.is_some();
        let exported = self.export_artifact.as_ref();
        let mut actions = Vec::new();
        let (severity, headline, detail) = match operation {
            Operation::Shortening { preview } => (
                Severity::Info,
                if preview {
                    "ui.short_previewing"
                } else {
                    "ui.short_exporting"
                }
                .into(),
                phase.clone(),
            ),
            Operation::CancellingShorts { .. } => (
                Severity::Warning,
                "ui.cancel_requested".into(),
                "ui.cancel_wait".into(),
            ),
            _ if failed => (
                Severity::Error,
                "ui.failed".into(),
                self.failure.clone().unwrap(),
            ),
            _ if exported.is_some() => (
                if exported.unwrap().revision == self.revision {
                    Severity::Success
                } else {
                    Severity::Info
                },
                if exported.unwrap().revision == self.revision {
                    "ui.short_exported"
                } else {
                    "ui.short_last_export"
                }
                .into(),
                Message::new(
                    "ui.caption_result",
                    &[exported.unwrap().path.display().to_string()],
                ),
            ),
            _ if self.preview_current() => (
                Severity::Success,
                "ui.short_preview_ready".into(),
                "ui.short_review".into(),
            ),
            _ => (
                Severity::Info,
                "ui.shorts".into(),
                preview_reason
                    .clone()
                    .unwrap_or_else(|| "ui.short_ready".into()),
            ),
        };
        if !locked && exported.is_some() {
            actions.extend([ContextAction::OpenShort, ContextAction::OpenShortFolder]);
        }
        if !locked && self.preview_current() {
            actions.push(ContextAction::PlayShort);
        }
        if !locked
            && self
                .destination_result
                .as_ref()
                .is_some_and(|r| r.suggestion.is_some())
        {
            actions.push(ContextAction::SuggestShort);
        }
        StatusView {
            destination: false,
            severity,
            headline,
            detail,
            third: notice.cloned().or_else(|| {
                if failed {
                    Some("ui.short_error_help".into())
                } else {
                    self.outcome.clone().or_else(|| {
                        (self.preview_artifact.is_some() && !self.preview_current())
                            .then(|| "ui.short_stale".into())
                    })
                }
            }),
            export: Availability {
                enabled: export_reason.is_none(),
                reason: export_reason.unwrap_or_else(|| "ui.available".into()),
            },
            preview: Availability {
                enabled: preview_reason.is_none(),
                reason: preview_reason.unwrap_or_else(|| "ui.available".into()),
            },
            actions,
            operation,
            montage: None,
        }
    }
    fn prepare(&mut self, request: ShortRequest, ctx: &egui::Context) {
        let revision = self.revision;
        let (tx, rx) = mpsc::sync_channel(1);
        let ctx = ctx.clone();
        self.running = Some(request.preview);
        self.operation_revision = revision;
        self.clear_failure();
        self.outcome = None;
        self.prepare_cancelled = false;
        let spawned = std::thread::Builder::new()
            .name("short-prepare".into())
            .spawn(move || {
                let result = (|| {
                    let snapshot = request.snapshot()?;
                    let mut request = request;
                    let owner = if request.preview {
                        let owner = PreviewFile::new().map_err(|e| {
                            noh::engine::EngineError::new(
                                "error.short_config",
                                "preview_workspace",
                                None,
                                e,
                            )
                        })?;
                        request.output = owner.path.clone();
                        Some(owner)
                    } else {
                        None
                    };
                    Ok(Prepared {
                        request,
                        snapshot,
                        owner,
                    })
                })();
                let _ = tx.send(WorkResult::Prepared(revision, result));
                ctx.request_repaint();
            });
        if let Err(error) = spawned {
            self.fail(
                &noh::engine::EngineError::new("error.short_config", "prepare", None, error),
                revision,
            );
            self.running = None;
        } else {
            self.work = Some(rx);
        }
    }
    pub fn cancel_preparation(&mut self) {
        // Retain admission occupancy until metadata IO returns. Repeated cancel
        // clicks must not accumulate detached filesystem workers.
        self.prepare_cancelled = true;
    }
    pub fn verify_preview(&mut self, ctx: &egui::Context) {
        if self.work.is_some() || !self.preview_current() {
            return;
        }
        let artifact = self.preview_artifact.as_ref().unwrap();
        let (snapshot, revision, path) = (
            artifact.snapshot.clone(),
            self.revision,
            artifact.path.clone(),
        );
        let (tx, rx) = mpsc::sync_channel(1);
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("short-verify".into())
            .spawn(move || {
                let result = snapshot.verify();
                let _ = tx.send(WorkResult::Verified(revision, path, result));
                ctx.request_repaint();
            }) {
            Ok(_) => self.work = Some(rx),
            Err(error) => self.fail(
                &noh::engine::EngineError::new("error.short_config", "verify_preview", None, error),
                self.revision,
            ),
        }
    }
    pub fn completed(
        &mut self,
        result: Result<noh::engine::ExportResult, noh::engine::EngineError>,
    ) {
        match result {
            Ok(result) if self.running == Some(true) => {
                if let Some(snapshot) = self.operation_snapshot.take() {
                    self.preview_artifact = Some(PreviewArtifact {
                        path: result.output,
                        revision: self.operation_revision,
                        snapshot,
                    });
                    self.preview_file = self.operation_preview.take();
                }
                self.clear_failure();
                self.outcome = None;
            }
            Ok(result) => {
                self.export_artifact = Some(app_state::Artifact {
                    path: result.output.clone(),
                    revision: self.operation_revision,
                    bytes: None,
                });
                self.destination_result = Some(app_destination::ResultView {
                    path: result.output,
                    issue: Some(app_destination::Issue::Exists),
                    suggestion: None,
                    bytes: None,
                });
                if let Some(destination) = &self.destination {
                    destination.update(Some(self.output.clone().into()), true);
                }
                self.clear_failure();
                self.outcome = None;
            }
            Err(error) => self.fail(&error, self.operation_revision),
        }
    }
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        fixture: bool,
    ) -> (Option<ShortRequest>, Option<PathBuf>) {
        if !fixture && (self.open || self.running.is_some()) {
            let destination = self
                .destination
                .get_or_insert_with(|| app_destination::Destination::new(ctx.clone()));
            if destination.update(Some(self.output.clone().into()), false) {
                self.destination_result = None;
            }
            if let Some(result) = destination.receive() {
                self.destination_result = Some(result);
            }
        }
        let mut start = None;
        let mut open = None;
        if let Some(result) = self.work.as_ref().map(|rx| rx.try_recv()) {
            match result {
                Ok(WorkResult::Prepared(revision, result)) => {
                    self.work = None;
                    if self.prepare_cancelled {
                        self.prepare_cancelled = false;
                        self.running = None;
                        self.outcome = Some("ui.short_cancelled".into());
                    } else if revision == self.revision && self.running.is_some() {
                        match result {
                            Ok(prepared) => {
                                self.operation_snapshot = Some(prepared.snapshot);
                                self.operation_preview = prepared.owner;
                                start = Some(prepared.request);
                            }
                            Err(error) => {
                                self.fail(&error, revision);
                                self.running = None;
                            }
                        }
                    }
                }
                Ok(WorkResult::Verified(revision, path, result)) => {
                    self.work = None;
                    if revision == self.revision
                        && self
                            .preview_artifact
                            .as_ref()
                            .is_some_and(|a| a.path == path)
                    {
                        match result {
                            Ok(()) => open = Some(path),
                            Err(error) => {
                                self.edited();
                                self.fail(&error, self.revision);
                                self.failure = Some("ui.short_inputs_changed".into());
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.work = None;
                    self.running = None;
                    self.fail(
                        &noh::engine::EngineError::new(
                            "error.short_backend",
                            "prepare",
                            None,
                            "Short setup worker disconnected",
                        ),
                        self.revision,
                    );
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        (start, open)
    }
    pub fn reaped(&mut self) {
        self.running = None;
        self.operation_preview = None;
        self.operation_snapshot = None;
    }
    pub fn suggest(&mut self) {
        if let Some(name) = self
            .destination_result
            .as_ref()
            .and_then(|r| r.suggestion.clone())
        {
            self.output = Path::new(&self.output)
                .with_file_name(name)
                .display()
                .to_string();
            self.destination_edited();
        }
    }
}
impl NohApp {
    pub(super) fn start_short(&mut self, ctx: &egui::Context, preview: bool) {
        if self.job.is_some()
            || self.captions.running.is_some()
            || self.cancelling
            || self.subtitles.running
            || self.shorts.running.is_some()
            || self.shorts.work.is_some()
        {
            return;
        }
        let request = self.shorts.request(self.ffmpeg.as_ref(), preview);
        if let Err(error) = request.validate() {
            self.shorts.fail(&error, self.shorts.revision);
            return;
        }
        if let Some(preflight) = &self.preflight {
            preflight.cancel();
        }
        self.inspecting = false;
        self.notice = None;
        self.started = Some(Instant::now());
        self.fraction = 0.0;
        self.phase = "short.preparing".into();
        self.log.clear();
        self.shorts.warnings.clear();
        // No preview plays or decodes while an export runs (as start_project).
        self.mini_preview.invalidate();
        self.shorts.prepare(request, ctx);
    }
    pub(super) fn receive_shorts(&mut self, ctx: &egui::Context) {
        let cancelling_preparation = self.shorts.running.is_some() && self.cancelling;
        let (start, open) = self.shorts.poll(ctx, self.capture_state.is_some());
        if let Some(request) = start {
            let ctx = ctx.clone();
            self.job = Some(Job::short(request, move || ctx.request_repaint()));
        }
        if let Some(path) = open {
            self.open(&path);
        }
        if cancelling_preparation && self.shorts.running.is_none() && self.job.is_none() {
            self.cancelling = false;
            self.started = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn workspace() -> Workspace {
        Workspace {
            source: "source.mp4".into(),
            start_ms: 135,
            end_ms: 1775,
            preview_artifact: Some(PreviewArtifact {
                path: "preview.mp4".into(),
                revision: 0,
                snapshot: Snapshot(vec![]),
            }),
            export_artifact: Some(app_state::Artifact {
                path: "short.mp4".into(),
                revision: 0,
                bytes: None,
            }),
            ..Default::default()
        }
    }
    #[test]
    fn request_keeps_integer_source_times_and_optional_captions_explicit() {
        let mut workspace = workspace();
        let request = workspace.request(None, false);
        assert_eq!((request.start_ms, request.end_ms), (135, 1775));
        assert_eq!(request.framing, Framing::Pad);
        assert!(request.captions.is_none());
        assert!(request.validate().is_ok());
        workspace.captions_enabled = true;
        assert_eq!(workspace.issue(true).unwrap().key, "error.caption_track");
        workspace.subtitles = "reviewed.srt".into();
        workspace.size = CaptionSize::Large;
        workspace.placement = CaptionPlacement::Top;
        workspace.framing = Framing::Crop;
        let request = workspace.request(None, true);
        let captions = request.captions.as_ref().unwrap();
        assert_eq!(captions.subtitles, PathBuf::from("reviewed.srt"));
        assert_eq!(captions.style.size, CaptionSize::Large);
        assert_eq!(captions.style.placement, CaptionPlacement::Top);
        assert_eq!(request.framing, Framing::Crop);
        assert!(request.preview);
        assert!(request.validate().is_ok());
        workspace.end_ms = workspace.start_ms;
        let issue = workspace.issue(true).unwrap();
        assert_eq!(issue.key, "error.short_range");
        for language in Language::ALL {
            assert_ne!(issue.render(language), "[missing translation]");
        }
    }
    #[test]
    fn destination_preserves_preview_but_interval_and_framing_edits_mark_history() {
        let mut workspace = workspace();
        workspace.output = "other folder/other.mp4".into();
        workspace.destination_edited();
        assert_eq!(workspace.revision, 0);
        assert!(workspace.preview_current());
        workspace.start_ms = 136;
        workspace.edited();
        workspace.framing = Framing::Crop;
        workspace.edited();
        assert_eq!(workspace.revision, 2);
        let view = workspace.status(Operation::Idle, &"short.preparing".into(), None);
        assert!(!workspace.preview_current());
        assert!(!view.actions.contains(&ContextAction::PlayShort));
        assert!(view.actions.contains(&ContextAction::OpenShort));
        assert!(view.actions.contains(&ContextAction::OpenShortFolder));
        assert_eq!(view.headline.key, "ui.short_last_export");
        for language in Language::ALL {
            assert_eq!(view.detail.render(language), "short.mp4");
        }
    }
    #[test]
    fn committed_short_after_cancel_routes_success_and_leaves_other_workspaces_intact() {
        let mut app = NohApp {
            shorts: workspace(),
            cancelling: true,
            preview_ready: true,
            export_artifact: Some(app_state::Artifact {
                path: "montage.mp4".into(),
                revision: 0,
                bytes: None,
            }),
            ..Default::default()
        };
        app.captions.open = true;
        app.captions.export_artifact = Some(app_state::Artifact {
            path: "captions.mp4".into(),
            revision: 4,
            bytes: None,
        });
        app.subtitles.artifact = Some(app_subtitles::Artifact {
            path: "speech.srt".into(),
            revision: 0,
            cues: 2,
        });
        app.shorts.running = Some(false);
        app.shorts.revision = 8;
        app.shorts.operation_revision = 8;
        assert_eq!(
            app.status().operation,
            Operation::CancellingShorts { preview: false }
        );
        assert!(
            app.receive_job_event(Event::Done(Ok(noh::engine::ExportResult {
                output: "new-short.mp4".into(),
                duration: 1.64
            })))
        );
        assert_eq!(app.shorts.export_artifact.as_ref().unwrap().revision, 8);
        assert_eq!(
            app.shorts.destination_result.as_ref().unwrap().issue,
            Some(app_destination::Issue::Exists)
        );
        assert!(app.shorts.outcome.is_none());
        assert_eq!(
            app.export_artifact.as_ref().unwrap().path,
            PathBuf::from("montage.mp4")
        );
        assert_eq!(app.captions.export_artifact.as_ref().unwrap().revision, 4);
        assert_eq!(app.subtitles.artifact.as_ref().unwrap().cues, 2);
        assert!(app.preview_ready);
        app.shorts.reaped();
        app.cancelling = false;
        app.invalidated();
        assert_eq!(
            app.shorts
                .status(Operation::Idle, &app.phase, None)
                .headline
                .key,
            "ui.short_exported"
        );
        app.shorts.edited();
        assert_eq!(
            app.shorts
                .status(Operation::Idle, &app.phase, None)
                .headline
                .key,
            "ui.short_last_export"
        );
    }
    #[test]
    fn pending_preparation_blocks_all_jobs_and_cancel_waits_for_occupied_worker() {
        let mut app = NohApp {
            shorts: workspace(),
            ..Default::default()
        };
        app.shorts.running = Some(true);
        let (tx, rx) = mpsc::sync_channel(1);
        app.shorts.work = Some(rx);
        let ctx = egui::Context::default();
        app.start(&ctx, false);
        app.start_subtitles(&ctx);
        app.start_caption(&ctx, false);
        app.start_short(&ctx, false);
        assert!(app.job.is_none() && !app.subtitles.running && app.captions.running.is_none());
        app.shorts.cancel_preparation();
        app.cancelling = true;
        assert!(app.shorts.work.is_some() && app.shorts.running.is_some());
        tx.send(WorkResult::Prepared(
            0,
            Ok(Prepared {
                request: app.shorts.request(None, true),
                snapshot: Snapshot(vec![]),
                owner: None,
            }),
        ))
        .unwrap();
        app.receive_shorts(&ctx);
        assert!(app.job.is_none() && app.shorts.running.is_none() && !app.cancelling);
        assert_eq!(
            app.shorts.outcome.as_ref().unwrap().key,
            "ui.short_cancelled"
        );
    }
    #[test]
    fn changed_external_source_or_srt_never_plays_a_previous_preview() {
        let mut workspace = workspace();
        let (tx, rx) = mpsc::sync_channel(1);
        workspace.work = Some(rx);
        tx.send(WorkResult::Verified(
            0,
            "preview.mp4".into(),
            Err(noh::engine::EngineError::new(
                "error.stale_inspection",
                "verify",
                None,
                "SRT input changed",
            )),
        ))
        .unwrap();
        let (start, open) = workspace.poll(&egui::Context::default(), true);
        assert!(start.is_none() && open.is_none());
        assert!(!workspace.preview_current());
        assert_eq!(
            workspace.failure.as_ref().unwrap().key,
            "ui.short_inputs_changed"
        );
        assert_eq!(
            workspace.failure_detail.as_deref(),
            Some("SRT input changed")
        );
        assert_eq!(workspace.export_artifact.as_ref().unwrap().revision, 0);
    }
}
