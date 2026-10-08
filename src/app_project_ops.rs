//! The single-screen project adapter. Preparation and verification perform no UI-thread IO.
use super::*;
use app_project::{Generation, ProjectArtifact, Running, Stamp};
use noh::{
    engine::EngineError,
    project::{ProjectRequest, ProjectShort},
    shorts::ShortCaptions,
    subtitles::{SubtitleRequest, SubtitleResult},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub(super) enum Prepared {
    Render {
        request: ProjectRequest,
        preview: Option<PreviewFile>,
    },
    Generate(SubtitleRequest),
}
pub(super) struct Verified {
    short: bool,
    stamp: Stamp,
    path: PathBuf,
    result: Result<(), EngineError>,
}

fn failure(operation: &str, path: Option<&Path>, detail: impl ToString) -> EngineError {
    EngineError::new("error.short_config", operation, path, detail)
}
fn cancelled() -> EngineError {
    EngineError::new(
        "status.export_cancelled",
        "prepare_project",
        None,
        "Operation cancelled.",
    )
}

impl NohApp {
    pub(super) fn start_project(&mut self, ctx: &egui::Context, previewing: bool, short: bool) {
        if self.job.is_some()
            || self.cancelling
            || self.project.busy()
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some()
        {
            return;
        }
        self.mini_preview.invalidate();
        if !short && !self.project.apply_subtitles {
            self.start(ctx, previewing);
            if self.job.is_some() {
                if previewing {
                    self.project.full_preview_artifact = None;
                } else {
                    self.project.full_export_artifact = None;
                }
            }
            return;
        }
        self.operation_revision = self.revision;
        let result = (|| {
            if short && let Some(issue) = self.project.short_name_issue() {
                return Err(EngineError::new(
                    issue.key(),
                    "prepare_project",
                    None,
                    "The short filename must be one valid MP4 filename inside the chosen destination folder.",
                ));
            }
            let mut montage = self.diagnostic_request().ok_or_else(|| {
                failure(
                    "prepare_project",
                    None,
                    "Wait for valid media and WAV metadata, and check the visual fades.",
                )
            })?;
            montage.preview = previewing;
            montage.output = if previewing {
                "preview.mp4".into()
            } else if short {
                self.project
                    .short_output(&self.output_name, &self.output_folder)
            } else {
                self.output.clone().ok_or_else(|| {
                    failure("prepare_project", None, "Choose a valid output filename.")
                })?
            };
            if let Some(issue) = app_destination::syntax(
                &montage
                    .output
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
            ) {
                return Err(EngineError::new(
                    issue.key(),
                    "prepare_project",
                    Some(&montage.output),
                    "Choose a valid MP4 filename.",
                ));
            }
            let selected = if short {
                let range = self.project.range.ok_or_else(|| {
                    EngineError::new(
                        "error.short_range",
                        "prepare_project",
                        None,
                        "Mark a short on the WAV timeline first.",
                    )
                })?;
                Some(ProjectShort {
                    start_ms: range.start_ms,
                    end_ms: range.end_ms,
                    restart_loops: self.project.restart_loops,
                    framing: self.project.framing,
                })
            } else {
                None
            };
            let captions = if self.project.apply_subtitles {
                if self.project.track.loaded.is_none() || self.project.track.validating() {
                    return Err(EngineError::new(
                        "error.caption_track",
                        "prepare_project",
                        self.project.track.path.as_deref(),
                        "Wait for a valid subtitle track on the current WAV clock.",
                    ));
                }
                Some(ShortCaptions {
                    subtitles: self.project.track.path.clone().unwrap(),
                    style: self.project.effective_caption_style(short),
                })
            } else {
                None
            };
            let request = ProjectRequest {
                montage,
                short: selected,
                captions,
            };
            request.validate()?;
            Ok(request)
        })();
        let request = match result {
            Ok(request) => request,
            Err(error) => {
                self.project_error(&error);
                return;
            }
        };
        let reviewed = self
            .project
            .apply_subtitles
            .then(|| self.project.track.loaded.as_ref().unwrap().snapshot.clone());
        let stamp = self.project.stamp(self.revision, short);
        self.project.running = Some(Running {
            preview: previewing,
            short,
            stamp,
            preview_file: None,
        });
        self.begin_project_operation(previewing, "short.preparing");
        self.prepare_project_work(ctx, move |cancel| {
            if cancel.load(Ordering::Acquire) {
                return Err(cancelled());
            }
            if let Some(reviewed) = &reviewed {
                reviewed.verify()?;
            }
            let preview = if previewing {
                Some(PreviewFile::new().map_err(|e| failure("prepare_preview", None, e))?)
            } else {
                None
            };
            let mut request = request;
            if let Some(preview) = &preview {
                request.montage.output = preview.path.clone();
            }
            // Reading the inputs' stamps fails early on a missing file.
            request.snapshot()?;
            if let Some(reviewed) = &reviewed {
                reviewed.verify()?;
            }
            if cancel.load(Ordering::Acquire) {
                return Err(cancelled());
            }
            Ok(Prepared::Render { request, preview })
        });
    }

    fn begin_project_operation(&mut self, previewing: bool, phase: &str) {
        if let Some(preflight) = &self.preflight {
            preflight.cancel();
        }
        self.inspecting = false;
        self.previewing = previewing;
        self.operation_revision = self.revision;
        self.cancelling = false;
        self.failure_revision = None;
        self.failure_path = None;
        self.error = None;
        self.notice = None;
        self.outcome = None;
        self.project.failure = None;
        self.project.outcome = None;
        self.started = Some(Instant::now());
        self.fraction = 0.0;
        self.phase = phase.into();
        self.log.clear();
        self.warnings.clear();
    }

    fn project_error(&mut self, error: &EngineError) {
        self.error = Some(error.message_code().into());
        self.project.failure = self.error.clone();
        self.failure_revision = Some(self.operation_revision);
        self.failure_path = error.path.clone();
        self.log.push_back(format!(
            "{}: {}\n{}",
            error.operation, error.detail, error.technical
        ));
        while self.log.len() > 300 {
            self.log.pop_front();
        }
    }

    fn prepare_project_work(
        &mut self,
        ctx: &egui::Context,
        prepare: impl FnOnce(&AtomicBool) -> Result<Prepared, EngineError> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let ctx = ctx.clone();
        self.project.work_cancel = Some(cancel);
        self.project.work = Some(rx);
        match std::thread::Builder::new()
            .name("project-prepare".into())
            .spawn(move || {
                let _ = tx.send(prepare(&worker_cancel));
                ctx.request_repaint();
            }) {
            Ok(_) => {}
            Err(error) => {
                self.project.work = None;
                self.project.work_cancel = None;
                self.project.running = None;
                self.project.generating = false;
                self.subtitles.running = false;
                self.started = None;
                self.project_error(&failure("prepare_project", None, error));
            }
        }
    }

    pub(super) fn cancel_project_preparation(&mut self) -> bool {
        if let Some(cancel) = &self.project.work_cancel {
            cancel.store(true, Ordering::Release);
            self.cancelling = true;
            return true;
        }
        false
    }

    pub(super) fn project_generate(&mut self, ctx: &egui::Context) {
        if self.job.is_some()
            || self.cancelling
            || self.project.busy()
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some()
        {
            return;
        }
        if let Some(issue) = self.resources.speech_issue() {
            self.subtitles.failure = Some(issue);
            self.subtitles.failure_revision = Some(self.subtitles.revision);
            self.settings_open = true;
            return;
        }
        let Some(wav) = self.wav.clone() else {
            self.subtitles.failure = Some("ui.subtitle_source_required".into());
            return;
        };
        let request = SubtitleRequest {
            source: wav.clone(),
            output: self.output_folder.join("subtitles.srt"),
            ffmpeg: self.ffmpeg.clone().unwrap_or_default(),
            transcriber: self.subtitles.transcriber.clone().into(),
            model: self.subtitles.model.clone().into(),
            vad_model: self.subtitles.vad_model.clone().into(),
            language: self.subtitles.language.clone(),
        };
        if let Err(error) = request.validate() {
            self.subtitles.failure = Some(error.message_code().into());
            self.subtitles.failure_revision = Some(self.subtitles.revision);
            return;
        }
        self.subtitles.source = wav.display().to_string();
        self.subtitles.edited();
        self.subtitles.operation_revision = self.subtitles.revision;
        self.subtitles.running = true;
        self.project.generating = true;
        self.project.generation = Some(Generation {
            wav_id: self.wav_id,
            wav,
            track_generation: self.project.track.generation(),
        });
        self.begin_project_operation(false, "subtitle.preparing");
        self.prepare_project_work(ctx, move |cancel| {
            let mut request = request;
            request.output = free_srt_path(
                &request.source,
                request.output.parent().unwrap_or(Path::new(".")),
                cancel,
            )?;
            if cancel.load(Ordering::Acquire) {
                return Err(cancelled());
            }
            Ok(Prepared::Generate(request))
        });
    }

    pub(super) fn poll_project(&mut self, ctx: &egui::Context) {
        if self.capture_state.is_none() {
            self.project.poll_input_identity(ctx);
            if std::mem::take(&mut self.project.input_changed) {
                // Refresh the soundtrack only. Media order and its existing metadata
                // do not affect the WAV envelope or subtitle source clock.
                self.invalidated();
                self.next_id = self.next_id.wrapping_add(1);
                self.wav_id = self.next_id;
                self.wav_seconds = None;
                self.wav_error = None;
                if let Some(wav) = &self.wav {
                    self.metadata
                        .request(self.wav_id, wav.clone(), true, self.ffmpeg.clone(), ctx);
                }
            }
            let output = self
                .project
                .short_output(&self.output_name, &self.output_folder);
            let valid = self.project.short_name_issue().is_none()
                && app_destination::syntax(
                    &output.file_name().unwrap_or_default().to_string_lossy(),
                )
                .is_none();
            let destination = self
                .project
                .short_destination
                .get_or_insert_with(|| app_destination::Destination::new(ctx.clone()));
            if destination.update(valid.then(|| output.clone()), false) {
                self.project.short_destination_result = None;
            }
            if let Some(result) = destination.receive() {
                self.project.short_destination_result = Some(result);
            }
            // As for the video: an automatic short name that is taken gives way
            // to the first free one, counted from `<video stem>-short.mp4`.
            let base = format!(
                "{}-short.mp4",
                Path::new(&self.output_name)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
            );
            if !self.project.busy()
                && self.job.is_none()
                && let Some(result) = &self.project.short_destination_result
                && let Some(name) = app_destination::adopt(
                    !self.project.short_named,
                    &output.file_name().unwrap_or_default().to_string_lossy(),
                    &base,
                    result,
                )
            {
                self.project.short_name = if name == base { String::new() } else { name };
            }
        }
        if self.project.seen_track_revision != self.project.track.revision {
            self.project.seen_track_revision = self.project.track.revision;
            if self.project.apply_subtitles {
                // An asynchronous attachment refresh is not a new user edit.
                let range_moved = self.project.range_moved;
                self.invalidated();
                self.project.range_moved = range_moved;
            }
        }
        if self
            .project
            .full_preview_artifact
            .as_ref()
            .is_some_and(|a| a.stamp != self.project.stamp(self.revision, false))
        {
            self.preview_ready = false;
        }
        let prepared = self.project.work.as_ref().map(|rx| rx.try_recv());
        match prepared {
            Some(Ok(result)) => {
                self.project.work = None;
                let cancelled = self
                    .project
                    .work_cancel
                    .take()
                    .is_some_and(|c| c.load(Ordering::Acquire));
                if cancelled {
                    self.finish_project_preparation_cancel();
                } else {
                    match result {
                        Ok(Prepared::Render { request, preview }) => {
                            let current = self.project.running.as_ref().is_some_and(|r| {
                                r.stamp == self.project.stamp(self.revision, r.short)
                            });
                            if current {
                                let running = self.project.running.as_mut().unwrap();
                                running.preview_file = preview;
                                let ctx = ctx.clone();
                                self.job = Some(Job::project_with_worker(
                                    request,
                                    self.worker.clone(),
                                    move || ctx.request_repaint(),
                                ));
                            } else {
                                self.project_error(&EngineError::new("error.stale_inspection", "prepare_project", None, "Project settings changed during preparation. Start the operation again."));
                                self.project.running = None;
                                self.started = None;
                            }
                        }
                        Ok(Prepared::Generate(request)) => {
                            let current = self.project.generation.as_ref().is_some_and(|g| {
                                g.wav_id == self.wav_id && self.wav.as_ref() == Some(&g.wav)
                            });
                            if current {
                                self.subtitles.output = request.output.display().to_string();
                                let ctx = ctx.clone();
                                self.job =
                                    Some(Job::transcribe(request, move || ctx.request_repaint()));
                            } else {
                                self.finish_project_preparation_cancel();
                            }
                        }
                        Err(error) => {
                            if self.project.generating {
                                self.subtitles.failure = Some(error.message_code().into());
                                self.subtitles.failure_revision =
                                    Some(self.subtitles.operation_revision);
                            } else {
                                self.project_error(&error);
                            }
                            self.project.running = None;
                            self.project.generating = false;
                            self.project.generation = None;
                            self.subtitles.running = false;
                            self.started = None;
                        }
                    }
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.project.work = None;
                self.project.work_cancel = None;
                self.project.running = None;
                self.project.generating = false;
                self.project.generation = None;
                self.subtitles.running = false;
                self.started = None;
                self.project_error(&failure(
                    "prepare_project",
                    None,
                    "Project preparation stopped unexpectedly.",
                ));
            }
            _ => {}
        }
        if let Some(reply) = self.project.verifying.as_ref().map(|rx| rx.try_recv()) {
            match reply {
                Ok(reply) => {
                    self.project.verifying = None;
                    if reply.stamp == self.project.stamp(self.revision, reply.short) {
                        match reply.result {
                            Ok(()) => self.open_embedded_preview(&reply.path, reply.short, ctx),
                            Err(error) => {
                                if reply.short {
                                    self.project.short_preview_artifact = None;
                                } else {
                                    self.preview_ready = false;
                                    self.preview_artifact = None;
                                }
                                self.project_error(&error);
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.project.verifying = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    fn finish_project_preparation_cancel(&mut self) {
        let generating = self.project.generating;
        self.project.running = None;
        self.project.generating = false;
        self.project.generation = None;
        self.subtitles.running = false;
        self.started = None;
        self.cancelling = false;
        let outcome: Message = if generating {
            "ui.subtitle_cancelled"
        } else if self.previewing {
            "ui.preview_cancelled_confirmed"
        } else {
            "ui.export_cancelled_confirmed"
        }
        .into();
        self.project.outcome = Some(outcome.clone());
        self.outcome = Some(outcome.clone());
        if generating {
            self.subtitles.outcome = Some(outcome);
        }
    }

    /// Called before the subtitle terminal handler. Its normal result UI
    /// still records the published file, even when attachment has become obsolete.
    /// True when the result was attached to the current song.
    pub(super) fn project_subtitled(
        &mut self,
        result: &Result<SubtitleResult, EngineError>,
    ) -> bool {
        self.project.generating = false;
        if let Some(generation) = self.project.generation.take()
            && generation.wav_id == self.wav_id
            && self.wav.as_ref() == Some(&generation.wav)
            && generation.track_generation == self.project.track.generation()
            && let Ok(result) = result
        {
            self.project
                .attach(result.output.clone(), self.wav.as_ref());
            return true;
        }
        false
    }

    /// Return true only for project-owned warnings/terminals. Progress and logs
    /// continue through the one existing operation bar and diagnostic buffer.
    pub(super) fn receive_project_event(&mut self, event: &Event) -> bool {
        if matches!(event, Event::Cancelled) && self.project.generating {
            self.project.generating = false;
            self.project.generation = None;
            return false;
        }
        if self.project.running.is_none() {
            return false;
        }
        match event {
            Event::Warning(reason) => {
                let message: Message = reason.to_wire().into();
                if !self.warnings.contains(&message) {
                    self.warnings.push(message);
                }
                true
            }
            Event::Done(result) => {
                let mut running = self.project.running.take().unwrap();
                match result {
                    Ok(result) => {
                        self.fraction = 1.0;
                        let current =
                            running.stamp == self.project.stamp(self.revision, running.short);
                        let artifact = ProjectArtifact {
                            path: result.output.clone(),
                            stamp: running.stamp.clone(),
                        };
                        if running.preview && running.short {
                            self.project.short_preview = running.preview_file.take();
                            self.project.short_preview_artifact = Some(artifact);
                            self.phase = "status.preview_ready".into();
                        } else if running.preview {
                            self.preview = running.preview_file.take();
                            self.preview_ready = current;
                            self.preview_artifact = Some(app_state::Artifact {
                                path: result.output.clone(),
                                revision: running.stamp.revision,
                                bytes: None,
                            });
                            self.project.full_preview_artifact = Some(artifact);
                            self.phase = "status.preview_ready".into();
                        } else if running.short {
                            self.project.short_export_artifact = Some(artifact);
                            self.project.short_destination_result =
                                Some(app_destination::ResultView {
                                    path: result.output.clone(),
                                    issue: Some(app_destination::Issue::Exists),
                                    suggestion: None,
                                    bytes: None,
                                });
                            if let Some(destination) = &self.project.short_destination {
                                destination.update(Some(result.output.clone()), true);
                            }
                            self.phase = "status.done".into();
                            self.project.outcome = Some("ui.short_exported".into());
                            self.last_export_short = true;
                        } else {
                            self.last_export_short = false;
                            self.completed = Some(result.output.clone());
                            self.export_artifact = Some(app_state::Artifact {
                                path: result.output.clone(),
                                revision: running.stamp.revision,
                                bytes: None,
                            });
                            self.project.full_export_artifact = Some(artifact);
                            self.destination_result = Some(app_destination::ResultView {
                                path: result.output.clone(),
                                issue: Some(app_destination::Issue::Exists),
                                suggestion: None,
                                bytes: None,
                            });
                            if let Some(destination) = &self.destination {
                                destination.update(self.output.clone(), true);
                            }
                            self.phase = "status.done".into();
                        }
                    }
                    Err(error) => {
                        self.project_error(error);
                        if error.code == "error.output_exists" && !running.preview {
                            self.open_sheet(running.short, None);
                        }
                        self.phase = if running.preview {
                            "status.preview_failed"
                        } else {
                            "status.export_failed"
                        }
                        .into();
                    }
                }
                true
            }
            Event::Cancelled => {
                let running = self.project.running.take().unwrap();
                let outcome: Message = if running.preview {
                    "ui.preview_cancelled_confirmed"
                } else {
                    "ui.export_cancelled_confirmed"
                }
                .into();
                self.project.outcome = Some(outcome.clone());
                self.outcome = Some(outcome);
                self.phase = if running.preview {
                    "status.preview_cancelled"
                } else {
                    "status.export_cancelled"
                }
                .into();
                true
            }
            _ => false,
        }
    }
}

fn free_srt_path(
    source: &Path,
    folder: &Path,
    cancel: &AtomicBool,
) -> Result<PathBuf, EngineError> {
    let stem = source
        .file_stem()
        .filter(|s| !s.is_empty())
        .unwrap_or(std::ffi::OsStr::new("subtitles"));
    let started = Instant::now();
    for index in 1..=1000 {
        if cancel.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        if started.elapsed() > Duration::from_secs(5) {
            break;
        }
        let mut name = stem.to_os_string();
        if index > 1 {
            name.push(format!("-{index}"));
        }
        name.push(".srt");
        let path = folder.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Err(error) => {
                return Err(EngineError::new(
                    "error.output_folder",
                    "choose_subtitle_output",
                    Some(folder),
                    error,
                ));
            }
            Ok(_) => {}
        }
    }
    Err(EngineError::new(
        "error.output_exists",
        "choose_subtitle_output",
        Some(folder),
        "No free subtitle name was found among the first 1000 names. Choose another destination folder.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subtitle_names_are_free_bounded_and_cancellable() {
        let folder = tempfile::tempdir().unwrap();
        let source = Path::new("voice.wav");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            free_srt_path(source, folder.path(), &cancel).unwrap(),
            folder.path().join("voice.srt")
        );
        std::fs::write(folder.path().join("voice.srt"), b"reviewed").unwrap();
        std::fs::write(folder.path().join("voice-2.srt"), b"reviewed").unwrap();
        assert_eq!(
            free_srt_path(source, folder.path(), &cancel).unwrap(),
            folder.path().join("voice-3.srt")
        );
        cancel.store(true, Ordering::Release);
        assert_eq!(
            free_srt_path(source, folder.path(), &cancel)
                .unwrap_err()
                .code,
            "status.export_cancelled"
        );
    }
    #[test]
    fn short_only_edits_and_destination_changes_do_not_stale_the_full_preview() {
        let mut state = app_project::State::default();
        state.duration_ms = 20_000;
        let full = state.stamp(3, false);
        let short = state.stamp(3, true);
        state.set_range(noh::timeline::Range::new(13_000, 18_000, 20_000));
        assert_eq!(full, state.stamp(3, false));
        assert_ne!(short, state.stamp(3, true));
        state.short_name = "renamed.mp4".into();
        assert_eq!(full, state.stamp(3, false));
        assert_eq!(
            state.short_output("main.mp4", Path::new("output")),
            Path::new("output/renamed.mp4")
        );
    }
    #[test]
    fn late_short_completion_does_not_become_current_or_replace_the_full_preview() {
        let mut app = NohApp::default();
        app.revision = 4;
        app.project.duration_ms = 20_000;
        app.project
            .set_range(noh::timeline::Range::new(13_000, 18_000, 20_000));
        app.preview_ready = true;
        app.preview_artifact = Some(app_state::Artifact {
            path: "full-preview.mp4".into(),
            revision: 4,
            bytes: None,
        });
        app.project.running = Some(Running {
            preview: true,
            short: true,
            stamp: app.project.stamp(4, true),
            preview_file: None,
        });
        app.project
            .set_range(noh::timeline::Range::new(12_000, 18_000, 20_000));
        assert!(
            app.receive_project_event(&Event::Done(Ok(noh::engine::ExportResult {
                output: "short-preview.mp4".into(),
                duration: 5.0
            })))
        );
        assert!(!app.project.short_preview_current(4));
        assert!(app.preview_ready);
        assert_eq!(
            app.preview_artifact.unwrap().path,
            PathBuf::from("full-preview.mp4")
        );
    }
    #[test]
    fn late_generation_cannot_replace_a_changed_wav_or_newer_track() {
        let mut app = NohApp::default();
        app.wav = Some("current.wav".into());
        app.wav_id = 7;
        app.project.duration_ms = 20_000;
        app.project.track.path = Some("reviewed.srt".into());
        app.project.generation = Some(Generation {
            wav_id: 6,
            wav: "old.wav".into(),
            track_generation: 0,
        });
        let result = Ok(SubtitleResult {
            output: "generated.srt".into(),
            track: noh::subtitle_srt::parse("1\n00:00:01,000 --> 00:00:02,000\nText\n", 20_000)
                .unwrap(),
        });
        app.project_subtitled(&result);
        assert_eq!(app.project.track.path, Some("reviewed.srt".into()));
        app.project.generation = Some(Generation {
            wav_id: 7,
            wav: "current.wav".into(),
            track_generation: 0,
        });
        app.project.track.clear();
        app.project_subtitled(&result);
        assert!(app.project.track.path.is_none());
    }
    #[test]
    fn short_filename_cannot_escape_the_chosen_folder() {
        let mut state = app_project::State::default();
        for name in [
            "../outside.mp4",
            "..\\outside.mp4",
            "C:\\outside.mp4",
            "/outside.mp4",
            "sub/video.mp4",
        ] {
            state.short_name = name.into();
            assert!(state.short_name_issue().is_some(), "{name}");
        }
        state.short_name = "Selected clip 日本語.mp4".into();
        assert!(state.short_name_issue().is_none());
    }
}
