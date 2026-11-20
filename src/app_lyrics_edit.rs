//! The lyrics being edited: the edited track lives in a session beside the
//! attachment (never in `Attachment.loaded`, which the 2 s check may reset),
//! and one undo history covers the lyrics screen's edits (app_lyrics_screen.rs)
//! and the style settings. Exports and the preview read a copy of the edited
//! track written to a temporary folder.
use super::*;
use noh::subtitle_track::SubtitleTrack;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

/// Edits of the attached subtitle file, kept in memory until saved (S4).
pub struct Session {
    /// The subtitle file these edits belong to.
    pub path: PathBuf,
    pub track: Arc<SubtitleTrack>,
    /// The file changed on disk (or went missing) since the edits started.
    pub outside_changed: bool,
    base_revision: u64,
    /// The edited track as a file, for exports and the preview.
    pub file: Option<PathBuf>,
}

#[derive(Clone, PartialEq)]
struct StyleSnap {
    effect: Option<super::app_effects::Choice>,
    caption_style: noh::captions::CaptionStyle,
}

#[derive(Clone)]
struct Snap {
    track: Option<Arc<SubtitleTrack>>,
    style: StyleSnap,
    /// What the step did, in the interface's language (« Annuler : … »); empty
    /// for steps that never named themselves.
    label: String,
}

#[derive(Default)]
pub struct State {
    history: Vec<Snap>,
    redo: Vec<Snap>,
    /// The last step may still grow: (its key, the history length and the
    /// edit revision it left). The lyrics screen types a line, or plays a sync
    /// game, as one undo step.
    run: Option<(u64, usize, u64)>,
    last_style: Option<StyleSnap>,
    style_pushed: Option<Instant>,
    /// The language packs (languages/ beside NOH), read on first use.
    packs: Option<Arc<noh::languages::Packs>>,
}

const HISTORY: usize = 200;
fn temp_folder() -> PathBuf {
    std::env::temp_dir().join(format!("noh-edited-lyrics-{}", std::process::id()))
}

/// The edited track as `<stem>-<n>.srt` (and its working file) in a temporary
/// folder: a new name per write (process-wide), so a running export keeps its file.
fn write_temp(source: &std::path::Path, track: &SubtitleTrack) -> std::io::Result<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    static CLEANED: std::sync::Once = std::sync::Once::new();
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let folder = temp_folder();
    CLEANED.call_once(|| remove_stale_folders(&folder));
    std::fs::create_dir_all(&folder)?;
    prune(&folder, n);
    let stem = source
        .file_stem()
        .map_or("lyrics".into(), |s| s.to_string_lossy().into_owned());
    let path = folder.join(format!("{stem}-{n}.srt"));
    std::fs::write(&path, track.to_srt().map_err(std::io::Error::other)?)?;
    // An export copy: the model that made the lyrics does not matter there.
    noh::subtitle_words::write_lyrics(&noh::subtitle_words::lyrics_path(&path), track, None)?;
    Ok(path)
}

