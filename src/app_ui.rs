//! Native presentation; filesystem and media work stay off the render thread.
use super::*;
use app_state::{Analysis, ContextAction, Operation};
use app_style::{self as style, Severity, space};
use egui::{Align, Align2, FontId, Layout, Rect, Sense, Stroke, Vec2, pos2, vec2};

/// A duration field in seconds with one decimal ("1,5 s"), the interface
/// rule; an edited value snaps to 0.1 s so the text always equals the value.
pub(super) fn seconds_control(
    ui: &mut egui::Ui,
    value: &mut f64,
    width: f32,
    l: Language,
) -> egui::Response {
    let response = ui.add_sized(
        vec2(width, style::CONTROL_H),
        egui::DragValue::new(value)
            .speed(0.1)
            .suffix(l.text("seconds_suffix"))
            .fixed_decimals(1)
            .custom_formatter(move |v, _| l.decimal(v, 1))
            .custom_parser(|s| s.trim().replace(',', ".").parse().ok()),
    );
    if response.changed() && value.is_finite() {
        *value = (*value * 10.0).round() / 10.0;
    }
    response
}
fn filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
fn short_build_value(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let short: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}
fn bounded_width<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let bounds = ui.available_rect_before_wrap();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("viewport-content")
            .max_rect(bounds),
    );
    let result = contents(&mut child);
    ui.allocate_space(vec2(bounds.width(), child.min_size().y));
    result
}
pub(super) fn middle_path(ui: &egui::Ui, path: &Path, width: f32) -> String {
    let text = path.display().to_string();
    let fits = |text: String| {
        ui.painter()
            .layout_no_wrap(text, FontId::proportional(12.0), ui.visuals().text_color())
            .size()
            .x
            <= width
    };
    if fits(text.clone()) {
        return text;
    }
    let chars: Vec<_> = text.chars().collect();
    for keep in (2..chars.len()).rev() {
        let left = keep / 2;
        let right = keep - left;
        let shortened = format!(
            "{}…{}",
            chars[..left].iter().collect::<String>(),
            chars[chars.len() - right..].iter().collect::<String>()
        );
        if fits(shortened.clone()) {
            return shortened;
        }
    }
    "…".into()
}
fn treatment(ui: &mut egui::Ui, copied: bool, l: Language) {
    ui.label(
        RichText::new(format!(
            "{} {}",
            if copied { "✓" } else { "↻" },
            l.text(if copied {
                "ui.copy"
            } else {
                "diagnosis.convert"
            })
        ))
        .size(12.0)
        .color(style::color(
            ui,
            if copied {
                Severity::Success
            } else {
                Severity::Warning
            },
        )),
    );
}
pub(super) fn diagnosis_clip_row(
    ui: &mut egui::Ui,
    index: usize,
    path: &Path,
    copied: bool,
    l: Language,
) -> (Rect, Rect, Rect) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let badge_text = format!(
        "{} {}",
        if copied { "✓" } else { "↻" },
        l.text(if copied {
            "ui.copy"
        } else {
            "diagnosis.convert"
        })
    );
    let badge_width = ui
        .painter()
        .layout_no_wrap(
            badge_text,
            FontId::proportional(12.0),
            ui.visuals().text_color(),
        )
        .size()
        .x;
    let badge = Rect::from_min_max(pos2(row.right() - badge_width, row.top()), row.max);
    let mut index_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("detail-index")
            .max_rect(Rect::from_min_size(row.min, vec2(24.0, 20.0))),
    );
    index_ui.small((index + 1).to_string());
    let mut name_ui = ui.new_child(egui::UiBuilder::new().id_salt("detail-name").max_rect(
        Rect::from_min_max(
            pos2(row.left() + 24.0 + space::S, row.top()),
            pos2(badge.left() - space::S, row.bottom()),
        ),
    ));
    name_ui
        .add(egui::Label::new(filename(path)).truncate())
        .on_hover_text(path.display().to_string());
    let mut badge_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("detail-badge")
            .max_rect(badge),
    );
    treatment(&mut badge_ui, copied, l);
    (row, name_ui.min_rect(), badge_ui.min_rect())
}
#[derive(Clone, Copy, Debug)]
struct BarLayout {
    height: f32,
    main_bounds: Rect,
}
impl NohApp {
    pub(super) fn ready(&self) -> bool {
        !self.clips.is_empty()
            && self.clips.iter().all(|c| c.info.is_some())
            && self.image_durations_valid()
            && self.wav_seconds.is_some()
    }
    fn image_durations_valid(&self) -> bool {
        self.clips.iter().all(|clip| match &clip.item {
            noh::input::MediaItem::Image { duration, .. } => {
                duration.is_finite() && *duration > 0.0
            }
            noh::input::MediaItem::Video { .. } => true,
        })
    }
    pub(super) fn set_image_duration(&mut self, id: u64, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }
        let Some(clip) = self.clips.iter_mut().find(|clip| clip.id == id) else {
            return false;
        };
        let no_change = match &mut clip.item {
            noh::input::MediaItem::Image { duration, .. } => {
                if *duration == value {
                    true
                } else {
                    *duration = value;
                    false
                }
            }
            noh::input::MediaItem::Video { .. } => return false,
        };
        if !no_change {
            self.invalidated();
            true
        } else {
            false
        }
    }
    pub(super) fn invalidated(&mut self) {
        self.project.range_moved = false;
        self.mini_preview.invalidate();
        self.revision = self.revision.wrapping_add(1);
        self.diagnosis_revision = None;
        self.diagnosis = None;
        self.diagnosis_error = None;
        self.preview_ready = false;
        self.fraction = 0.0;
        self.error = None;
        self.failure_revision = None;
        self.failure_path = None;
        self.notice = None;
        self.outcome = None;
        // A refused drop or a failed generation is explained until the next edit.
        self.drop_error = None;
        self.subtitles.failure = None;
        self.warnings.clear();
    }
    pub(super) fn destination_edited(&mut self) {
        self.output = app_destination::syntax(&self.output_name)
            .is_none()
            .then(|| self.output_folder.join(&self.output_name));
        self.destination_result = None;
        self.notice = None;
        // An automatic short name follows the video's name.
        if !self.project.short_named {
            self.project.short_name.clear();
        }
        if self.error.as_ref().is_some_and(|e| {
            matches!(
                e.key.as_str(),
                "error.output_exists" | "error.output_folder" | "error.mp4" | "error.choose_output"
            )
        }) {
            self.error = None;
            self.failure_revision = None;
        }
    }
    pub(super) fn fades_valid(&self) -> bool {
        let values = [
            if self.in_enabled { self.fade_in } else { 0.0 },
            if self.out_enabled { self.fade_out } else { 0.0 },
        ];
        values.iter().all(|v| v.is_finite() && *v >= 0.0)
            && self
                .wav_seconds
                .is_none_or(|total| values.iter().sum::<f64>() <= total)
    }
    pub(super) fn diagnosis_current(&self) -> bool {
        self.diagnosis_revision == Some(self.revision) && self.diagnosis.is_some()
    }
    pub(super) fn diagnostic_request(&self) -> Option<Request> {
        if !self.ready() || !self.fades_valid() {
            return None;
        }
        Some(Request {
            items: self.clips.iter().map(|c| c.item.clone()).collect(),
            wav: self.wav.clone()?,
            output: PathBuf::from("diagnosis.mp4"),
            ffmpeg: self.ffmpeg.clone().unwrap_or_default(),
            fade_in: if self.in_enabled { self.fade_in } else { 0.0 },
            fade_out: if self.out_enabled { self.fade_out } else { 0.0 },
            partial_fades: self.partial,
            preview: false,
            clip_audio: self.clip_audio,
            force_encode: !self.partial,
        })
    }
    fn prepare_views(&mut self, ctx: &egui::Context) {
        if self.capture_state.is_some() {
            self.project.duration_ms = (self.wav_seconds.unwrap_or(0.0) * 1000.0).round() as u64;
            return;
        }
        let resources = self.resource_inputs();
        self.resources.sync(resources, ctx);
        self.retry_resource_media(ctx);
        self.project.sync_inputs(
            self.wav_id,
            self.wav.as_ref(),
            self.ffmpeg.as_ref(),
            self.wav_seconds,
            ctx,
        );
        self.poll_project(ctx);
        if self.output.is_none() && app_destination::syntax(&self.output_name).is_none() {
            self.output = Some(self.output_folder.join(&self.output_name));
        }
        let destination = self
            .destination
            .get_or_insert_with(|| app_destination::Destination::new(ctx.clone()));
        if destination.update(self.output.clone(), false) {
            self.destination_result = None;
        }
        if let Some(result) = destination.receive() {
            if let Some(artifact) = &mut self.export_artifact
                && artifact.path == result.path
            {
                artifact.bytes = result.bytes;
            }
            self.destination_result = Some(result);
        }
        // One-click export: a taken automatic name gives way to the first free
        // one, without a revision change. Not while exporting.
        if self.job.is_none()
            && !self.project.busy()
            && let Some(result) = &self.destination_result
            && let Some(name) = app_destination::adopt(
                self.output_auto,
                &self.output_name,
                &self.output_base,
                result,
            )
        {
            self.output_name = name;
            self.destination_edited();
        }
        if self.job.is_some()
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some()
            || (self.capture_overlay.as_deref() == Some("analyzing")
                && self.capture_overlay_step >= 2)
        {
            return;
        }
        let request = self.diagnostic_request();
        let worker = self.worker.clone();
        let preflight = self
            .preflight
            .get_or_insert_with(|| app_diagnosis::Preflight::new(ctx.clone(), worker));
        if preflight.update(request.clone(), false) {
            self.diagnosis = None;
            self.diagnosis_revision = None;
            self.diagnosis_error = None;
            self.inspecting = request.is_some();
        }
        if self.inspecting {
            if let Some((fraction, phase)) = preflight.progress() {
                self.analysis_fraction = fraction;
                self.analysis_phase = Some(phase);
            } else {
                self.analysis_fraction = 0.0;
                self.analysis_phase = None;
            }
        }
        if let Some(result) = preflight.receive() {
            self.inspecting = false;
            match result {
                Ok(result) => {
                    self.diagnosis = Some(result);
                    self.diagnosis_revision = Some(self.revision);
                    self.diagnosis_error = None;
                }
                Err(error) => {
                    self.diagnosis = None;
                    self.diagnosis_revision = None;
                    self.diagnosis_error = Some(error.message_code().into());
                    self.log.push_back(error.to_string());
                    if error.code == "error.stale_inspection" {
                        self.refresh_metadata(ctx);
                        if let Some(p) = &self.preflight {
                            p.update(None, true);
                        }
                        self.inspecting = false;
                    }
                }
            }
        }
    }
    pub(super) fn status(&self) -> app_state::StatusView {
        let operation = if self.project.generating {
            if self.cancelling {
                Operation::CancellingSubtitles
            } else {
                Operation::Transcribing
            }
        } else if let Some(running) = &self.project.running {
            if self.cancelling {
                Operation::Cancelling {
                    preview: running.preview,
                }
            } else {
                Operation::Running {
                    preview: running.preview,
                }
            }
        } else if let Some(preview) = self.shorts.running {
            if self.cancelling {
                Operation::CancellingShorts { preview }
            } else {
                Operation::Shortening { preview }
            }
        } else if let Some(preview) = self.captions.running {
            if self.cancelling {
                Operation::CancellingCaptions { preview }
            } else {
                Operation::Burning { preview }
            }
        } else if self.subtitles.running {
            if self.cancelling {
                Operation::CancellingSubtitles
            } else {
                Operation::Transcribing
            }
        } else if self.cancelling {
            Operation::Cancelling {
                preview: self.previewing,
            }
        } else if self.job.is_some() {
            Operation::Running {
                preview: self.previewing,
            }
        } else {
            Operation::Idle
        };
        if self.shorts.running.is_some() || (self.shorts.open && operation == Operation::Idle) {
            return self
                .shorts
                .status(operation, &self.phase, self.notice.as_ref());
        }
        if self.captions.running.is_some() || (self.captions.open && operation == Operation::Idle) {
            return self
                .captions
                .status(operation, &self.phase, self.notice.as_ref());
        }
        if (self.subtitles.running && !self.project.generating)
            || (self.subtitles.open && operation == Operation::Idle)
        {
            return self
                .subtitles
                .status(operation, &self.phase, self.notice.as_ref());
        }
        let analysis = if self.inspecting {
            Analysis::Pending
        } else if self.diagnosis_current() {
            Analysis::Current
        } else if self
            .diagnosis_error
            .as_ref()
            .is_some_and(|e| e.key == "diagnosis.cancelled")
        {
            Analysis::Cancelled
        } else if self.diagnosis_error.is_some() {
            Analysis::Failed
        } else {
            Analysis::Missing
        };
        let issue = app_destination::syntax(&self.output_name).or_else(|| {
            self.destination_result
                .as_ref()
                .and_then(|d| d.issue.clone())
        });
        let short_issue = self.project.short_name_issue().or_else(|| {
            self.project
                .short_destination_result
                .as_ref()
                .and_then(|d| d.issue.clone())
        });
        let unreadable = self
            .clips
            .iter()
            .find(|c| c.error.is_some() || self.failure_path.as_ref() == Some(c.item.path()))
            .map(|c| c.id);
        let last_export = self.last_export();
        let mut view = app_state::status_view(app_state::Inputs {
            operation,
            inputs_complete: !self.clips.is_empty() && self.wav.is_some(),
            missing_song: self.wav.is_none(),
            missing_visuals: self.clips.is_empty(),
            drop_error: self.drop_error.as_ref(),
            lyrics_failure: self
                .subtitles
                .failure
                .as_ref()
                .filter(|_| !self.project.generating),
            lyrics_ready: self
                .lyrics_ready
                .as_ref()
                .filter(|path| self.project.track.path.as_ref() == Some(*path))
                .map(|path| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }),
            // The engine's own count for that file: the chip may still be reading it.
            lyrics_empty: self.subtitles.artifact.as_ref().is_some_and(|artifact| {
                self.lyrics_ready.as_ref() == Some(&artifact.path) && artifact.cues == 0
            }),
            reading: self
                .clips
                .iter()
                .any(|c| c.info.is_none() && c.error.is_none())
                || (self.wav.is_some() && self.wav_seconds.is_none() && self.wav_error.is_none()),
            unreadable,
            wav_unreadable: self.wav_error.is_some(),
            fades_valid: self.fades_valid(),
            durations_valid: self.image_durations_valid(),
            analysis,
            analysis_cause: self.diagnosis_error.as_ref(),
            destination_issue: issue,
            destination_pending: self.destination_result.is_none(),
            suggestion: self
                .destination_result
                .as_ref()
                .is_some_and(|d| d.suggestion.is_some()),
            auto_name: self.output_auto,
            range: self.project.range.is_some(),
            short_issue,
            short_pending: self.project.short_destination_result.is_none(),
            short_auto: !self.project.short_named,
            last_export: last_export
                .as_ref()
                .map(|(path, current, short)| app_state::LastExport {
                    path,
                    current: *current,
                    short: *short,
                }),
            failure: self
                .error
                .as_ref()
                .filter(|_| self.failure_revision == Some(self.revision)),
            notice: self.notice.as_ref(),
            outcome: self.outcome.as_ref(),
            phase: if self.inspecting {
                self.analysis_phase.as_ref().unwrap_or(&self.phase)
            } else {
                &self.phase
            },
            fraction: self.fraction,
        });
        // Shown lyrics need their track read on the current song clock.
        if self.project.apply_subtitles
            && (self.project.track.loaded.is_none() || self.project.track.validating())
            && operation == Operation::Idle
        {
            for availability in [
                Some(&mut view.export),
                view.montage.as_mut().and_then(|m| m.short.as_mut()),
            ]
            .into_iter()
            .flatten()
            {
                availability.enabled = false;
                availability.reason = if self
                    .project
                    .track
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code == "lyrics.after_end")
                {
                    "lyrics.after_end"
                } else {
                    "error.caption_track"
                }
                .into();
            }
        }
        if operation == Operation::Idle && self.project.range_moved && view.third.is_none() {
            view.third = Some("range.moved".into());
        }
        view
    }
    /// The most recent export, video or short: its file, whether it matches
    /// the current settings, and whether it is the short.
    pub(super) fn last_export(&self) -> Option<(PathBuf, bool, bool)> {
        let video = self
            .export_artifact
            .as_ref()
            .map(|a| (a.path.clone(), a.revision == self.revision, false));
        let short = self.project.short_export_artifact.as_ref().map(|a| {
            (
                a.path.clone(),
                self.project.short_export_current(self.revision),
                true,
            )
        });
        if self.last_export_short {
            short.or(video)
        } else {
            video.or(short)
        }
    }
    fn analyze_again(&mut self, ctx: &egui::Context) {
        let request = self.diagnostic_request();
        let worker = self.worker.clone();
        let p = self
            .preflight
            .get_or_insert_with(|| app_diagnosis::Preflight::new(ctx.clone(), worker));
        p.update(request.clone(), true);
        self.diagnosis = None;
        self.diagnosis_revision = None;
        self.diagnosis_error = None;
        self.inspecting = request.is_some();
    }
    fn bar_action(&mut self, action: ContextAction, ctx: &egui::Context) {
        match action {
            ContextAction::Analyze => self.analyze_again(ctx),
            ContextAction::CancelAnalysis => {
                if let Some(p) = &self.preflight {
                    p.cancel();
                }
                self.inspecting = false;
                self.diagnosis = None;
                self.diagnosis_revision = None;
                self.diagnosis_error = Some("diagnosis.cancelled".into());
            }
            ContextAction::OpenVideo => {
                if let Some((path, _, _)) = self.last_export() {
                    self.open(&path);
                }
            }
            ContextAction::OpenFolder => {
                if let Some(path) = self
                    .last_export()
                    .and_then(|(path, _, _)| path.parent().map(Path::to_path_buf))
                {
                    self.open(&path);
                }
            }
            ContextAction::Remove(id) => {
                self.clips.retain(|c| c.id != id);
                self.invalidated();
            }
            ContextAction::Replace(id) => self.replace_clip(id, ctx),
            ContextAction::ImportLyrics => {
                self.lyrics_ready = None;
                self.pick_lyrics(ctx);
            }
            ContextAction::ReviewLyrics => {
                self.lyrics_ready = None;
                if let Some(path) = self.project.track.path.clone() {
                    self.open(&path);
                }
            }
            ContextAction::Suggest => {
                if let Some(name) = self
                    .destination_result
                    .as_ref()
                    .and_then(|d| d.suggestion.clone())
                {
                    self.output_name = name;
                    self.destination_edited();
                }
            }
            ContextAction::ChooseEngine => self.settings_open = true,
            ContextAction::ChooseFolder => self.open_sheet(false, None),
            ContextAction::OpenSubtitle => {
                if let Some(artifact) = &self.subtitles.artifact {
                    let path = artifact.path.clone();
                    self.open(&path);
                }
            }
            ContextAction::OpenSubtitleFolder => {
                if let Some(path) = self
                    .subtitles
                    .artifact
                    .as_ref()
                    .and_then(|a| a.path.parent())
                {
                    let path = path.to_path_buf();
                    self.open(&path);
                }
            }
            ContextAction::PlayCaption => self.captions.verify_preview(ctx),
            ContextAction::SuggestCaption => self.captions.suggest(),
            ContextAction::PlayShort => self.shorts.verify_preview(ctx),
            ContextAction::SuggestShort => self.shorts.suggest(),
            ContextAction::OpenShort => {
                if let Some(a) = &self.shorts.export_artifact {
                    let path = a.path.clone();
                    self.open(&path);
                }
            }
            ContextAction::OpenShortFolder => {
                if let Some(path) = self
                    .shorts
                    .export_artifact
                    .as_ref()
                    .and_then(|a| a.path.parent())
                {
                    let path = path.to_path_buf();
                    self.open(&path);
                }
            }
            ContextAction::OpenCaption => {
                if let Some(a) = &self.captions.export_artifact {
                    let path = a.path.clone();
                    self.open(&path);
                }
            }
            ContextAction::OpenCaptionFolder => {
                if let Some(path) = self
                    .captions
                    .export_artifact
                    .as_ref()
                    .and_then(|a| a.path.parent())
                {
                    let path = path.to_path_buf();
                    self.open(&path);
                }
            }
        }
    }
    fn operation_bar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        compact: bool,
    ) -> BarLayout {
        ui.spacing_mut().item_spacing = vec2(space::S, space::XS);
        let l = self.locale.language;
        let view = self.status();
        let mut action = None;
        let mut preview = false;
        let mut export = false;
        let mut export_short = false;
        let mut cancel = false;
        let short_workspace = self.shorts.running.is_some()
            || (self.shorts.open && view.operation == Operation::Idle);
        let caption_workspace = self.captions.running.is_some()
            || (self.captions.open && view.operation == Operation::Idle);
        // Generation from the lyrics chip belongs to the montage.
        let subtitle_workspace = (self.subtitles.running && !self.project.generating)
            || (self.subtitles.open && view.operation == Operation::Idle);
        let main_bounds = std::cell::Cell::new(Rect::NOTHING);
        let montage = !short_workspace && !caption_workspace && !subtitle_workspace;
        let bar = view.montage.clone().filter(|_| montage).unwrap_or_default();
        let change = std::cell::Cell::new(None::<egui::Id>);
        // The export button pressed, for focus when the sheet closes.
        let export_from = std::cell::Cell::new(None::<egui::Id>);
        let exporting_short = self.project.running.as_ref().is_some_and(|r| r.short);
        let short_target = self
            .project
            .short_output(&self.output_name, &self.output_folder);
        // Line 2: the last export, the short being exported, or
        // the pre-filled destination.
        let shown = bar.path.clone().unwrap_or_else(|| {
            if exporting_short {
                short_target.clone()
            } else {
                self.output_folder.join(&self.output_name)
            }
        });
        let place = |path: &Path| {
            let folder = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!(
                "{folder} › {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        };
        let destination_text = place(&shown);
        // The real targets, for the export buttons' names.
        let video_to = l.format(
            "bar.export_to",
            &[place(&self.output_folder.join(&self.output_name))],
        );
        let short_to = l.format("bar.export_to", &[place(&short_target)]);
        let inspecting = self.inspecting && montage;
        let fraction = if inspecting && view.operation == Operation::Idle {
            self.analysis_fraction
        } else {
            self.fraction
        };
        // Status block: icon 20, line 1 14/600 (+ %), progress 6,
        // line 2 12 (destination with "Change…", or the detail).
        let status_text = |ui: &mut egui::Ui| {
            let t = style::tokens(ui);
            ui.horizontal_top(|ui| {
                let (icon_rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                let busy_icon = view.operation != Operation::Idle || inspecting;
                if busy_icon {
                    ui.put(
                        icon_rect.shrink(2.0),
                        egui::Spinner::new().size(16.0).color(t.accent),
                    );
                } else {
                    app_icons::paint(
                        ui.painter(),
                        icon_rect,
                        match view.severity {
                            Severity::Success => app_icons::Icon::Ok,
                            Severity::Info => app_icons::Icon::Info,
                            Severity::Warning => app_icons::Icon::Warning,
                            Severity::Error => app_icons::Icon::Error,
                        },
                        style::color(ui, view.severity),
                    );
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = space::XS;
                    // Text lines, not 32 px control rows: rows take their minimum
                    // height from this when they are created.
                    ui.spacing_mut().interact_size.y = 16.0;
                    let running_export =
                        montage && matches!(view.operation, Operation::Running { preview: false });
                    let headline = if running_export {
                        l.text(if exporting_short {
                            "bar.exporting_short"
                        } else {
                            "bar.exporting_video"
                        })
                        .to_owned()
                    } else {
                        view.headline.render(l)
                    };
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(style::strong(&headline, style::text::BODY))
                                .truncate(),
                        )
                        .on_hover_text(&headline);
                        if running_export
                            || (matches!(
                                view.operation,
                                Operation::Transcribing | Operation::CancellingSubtitles
                            ) && fraction > 0.0)
                        {
                            ui.label(
                                RichText::new(format!("{} %", (fraction * 100.0).round() as u32))
                                    .color(t.text_2),
                            );
                        }
                    });
                    let indeterminate = matches!(
                        view.operation,
                        Operation::Transcribing | Operation::CancellingSubtitles
                    ) && fraction <= 0.0;
                    // Cancelling : the progress freezes and turns grey.
                    let cancelling = matches!(
                        view.operation,
                        Operation::Cancelling { .. }
                            | Operation::CancellingSubtitles
                            | Operation::CancellingCaptions { .. }
                            | Operation::CancellingShorts { .. }
                    );
                    if busy_icon {
                        let width = ui.available_width().min(style::PROGRESS_MAX_W);
                        let (track, _) =
                            ui.allocate_exact_size(vec2(width, style::PROGRESS_H), Sense::hover());
                        ui.painter().rect_filled(track, 3, t.surface_3);
                        let fill = if indeterminate && cancelling {
                            // Stopped segment in the middle: no motion, no fake percentage.
                            Rect::from_center_size(
                                track.center(),
                                vec2(track.width() * 0.3, track.height()),
                            )
                        } else if indeterminate {
                            // No fake percentage: a moving segment.
                            let phase = (ui.input(|i| i.time) * 0.6).fract() as f32;
                            let x =
                                track.left() + (track.width() * 1.3) * phase - track.width() * 0.3;
                            Rect::from_min_max(
                                pos2(x.max(track.left()), track.top()),
                                pos2((x + track.width() * 0.3).min(track.right()), track.bottom()),
                            )
                        } else {
                            Rect::from_min_size(
                                track.min,
                                vec2(track.width() * fraction.clamp(0.0, 1.0), track.height()),
                            )
                        };
                        if fill.is_positive() {
                            let color = if cancelling { t.text_3 } else { t.accent };
                            ui.painter().rect_filled(fill, 3, color);
                        }
                        if indeterminate && !cancelling {
                            ui.ctx().request_repaint();
                        }
                    }
                    if view.destination && montage {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S;
                            ui.spacing_mut().button_padding = vec2(0.0, 0.0);
                            let (folder, _) =
                                ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
                            app_icons::paint(
                                ui.painter(),
                                Rect::from_center_size(folder.center(), vec2(14.0, 14.0)),
                                app_icons::Icon::Folder,
                                t.text_3,
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&destination_text)
                                        .size(style::text::SMALL)
                                        .color(t.text_2),
                                )
                                .truncate(),
                            )
                            .on_hover_text(shown.display().to_string());
                            if let Some(tag) = &bar.tag {
                                style::tag(ui, &tag.render(l), true);
                            }
                            if view.operation == Operation::Idle && bar.change {
                                let link = ui.add(
                                    egui::Button::new(
                                        RichText::new(l.text("bar.change"))
                                            .size(style::text::SMALL)
                                            .underline()
                                            .color(t.accent_text),
                                    )
                                    .frame(false),
                                );
                                style::focus(ui, &link);
                                if link.clicked() {
                                    change.set(Some(link.id));
                                }
                            } else if view.operation != Operation::Idle && !compact {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(view.detail.render(l))
                                            .size(style::text::SMALL)
                                            .color(t.text_3),
                                    )
                                    .truncate(),
                                );
                            }
                        });
                    } else {
                        let detail = view.detail.render(l);
                        let label = egui::Label::new(
                            RichText::new(&detail)
                                .size(style::text::SMALL)
                                .color(t.text_2),
                        );
                        ui.add(
                            if view.headline.key == "ui.subtitle_succeeded"
                                || view.headline.key == "ui.caption_exported"
                                || view.headline.key == "ui.caption_last_export"
                                || view.headline.key == "ui.short_exported"
                                || view.headline.key == "ui.short_last_export"
                            {
                                label.truncate()
                            } else {
                                label.wrap()
                            },
                        )
                        .on_hover_text(&detail);
                    }
                    // The montage shows locking on the controls themselves.
                    let third = view
                        .third
                        .as_ref()
                        .filter(|third| !(montage && third.key == "ui.locked"));
                    if let Some(third) = third {
                        ui.add(
                            egui::Label::new(
                                RichText::new(third.render(l))
                                    .size(style::text::SMALL)
                                    .color(t.text_3),
                            )
                            .wrap(),
                        );
                    }
                    // A disabled export says why, unless line 2 already does.
                    if view.operation == Operation::Idle
                        && !view.export.enabled
                        && view.export.reason != view.detail
                        && !(montage && view.export.reason.key == "ui.analysis_required")
                    {
                        ui.add(
                            egui::Label::new(
                                RichText::new(view.export.reason.render(l))
                                    .size(style::text::SMALL)
                                    .color(t.text_2),
                            )
                            .wrap(),
                        );
                    }
                    if view.operation == Operation::Idle
                        && !montage
                        && !subtitle_workspace
                        && !view.preview.enabled
                        && view.preview.reason != view.export.reason
                        && view.preview.reason != view.detail
                    {
                        ui.add(
                            egui::Label::new(
                                RichText::new(view.preview.reason.render(l))
                                    .size(style::text::SMALL)
                                    .color(t.text_2),
                            )
                            .wrap(),
                        );
                    }
                });
            });
        };
        let action_label = |intent: &ContextAction| -> String {
            match intent {
                ContextAction::Analyze => l.text("bar.check_again").into(),
                ContextAction::CancelAnalysis => l.text("bar.check_cancel").into(),
                ContextAction::OpenVideo => l.text("open_video").into(),
                ContextAction::OpenFolder => l.text("open_folder").into(),
                ContextAction::Remove(id) => l.format(
                    "bar.remove_file",
                    &[self
                        .clips
                        .iter()
                        .find(|c| c.id == *id)
                        .and_then(|c| c.item.path().file_name())
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()],
                ),
                ContextAction::Replace(_) => l.text("bar.replace_file").into(),
                ContextAction::ReviewLyrics => l.text("lyrics.review").into(),
                ContextAction::ImportLyrics => l.text("lyrics.import").into(),
                ContextAction::Suggest => l.format(
                    "ui.use_name",
                    &[self
                        .destination_result
                        .as_ref()
                        .and_then(|d| d.suggestion.clone())
                        .unwrap_or_default()],
                ),
                ContextAction::ChooseEngine => l.text("choose_ffmpeg").into(),
                ContextAction::ChooseFolder => l.text("bar.change").into(),
                ContextAction::OpenSubtitle => l.text("ui.subtitle_open").into(),
                ContextAction::OpenSubtitleFolder => l.text("open_folder").into(),
                ContextAction::PlayCaption => l.text("ui.play_preview").into(),
                ContextAction::OpenCaption => l.text("open_video").into(),
                ContextAction::OpenCaptionFolder => l.text("open_folder").into(),
                ContextAction::SuggestCaption => l.text("ui.caption_free_name").into(),
                ContextAction::PlayShort => l.text("ui.play_preview").into(),
                ContextAction::OpenShort => l.text("open_video").into(),
                ContextAction::OpenShortFolder => l.text("open_folder").into(),
                ContextAction::SuggestShort => l.text("ui.caption_free_name").into(),
            }
        };
        // The completed-export state opens the video first (primary); opening a folder, stopping
        // the check and the stale row's "Open the video" are quiet.
        let action_kind = |index: usize, intent: &ContextAction| {
            if bar.done && index == 0 {
                style::Kind::Primary
            } else if matches!(
                intent,
                ContextAction::OpenFolder | ContextAction::CancelAnalysis
            ) || (montage && !bar.done && *intent == ContextAction::OpenVideo)
            {
                style::Kind::Quiet
            } else {
                style::Kind::Secondary
            }
        };
        let quiet_only = !view.actions.is_empty()
            && view
                .actions
                .iter()
                .enumerate()
                .all(|(i, a)| action_kind(i, a) == style::Kind::Quiet);
        let measure = |label: &str, primary: bool| {
            ui.painter()
                .layout_no_wrap(
                    label.into(),
                    if primary {
                        style::semibold(14.0)
                    } else {
                        FontId::proportional(14.0)
                    },
                    ui.visuals().text_color(),
                )
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x
        };
        // `widths`: one width per action (compact halves), or their own.
        let contextual = |ui: &mut egui::Ui,
                          action: &mut Option<ContextAction>,
                          height: f32,
                          widths: Option<f32>| {
            ui.horizontal_wrapped(|ui| {
                for (index, intent) in view.actions.iter().enumerate() {
                    let response = style::button_kind(
                        ui,
                        action_label(intent),
                        true,
                        action_kind(index, intent),
                        widths.unwrap_or(0.0),
                        height,
                    );
                    if response.clicked() {
                        *action = Some(intent.clone());
                    }
                }
            });
        };
        let contextual_width: f32 = view
            .actions
            .iter()
            .enumerate()
            .map(|(i, a)| measure(&action_label(a), action_kind(i, a) == style::Kind::Primary))
            .sum::<f32>()
            + space::S * view.actions.len().saturating_sub(1) as f32;
        // Bar buttons are 36 high (control-h-lg).
        let lg = |ui: &mut egui::Ui, label: &str, enabled: bool, primary: bool, width: f32| {
            style::button_kind(
                ui,
                label,
                enabled,
                if primary {
                    style::Kind::Primary
                } else {
                    style::Kind::Secondary
                },
                width,
                style::CONTROL_H_LG,
            )
        };
        let short_available = bar.short.clone().filter(|_| montage);
        let main = |ui: &mut egui::Ui,
                    preview: &mut bool,
                    export: &mut bool,
                    export_short: &mut bool,
                    cancel: &mut bool| {
            match view.operation {
                Operation::Idle => {
                    if subtitle_workspace {
                        let response = lg(
                            ui,
                            l.text("ui.subtitle_generate"),
                            view.export.enabled,
                            true,
                            if compact { ui.available_width() } else { 128.0 },
                        )
                        .on_hover_text(view.export.reason.render(l));
                        main_bounds.set(main_bounds.get().union(response.rect));
                        *export = response.clicked();
                        return;
                    }
                    if montage {
                        // [Export the short] [Export the video]; both secondary
                        // once the last export is current.
                        let count = if short_available.is_some() { 2.0 } else { 1.0 };
                        let width = if compact {
                            (ui.available_width() - space::S * (count - 1.0)) / count
                        } else {
                            0.0
                        };
                        let named = |response: egui::Response,
                                     visible: &str,
                                     name: &str,
                                     reason: &Message| {
                            if response.clicked() {
                                export_from.set(Some(response.id));
                            }
                            // The target is part of the name and the tooltip.
                            let enabled = response.enabled();
                            let label = format!("{} — {name}", visible);
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &label)
                            });
                            if enabled {
                                response.on_hover_text(name)
                            } else {
                                response.on_disabled_hover_text(reason.render(l))
                            }
                        };
                        if let Some(short) = &short_available {
                            let response =
                                lg(ui, l.text("bar.export_short"), short.enabled, false, width);
                            let response = named(
                                response,
                                l.text("bar.export_short"),
                                &short_to,
                                &short.reason,
                            );
                            main_bounds.set(main_bounds.get().union(response.rect));
                            *export_short = response.clicked();
                        }
                        let response = lg(
                            ui,
                            l.text("bar.export_video"),
                            view.export.enabled,
                            !bar.done,
                            if compact { width } else { 128.0 },
                        );
                        let response = named(
                            response,
                            l.text("bar.export_video"),
                            &video_to,
                            &view.export.reason,
                        );
                        main_bounds.set(main_bounds.get().union(response.rect));
                        *export = response.clicked();
                        return;
                    }
                    let width = if compact {
                        (ui.available_width() - space::S) / 2.0
                    } else {
                        0.0
                    };
                    let response = lg(
                        ui,
                        l.text("preview.play_full"),
                        view.preview.enabled,
                        false,
                        width,
                    )
                    .on_hover_text(view.preview.reason.render(l));
                    main_bounds.set(main_bounds.get().union(response.rect));
                    *preview = response.clicked();
                    let response = lg(
                        ui,
                        l.text(if short_workspace {
                            "ui.short_export"
                        } else if caption_workspace {
                            "ui.caption_export"
                        } else {
                            "export"
                        }),
                        view.export.enabled,
                        true,
                        width.max(if compact { 0.0 } else { 128.0 }),
                    )
                    .on_hover_text(view.export.reason.render(l));
                    main_bounds.set(main_bounds.get().union(response.rect));
                    *export = response.clicked();
                }
                Operation::Running { preview }
                | Operation::Burning { preview }
                | Operation::Shortening { preview } => {
                    let response = lg(
                        ui,
                        l.text(if preview {
                            "ui.cancel_preview"
                        } else if montage {
                            "bar.cancel"
                        } else {
                            "ui.cancel_export"
                        }),
                        true,
                        false,
                        if compact { ui.available_width() } else { 128.0 },
                    );
                    main_bounds.set(main_bounds.get().union(response.rect));
                    *cancel = response.clicked();
                }
                Operation::Transcribing => {
                    let label = if montage {
                        l.text("bar.cancel")
                    } else {
                        l.text("ui.subtitle_cancel")
                    };
                    let response = lg(ui, label, true, false, 128.0);
                    main_bounds.set(main_bounds.get().union(response.rect));
                    *cancel = response.clicked();
                }
                Operation::Cancelling { .. }
                | Operation::CancellingSubtitles
                | Operation::CancellingCaptions { .. }
                | Operation::CancellingShorts { .. } => {
                    let response = lg(ui, l.text("status.cancelling"), false, false, 128.0);
                    main_bounds.set(main_bounds.get().union(response.rect));
                }
            }
        };
        if compact {
            status_text(ui);
            ui.add_space(space::XS);
            if bar.done {
                // "Open the video" and "Open the folder" take the
                // main buttons' place (completed-export state).
                let half = (ui.available_width() - space::S) / 2.0;
                ui.horizontal(|ui| contextual(ui, &mut action, style::CONTROL_H_LG, Some(half)));
            } else {
                if !view.actions.is_empty() && !quiet_only {
                    ui.horizontal(|ui| {
                        ui.add_space(30.0);
                        contextual(ui, &mut action, style::CONTROL_H, None);
                    });
                    ui.add_space(space::XS);
                }
                ui.horizontal(|ui| {
                    main(
                        ui,
                        &mut preview,
                        &mut export,
                        &mut export_short,
                        &mut cancel,
                    )
                });
                if quiet_only {
                    // the quiet "Open the video" under the exports.
                    let full = ui.available_width();
                    contextual(ui, &mut action, style::CONTROL_H, Some(full));
                }
            }
        } else {
            let main_width = match view.operation {
                Operation::Idle => {
                    if subtitle_workspace {
                        measure(l.text("ui.subtitle_generate"), true).max(128.0)
                    } else if montage {
                        measure(l.text("bar.export_video"), !bar.done).max(128.0)
                            + short_available.as_ref().map_or(0.0, |_| {
                                measure(l.text("bar.export_short"), false) + space::S
                            })
                    } else {
                        measure(l.text("preview.play_full"), false)
                            + space::S
                            + measure(
                                l.text(if short_workspace {
                                    "ui.short_export"
                                } else if caption_workspace {
                                    "ui.caption_export"
                                } else {
                                    "export"
                                }),
                                true,
                            )
                            .max(128.0)
                    }
                }
                Operation::Running { preview }
                | Operation::Burning { preview }
                | Operation::Shortening { preview } => measure(
                    l.text(if preview {
                        "ui.cancel_preview"
                    } else if montage {
                        "bar.cancel"
                    } else {
                        "ui.cancel_export"
                    }),
                    false,
                )
                .max(128.0),
                Operation::Transcribing => measure(
                    l.text(if montage {
                        "bar.cancel"
                    } else {
                        "ui.subtitle_cancel"
                    }),
                    false,
                )
                .max(128.0),
                Operation::Cancelling { .. }
                | Operation::CancellingSubtitles
                | Operation::CancellingCaptions { .. }
                | Operation::CancellingShorts { .. } => {
                    measure(l.text("status.cancelling"), false).max(128.0)
                }
            };
            let bounds = ui.available_rect_before_wrap();
            // Contextual buttons join the main ones at the right when the text
            // keeps room; otherwise they go under the text.
            let inline = montage
                && !view.actions.is_empty()
                && bounds.width() - main_width - contextual_width - 2.0 * space::L >= 280.0;
            let right_width = if inline {
                main_width + contextual_width + space::S
            } else {
                main_width
            };
            let mut status_ui =
                ui.new_child(egui::UiBuilder::new().id_salt("bar-status").max_rect(
                    Rect::from_min_max(
                        bounds.min,
                        pos2(bounds.right() - right_width - space::L, bounds.bottom()),
                    ),
                ));
            status_text(&mut status_ui);
            if !view.actions.is_empty() && !inline {
                status_ui.add_space(space::XS);
                status_ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    contextual(ui, &mut action, style::CONTROL_H, None);
                });
            }
            // Main buttons use a reserved region, independent of status text length.
            // Render them last so keyboard traversal follows contextual actions.
            let mut main_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("bar-main")
                    .max_rect(Rect::from_min_max(
                        pos2(bounds.right() - right_width, bounds.top()),
                        bounds.max,
                    ))
                    .layout(Layout::left_to_right(Align::Min)),
            );
            main_ui.spacing_mut().item_spacing.x = space::S;
            if inline {
                contextual(&mut main_ui, &mut action, style::CONTROL_H_LG, None);
            }
            main(
                &mut main_ui,
                &mut preview,
                &mut export,
                &mut export_short,
                &mut cancel,
            );
            let height = status_ui
                .min_rect()
                .height()
                .max(main_ui.min_rect().height());
            ui.allocate_space(vec2(bounds.width(), height));
        }
        if let Some(from) = change.get() {
            self.open_sheet(false, Some(from));
        }
        if let Some(action) = action {
            self.bar_action(action, ctx);
        }
        if preview {
            if short_workspace {
                self.start_short(ctx, true);
            } else if caption_workspace {
                self.start_caption(ctx, true);
            } else {
                self.play_live_scope(ctx, false);
            }
        }
        if export {
            if short_workspace {
                self.start_short(ctx, false);
            } else if caption_workspace {
                self.start_caption(ctx, false);
            } else if subtitle_workspace {
                self.start_subtitles(ctx);
            } else if bar.conflict {
                // A name the person chose is taken: ask, never replace.
                self.open_sheet(false, export_from.get());
            } else {
                self.start_project(ctx, false, false);
            }
        }
        if export_short {
            if bar.short_conflict {
                self.open_sheet(true, export_from.get());
            } else {
                self.start_project(ctx, false, true);
            }
        }
        if cancel && self.cancel_project_preparation() {
            self.cancelling = true;
        } else if cancel && let Some(job) = &self.job {
            job.cancel();
            self.cancelling = true;
        } else if cancel && self.shorts.running.is_some() {
            self.shorts.cancel_preparation();
            self.cancelling = true;
        } else if cancel && self.captions.running.is_some() {
            self.captions.cancel_preparation();
            self.cancelling = true;
        }
        BarLayout {
            height: ui.min_rect().height(),
            main_bounds: main_bounds.get(),
        }
    }
    pub(super) fn build_information(&self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let l = self.locale.language;
        let info = noh::build_info::current();

        ui.horizontal_wrapped(|ui| {
            ui.label(style::strong(format!("{}:", l.text("ui.version")), 13.0));
            ui.label(info.package_version);
        });

        let revision = info
            .git_revision
            .map(|value| short_build_value(value, 10))
            .unwrap_or_else(|| l.text("ui.unavailable").to_owned());
        let state_key = match info.git_dirty {
            Some(false) => "ui.build_clean",
            Some(true) => "ui.build_modified",
            None => "ui.unavailable",
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(style::strong(
                format!("{}:", l.text("ui.git_revision")),
                13.0,
            ));
            ui.label(revision);
            if info.git_revision.is_some() || info.git_dirty.is_some() {
                ui.label("·");
                ui.label(l.text(state_key));
            }
        });

        let fingerprint = short_build_value(info.build_fingerprint, 12);
        ui.add(egui::Label::new(l.format("ui.build_fingerprint", &[fingerprint])).wrap());

        if style::button(ui, l.text("ui.copy_build_information"), true, false, 0.0).clicked() {
            ctx.copy_text(info.json());
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) -> egui::InnerResponse<Rect> {
        let l = self.locale.language;
        let viewport_clip = ui.clip_rect();
        let compact_header = ui.available_width() - 32.0 < style::COMPACT_BELOW;
        // One compact title row; all project operations share the main view.
        egui::Panel::top("app-header")
            .min_size(style::HEADER_H)
            .frame(
                egui::Frame::new()
                    .fill(style::background(ui))
                    // Design: 24 left, 16 right (16 and 8 compact).
                    .inner_margin(egui::Margin {
                        left: if compact_header { 16 } else { 24 },
                        right: if compact_header { 8 } else { 16 },
                        top: 8,
                        bottom: 8,
                    }),
            )
            .show(ui, |ui| {
                // Panel initially clips to its previous height; let native content
                // measurement grow the header on its first frame or after wrapping.
                ui.set_clip_rect(viewport_clip);
                ui.horizontal(|ui| {
                    // The wordmark is centred in the header row (14.5-point capitals).
                    let (w, h) = app_icons::WORDMARK;
                    let (logo, response) =
                        ui.allocate_exact_size(vec2(16.0 * w / h, 16.0), Sense::hover());
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "NOH")
                    });
                    app_icons::wordmark(ui.painter(), logo, style::tokens(ui).text);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let r = style::icon_button(
                            ui,
                            app_icons::Icon::Settings,
                            l.text("settings"),
                            style::CONTROL_H,
                        );
                        self.options.button = Some(r.id);
                        if r.clicked() {
                            self.settings_open = !self.settings_open;
                        }
                        if self.resources.unavailable() {
                            let warning = style::icon_button(
                                ui,
                                app_icons::Icon::Warning,
                                l.text("resources.resolve"),
                                style::CONTROL_H,
                            )
                            .on_hover_text(l.text("resources.unavailable"));
                            if warning.clicked() {
                                self.settings_open = true;
                            }
                        }
                    });
                });
                ui.clip_rect()
            })
    }
    pub(super) fn draw(&mut self, ui: &mut egui::Ui) -> egui::scroll_area::ScrollAreaOutput<()> {
        let ctx = ui.ctx().clone();
        let l = self.locale.language;
        style::apply(&ctx);
        if self.capture_state.as_deref() == Some("components") {
            let gallery = self.components_gallery(ui);
            self.capture_frame(&ctx);
            return gallery;
        }
        self.receive();
        #[cfg(feature = "updates")]
        self.poll_updates(&ctx);
        #[cfg(feature = "updates")]
        if self.update_blocks_session()
            || (self.updates.capture_fixture && self.updates.confirm_install)
        {
            // No media controls or project shortcuts run after safe-close consent.
            // The old process lease remains alive until normal application exit.
            ctx.request_repaint_after(Duration::from_millis(100));
            return egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Window::new(l.text("updates.title"))
                    .id(egui::Id::new("update-session-close"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .default_width(460.0)
                    .max_width((ui.available_width() - 32.0).max(100.0))
                    .show(&ctx, |ui| self.update_shutdown_controls(ui, &ctx));
                self.capture_frame(&ctx);
            });
        }
        self.receive_captions(&ctx);
        self.receive_shorts(&ctx);
        self.prepare_views(&ctx);
        self.poll_mini_preview(&ctx);
        let busy = self.project.busy()
            || self.job.is_some()
            || self.cancelling
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some();
        let compact = ui.available_width() - 32.0 < style::COMPACT_BELOW;
        if busy && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_question = true;
        }
        // An empty project is only the drop area, without export bar.
        let empty =
            self.clips.is_empty() && self.wav.is_none() && self.project.track.path.is_none();
        self.header(ui);
        // Reserve space first, render its interactive controls last to preserve Tab order.
        let height_id = ui
            .id()
            .with(("operation-height", self.locale.language.code(), compact));
        let bar_width = ui.available_width();
        let bar_height = ctx
            .data(|data| data.get_temp::<(f32, f32)>(height_id))
            .filter(|(width, _)| (width - bar_width).abs() < 0.5)
            .map(|(_, height)| height)
            .unwrap_or(if compact { 176.0 } else { 112.0 });
        let bar_pad = if compact { 12.0 } else { 14.0 };
        let bar_rect = (!empty).then(|| {
            egui::Panel::bottom("operations")
                .exact_size(bar_height)
                .resizable(false)
                .frame(egui::Frame::new().fill(style::surface(ui)))
                .show(ui, |ui| ui.available_rect_before_wrap())
                .inner
        });
        let mut body_rect = Rect::NOTHING;
        let scroll = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(style::background(ui)))
            .show(ui, |ui| {
                body_rect = ui.max_rect();
                let nested = ui.spacing().scroll;
                ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                // Capture states use their configured `scroll=` offset.
                let capture_scroll = self
                    .capture_overlay
                    .as_ref()
                    .map(|_| ())
                    .or_else(|| {
                        (std::env::var_os("NOH_CAPTURE_UI").is_some()
                            && std::env::var("NOH_CAPTURE_SCRUB").as_deref() == Ok("1"))
                        .then_some(())
                    })
                    .and_then(|_| app_capture::parse_tweaks(&app_capture::tweaks_text()).ok())
                    .map(|tweaks| tweaks.scroll)
                    .filter(|scroll| *scroll > 0.0);
                let area = egui::ScrollArea::vertical();
                let area = match capture_scroll {
                    Some(offset) => area.vertical_scroll_offset(offset),
                    None => area,
                };
                area.id_salt("montage")
                    // Outer margin 24 (16 compact), zones 20 apart (12 compact).
                    .content_margin(egui::Margin::symmetric(
                        (if compact { space::L } else { space::XL }) as i8,
                        8,
                    ))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        bounded_width(ui, |ui| {
                            ui.spacing_mut().scroll = nested;
                            ui.spacing_mut().item_spacing.y = if compact { space::M } else { 20.0 };
                            let render_busy =
                                busy && !self.subtitles.running && !self.project.generating;
                            if empty {
                                self.resource_banner(ui);
                            }
                            if empty {
                                self.drop_area(ui, &ctx, (ui.available_height() - 16.0).max(80.0));
                                return;
                            }
                            // Compose the strip, stage, timeline and range controls.
                            self.strip_ui(ui, &ctx, render_busy);
                            if self.clips.is_empty() {
                                self.missing_panel(ui, &ctx, false);
                            } else {
                                self.stage_ui(ui, render_busy);
                                if self.wav.is_none() {
                                    self.missing_panel(ui, &ctx, true);
                                }
                            }
                            if self.project.duration_ms > 0 {
                                self.timeline_zone(ui, render_busy);
                            }
                        });
                    })
            })
            .inner;
        if let Some(bar_rect) = bar_rect {
            let mut bar_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("operation-content")
                    // Padding 14 / 16 / 14 / 24 (12 / 16 compact).
                    .max_rect(Rect::from_min_max(
                        bar_rect.min + vec2(if compact { 16.0 } else { 24.0 }, bar_pad),
                        bar_rect.max - vec2(16.0, bar_pad),
                    )),
            );
            ui.painter().line_segment(
                [bar_rect.left_top(), bar_rect.right_top()],
                Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            );
            let layout = self.operation_bar(&mut bar_ui, &ctx, compact);
            debug_assert!(layout.main_bounds.right() <= bar_rect.right() + 0.5);
            let measured_height =
                (layout.height + 2.0 * bar_pad).max(if compact { 0.0 } else { 72.0 });
            ctx.data_mut(|data| data.insert_temp(height_id, (bar_width, measured_height)));
            if (measured_height - bar_height).abs() > 0.5 {
                ctx.request_repaint();
            }
        }
        self.location_sheet(&ctx);
        let paths = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        if !paths.is_empty() {
            self.drop_files(paths, &ctx);
        }
        self.drop_overlay(&ctx, body_rect);
        if ctx.input(|i| i.pointer.any_released() || i.key_pressed(egui::Key::Escape)) {
            self.dragging = None;
        }
        self.options_modal(&ctx);
        if self.close_question {
            egui::Window::new(l.text("close_title"))
                .id(egui::Id::new("confirm-close"))
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(l.text("close_body"));
                    ui.horizontal(|ui| {
                        if style::button(ui, l.text("continue"), true, false, 0.0).clicked() {
                            self.close_question = false;
                        }
                        if style::button(ui, l.text("stop_quit"), true, false, 0.0).clicked() {
                            self.job.take();
                            self.cancel_project_preparation();
                            self.project.work = None;
                            self.project.work_cancel = None;
                            self.project.running = None;
                            self.project.generating = false;
                            self.project.generation = None;
                            if self.shorts.running.is_some() {
                                self.shorts.cancel_preparation();
                                self.shorts.reaped();
                            }
                            if self.captions.running.is_some() {
                                self.captions.cancel_preparation();
                                self.captions.reaped();
                            }
                            self.subtitles.running = false;
                            self.cancelling = false;
                            self.close_question = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
        }
        self.capture_frame(&ctx);
        if busy {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        scroll
    }
}
impl eframe::App for NohApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.scrub_capture_input(ctx, input);
    }
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        #[cfg(feature = "updates")]
        self.updates.worker.take();
        if let Some(scrubber) = &mut self.mini_preview.scrubber {
            scrubber.close(gl);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(scrubber) = &self.mini_preview.scrubber {
            scrubber.service(ui.ctx());
        }
        self.draw(ui);
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn default_screen_hides_engine_vocabulary_and_explains_a_moved_range() {
        for language in Language::ALL {
            let ctx = egui::Context::default();
            app_locale::install_fonts(&ctx);
            style::apply(&ctx);
            let mut app = NohApp::default();
            app.locale.language = language;
            app.capture_state = Some("ready".into());
            app.wav = Some("song.wav".into());
            app.wav_seconds = Some(20.0);
            let mut info = noh::media::MediaInfo::default();
            info.seconds = 5.0;
            app.clips.push(Clip {
                id: 1,
                request_id: 1,
                item: "picture.png".into(),
                info: Some(info),
                error: None,
            });
            app.project.range = noh::timeline::Range::new(5_000, 10_000, 20_000);
            app.project.range_moved = true;
            app.project.apply_subtitles = true;
            app.project.track.revision = 1;
            app.poll_project(&ctx);
            assert_eq!(app.status().third.unwrap().key, "range.moved");
            let mut painted = Vec::new();
            for _ in 0..3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            vec2(980.0, 850.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        app.draw(ui);
                    },
                );
                output.textures_delta.clear();
                painted = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                        _ => None,
                    })
                    .collect();
            }
            assert!(
                painted
                    .iter()
                    .any(|text| text == language.text("range.moved"))
            );
            for forbidden in [
                language.text("ui.mode"),
                language.text("ui.processing_log"),
                language.text("diagnosis.partial"),
                language.text("diagnosis.color_limit"),
                "H.264",
                "AAC",
                "h264",
                "aac_320",
            ] {
                assert!(
                    !painted.iter().any(|text| text.contains(forbidden)),
                    "{language:?}: forbidden {forbidden:?} in {painted:?}"
                );
            }
            app.invalidated();
            assert!(!app.project.range_moved);
        }
    }

    #[test]
    fn caption_workspace_and_bar_fit_both_viewports_in_all_languages() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        for language in Language::ALL {
            for (width, height) in [(420.0, 540.0), (980.0, 850.0)] {
                for state in [
                    "ready",
                    "preview",
                    "failed",
                    "cancelling",
                    "exported",
                    "stale",
                ] {
                    let mut app = NohApp::default();
                    app.locale.language = language;
                    app.capture_state = Some(state.into());
                    app.captions.open = true;
                    app.captions.source = format!(
                        "C:/noh-qa/{}.mp4",
                        "A long 日本語 한국어 filename ".repeat(10)
                    );
                    app.captions.subtitles = "C:/noh-qa/Reviewed captions.srt".into();
                    app.captions.destination_result = Some(app_destination::ResultView {
                        path: app.captions.output.clone().into(),
                        issue: None,
                        suggestion: None,
                        bytes: None,
                    });
                    match state {
                        "preview" => {
                            app.captions.preview_artifact = Some(app_captions::PreviewArtifact {
                                path: "preview.mp4".into(),
                                revision: 0,
                                snapshot: noh::inspection::Snapshot(vec![]),
                            })
                        }
                        "cancelling" => {
                            app.captions.running = Some(false);
                            app.cancelling = true;
                        }
                        "failed" => {
                            app.captions.failure = Some("error.caption_layout".into());
                            app.captions.failure_revision = Some(0);
                            app.captions.failure_detail =
                                Some("SRT block 2, line 7: unsupported character U+1F600".into());
                        }
                        "exported" | "stale" => {
                            app.captions.export_artifact = Some(app_state::Artifact {
                                path: format!(
                                    "C:/noh-qa/{}/captioned.mp4",
                                    "Long folder 日本語/".repeat(30)
                                )
                                .into(),
                                revision: 0,
                                bytes: None,
                            });
                            if state == "stale" {
                                app.captions.revision = 1;
                            }
                        }
                        _ => {}
                    }
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                pos2(0.0, 0.0),
                                vec2(width, height),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            app.draw(ui);
                            assert!(
                                ui.min_rect().right() <= width + 0.5,
                                "{} {width} {state}: {:?}",
                                language.code(),
                                ui.min_rect()
                            );
                            assert!(
                                ui.min_rect().bottom() <= height + 0.5,
                                "{} {width} {state}: {:?}",
                                language.code(),
                                ui.min_rect()
                            );
                        },
                    );
                    output.textures_delta.clear();
                }
            }
        }
    }

    #[test]
    fn shorts_workspace_and_bar_fit_all_languages_at_compact_and_normal_sizes() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        for (width, height) in [(420.0, 540.0), (980.0, 850.0)] {
            for language in Language::ALL {
                for state in [
                    "ready",
                    "preview",
                    "stale",
                    "rendering",
                    "cancelling",
                    "failed",
                    "exported",
                ] {
                    let mut app = NohApp::default();
                    app.locale.language = language;
                    app.capture_state = Some(state.into());
                    app.shorts.open = true;
                    app.shorts.source = format!(
                        "C:/noh-qa/{}.mp4",
                        "A long source 日本語 한국어 中文 ".repeat(12)
                    );
                    app.shorts.start_ms = 135;
                    app.shorts.end_ms = 1775;
                    app.shorts.captions_enabled = true;
                    app.shorts.subtitles = "C:/noh-qa/Reviewed subtitles.srt".into();
                    app.shorts.destination_result = Some(app_destination::ResultView {
                        path: app.shorts.output.clone().into(),
                        issue: None,
                        suggestion: None,
                        bytes: None,
                    });
                    match state {
                        "preview" => {
                            app.shorts.preview_artifact = Some(app_shorts::PreviewArtifact {
                                path: "preview.mp4".into(),
                                revision: 0,
                                snapshot: noh::inspection::Snapshot(vec![]),
                            })
                        }
                        "rendering" | "cancelling" => {
                            app.shorts.running = Some(false);
                            app.cancelling = state == "cancelling";
                            app.phase = "short.rendering".into();
                        }
                        "failed" => {
                            app.shorts.failure = Some("error.short_range".into());
                            app.shorts.failure_revision = Some(0);
                            app.shorts.failure_detail = Some(
                                "The selected end is beyond the source video duration.".into(),
                            );
                        }
                        "exported" | "stale" => {
                            app.shorts.export_artifact = Some(app_state::Artifact {
                                path: format!(
                                    "C:/noh-qa/{}/short.mp4",
                                    "Long folder 日本語/".repeat(30)
                                )
                                .into(),
                                revision: 0,
                                bytes: None,
                            });
                            app.shorts.revision = u64::from(state == "stale");
                        }
                        _ => {}
                    }
                    for _ in 0..2 {
                        let mut output = ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(Rect::from_min_size(
                                    pos2(0.0, 0.0),
                                    vec2(width, height),
                                )),
                                ..Default::default()
                            },
                            |ui| {
                                app.draw(ui);
                                assert!(
                                    ui.min_rect().right() <= width + 0.5
                                        && ui.min_rect().bottom() <= height + 0.5,
                                    "{} {width} {state}: viewport overflow {:?}",
                                    language.code(),
                                    ui.min_rect()
                                );
                            },
                        );
                        output.textures_delta.clear();
                    }
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                pos2(0.0, 0.0),
                                vec2(width, height),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let bounds =
                                Rect::from_min_size(pos2(16.0, 0.0), vec2(width - 32.0, 240.0));
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .id_salt("short-bar-test")
                                    .max_rect(bounds),
                            );
                            let layout =
                                app.operation_bar(&mut child, &ctx, width < style::COMPACT_BELOW);
                            assert!(
                                bounds.expand(0.5).contains_rect(layout.main_bounds),
                                "{} {width} {state}: actions clipped",
                                language.code()
                            );
                            assert!(
                                child.min_rect().right() <= bounds.right() + 0.5
                                    && child.min_rect().bottom() <= bounds.bottom() + 0.5,
                                "{} {width} {state}: bar overflow",
                                language.code()
                            );
                        },
                    );
                    output.textures_delta.clear();
                }
            }
        }
    }

    #[test]
    fn single_view_header_stays_compact_in_all_languages() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        for (width, height) in [(420.0, 540.0), (980.0, 850.0)] {
            for language in Language::ALL {
                for selected in 0..4 {
                    let mut app = NohApp::default();
                    app.locale.language = language;
                    app.subtitles.open = selected == 1;
                    app.captions.open = selected == 2;
                    app.shorts.open = selected == 3;
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                pos2(0.0, 0.0),
                                vec2(width, height),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let header = app.header(ui);
                            let clip = header.inner;
                            let bounds = header.response.rect;
                            assert!(bounds.height() <= style::HEADER_H + 16.0);
                            assert!(clip.width() <= width + 0.5);
                        },
                    );
                    output.textures_delta.clear();
                }
            }
        }
    }

    #[test]
    fn subtitle_operation_bar_fits_short_narrow_viewports_in_all_languages() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        for language in Language::ALL {
            for state in [
                "setup",
                "ready",
                "running",
                "cancelling",
                "failed",
                "succeeded",
                "stale",
            ] {
                let mut app = NohApp::default();
                app.locale.language = language;
                app.subtitles.open = true;
                app.subtitles.source = "speech.wav".into();
                app.subtitles.transcriber = "whisper-cli.exe".into();
                app.subtitles.model = "model.bin".into();
                app.subtitles.vad_model = "vad.bin".into();
                match state {
                    "setup" => app.subtitles.model.clear(),
                    "running" | "cancelling" => {
                        app.subtitles.running = true;
                        app.cancelling = state == "cancelling";
                        app.phase = "subtitle.transcribing".into();
                        app.started = Some(Instant::now());
                    }
                    "failed" => {
                        app.subtitles.failure = Some("error.subtitle_backend".into());
                        app.subtitles.failure_revision = Some(0);
                    }
                    "succeeded" | "stale" => {
                        app.subtitles.artifact = Some(app_subtitles::Artifact {
                            path: format!(
                                "C:/noh-qa/{}.srt",
                                "A long subtitle filename with Unicode 日本語 한국어 中文 "
                                    .repeat(8)
                            )
                            .into(),
                            revision: 0,
                            cues: 42,
                        });
                        if state == "stale" {
                            app.subtitles.revision = 1;
                        }
                    }
                    _ => {}
                }
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 540.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let bounds = Rect::from_min_size(pos2(16.0, 340.0), vec2(388.0, 188.0));
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt((language.code(), state))
                                .max_rect(bounds),
                        );
                        let layout = app.operation_bar(&mut child, &ctx, true);
                        assert!(
                            layout.main_bounds.right() <= bounds.right() + 0.5,
                            "{} {state}: action overflow",
                            language.code()
                        );
                        assert!(
                            child.min_rect().right() <= bounds.right() + 0.5,
                            "{} {state}: bar overflow {:?}",
                            language.code(),
                            child.min_rect()
                        );
                        assert!(
                            child.min_rect().bottom() <= bounds.bottom() + 0.5,
                            "{} {state}: bar too tall {:?}",
                            language.code(),
                            child.min_rect()
                        );
                    },
                );
                output.textures_delta.clear();
            }
        }
    }

    #[test]
    fn narrow_diagnosis_rows_bound_long_names_before_the_badge() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        for language in Language::ALL {
            for copied in [false, true] {
                let mut out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 540.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let bounds = Rect::from_min_size(pos2(44.0, 0.0), vec2(348.0, 20.0));
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt((language.code(), copied))
                                .max_rect(bounds),
                        );
                        let (row, name, badge) = diagnosis_clip_row(
                            &mut child,
                            1,
                            Path::new("A very long video name — 動画 테스트 视频.mp4"),
                            copied,
                            language,
                        );
                        assert!(
                            name.right() + space::S <= badge.left() + 0.5,
                            "{}: name overlaps badge",
                            language.code()
                        );
                        assert!(
                            badge.right() <= row.right() + 0.5,
                            "{}: badge exceeds the row",
                            language.code()
                        );
                        assert!(
                            child.min_rect().right() <= bounds.right() + 0.5,
                            "{}: expanded row exceeds viewport",
                            language.code()
                        );
                    },
                );
                out.textures_delta.clear();
            }
        }
    }

    #[test]
    fn ready_bar_uses_content_height_and_wide_buttons_stay_on_the_right() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        for width in [420.0, 980.0] {
            for l in Language::ALL {
                let mut app = NohApp::default();
                app.locale.language = l;
                app.capture_state = Some("ready".into());
                app.wav = Some("music.wav".into());
                app.wav_seconds = Some(30.0);
                let mut info = noh::media::MediaInfo::default();
                info.seconds = 10.0;
                app.clips.push(Clip {
                    id: 1,
                    request_id: 1,
                    item: "clip.mp4".into(),
                    info: Some(info),
                    error: None,
                });
                let request = app.diagnostic_request().unwrap();
                app.diagnosis = Some(Box::new(noh::inspection::Diagnosis {
                    version: 1,
                    level: noh::inspection::Level::Exact,
                    request,
                    snapshot: noh::inspection::Snapshot(vec![]),
                    duration: 30.0,
                    container: "mp4".into(),
                    audio: "aac_320".into(),
                    quick_media: vec![],
                    notes: vec![],
                    plan: None,
                }));
                app.diagnosis_revision = Some(app.revision);
                app.destination_result = Some(app_destination::ResultView {
                    path: "montage.mp4".into(),
                    issue: None,
                    suggestion: None,
                    bytes: None,
                });
                let mut out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 850.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let bounds =
                            Rect::from_min_size(pos2(16.0, 0.0), vec2(width - 32.0, 200.0));
                        let mut child = ui.new_child(
                            egui::UiBuilder::new().id_salt("ready-bar").max_rect(bounds),
                        );
                        let layout =
                            app.operation_bar(&mut child, &ctx, width < style::COMPACT_BELOW);
                        assert!(
                            layout.height + 24.0
                                <= if width < style::COMPACT_BELOW {
                                    112.0
                                } else {
                                    80.0
                                },
                            "{} at {width}: ready bar height {}",
                            l.code(),
                            layout.height + 24.0
                        );
                        assert!(
                            (layout.main_bounds.right() - bounds.right()).abs() < 0.5,
                            "{} at {width}: buttons not right aligned {:?}",
                            l.code(),
                            layout.main_bounds
                        );
                        if width >= style::COMPACT_BELOW {
                            assert!(
                                (layout.main_bounds.top() - bounds.top()).abs() < 0.5,
                                "{}: wide buttons moved below status",
                                l.code()
                            );
                        }
                    },
                );
                out.textures_delta.clear();
                for _ in 0..4 {
                    let mut out = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                pos2(0.0, 0.0),
                                vec2(width, 540.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let height_id = ui.id().with((
                                "operation-height",
                                l.code(),
                                width - 32.0 < style::COMPACT_BELOW,
                            ));
                            app.draw(ui);
                            let (_, height) = ctx
                                .data(|data| data.get_temp::<(f32, f32)>(height_id))
                                .unwrap();
                            assert!(
                                height
                                    <= if width < style::COMPACT_BELOW {
                                        112.0
                                    } else {
                                        80.0
                                    }
                            );
                        },
                    );
                    out.textures_delta.clear();
                }
            }
        }
    }

    #[test]
    fn fixed_bar_fits_compact_window_in_all_languages() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        for l in Language::ALL {
            for state in [
                "empty",
                "analyzing",
                "preview",
                "cancelling",
                "failed",
                "succeeded",
            ] {
                let mut app = NohApp::default();
                app.locale.language = l;
                if state != "empty" {
                    app.wav = Some("music.wav".into());
                    app.wav_seconds = Some(40.0);
                    let mut info = noh::media::MediaInfo::default();
                    info.seconds = 10.0;
                    app.clips.push(Clip {
                        id: 1,
                        request_id: 1,
                        item: "clip.mp4".into(),
                        info: Some(info),
                        error: None,
                    });
                }
                match state {
                    "analyzing" => app.inspecting = true,
                    "cancelling" => {
                        app.cancelling = true;
                        app.fraction = 0.46;
                    }
                    "failed" => {
                        app.error = Some("error.video".into());
                        app.failure_revision = Some(app.revision);
                    }
                    "preview" => {
                        app.preview_artifact = Some(app_state::Artifact {
                            path: "preview.mp4".into(),
                            revision: app.revision,
                            bytes: None,
                        })
                    }
                    "succeeded" => {
                        app.export_artifact = Some(app_state::Artifact {
                            path: "result.mp4".into(),
                            revision: app.revision,
                            bytes: Some(2_000_000),
                        })
                    }
                    _ => {}
                }
                let mut out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 540.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let bounds = Rect::from_min_size(pos2(16.0, 340.0), vec2(388.0, 188.0));
                        let mut child =
                            ui.new_child(egui::UiBuilder::new().id_salt(state).max_rect(bounds));
                        app.operation_bar(&mut child, &ctx, true);
                        assert!(
                            child.min_rect().right() <= bounds.right() + 0.5,
                            "{} {state}: bar overflow {}",
                            l.code(),
                            child.min_rect().right()
                        );
                        assert!(
                            child.min_rect().bottom() <= bounds.bottom() + 0.5,
                            "{} {state}: bar too tall {}",
                            l.code(),
                            child.min_rect().bottom()
                        );
                    },
                );
                out.textures_delta.clear();
            }
        }
    }
    #[test]
    fn cancelling_freezes_the_progress_in_grey() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        // Painted progress fills (6 px high, not the track) and the theme's colours.
        let fills = |app: &mut NohApp| {
            let mut found = (Vec::new(), style::tokens_for(false));
            let mut out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(980.0, 850.0))),
                    ..Default::default()
                },
                |ui| {
                    found.1 = style::tokens(ui);
                    app.operation_bar(ui, &ctx, false);
                },
            );
            out.textures_delta.clear();
            let t = found.1;
            found.0 = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(r)
                        if (r.rect.height() - style::PROGRESS_H).abs() < 0.1
                            && r.fill != t.surface_3 =>
                    {
                        Some((r.rect.width(), r.fill))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            found
        };
        let mut app = NohApp::default();
        app.wav = Some("music.wav".into());
        app.wav_seconds = Some(40.0);
        app.fraction = 0.46;
        app.cancelling = true;
        let (painted, t) = fills(&mut app);
        let track = style::PROGRESS_MAX_W;
        assert_eq!(painted.len(), 1, "{painted:?}");
        assert!((painted[0].0 - track * 0.46).abs() < 1.0, "{painted:?}");
        assert_eq!(painted[0].1, t.text_3);
        assert_ne!(painted[0].1, t.accent);
        // A late worker update does not move the frozen bar.
        app.receive_job_event(app_job::Event::Progress {
            percent: 90,
            phase: noh::engine::Message {
                code: "progress.assemble".into(),
                args: vec![],
            },
        });
        assert_eq!(app.fraction, 0.46);
        // Progress that is not being cancelled keeps the accent.
        app.cancelling = false;
        app.inspecting = true;
        app.analysis_fraction = 0.3;
        let (painted, t) = fills(&mut app);
        assert_eq!(painted.len(), 1, "{painted:?}");
        assert_eq!(painted[0].1, t.accent);
    }
    #[test]
    fn setup_errors_are_visible_before_a_worker_starts() {
        let ctx = egui::Context::default();
        for (wav, output, expected) in [
            (None, None, "error.select_files"),
            (Some("music.wav"), None, "error.choose_output"),
            (Some("music.wav"), Some("output.mkv"), "error.mp4"),
            (Some("music.wav"), Some("output.mp4"), "diagnosis.required"),
        ] {
            let mut app = NohApp {
                revision: 7,
                wav: wav.map(PathBuf::from),
                output: output.map(PathBuf::from),
                failure_path: Some("previous-clip.mp4".into()),
                ..Default::default()
            };
            app.start(&ctx, false);
            let view = app.status();
            assert_eq!(view.headline.key, "ui.failed");
            assert_eq!(view.detail.key, expected);
            assert!(app.job.is_none());
            assert!(app.failure_path.is_none());
            app.invalidated();
            assert_ne!(app.status().headline.key, "ui.failed");
        }
    }
    #[test]
    fn filename_edits_preserve_project_revision_and_preview() {
        let mut app = NohApp::default();
        app.revision = 7;
        app.preview_artifact = Some(app_state::Artifact {
            path: "preview.mp4".into(),
            revision: 7,
            bytes: None,
        });
        for name in ["", "wrong.mov", "another.mp4"] {
            app.output_name = name.into();
            app.destination_edited();
            assert_eq!(app.revision, 7);
            assert_eq!(app.preview_artifact.as_ref().unwrap().revision, 7);
        }
        app.invalidated();
        assert_eq!(app.revision, 8);
        assert!(app.diagnosis_revision.is_none());
        assert_eq!(app.preview_artifact.as_ref().unwrap().revision, 7);
    }

    #[test]
    fn translated_expanded_diagnosis_keeps_scrollbar_inside_window() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        app_locale::install_fonts(&ctx);
        ctx.global_style_mut(|style| {
            style.animation_time = 0.0;
            style.spacing.item_spacing = vec2(8.0, 7.0);
            style.spacing.button_padding = vec2(10.0, 7.0);
        });
        let mut app = NohApp::default();
        app.capture_state = Some("mixed".into());
        app.wav = Some("music.wav".into());
        app.wav_seconds = Some(236.8);
        app.output = Some("music_noh.mp4".into());
        app.warnings = vec![
            "warning.harmonize|1920|1080|29.970".into(),
            "warning.convert_clip|2|3|hevc".into(),
        ];
        let mut info = noh::media::MediaInfo::default();
        info.seconds = 10.0;
        app.clips.push(Clip {
            id: 1,
            request_id: 1,
            item: "A very long video name — 動画 테스트 视频.mp4".into(),
            info: Some(info),
            error: None,
        });
        app.clips.push(Clip {
            id: 2,
            request_id: 2,
            item: noh::input::MediaItem::Image {
                path: "A very long image name — 画像 이미지 图片.png".into(),
                duration: 3.12,
            },
            info: Some(noh::media::MediaInfo::default()),
            error: None,
        });
        let request = app.diagnostic_request().unwrap();
        app.preflight = Some(app_diagnosis::Preflight::dormant(Some(request.clone())));
        app.diagnosis = Some(Box::new(noh::inspection::Diagnosis {
            version: 1,
            level: noh::inspection::Level::Exact,
            request,
            snapshot: noh::inspection::Snapshot(vec![]),
            duration: 236.8,
            container: "mp4".into(),
            audio: "aac_320".into(),
            quick_media: vec![],
            notes: vec!["diagnosis.partial".into(), "diagnosis.color_limit".into()],
            plan: Some(noh::plan::RenderPlan {
                exact_inspection: true,
                stage: "preparation".into(),
                segments: vec![],
                target: noh::plan::Target {
                    width: 1920,
                    height: 1080,
                    rate_num: 30000,
                    rate_den: 1001,
                    codec: "h264".into(),
                    pixel_format: "yuv420p".into(),
                },
                clips: vec![
                    noh::plan::ClipPlan {
                        path: "A very long video name — 動画 테스트 视频.mp4".into(),
                        treatment: noh::plan::Treatment::Convert,
                        reasons: vec!["dimensions".into(), "warning.partial_codec".into()],
                        timestamp_normalization: true,
                        source_seconds: 10.0,
                    },
                    noh::plan::ClipPlan {
                        path: "A very long image name — 画像 이미지 图片.png".into(),
                        treatment: noh::plan::Treatment::Convert,
                        reasons: vec!["image".into()],
                        timestamp_normalization: true,
                        source_seconds: 3.12,
                    },
                ],
            }),
        }));
        app.diagnosis_revision = Some(app.revision);
        let mut failures = Vec::new();
        for completed in [false, true] {
            app.completed = completed.then(|| "music_noh.mp4".into());
            for width in [420.0, 720.0, 980.0] {
                for language in Language::ALL {
                    app.locale.language = language;
                    for pass in 0..3 {
                        let input = egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                pos2(0.0, 0.0),
                                vec2(width, 540.0),
                            )),
                            ..Default::default()
                        };
                        let mut output = ctx.run_ui(input, |ui| {
                            let scroll = app.draw(ui);
                            if pass == 2 && scroll.content_size.x > scroll.inner_rect.width() + 0.5 {
                                failures.push(format!("{} at {width}, completed={completed}: content {} > viewport {}", language.code(), scroll.content_size.x, scroll.inner_rect.width()));
                            }
                            if pass == 2 {
                                assert!(scroll.content_size.y > scroll.inner_rect.height());
                                let bar = ctx.read_response(scroll.id.with(1_usize)).expect("vertical scrollbar");
                                if bar.rect.right() > width + 0.5 { failures.push(format!("{} at {width}, completed={completed}: scrollbar right {}", language.code(), bar.rect.right())); }
                            }
                        });
                        output.textures_delta.clear();
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
