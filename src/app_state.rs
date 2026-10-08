//! Pure status and action decisions. No filesystem calls or renderer state.
use super::{app_destination::Issue, app_style::Severity};
use noh::i18n::Message;
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub struct Artifact {
    pub path: PathBuf,
    pub revision: u64,
    pub bytes: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Idle,
    Running { preview: bool },
    Cancelling { preview: bool },
    Transcribing,
    CancellingSubtitles,
    Burning { preview: bool },
    CancellingCaptions { preview: bool },
    Shortening { preview: bool },
    CancellingShorts { preview: bool },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Analysis {
    Missing,
    Pending,
    Current,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextAction {
    Analyze,
    CancelAnalysis,
    /// Open the last export, video or short.
    OpenVideo,
    OpenFolder,
    Remove(u64),
    /// Replace an unreadable clip at the same position.
    Replace(u64),
    /// Open the attached lyrics in the external editor.
    ReviewLyrics,
    /// Pick a .srt after a generation found no lyrics.
    ImportLyrics,
    Suggest,
    ChooseEngine,
    /// "Change…": the location sheet.
    ChooseFolder,
    OpenSubtitle,
    OpenSubtitleFolder,
    PlayCaption,
    OpenCaption,
    OpenCaptionFolder,
    SuggestCaption,
    PlayShort,
    OpenShort,
    OpenShortFolder,
    SuggestShort,
}
#[derive(Clone, Debug)]
pub struct Availability {
    pub enabled: bool,
    pub reason: Message,
}
/// What only the montage bar shows.
#[derive(Clone, Debug, Default)]
pub struct MontageBar {
    /// "Export the short", only while a range exists.
    pub short: Option<Availability>,
    /// A name the person chose is taken: Export opens the sheet with the
    /// conflict instead of exporting.
    pub conflict: bool,
    pub short_conflict: bool,
    /// the last export is current. The export buttons turn secondary,
    /// and compact shows "Open the video" in their place.
    pub done: bool,
    /// Line 2 shows this file instead of the pre-filled destination.
    pub path: Option<PathBuf>,
    /// Line 2 offers "Change…".
    pub change: bool,
    /// Tag after line 2.
    pub tag: Option<Message>,
}
#[derive(Clone, Debug)]
pub struct StatusView {
    /// Line 2 shows the export destination (with "Change…" when idle).
    pub destination: bool,
    pub severity: Severity,
    pub headline: Message,
    pub detail: Message,
    pub third: Option<Message>,
    pub export: Availability,
    pub preview: Availability,
    pub actions: Vec<ContextAction>,
    pub operation: Operation,
    pub montage: Option<MontageBar>,
}
/// The most recent export of this project, video or short.
#[derive(Clone, Copy, Debug)]
pub struct LastExport<'a> {
    pub path: &'a Path,
    pub current: bool,
    pub short: bool,
}
pub struct Inputs<'a> {
    pub operation: Operation,
    pub inputs_complete: bool,
    pub missing_song: bool,
    pub missing_visuals: bool,
    /// A refused drop or selection, until the next edit.
    pub drop_error: Option<&'a Message>,
    /// The lyrics generation failed, until the next edit.
    pub lyrics_failure: Option<&'a Message>,
    /// File name of lyrics just generated.
    pub lyrics_ready: Option<String>,
    /// That generation found no cue: say so instead of "ready".
    pub lyrics_empty: bool,
    pub reading: bool,
    pub unreadable: Option<u64>,
    pub wav_unreadable: bool,
    pub fades_valid: bool,
    pub durations_valid: bool,
    pub analysis: Analysis,
    /// Why the check is not current (failed or cancelled).
    pub analysis_cause: Option<&'a Message>,
    pub destination_issue: Option<Issue>,
    pub destination_pending: bool,
    pub suggestion: bool,
    /// The file name is automatic: a taken name gives way to the first free
    /// one instead of asking (one-click export).
    pub auto_name: bool,
    /// A range exists: the short's export and its destination.
    pub range: bool,
    pub short_issue: Option<Issue>,
    pub short_pending: bool,
    pub short_auto: bool,
    pub last_export: Option<LastExport<'a>>,
    pub failure: Option<&'a Message>,
    pub notice: Option<&'a Message>,
    pub outcome: Option<&'a Message>,
    pub phase: &'a Message,
    pub fraction: f32,
}
fn availability(reason: Option<Message>) -> Availability {
    Availability {
        enabled: reason.is_none(),
        reason: reason.unwrap_or_else(|| "ui.available".into()),
    }
}
/// A destination's reason to disable its export button. A taken automatic
/// name is about to give way to a free one (still checking); a taken name the
/// person chose opens the sheet instead, so it is no reason.
fn destination_reason(issue: Option<&Issue>, pending: bool, auto: bool) -> Option<Message> {
    match issue {
        Some(Issue::Exists) if auto => Some("ui.destination_checking".into()),
        Some(Issue::Exists) => None,
        Some(issue) => Some(issue.key().into()),
        None => pending.then(|| "ui.destination_checking".into()),
    }
}
/// The bar's rows in priority order; the first match wins.
pub fn status_view(s: Inputs<'_>) -> StatusView {
    let idle = s.operation == Operation::Idle;
    let common_reason: Option<Message> = if !idle {
        Some("ui.locked".into())
    } else if !s.inputs_complete {
        Some(
            if s.missing_song && !s.missing_visuals {
                "bar.missing_song_help"
            } else if s.missing_visuals && !s.missing_song {
                "bar.missing_visuals_help"
            } else {
                "ui.missing_inputs"
            }
            .into(),
        )
    } else if s.unreadable.is_some() || s.wav_unreadable {
        Some("ui.unreadable_help".into())
    } else if s.reading {
        Some("status.reading".into())
    } else if !s.fades_valid {
        Some("error.fades".into())
    } else if !s.durations_valid {
        Some("error.image_duration".into())
    } else {
        None
    };
    let conflict = !s.auto_name && s.destination_issue == Some(Issue::Exists);
    let export = availability(
        common_reason
            .clone()
            .or_else(|| (s.analysis != Analysis::Current).then(|| "ui.analysis_required".into()))
            .or_else(|| {
                destination_reason(
                    s.destination_issue.as_ref(),
                    s.destination_pending,
                    s.auto_name,
                )
            }),
    );
    // The short's pipeline does not use the montage check.
    let short =
        s.range.then(|| {
            availability(common_reason.clone().or_else(|| {
                destination_reason(s.short_issue.as_ref(), s.short_pending, s.short_auto)
            }))
        });
    let mut bar = MontageBar {
        short,
        conflict,
        short_conflict: s.range && !s.short_auto && s.short_issue == Some(Issue::Exists),
        ..Default::default()
    };
    let mut actions = Vec::new();
    let mut destination = false;
    let (severity, headline, detail) = match s.operation {
        Operation::Cancelling { .. }
        | Operation::CancellingSubtitles
        | Operation::CancellingCaptions { .. }
        | Operation::CancellingShorts { .. } => (
            Severity::Warning,
            "ui.cancel_requested".into(),
            "ui.cancel_wait".into(),
        ),
        Operation::Running { preview }
        | Operation::Burning { preview }
        | Operation::Shortening { preview } => {
            destination = matches!(s.operation, Operation::Running { preview: false });
            (
                Severity::Info,
                Message::new(
                    if preview {
                        "ui.preview_running"
                    } else {
                        "ui.export_running"
                    },
                    &[format!("{}", (s.fraction * 100.0).round() as u32)],
                ),
                s.phase.clone(),
            )
        }
        Operation::Transcribing => (Severity::Info, "bar.generating".into(), s.phase.clone()),
        Operation::Idle => {
            if let Some(failure) = s.failure {
                if let Some(id) = s.unreadable {
                    actions.push(ContextAction::Remove(id));
                }
                if failure.key == "error.ffmpeg" || failure.key == "error.start" {
                    actions.push(ContextAction::ChooseEngine);
                }
                if matches!(
                    failure.key.as_str(),
                    "error.output_folder" | "error.output_exists"
                ) {
                    actions.push(ContextAction::ChooseFolder);
                }
                (Severity::Error, "ui.failed".into(), failure.clone())
            } else if let Some(refused) = s.drop_error {
                (Severity::Error, "drop.refused".into(), refused.clone())
            } else if let Some(failure) = s.lyrics_failure {
                (Severity::Error, "bar.lyrics_failed".into(), failure.clone())
            } else if !s.inputs_complete {
                let (headline, detail) = if s.missing_song && !s.missing_visuals {
                    ("bar.missing_song", "bar.missing_song_help")
                } else if s.missing_visuals && !s.missing_song {
                    ("bar.missing_visuals", "bar.missing_visuals_help")
                } else {
                    ("ui.incomplete", "ui.missing_inputs")
                };
                (Severity::Info, headline.into(), detail.into())
            } else if s.reading {
                (
                    Severity::Info,
                    "status.reading".into(),
                    "ui.reading_help".into(),
                )
            } else if let Some(id) = s.unreadable {
                actions.extend([ContextAction::Remove(id), ContextAction::Replace(id)]);
                (
                    Severity::Error,
                    "ui.unreadable".into(),
                    "ui.unreadable_help".into(),
                )
            } else if s.wav_unreadable {
                (
                    Severity::Error,
                    "bar.unreadable_song".into(),
                    "bar.unreadable_song_help".into(),
                )
            } else if !s.fades_valid {
                (Severity::Error, "error.fades".into(), "ui.fade_help".into())
            } else if !s.durations_valid {
                (
                    Severity::Error,
                    "error.image_duration".into(),
                    "ui.duration_help".into(),
                )
            } else if s.analysis == Analysis::Pending {
                // "Video 2 of 3", from the check's own progress.
                actions.push(ContextAction::CancelAnalysis);
                let step = if s.phase.key == "progress.analyze_clip" {
                    Message::new("bar.checking_step", &s.phase.args)
                } else {
                    "diagnosis.reading".into()
                };
                (Severity::Info, "bar.checking".into(), step)
            } else if s.analysis != Analysis::Current {
                actions.push(ContextAction::Analyze);
                (
                    Severity::Info,
                    "bar.check_needed".into(),
                    s.analysis_cause
                        .cloned()
                        .unwrap_or_else(|| "ui.analysis_required".into()),
                )
            } else if let Some(last) = s.last_export.filter(|l| l.current) {
                // open what was just made; exporting again stays possible.
                actions.extend([ContextAction::OpenVideo, ContextAction::OpenFolder]);
                destination = true;
                bar.done = true;
                bar.path = Some(last.path.to_path_buf());
                (
                    Severity::Success,
                    if last.short {
                        "bar.done_short"
                    } else {
                        "bar.done_video"
                    }
                    .into(),
                    "ui.current_settings".into(),
                )
            } else if conflict {
                // a name the person chose is taken; never replaced silently.
                if s.suggestion {
                    actions.push(ContextAction::Suggest);
                }
                destination = true;
                bar.change = true;
                (
                    Severity::Info,
                    "bar.name_taken".into(),
                    "error.output_exists".into(),
                )
            } else if let Some(issue) = s
                .destination_issue
                .as_ref()
                .filter(|i| **i != Issue::Exists)
            {
                actions.push(ContextAction::ChooseFolder);
                (Severity::Warning, "ui.fix_name".into(), issue.key().into())
            } else if s.lyrics_ready.is_some() && s.lyrics_empty {
                // An instrumental or unrecognized voice is not a success.
                actions.push(ContextAction::ImportLyrics);
                (
                    Severity::Info,
                    "bar.lyrics_none".into(),
                    "bar.lyrics_none_help".into(),
                )
            } else if let Some(name) = &s.lyrics_ready {
                actions.push(ContextAction::ReviewLyrics);
                (
                    Severity::Success,
                    Message::new("bar.lyrics_ready", std::slice::from_ref(name)),
                    "lyrics.review_help".into(),
                )
            } else if let Some(last) = s.last_export {
                // the previous export, tagged; Export writes a new file.
                actions.push(ContextAction::OpenVideo);
                destination = true;
                bar.path = Some(last.path.to_path_buf());
                bar.tag = Some("bar.old_tag".into());
                (
                    Severity::Info,
                    "bar.stale".into(),
                    "ui.current_settings".into(),
                )
            } else {
                destination = true;
                bar.change = true;
                (
                    Severity::Success,
                    "bar.ready".into(),
                    "ui.current_settings".into(),
                )
            }
        }
    };
    let third = if !idle {
        Some("ui.locked".into())
    } else {
        s.notice.cloned().or_else(|| s.outcome.cloned())
    };
    StatusView {
        destination,
        severity,
        headline,
        detail,
        third,
        preview: export.clone(),
        export,
        actions,
        operation: s.operation,
        montage: Some(bar),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn inputs<'a>(phase: &'a Message) -> Inputs<'a> {
        Inputs {
            operation: Operation::Idle,
            inputs_complete: true,
            missing_song: false,
            missing_visuals: false,
            drop_error: None,
            lyrics_failure: None,
            lyrics_ready: None,
            lyrics_empty: false,
            reading: false,
            unreadable: None,
            wav_unreadable: false,
            fades_valid: true,
            durations_valid: true,
            analysis: Analysis::Current,
            analysis_cause: None,
            destination_issue: None,
            destination_pending: false,
            suggestion: false,
            auto_name: true,
            range: false,
            short_issue: None,
            short_pending: false,
            short_auto: true,
            last_export: None,
            failure: None,
            notice: None,
            outcome: None,
            phase,
            fraction: 0.0,
        }
    }
    fn bar(v: &StatusView) -> &MontageBar {
        v.montage.as_ref().unwrap()
    }

    #[test]
    fn disabled_exports_have_translated_reasons_across_capture_states() {
        use noh::i18n::Language;
        let phase = "progress.assemble".into();
        for state in [
            "empty",
            "partial",
            "song-only",
            "ready",
            "lyrics-menu",
            "lyrics-generating",
            "lyrics-track-menu",
            "short",
            "exporting",
            "done",
            "stale",
            "exists",
            "unreadable",
            "analyzing",
            "cancelling",
            "details",
            "options",
            "reading",
            "invalid-fades",
            "invalid-duration",
        ] {
            let mut s = inputs(&phase);
            s.range = true;
            match state {
                "empty" | "partial" | "song-only" => {
                    s.inputs_complete = false;
                    s.missing_song = state != "song-only";
                    s.missing_visuals = state != "partial";
                }
                "lyrics-generating" => s.operation = Operation::Transcribing,
                "exporting" => s.operation = Operation::Running { preview: false },
                "cancelling" => s.operation = Operation::Cancelling { preview: false },
                "unreadable" => s.unreadable = Some(2),
                "analyzing" => s.analysis = Analysis::Pending,
                "reading" => s.reading = true,
                "invalid-fades" => s.fades_valid = false,
                "invalid-duration" => s.durations_valid = false,
                "exists" => {
                    s.auto_name = false;
                    s.destination_issue = Some(Issue::Exists);
                }
                "done" | "stale" => {
                    s.last_export = Some(LastExport {
                        path: Path::new("video.mp4"),
                        current: state == "done",
                        short: false,
                    })
                }
                _ => {}
            }
            let view = status_view(s);
            for availability in [&view.export, bar(&view).short.as_ref().unwrap()] {
                if !availability.enabled {
                    assert!(!availability.reason.key.is_empty(), "{state}");
                    for language in Language::ALL {
                        let text = availability.reason.render(language);
                        assert!(!text.trim().is_empty(), "{state} {language:?}");
                        assert_ne!(text, availability.reason.key, "{state} {language:?}");
                    }
                }
            }
        }
    }
    /// Generating lyrics, a failed generation and
    /// lyrics just generated, each in its place in the priority order.
    #[test]
    fn lyrics_rows_follow_the_bar_priority() {
        let phase = "subtitle.preparing".into();
        let mut s = inputs(&phase);
        s.operation = Operation::Transcribing;
        let v = status_view(s);
        assert_eq!(v.headline.key, "bar.generating");
        assert_eq!(v.detail.key, "subtitle.preparing");
        assert!(!v.export.enabled);
        let failed: Message = "error.speech".into();
        let mut s = inputs(&phase);
        s.lyrics_failure = Some(&failed);
        s.lyrics_ready = Some("song.srt".into());
        let v = status_view(s);
        assert_eq!(v.headline.key, "bar.lyrics_failed");
        assert_eq!(v.detail.key, "error.speech");
        assert!(
            v.export.enabled,
            "a failed generation does not block the export"
        );
        let mut s = inputs(&phase);
        s.lyrics_ready = Some("song.srt".into());
        let v = status_view(s);
        assert_eq!(v.headline.key, "bar.lyrics_ready");
        assert_eq!(v.headline.args, ["song.srt"]);
        assert_eq!(v.detail.key, "lyrics.review_help");
        assert_eq!(v.actions, [ContextAction::ReviewLyrics]);
        assert!(v.export.enabled);
        // A generation that found nothing says so, and offers an import.
        let mut s = inputs(&phase);
        s.lyrics_ready = Some("song.srt".into());
        s.lyrics_empty = true;
        let v = status_view(s);
        assert_eq!(v.headline.key, "bar.lyrics_none");
        assert_eq!(v.detail.key, "bar.lyrics_none_help");
        assert_eq!(v.severity, Severity::Info);
        assert_eq!(v.actions, [ContextAction::ImportLyrics]);
        assert!(v.export.enabled);
        // Missing inputs rank above it.
        let mut s = inputs(&phase);
        s.lyrics_ready = Some("song.srt".into());
        s.inputs_complete = false;
        s.missing_visuals = true;
        assert_eq!(status_view(s).headline.key, "bar.missing_visuals");
    }

    /// Each status condition gives its headline, its
    /// contextual actions and the export buttons' state.
    #[test]
    fn status_rows_follow_the_design_table() {
        let phase: Message = "status.ready".into();
        let failure: Message = "error.ffmpeg".into();
        let cancelled: Message = "diagnosis.cancelled".into();
        let video = Path::new("C:/Videos/song_noh.mp4");
        let short = Path::new("C:/Videos/song_noh-short.mp4");
        type Case = (&'static str, fn(&mut Inputs<'_>));
        let cases: Vec<Case> = vec![
            ("ui.cancel_requested", |s| {
                s.operation = Operation::Cancelling { preview: false }
            }),
            ("ui.export_running", |s| {
                s.operation = Operation::Running { preview: false }
            }),
            ("bar.generating", |s| s.operation = Operation::Transcribing),
            ("bar.missing_song", |s| {
                s.inputs_complete = false;
                s.missing_song = true;
            }),
            ("bar.missing_visuals", |s| {
                s.inputs_complete = false;
                s.missing_visuals = true;
            }),
            ("status.reading", |s| s.reading = true),
            ("ui.unreadable", |s| s.unreadable = Some(7)),
            ("bar.unreadable_song", |s| s.wav_unreadable = true),
            ("error.fades", |s| s.fades_valid = false),
            ("bar.checking", |s| s.analysis = Analysis::Pending),
            ("bar.check_needed", |s| s.analysis = Analysis::Cancelled),
            ("bar.name_taken", |s| {
                s.auto_name = false;
                s.destination_issue = Some(Issue::Exists);
                s.suggestion = true;
            }),
            ("bar.ready", |_| {}),
        ];
        for (expected, apply) in cases {
            let mut s = inputs(&phase);
            apply(&mut s);
            let running = s.operation != Operation::Idle;
            let v = status_view(s);
            assert_eq!(v.headline.key, expected);
            if running {
                assert!(!v.export.enabled && v.export.reason.key == "ui.locked");
            }
        }
        // a relevant failure with its recovery; exports disabled only by it.
        let mut s = inputs(&phase);
        s.failure = Some(&failure);
        let v = status_view(s);
        assert_eq!(v.headline.key, "ui.failed");
        assert_eq!(v.actions, [ContextAction::ChooseEngine]);
        // "Video 2 of 3", Stop checking; export waits for the check.
        let step = Message::new("progress.analyze_clip", &["2".into(), "3".into()]);
        let mut s = inputs(&step);
        s.analysis = Analysis::Pending;
        let v = status_view(s);
        assert_eq!(v.detail.key, "bar.checking_step");
        assert_eq!(v.detail.args, ["2", "3"]);
        assert_eq!(v.actions, [ContextAction::CancelAnalysis]);
        assert_eq!(v.export.reason.key, "ui.analysis_required");
        // the cause, and Check again.
        let mut s = inputs(&phase);
        s.analysis = Analysis::Cancelled;
        s.analysis_cause = Some(&cancelled);
        let v = status_view(s);
        assert_eq!(v.detail.key, "diagnosis.cancelled");
        assert_eq!(v.actions, [ContextAction::Analyze]);
        assert!(!v.export.enabled);
        // the short just exported; open it, export buttons secondary.
        let mut s = inputs(&phase);
        s.range = true;
        s.last_export = Some(LastExport {
            path: short,
            current: true,
            short: true,
        });
        let v = status_view(s);
        assert_eq!(v.headline.key, "bar.done_short");
        assert_eq!(
            v.actions,
            [ContextAction::OpenVideo, ContextAction::OpenFolder]
        );
        assert!(bar(&v).done && bar(&v).path.as_deref() == Some(short));
        assert!(v.export.enabled && bar(&v).short.as_ref().unwrap().enabled);
        // a chosen name that is taken; Export stays enabled and asks.
        let mut s = inputs(&phase);
        s.auto_name = false;
        s.destination_issue = Some(Issue::Exists);
        s.suggestion = true;
        let v = status_view(s);
        assert_eq!(v.actions, [ContextAction::Suggest]);
        assert!(v.export.enabled && bar(&v).conflict && bar(&v).change);
        // previous export tagged, Open the video quiet, export normal.
        let mut s = inputs(&phase);
        s.last_export = Some(LastExport {
            path: video,
            current: false,
            short: false,
        });
        let v = status_view(s);
        assert_eq!(v.headline.key, "bar.stale");
        assert_eq!(v.actions, [ContextAction::OpenVideo]);
        assert_eq!(bar(&v).tag.as_ref().unwrap().key, "bar.old_tag");
        assert!(v.export.enabled && !bar(&v).done);
        // destination with Change…; the short button only with a range.
        let v = status_view(inputs(&phase));
        assert!(v.destination && bar(&v).change && bar(&v).short.is_none());
    }

    /// A taken automatic name is about to give way to a free one: the export
    /// waits for it instead of asking; other name problems disable it.
    #[test]
    fn automatic_names_wait_and_invalid_names_block() {
        let phase = "status.ready".into();
        let mut s = inputs(&phase);
        s.destination_issue = Some(Issue::Exists);
        let v = status_view(s);
        assert_eq!(v.headline.key, "bar.ready");
        assert_eq!(v.export.reason.key, "ui.destination_checking");
        assert!(!bar(&v).conflict);
        let mut s = inputs(&phase);
        s.destination_issue = Some(Issue::Invalid);
        let v = status_view(s);
        assert_eq!(v.headline.key, "ui.fix_name");
        assert_eq!(v.actions, [ContextAction::ChooseFolder]);
        assert_eq!(v.export.reason.key, "ui.name_invalid");
        // The short has its own destination and needs no montage check.
        let mut s = inputs(&phase);
        s.range = true;
        s.analysis = Analysis::Missing;
        s.short_auto = false;
        s.short_issue = Some(Issue::Exists);
        let v = status_view(s);
        assert!(!v.export.enabled);
        assert!(bar(&v).short.as_ref().unwrap().enabled && bar(&v).short_conflict);
    }

    #[test]
    fn invalid_image_duration_blocks_export_with_a_specific_reason() {
        let phase = "status.ready".into();
        let mut s = inputs(&phase);
        s.durations_valid = false;
        s.range = true;
        let v = status_view(s);
        assert!(!v.export.enabled);
        assert_eq!(v.headline.key, "error.image_duration");
        assert_eq!(v.export.reason.key, "error.image_duration");
        assert_eq!(
            bar(&v).short.as_ref().unwrap().reason.key,
            "error.image_duration"
        );
    }
    #[test]
    fn cancellation_is_not_an_outcome_until_terminal_event() {
        let phase = "status.ready".into();
        let mut s = inputs(&phase);
        s.operation = Operation::Cancelling { preview: false };
        let v = status_view(s);
        assert_eq!(v.headline.key, "ui.cancel_requested");
        assert!(!v.export.enabled);
    }
}