/// Folders left by earlier runs (older than a day) are removed.
fn remove_stale_folders(own: &std::path::Path) {
    let Some(parent) = own.parent() else { return };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let day = Duration::from_secs(24 * 3600);
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("noh-edited-lyrics-"));
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if ours && path != own && old {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Keep the latest copies (a running export may still read a recent one).
fn prune(folder: &std::path::Path, newest: u64) {
    const KEEP: u64 = 20;
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let number = name
            .split('.')
            .next()
            .and_then(|stem| stem.rsplit('-').next())
            .and_then(|n| n.parse::<u64>().ok());
        if number.is_some_and(|k| k + KEEP < newest) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

impl NohApp {
    /// The lyrics shown and exported: the edited track, else the attached one.
    pub(super) fn lyrics_track(&self) -> Option<Arc<SubtitleTrack>> {
        match &self.project.edit {
            Some(s) => Some(s.track.clone()),
            None => self.project.track.loaded.as_ref().map(|l| l.track.clone()),
        }
    }

    pub(super) fn has_lyrics(&self) -> bool {
        self.project.edit.is_some() || self.project.track.loaded.is_some()
    }

    /// Lyrics an export or the preview can use now: edits, or a checked file.
    pub(super) fn lyrics_ready(&self) -> bool {
        // An unanswered difference: the export waits for the answer.
        !self.lyrics_question_pending()
            && (self.project.edit.as_ref().is_some_and(|s| s.file.is_some())
                || (self.project.edit.is_none()
                    && self.project.track.loaded.is_some()
                    && !self.project.track.validating()))
    }

    /// The subtitle file exports and the preview read, with the snapshot that
    /// guards it (lyrics plan 9: always the lyrics). With edits, the edited copy:
    /// the lyrics in memory, written or not. Without, the attached .srt, which
    /// says what the lyrics file says (a difference is answered before any
    /// export, and an unusable lyrics file leaves the .srt as the lyrics); or,
    /// when the .srt is missing (its rewrite failed), a copy of the lyrics in
    /// memory.
    pub(super) fn captions_file(&self) -> Option<(PathBuf, noh::inspection::Snapshot)> {
        let copy = |path: PathBuf| {
            let stamp = noh::inspection::FileStamp::read(&path).ok()?;
            Some((path, noh::inspection::Snapshot(vec![stamp])))
        };
        match &self.project.edit {
            Some(s) => copy(s.file.clone()?),
            None => {
                let loaded = self.project.track.loaded.as_ref()?;
                if loaded.srt_stamp.is_none() {
                    // The copy made once for this load (`ensure_export_copy`).
                    let (loads, path) = self.lyrics_pair.export_copy.clone()?;
                    (loads == self.project.track.loads)
                        .then_some(())
                        .and_then(|()| copy(path))
                } else {
                    Some((self.project.track.path.clone()?, loaded.snapshot.clone()))
                }
            }
        }
    }

    /// With no .srt on disk (a rewrite that failed): one copy of the loaded
    /// lyrics per load, for the export and the preview to read.
    pub(super) fn ensure_export_copy(&mut self) {
        let loads = self.project.track.loads;
        let needed = self.project.edit.is_none()
            && self
                .project
                .track
                .loaded
                .as_ref()
                .is_some_and(|l| l.srt_stamp.is_none());
        if !needed {
            self.lyrics_pair.export_copy = None;
            return;
        }
        if self
            .lyrics_pair
            .export_copy
            .as_ref()
            .is_some_and(|(n, p)| *n == loads && p.exists())
        {
            return;
        }
        let (Some(path), Some(loaded)) = (&self.project.track.path, &self.project.track.loaded)
        else {
            return;
        };
        self.lyrics_pair.export_copy = write_temp(path, &loaded.track).ok().map(|p| (loads, p));
    }

    /// Before an export with lyrics: both files are checked against what NOH
    /// last read or wrote (plan 5; a change on disk is read again and the
    /// export does not start, Err), a pending change is written first (when
    /// NOH may), then what the export's report says about the lyrics it reads.
    pub(super) fn prepare_lyrics_export(&mut self) -> Result<Vec<Message>, ()> {
        if !self.project.apply_subtitles || self.lyrics_question_pending() {
            return Ok(Vec::new());
        }
        if self.project.edit.is_none()
            && let Some(path) = self.project.track.path.clone()
            && self.project.track.loaded.is_some()
            && !self.pair_unchanged(&path)
        {
            self.project.track.recheck();
            return Err(());
        }
        self.flush_lyrics();
        self.ensure_export_copy();
        let mut warnings = Vec::new();
        let unwritten = self
            .project
            .track
            .loaded
            .as_ref()
            .is_some_and(|l| l.lyrics_unwritten.is_some());
        let mark = self.lyrics_save.mark;
        // The lyrics file written and the .srt not: the export reads the
        // lyrics (in memory, the same as the file).
        if mark == Some(super::app_lyrics_save::Mark::SrtStale) {
            warnings.push("lyrics.export_srt_stale".into());
        } else if self.has_unsaved_lyrics()
            || unwritten
            || mark == Some(super::app_lyrics_save::Mark::Unsaved)
        {
            warnings.push("lyrics.export_unsaved".into());
        }
        if matches!(
            self.project.track.loaded.as_ref().map(|l| &l.pairing),
            Some(super::app_track::Pairing::LyricsUnusable { .. })
        ) {
            warnings.push("lyrics.export_unusable".into());
        }
        Ok(warnings)
    }

    fn style_snap(&self) -> StyleSnap {
        StyleSnap {
            effect: self.project.effect.clone(),
            caption_style: self.project.caption_style.clone(),
        }
    }

    fn snap(&self) -> Snap {
        Snap {
            track: self.project.edit.as_ref().map(|s| s.track.clone()),
            style: self.style_snap(),
            label: String::new(),
        }
    }

    fn push_history(&mut self) {
        self.push_history_as(String::new());
    }

    fn push_history_as(&mut self, label: String) {
        let mut snap = self.snap();
        snap.label = label;
        let h = &mut self.lyrics_edit.history;
        h.push(snap);
        if h.len() > HISTORY {
            h.remove(0);
        }
        self.lyrics_edit.redo.clear();
        self.lyrics_edit.style_pushed = None;
    }

    /// Apply `f` to a copy of the shown track; a change becomes one undo step.
    pub(super) fn edit_track(&mut self, f: impl FnOnce(&mut SubtitleTrack) -> bool) -> bool {
        self.edit_track_as(String::new(), f)
    }

    /// `edit_track`, the undo step named `label` (« Annuler : … »).
    pub(super) fn edit_track_as(
        &mut self,
        label: String,
        f: impl FnOnce(&mut SubtitleTrack) -> bool,
    ) -> bool {
        // An unanswered difference: no edit until it is answered. A job that
        // locks the lyrics: no new edit until it ends, whatever asked for it
        // (undo and redo are held back by their callers: the keys here, the
        // lyrics screen's arrows and its game).
        if self.lyrics_question_pending() || self.lyrics_locked() {
            return false;
        }
        let Some(current) = self.lyrics_track() else {
            return false;
        };
        let mut next = (*current).clone();
        if !f(&mut next) || next == *current {
            return false;
        }
        self.push_history_as(label);
        self.set_edited(Some(Arc::new(next)));
        self.lyrics_edited();
        true
    }

    /// `edit_track_as` for an edit that comes in pieces (a line being typed, a
    /// sync game's holds): the pieces with the same `key` are one undo step,
    /// which takes the last piece's label, as long as nothing else changed the
    /// lyrics or the history in between.
    pub(super) fn edit_track_run(
        &mut self,
        key: u64,
        label: String,
        f: impl FnOnce(&mut SubtitleTrack) -> bool,
    ) -> bool {
        let live = self.lyrics_edit.run
            == Some((
                key,
                self.lyrics_edit.history.len(),
                self.project.edit_revision,
            ));
        if !live {
            let done = self.edit_track_as(label, f);
            // Not while the first-edit question may still take the step back.
            self.lyrics_edit.run = (done && !self.lyrics_question_open()).then_some((
                key,
                self.lyrics_edit.history.len(),
                self.project.edit_revision,
            ));
            return done;
        }
        // As `edit_track_as`: an unanswered difference, or a job that locks
        // the lyrics, lets no edit through.
        if self.lyrics_question_pending() || self.lyrics_locked() {
            return false;
        }
        let Some(current) = self.lyrics_track() else {
            return false;
        };
        let mut next = (*current).clone();
        if !f(&mut next) || next == *current {
            return false;
        }
        if let Some(step) = self.lyrics_edit.history.last_mut() {
            step.label = label;
        }
        self.set_edited(Some(Arc::new(next)));
        self.lyrics_edit.run = Some((
            key,
            self.lyrics_edit.history.len(),
            self.project.edit_revision,
        ));
        true
    }

    /// Name the step just made (an edit that learns its name while it applies).
    pub(super) fn name_last_step(&mut self, label: String) {
        if let Some(step) = self.lyrics_edit.history.last_mut() {
            step.label = label;
        }
    }

    /// The step `key` made was left unarmed (a question was open on it): its
    /// next pieces grow it again.
    pub(super) fn rearm_edit_run(&mut self, key: u64) {
        if !self.lyrics_edit.history.is_empty() {
            self.lyrics_edit.run = Some((
                key,
                self.lyrics_edit.history.len(),
                self.project.edit_revision,
            ));
        }
    }

    /// The step in progress is over: the next piece starts a new undo step.
    pub(super) fn end_edit_run(&mut self) {
        self.lyrics_edit.run = None;
    }

    /// What Ctrl+Z and Ctrl+Y would do: None when there is nothing, else the
    /// step's label (empty for a step that has none).
    pub(super) fn history_labels(&self) -> (Option<&str>, Option<&str>) {
        fn label(s: Option<&Snap>) -> Option<&str> {
            s.map(|s| s.label.as_str())
        }
        (
            label(self.lyrics_edit.history.last()),
            label(self.lyrics_edit.redo.last()),
        )
    }

    fn set_edited(&mut self, track: Option<Arc<SubtitleTrack>>) {
        self.project.edit_revision = self.project.edit_revision.wrapping_add(1);
        match track {
            None => self.project.edit = None,
            Some(track) => {
                let path = self.project.track.path.clone().unwrap_or_default();
                let file = write_temp(&path, &track).ok();
                match &mut self.project.edit {
                    Some(s) => {
                        s.track = track;
                        s.file = file;
                    }
                    None => {
                        self.project.edit = Some(Session {
                            path,
                            track,
                            outside_changed: false,
                            base_revision: self.project.track.revision,
                            file,
                        });
                    }
                }
            }
        }
        // Written about 1 s later.
        self.lyrics_changed();
        if self.project.apply_subtitles {
            // A sync game writes each take while the song plays: stopping the
            // player would end the game at its first take. The monitor's
            // pictures are dropped once the game is over (`poll_mini_preview`).
            if self.lyrics_game_running() {
                self.changed_under_playback();
            } else {
                self.invalidated();
            }
        }
    }

    fn restore(&mut self, snap: Snap) {
        // As a pick would: preview-failure state, layer release, export dates.
        self.choose_effect(snap.style.effect.clone());
        self.project.caption_style = snap.style.caption_style.clone();
        self.lyrics_edit.last_style = Some(snap.style);
        self.set_edited(snap.track);
        self.invalidated();
    }

    /// Before a save replaces the file version: history steps meaning "the file"
    /// (no edits) keep the file as it was. Without it (missing file), the
    /// history is cleared rather than left pointing at the saved version.
    pub(super) fn pin_file_version(&mut self, before: Option<Arc<SubtitleTrack>>) {
        let h = &mut self.lyrics_edit;
        match before {
            Some(track) => {
                for snap in h.history.iter_mut().chain(h.redo.iter_mut()) {
                    if snap.track.is_none() {
                        snap.track = Some(track.clone());
                    }
                }
            }
            None => {
                h.history.clear();
                h.redo.clear();
            }
        }
    }

    pub(super) fn undo(&mut self) -> bool {
        let Some(snap) = self.lyrics_edit.history.pop() else {
            return false;
        };
        let mut now = self.snap();
        now.label = snap.label.clone();
        self.lyrics_edit.redo.push(now);
        self.restore(snap);
        true
    }

    /// A job locks the lyrics, or a difference waits for its answer.
    pub(super) fn lyrics_frozen(&self) -> bool {
        self.lyrics_locked() || self.lyrics_question_pending()
    }

    /// The lyrics become `track`, as one undo step, without the first-edit
    /// question (an answer to the source-of-truth question).
    pub(super) fn replace_lyrics(&mut self, track: Arc<SubtitleTrack>) {
        self.push_history();
        self.set_edited(Some(track));
    }

    /// The lyrics shown so far (`shown`) become one undo step, and the
    /// attached lyrics are shown: a lyrics file reloaded after a change on disk.
    pub(super) fn keep_as_undo_step(&mut self, shown: Arc<SubtitleTrack>) {
        let mut snap = self.snap();
        snap.track = Some(shown);
        let h = &mut self.lyrics_edit.history;
        h.push(snap);
        if h.len() > HISTORY {
            h.remove(0);
        }
        self.lyrics_edit.redo.clear();
        self.set_edited(None);
    }

    /// Undo the last edit and forget it (no redo): an edit the first-edit
    /// question was cancelled for.
    pub(super) fn take_back_edit(&mut self) -> bool {
        let undone = self.undo();
        if undone {
            self.lyrics_edit.redo.pop();
        }
        self.lyrics_save.write_at = None;
        undone
    }

    pub(super) fn redo(&mut self) -> bool {
        let Some(snap) = self.lyrics_edit.redo.pop() else {
            return false;
        };
        let mut now = self.snap();
        now.label = snap.label.clone();
        self.lyrics_edit.history.push(now);
        self.restore(snap);
        true
    }

    /// Once per frame: style changes join the same history (a slider drag within
    /// a second is one step), and the session follows its subtitle file.
    pub(super) fn lyrics_edit_frame(&mut self, ctx: &egui::Context) {
        let now = self.style_snap();
        match &self.lyrics_edit.last_style {
            None => self.lyrics_edit.last_style = Some(now),
            Some(previous) if *previous != now => {
                let recent = self
                    .lyrics_edit
                    .style_pushed
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(1));
                if !recent {
                    let snap = Snap {
                        track: self.project.edit.as_ref().map(|s| s.track.clone()),
                        style: previous.clone(),
                        label: String::new(),
                    };
                    self.lyrics_edit.history.push(snap);
                    self.lyrics_edit.redo.clear();
                }
                self.lyrics_edit.style_pushed = Some(Instant::now());
                self.lyrics_edit.last_style = Some(now);
            }
            Some(_) => {}
        }
        if let Some(s) = &mut self.project.edit {
            if self.project.track.path.as_ref() != Some(&s.path) {
                // Another subtitle file (generated, replaced or removed): its edits end.
                self.project.edit = None;
                self.lyrics_edit = Default::default();
                self.lyrics_edit.last_style = Some(self.style_snap());
            } else if self.project.track.revision != s.base_revision {
                // Changed (or gone) outside NOH: the edits stay, and say so.
                s.outside_changed = true;
                s.base_revision = self.project.track.revision;
            }
        }
        // A new load says how the .srt and the lyrics file stand.
        self.lyrics_pair_frame();
        self.lyrics_autosave(ctx);
        // Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z), unless typing.
        // A lyrics question open: no key edits behind it.
        // During a sync game every key is the game's (app_lyrics_screen/game.rs).
        let keys = !ctx.egui_wants_keyboard_input()
            && !self.lyrics_frozen()
            && !self.lyrics_question_open()
            && !self.lyrics_game_running();
        // While an export or a render runs the lyrics are locked: these keys
        // too, so no edit changes the copy a running job checks.
        if keys {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
                self.save_lyrics();
            }
            let (undo, redo) = ctx.input_mut(|i| {
                let redo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
                    || i.consume_key(
                        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                        egui::Key::Z,
                    );
                let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
                (undo, redo)
            });
            if undo {
                self.undo();
            } else if redo {
                self.redo();
            }
        }
    }

    /// The lyrics' lock: a job runs that is not a transcription (which leaves
    /// them free). The lyrics chip's « Modifier », the lyrics lane and the
    /// lyrics screen all follow it.
    pub(super) fn lyrics_locked(&self) -> bool {
        let busy = self.project.busy()
            || self.job.is_some()
            || self.cancelling
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some();
        // A regeneration replaces the track at its end: no edits meanwhile.
        (busy && !self.subtitles.running && !self.project.generating)
            || (self.project.generating && self.has_lyrics())
    }

    /// The language packs, read from languages/ beside NOH on first use.
    /// Tests give the packs instead of reading languages/ beside NOH.
    #[cfg(test)]
    pub(super) fn set_language_packs(&mut self, packs: noh::languages::Packs) {
        self.lyrics_edit.packs = Some(Arc::new(packs));
    }

    pub(super) fn language_packs(&mut self) -> Arc<noh::languages::Packs> {
        self.lyrics_edit
            .packs
            .get_or_insert_with(|| {
                Arc::new(noh::languages::scan(
                    &noh::languages::bundled_dir()
                        .into_iter()
                        .collect::<Vec<_>>(),
                ))
            })
            .clone()
    }

    /// The language the lyrics are sung in: the track's (from the
    /// transcription), else « Langue chantée » when it is not automatic. An
    /// imported .srt has no language of its own.
    pub(super) fn sung_language(&self) -> Option<String> {
        self.lyrics_track()
            .and_then(|t| t.language.clone())
            .filter(|l| !l.is_empty() && l != "auto")
            .or_else(|| {
                (self.subtitles.language != "auto" && !self.subtitles.language.is_empty())
                    .then(|| self.subtitles.language.clone())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noh::subtitle_edit as edit;

    fn app_with_track(folder: &std::path::Path) -> NohApp {
        let mut app = NohApp::default();
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nSomething had to give\n\n2\n00:00:04,000 --> 00:00:06,000\nand nothing disappears\n\n";
        let path = folder.join("song.srt");
        std::fs::write(&path, srt).unwrap();
        let track = noh::subtitle_srt::parse(srt, 20_000).unwrap();
        app.project.track.path = Some(path.clone());
        app.project.track.loaded = Some(app_track::Loaded {
            track: Arc::new(track),
            snapshot: noh::inspection::Snapshot(vec![
                noh::inspection::FileStamp::read(&path).unwrap(),
            ]),
            model: None,
            lyrics_unwritten: None,
            srt_stamp: noh::inspection::FileStamp::read(&path).ok(),
            lyrics_stamp: None,
            pairing: Default::default(),
        });
        app.project.apply_subtitles = true;
        app
    }

    #[test]
    fn edits_live_in_a_session_and_undo_restores_the_file_version() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let file = app.project.track.loaded.as_ref().unwrap().track.clone();
        let revision = app.revision;
        assert!(app.edit_track(|t| edit::set_text(t, 0, "Something has to give")));
        assert_ne!(
            app.revision, revision,
            "an edit of shown lyrics invalidates"
        );
        assert_eq!(
            app.lyrics_track().unwrap().cues[0].text,
            "Something has to give"
        );
        assert_eq!(
            app.project.track.loaded.as_ref().unwrap().track,
            file,
            "the attachment keeps the file version"
        );
        // Exports read the edited copy, guarded by its own snapshot.
        let (path, snapshot) = app.captions_file().unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("Something has to give")
        );
        snapshot.verify().unwrap();
        assert!(app.undo());
        assert!(app.project.edit.is_none());
        assert_eq!(app.lyrics_track().unwrap(), file);
        assert!(app.redo());
        assert_eq!(
            app.lyrics_track().unwrap().cues[0].text,
            "Something has to give"
        );
    }

    #[test]
    fn style_changes_share_the_undo_history_with_lyrics_edits() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let ctx = egui::Context::default();
        app.lyrics_edit_frame(&ctx);
        app.edit_track(|t| edit::move_cue(t, 0, 200) != 0);
        app.project.caption_style.size = noh::captions::CaptionSize::Large;
        app.lyrics_edit_frame(&ctx);
        assert!(app.undo(), "first the size");
        assert_eq!(
            app.project.caption_style.size,
            noh::captions::CaptionSize::Medium
        );
        assert_eq!(app.lyrics_track().unwrap().cues[0].start_ms, 1200);
        assert!(app.undo(), "then the move");
        assert_eq!(app.lyrics_track().unwrap().cues[0].start_ms, 1000);
    }

    #[test]
    fn an_outside_change_keeps_the_edits_and_another_file_ends_them() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let ctx = egui::Context::default();
        app.edit_track(|t| edit::delete(t, 1).is_some());
        // The 2 s check reloads a changed file: the session is not touched.
        app.project.track.revision = app.project.track.revision.wrapping_add(1);
        app.project.track.loaded = None;
        app.lyrics_edit_frame(&ctx);
        assert!(app.project.edit.as_ref().unwrap().outside_changed);
        assert_eq!(app.lyrics_track().unwrap().cues.len(), 1);
        assert!(app.lyrics_ready(), "the edits stay exportable");
        // A different subtitle file ends them.
        app.project.track.path = Some(folder.path().join("other.srt"));
        app.lyrics_edit_frame(&ctx);
        assert!(app.project.edit.is_none());
    }

    #[test]
    fn history_keys_wait_while_the_lyrics_are_locked() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.edit_track(|t| edit::move_cue(t, 0, 100) != 0);
        app.cancelling = true;
        assert!(app.lyrics_locked());
        let file = app.project.edit.as_ref().unwrap().file.clone();
        frame(&mut app, key(egui::Key::Z, egui::Modifiers::COMMAND).into());
        assert_eq!(
            app.lyrics_track().unwrap().cues[0].start_ms,
            1100,
            "no undo while locked"
        );
        assert_eq!(
            app.project.edit.as_ref().unwrap().file,
            file,
            "the copy a job checks is untouched"
        );
    }

    #[test]
    fn a_regeneration_over_existing_lyrics_locks_the_lane() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        assert!(!app.lyrics_locked());
        app.project.generating = true;
        assert!(
            app.lyrics_locked(),
            "edits made now would be replaced by the new track"
        );
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> [egui::Event; 2] {
        [true, false].map(|pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        })
    }

    fn frame(app: &mut NohApp, events: Vec<egui::Event>) {
        let ctx = egui::Context::default();
        ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| app.lyrics_edit_frame(ui.ctx()),
        )
        .textures_delta
        .clear();
    }
}
