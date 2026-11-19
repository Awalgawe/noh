//! The lyrics lane as the editor (fused mockup v9, Task 41 S3): the edited track
//! lives in a session beside the attachment (never in `Attachment.loaded`, which
//! the 2 s check may reset), one undo history covers lane edits and style
//! settings, and the bubble above the chosen cue edits it. Exports and the
//! preview read a copy of the edited track written to a temporary folder.
use super::*;
use app_style::{self as style, space};
use egui::{Rect, vec2};
use noh::{
    subtitle_edit::{self as edit, Edge},
    subtitle_track::{SubtitleCue, SubtitleTrack, WordSource, WordTime},
};
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
    /// The cue whose bubble is open.
    pub selected: Option<usize>,
    /// The bubble's text while it is being typed: (cue, text).
    draft: Option<(usize, String)>,
    history: Vec<Snap>,
    redo: Vec<Snap>,
    /// The last step may still grow: (its key, the history length and the
    /// edit revision it left). The lyrics screen types a line, or plays a sync
    /// game, as one undo step.
    run: Option<(u64, usize, u64)>,
    last_style: Option<StyleSnap>,
    style_pushed: Option<Instant>,
    /// "Subtitle deleted · Undo" until this time.
    toast: Option<Instant>,
    /// « Caler les mots » on the chosen cue (S5).
    pub words: Option<WordMode>,
    /// A button of the bubble had the focus last frame: word mode keys still work.
    bubble_focus: bool,
    /// The language packs (languages/ beside NOH), read on first use.
    packs: Option<Arc<noh::languages::Packs>>,
    /// Tests stand in for the live player's clock while tapping.
    #[cfg(test)]
    test_clock: Option<u64>,
}

/// Word mode: the view zooms onto the line, its words are placed by hand.
pub struct WordMode {
    pub cue: usize,
    /// The chosen word (index in the cue).
    pub word: usize,
    pub details: bool,
    /// The view before the zoom, given back at the end.
    view: Option<noh::timeline::Viewport>,
    /// While tapping in rhythm: the song time of each tap so far.
    pub taps: Option<Vec<u64>>,
    /// Playback has started since « Caler en rythme »; the player's time of the last frame.
    pub tap_clock: Option<u64>,
    /// Where « Caler en rythme » sought: playback counts as started only near it.
    tap_target: u64,
    /// Why the last tapping ended without a change (a locale key).
    notice: Option<&'static str>,
    /// The syllable split being typed for a word: (word, text).
    syl_draft: Option<(usize, String)>,
    /// Why the typed split was refused (a locale key).
    syl_error: Option<&'static str>,
    /// Where the last proposed split came from (shown under the field).
    /// The last proposal: (word, its syllables, the note). The note shows only
    /// while that word still has exactly those syllables (not after Ctrl+Z, a
    /// typed split, or a text edit that moved the word).
    syl_source: Option<(usize, Vec<String>, String)>,
}

/// The player may publish its old position once more just after a seek (a
/// tick already running, playback.rs): playback counts as started only on a
/// time near the seek target.
fn near_target(now: u64, target: u64) -> bool {
    now + 250 >= target && now <= target + 1500
}

/// While tapping: playback went 2 s past the line's end, jumped back (the
/// excerpt loops) or stopped. Tapping then ends, and nothing is applied.
fn tap_overrun(now: Option<u64>, last: u64, end_ms: u64) -> bool {
    match now {
        None => true,
        Some(now) => now > end_ms + 2000 || now + 250 < last,
    }
}

/// A cue's units (words, or syllables of a word cut into syllables): (word,
/// syllable, start, text, source), the letter split when it has no word times.
fn cue_units(cue: &SubtitleCue) -> Vec<(usize, Option<usize>, u64, String, WordSource)> {
    let words = edit::words_or_estimate(cue);
    let mut probe = cue.clone();
    probe.words = Some(words.clone());
    edit::units(&probe)
        .into_iter()
        .map(|(wi, si, ms)| {
            let (text, source) = match (si, &words[wi].syllables) {
                (Some(k), Some(s)) => (s[k].text.clone(), s[k].source),
                _ => (words[wi].text.clone(), words[wi].source),
            };
            (wi, si, ms, text, source)
        })
        .collect()
}

/// The start of each unit of a cue (capture states).
pub(super) fn unit_starts(cue: &SubtitleCue) -> Vec<u64> {
    cue_units(cue).into_iter().map(|u| u.2).collect()
}

/// The proposal's note for word `wi`, while its syllables are still the proposed ones.
fn syl_note<'a>(w: &'a WordMode, wi: usize, word: &WordTime) -> Option<&'a str> {
    let (k, parts, note) = w.syl_source.as_ref()?;
    let current = word.syllables.as_ref()?;
    (*k == wi
        && current
            .iter()
            .map(|s| s.text.as_str())
            .eq(parts.iter().map(String::as_str)))
    .then_some(note.as_str())
}

fn source_key(source: WordSource) -> &'static str {
    match source {
        WordSource::Whisper => "words.src_whisper",
        WordSource::Hand => "words.src_hand",
        WordSource::Estimated => "words.src_estimated",
    }
}

/// What the word-mode bubble asked for, applied after it is drawn.
enum WordAction {
    Tap,
    Play,
    Details,
    Done,
    Word(usize),
    Nudge(usize, i64),
    Hit,
    Stop,
    /// The syllable field: typed, applied (Entrée or leaving it), or « Réunir en un mot ».
    SylType(usize, String),
    SylApply(usize, String),
    SylJoin(usize),
    /// « Couper en syllabes »: the language's proposal.
    SylPropose(usize),
}

const HISTORY: usize = 200;
const TOAST: Duration = Duration::from_secs(8);

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
        // An unanswered difference: no edit until it is answered.
        if self.lyrics_question_pending() {
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
        // As `edit_track_as`: an unanswered difference lets no edit through.
        if self.lyrics_question_pending() {
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
        if let Some(i) = self.lyrics_edit.selected
            && self.lyrics_track().is_none_or(|t| i >= t.cues.len())
        {
            self.lyrics_edit.selected = None;
        }
        self.lyrics_edit.draft = None;
        if self.project.apply_subtitles {
            self.invalidated();
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

    /// The lane is locked, or a difference waits for its answer.
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
                self.end_words();
                self.lyrics_edit = Default::default();
                self.lyrics_edit.last_style = Some(self.style_snap());
            } else if self.project.track.revision != s.base_revision {
                // Changed (or gone) outside NOH: the edits stay, and say so.
                s.outside_changed = true;
                s.base_revision = self.project.track.revision;
            }
        }
        // Word mode lives on its chosen cue, while the lane is free.
        if let Some(w) = &self.lyrics_edit.words
            && (self.lyrics_edit.selected != Some(w.cue)
                || self.lyrics_track().is_none_or(|t| w.cue >= t.cues.len())
                || self.lyrics_locked())
        {
            self.end_words();
        }
        // A new load says how the .srt and the lyrics file stand.
        self.lyrics_pair_frame();
        self.lyrics_autosave(ctx);
        // Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z), Suppr on the chosen cue, unless typing.
        let focus_free = ctx.memory(|m| m.focused().is_none());
        // A lyrics question open: no key edits behind it.
        let keys = !ctx.egui_wants_keyboard_input()
            && !self.lyrics_frozen()
            && !self.lyrics_question_open();
        if keys {
            self.word_keys(ctx, focus_free || self.lyrics_edit.bubble_focus);
        }
        // While an export or a render runs the lane is locked (app_project_ui.rs):
        // its keys too, so no edit changes the copy a running job checks.
        if keys {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
                self.save_lyrics();
            }
            let (undo, redo, delete) = ctx.input_mut(|i| {
                let redo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
                    || i.consume_key(
                        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                        egui::Key::Z,
                    );
                let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
                // Suppr belongs to the chosen cue only when no widget has the focus
                // (a focused clip tile, for one, deletes itself).
                let delete = self.lyrics_edit.selected.is_some()
                    && focus_free
                    && i.consume_key(egui::Modifiers::NONE, egui::Key::Delete);
                (undo, redo, delete)
            });
            if undo {
                self.undo();
            } else if redo {
                self.redo();
            }
            if delete && let Some(i) = self.lyrics_edit.selected {
                self.delete_cue(i);
            }
        }
    }

    /// The lane's lock, as the timeline zone gets it: a job runs that is not a
    /// transcription (which leaves the lane free).
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

    /// Word mode keys. Tapping: each Space press (not its repeat) is a tap,
    /// whatever has the focus, Échap stops. Otherwise ← → choose the word,
    /// Alt+← → move its start by 0.05 s, Échap ends word mode.
    fn word_keys(&mut self, ctx: &egui::Context, focus_free: bool) {
        self.watch_taps();
        let Some(w) = &self.lyrics_edit.words else {
            return;
        };
        let cue = w.cue;
        if w.taps.is_some() {
            let (taps, stop) = ctx.input_mut(|i| {
                let mut taps = 0;
                i.events.retain(|e| match e {
                    egui::Event::Key {
                        key: egui::Key::Space,
                        pressed,
                        repeat,
                        ..
                    } => {
                        taps += usize::from(*pressed && !*repeat);
                        false
                    }
                    _ => true,
                });
                (
                    taps,
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            for _ in 0..taps {
                self.tap();
            }
            if stop {
                self.stop_tap();
            }
            return;
        }
        if !focus_free {
            return;
        }
        // Alt first: a plain-arrow pattern also matches Alt+arrow (egui ignores extra Alt).
        let (earlier, later, left, right, done) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft),
                i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        let count = self
            .lyrics_track()
            .and_then(|t| t.cues.get(cue).map(|c| edit::words_or_estimate(c).len()))
            .unwrap_or(0);
        if let Some(w) = &mut self.lyrics_edit.words {
            if left {
                w.word = w.word.saturating_sub(1);
            }
            if right && w.word + 1 < count {
                w.word += 1;
            }
        }
        if (earlier || later)
            && let Some(word) = self.lyrics_edit.words.as_ref().map(|w| w.word)
        {
            if let Some(k) = self
                .lyrics_track()
                .and_then(|t| cue_units(&t.cues[cue]).iter().position(|u| u.0 == word))
            {
                self.nudge_unit(k, if later { 50 } else { -50 });
            }
        }
        if done {
            self.end_words();
        }
    }

    /// « Caler les mots »: the view zooms onto the line, 0.6 s on each side.
    pub(super) fn start_words(&mut self, i: usize) {
        self.commit_draft();
        let Some(cue) = self.lyrics_track().and_then(|t| t.cues.get(i).cloned()) else {
            return;
        };
        let view = self.project.viewport;
        let duration = self.project.duration_ms;
        let start = cue.start_ms.saturating_sub(600);
        let end = (cue.end_ms + 600).min(duration);
        if let Some(range) = noh::timeline::Range::place(start, end.saturating_sub(start), duration)
        {
            self.project.viewport = Some(noh::timeline::Viewport(range));
        }
        self.lyrics_edit.words = Some(WordMode {
            cue: i,
            word: 0,
            details: false,
            view,
            taps: None,
            tap_clock: None,
            tap_target: 0,
            notice: None,
            syl_draft: None,
            syl_error: None,
            syl_source: None,
        });
    }

    /// « Terminer » or Échap: the view from before the zoom comes back.
    pub(super) fn end_words(&mut self) {
        if let Some(w) = self.lyrics_edit.words.take() {
            if w.taps.is_some() {
                self.mini_pause();
            }
            self.project.viewport = w.view;
        }
    }

    /// Move unit `k` of the word-mode cue by `delta` ms, by hand.
    fn nudge_unit(&mut self, k: usize, delta: i64) {
        let Some(cue) = self.lyrics_edit.words.as_ref().map(|w| w.cue) else {
            return;
        };
        let Some(from) = self
            .lyrics_track()
            .and_then(|t| cue_units(&t.cues[cue]).get(k).map(|u| u.2))
        else {
            return;
        };
        let to = from.saturating_add_signed(delta);
        self.edit_track(|t| edit::set_unit_start(t, cue, k, to).is_some());
    }

    /// The live player's time while it plays; None while it opens or is paused.
    fn tap_time(&self) -> Option<u64> {
        #[cfg(test)]
        if self.lyrics_edit.test_clock.is_some() {
            return self.lyrics_edit.test_clock;
        }
        self.mini_playing().then(|| self.mini_position())
    }

    /// Each frame while tapping: note when playback has started, and end the
    /// tapping (nothing applied) once it overruns the line or stops.
    fn watch_taps(&mut self) {
        let Some(w) = &self.lyrics_edit.words else {
            return;
        };
        // A staged capture state has no player: it keeps its tapping.
        if w.taps.is_none() || self.capture_overlay.is_some() {
            return;
        }
        let (cue, last) = (w.cue, w.tap_clock);
        let Some(end) = self
            .lyrics_track()
            .and_then(|t| t.cues.get(cue).map(|c| c.end_ms))
        else {
            return;
        };
        let now = self.tap_time();
        let target = w.tap_target;
        match last {
            // Still opening (or a stale time from before the seek): wait.
            None if !now.is_some_and(|now| near_target(now, target)) => {}
            Some(last) if tap_overrun(now, last, end) => {
                self.stop_tap();
                if let Some(w) = &mut self.lyrics_edit.words {
                    w.notice = Some("words.tap_dropped");
                }
            }
            _ => {
                if let Some(w) = &mut self.lyrics_edit.words {
                    w.tap_clock = now;
                }
            }
        }
    }

    /// « Caler en rythme »: the line plays from 1 s before its start.
    fn start_tap(&mut self, ctx: &egui::Context) {
        let Some(w) = &mut self.lyrics_edit.words else {
            return;
        };
        w.taps = Some(Vec::new());
        w.tap_clock = None;
        w.notice = None;
        let cue = w.cue;
        if let Some(start) = self
            .lyrics_track()
            .and_then(|t| t.cues.get(cue).map(|c| c.start_ms))
        {
            // mini_seek keeps the time inside what the preview plays.
            let (lo, hi) = self.mini_bounds();
            let target = start
                .saturating_sub(1000)
                .clamp(lo, hi.saturating_sub(1).max(lo));
            if let Some(w) = &mut self.lyrics_edit.words {
                w.tap_target = target;
            }
            self.mini_play_from(target, ctx);
        }
    }

    /// One tap, at the player's time; ignored until playback has started. The
    /// last one applies every tap at once (one undo step) and pauses.
    fn tap(&mut self) {
        let Some(at) = self.tap_time() else { return };
        let Some(w) = &mut self.lyrics_edit.words else {
            return;
        };
        if w.tap_clock.is_none() {
            return;
        }
        let (cue, Some(taps)) = (w.cue, &mut w.taps) else {
            return;
        };
        taps.push(at);
        let taps = taps.clone();
        let total = self
            .lyrics_track()
            .and_then(|t| t.cues.get(cue).map(|c| cue_units(c).len()))
            .unwrap_or(0);
        if taps.len() >= total {
            let applied = self.edit_track(|t| edit::apply_taps(t, cue, &taps));
            self.mini_pause();
            if let Some(w) = &mut self.lyrics_edit.words {
                w.taps = None;
                w.notice = (!applied).then_some("words.tap_failed");
            }
        }
    }

    /// The split typed for word `wi` of cue `i` ("" joins it back). A split
    /// that drops or adds letters, or has no room, changes nothing and says why.
    /// The language packs, read from languages/ beside NOH on first use.
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

    /// « Couper en syllabes »: the language pack's proposal for word `wi` of
    /// cue `i`, applied as one edit, then shown in the field for correction.
    pub(super) fn propose_syllables(&mut self, i: usize, wi: usize) {
        let Some(word) = self.lyrics_track().and_then(|t| {
            t.cues
                .get(i)
                .and_then(|c| edit::words_or_estimate(c).get(wi).cloned())
        }) else {
            return;
        };
        let language = self.sung_language();
        let packs = self.language_packs();
        let (parts, source, dictionary) = packs.propose(&word.text, language.as_deref());
        let l = self.locale.language;
        let note = match source {
            noh::syllables::Source::Script => l.text("words.syl_from_script").to_string(),
            noh::syllables::Source::Dictionary => l.format(
                "words.syl_from_dictionary",
                &[dictionary.unwrap_or_default()],
            ),
            noh::syllables::Source::Rule => l.text("words.syl_from_rule").to_string(),
        };
        let error = if parts.len() < 2 {
            Some("words.syl_one")
        } else {
            (!self.edit_track(|t| edit::set_syllables(t, i, wi, &parts)))
                .then_some("words.syl_room")
        };
        if let Some(w) = &mut self.lyrics_edit.words {
            w.syl_draft = None;
            w.syl_error = error;
            w.syl_source = error.is_none().then_some((wi, parts, note));
        }
    }

    pub(super) fn apply_syllables(&mut self, i: usize, wi: usize, typed: &str) {
        let Some(word) = self.lyrics_track().and_then(|t| {
            t.cues
                .get(i)
                .and_then(|c| edit::words_or_estimate(c).get(wi).cloned())
        }) else {
            return;
        };
        let parts = if typed.is_empty() {
            Some(Vec::new())
        } else {
            edit::parse_syllables(typed, &word.text)
        };
        let error = match parts {
            None => Some("words.syl_letters"),
            // Unchanged (one part on a whole word, or the same split): nothing to do.
            Some(p) if p.len() < 2 && word.syllables.is_none() => None,
            Some(p)
                if word.syllables.as_ref().is_some_and(|s| {
                    s.iter()
                        .map(|s| s.text.as_str())
                        .eq(p.iter().map(String::as_str))
                }) =>
            {
                None
            }
            Some(p) => (!self.edit_track(|t| edit::set_syllables(t, i, wi, &p)))
                .then_some("words.syl_room"),
        };
        if let Some(w) = &mut self.lyrics_edit.words {
            w.syl_draft = None;
            w.syl_error = error;
            w.syl_source = None;
        }
    }

    /// Arrêter or Échap: nothing is applied.
    fn stop_tap(&mut self) {
        if let Some(w) = &mut self.lyrics_edit.words {
            w.taps = None;
        }
        self.mini_pause();
    }

    /// The word-mode bubble: « Caler les mots », or « Caler en rythme » while tapping.
    fn words_body(&mut self, ui: &mut egui::Ui, cue: &SubtitleCue) -> (Option<WordAction>, bool) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let Some(w) = &self.lyrics_edit.words else {
            return (None, false);
        };
        let clock = |ms| app_timeline::clock_cs(ms, l);
        let units = cue_units(cue);
        let mut action = None;
        let mut focus = false;
        let small = |text: String, color| {
            egui::RichText::new(text)
                .size(style::text::SMALL)
                .color(color)
        };
        if let Some(taps) = &w.taps {
            ui.label(egui::RichText::new(l.text("words.tap")).strong());
            ui.add(egui::Label::new(small(l.text("words.tap_help").into(), t.text_2)).wrap());
            ui.horizontal_wrapped(|ui| {
                ui.label(l.format(
                    "words.tap_count",
                    &[taps.len().to_string(), units.len().to_string()],
                ));
                if let Some(next) = units.get(taps.len()) {
                    ui.label(small(
                        l.format("words.tap_next", &[next.3.clone()]),
                        t.text_2,
                    ));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let stop = style::button(ui, l.text("words.tap_stop"), true, false, 0.0);
                    // Playback is opening until its clock runs: no taps yet.
                    let hit = style::button(
                        ui,
                        l.text("words.tap_key"),
                        w.tap_clock.is_some(),
                        true,
                        0.0,
                    );
                    focus |= stop.has_focus() || hit.has_focus();
                    if hit.clicked() {
                        action = Some(WordAction::Hit);
                    }
                    if stop.clicked() {
                        action = Some(WordAction::Stop);
                    }
                });
            });
            ui.add(egui::Label::new(small(l.text("words.tap_note").into(), t.text_3)).wrap());
            return (action, focus);
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(l.text("words.title")).strong());
            ui.label(small(
                format!(
                    "{} · {} – {}",
                    cue.text.replace('\n', " / "),
                    clock(cue.start_ms),
                    clock(cue.end_ms)
                ),
                t.text_2,
            ));
        });
        ui.horizontal_wrapped(|ui| {
            let (lo, hi) = self.mini_bounds();
            let playable = cue.start_ms >= lo && cue.end_ms <= hi;
            let tap = style::button(ui, l.text("words.tap"), playable, false, 0.0);
            let tap = if playable {
                tap
            } else {
                tap.on_disabled_hover_text(l.text("words.tap_outside"))
            };
            let play = style::button(ui, l.text("words.play"), true, false, 0.0);
            let details = ui.add(
                egui::Button::selectable(w.details, l.text("words.details"))
                    .min_size(vec2(0.0, style::CONTROL_H)),
            );
            let done = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    style::button(ui, l.text("words.done"), true, true, 0.0)
                })
                .inner;
            focus |= [&tap, &play, &details, &done].iter().any(|r| r.has_focus());
            if tap.clicked() {
                action = Some(WordAction::Tap);
            }
            if play.clicked() {
                action = Some(WordAction::Play);
            }
            if details.clicked() {
                action = Some(WordAction::Details);
            }
            if done.clicked() {
                action = Some(WordAction::Done);
            }
        });
        if let Some(notice) = w.notice {
            ui.add(egui::Label::new(small(l.text(notice).into(), t.warn)).wrap());
        }
        if w.details {
            let words = edit::words_or_estimate(cue);
            ui.horizontal_wrapped(|ui| {
                for (wi, word) in words.iter().enumerate() {
                    let text = word.syllables.as_ref().map_or(word.text.clone(), |s| {
                        s.iter()
                            .map(|s| s.text.as_str())
                            .collect::<Vec<_>>()
                            .join("·")
                    });
                    let label = format!(
                        "{text}\n{}  {}",
                        clock(word.start_ms),
                        l.text(source_key(word.source))
                    );
                    let card = ui.add(egui::Button::selectable(wi == w.word, label));
                    focus |= card.has_focus();
                    if card.clicked() {
                        action = Some(WordAction::Word(wi));
                    }
                }
            });
            ui.separator();
            let step = l.decimal(0.05, 2);
            for (k, u) in units.iter().enumerate().filter(|(_, u)| u.0 == w.word) {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        vec2(84.0, style::CONTROL_H),
                        egui::Label::new(u.3.clone()).truncate(),
                    );
                    ui.label(clock(u.2));
                    let minus = style::button(ui, format!("−{step} s"), true, false, 0.0)
                        .on_hover_text(l.format("words.minus_tip", &[step.clone()]));
                    let plus = style::button(ui, format!("+{step} s"), true, false, 0.0)
                        .on_hover_text(l.format("words.plus_tip", &[step.clone()]));
                    focus |= minus.has_focus() || plus.has_focus();
                    if minus.clicked() {
                        action = Some(WordAction::Nudge(k, -50));
                    }
                    if plus.clicked() {
                        action = Some(WordAction::Nudge(k, 50));
                    }
                    ui.label(small(l.text(source_key(u.4)).into(), t.text_3));
                });
            }
        }
        if w.details
            && let Some(word) = edit::words_or_estimate(cue).get(w.word).cloned()
        {
            // Syllables, typed with hyphens (the proposed split waits for the maintainer's choice).
            let wi = w.word;
            let shown = match &w.syl_draft {
                Some((k, text)) if *k == wi => text.clone(),
                _ => word.syllables.as_ref().map_or(word.text.clone(), |s| {
                    s.iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join("-")
                }),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(small(l.text("words.syl_label").into(), t.text_2));
                let mut text = shown.clone();
                let field = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .id(egui::Id::new(("syllables", wi)))
                        .desired_width(160.0),
                );
                if field.changed() {
                    action = Some(WordAction::SylType(wi, text.clone()));
                }
                let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if field.lost_focus() && !escape {
                    // Leaving the field unchanged applies nothing.
                    if text != shown {
                        action = Some(WordAction::SylApply(wi, text));
                    } else {
                        action = Some(WordAction::SylType(wi, String::new()));
                    }
                } else if field.lost_focus() {
                    action = Some(WordAction::SylType(wi, String::new()));
                }
                if word.syllables.is_none() {
                    let cut = style::button(ui, l.text("words.syl_cut"), true, false, 0.0);
                    focus |= cut.has_focus();
                    if cut.clicked() {
                        action = Some(WordAction::SylPropose(wi));
                    }
                }
                if word.syllables.is_some() {
                    let join = style::button(ui, l.text("words.syl_join"), true, false, 0.0);
                    focus |= join.has_focus();
                    if join.clicked() {
                        action = Some(WordAction::SylJoin(wi));
                    }
                }
            });
            if let Some(source) = syl_note(w, wi, &word) {
                ui.label(small(source.to_string(), t.text_2));
            }
            ui.label(small(l.text("words.syl_help").into(), t.text_3));
            if let Some(error) = w.syl_error {
                ui.add(egui::Label::new(small(l.text(error).into(), t.warn)).wrap());
            }
        }
        ui.add(
            egui::Label::new(small(
                l.format("words.hint", &[l.decimal(0.05, 2)]),
                t.text_3,
            ))
            .wrap(),
        );
        (action, focus)
    }

    fn delete_cue(&mut self, i: usize) {
        if self.edit_track(|t| edit::delete(t, i).is_some()) {
            self.lyrics_edit.selected = None;
            self.lyrics_edit.toast = Some(Instant::now() + TOAST);
        }
    }

    /// A lane gesture from the timeline.
    /// Typed text that differs from the cue is kept when the bubble is left.
    fn commit_draft(&mut self) {
        if let Some((i, text)) = self.lyrics_edit.draft.take() {
            let text = text.trim().to_string();
            let changed = self
                .lyrics_track()
                .and_then(|t| t.cues.get(i).map(|c| c.text != text))
                .unwrap_or(false);
            if changed && !text.is_empty() {
                self.edit_track(|t| edit::set_text(t, i, &text));
            }
        }
    }

    pub(super) fn lane_event(&mut self, event: app_timeline::CueEvent) {
        use app_timeline::{CueEvent, CuePart};
        if !matches!(event, CueEvent::Select(i) if self.lyrics_edit.selected == Some(i)) {
            self.commit_draft();
        }
        match event {
            CueEvent::Select(i) => {
                if self.lyrics_edit.selected != Some(i) {
                    self.lyrics_edit.draft = None;
                    self.end_words();
                }
                self.lyrics_edit.selected = Some(i);
            }
            CueEvent::Clear => {
                self.end_words();
                self.lyrics_edit.selected = None;
                self.lyrics_edit.draft = None;
            }
            CueEvent::Word(wi) => {
                if let Some(w) = &mut self.lyrics_edit.words {
                    w.word = wi;
                }
            }
            CueEvent::UnitStart { cue, unit, to_ms } => {
                self.edit_track(|t| edit::set_unit_start(t, cue, unit, to_ms).is_some());
                let word = self
                    .lyrics_track()
                    .and_then(|t| cue_units(&t.cues[cue]).get(unit).map(|u| u.0));
                if let (Some(w), Some(word)) = (&mut self.lyrics_edit.words, word) {
                    w.word = word;
                }
            }
            CueEvent::Drag {
                cue,
                part,
                delta_ms,
                to_ms,
            } => {
                self.edit_track(|t| match part {
                    CuePart::Start => {
                        edit::set_edge(t, cue, Edge::Start, to_ms) != t.cues[cue].start_ms || true
                    }
                    CuePart::End => edit::set_edge(t, cue, Edge::End, to_ms) != 0,
                    CuePart::Body => edit::move_cue(t, cue, delta_ms) != 0,
                });
                self.lyrics_edit.selected = Some(cue);
            }
            CueEvent::Add(ms) => {
                let text = self.locale.language.text("edit.new_text").to_string();
                let mut at = None;
                self.edit_track(|t| {
                    at = edit::add_cue(t, ms, &text);
                    at.is_some()
                });
                if let Some(i) = at {
                    self.lyrics_edit.selected = Some(i);
                    self.lyrics_edit.draft = Some((i, String::new()));
                }
            }
        }
    }

    /// The bubble above the chosen cue, and the delete toast.
    pub(super) fn lyrics_bubble(
        &mut self,
        ui: &mut egui::Ui,
        cue_rect: Option<Rect>,
        lane: Option<Rect>,
    ) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let ctx = ui.ctx().clone();
        if let Some(until) = self.lyrics_edit.toast {
            if Instant::now() >= until {
                self.lyrics_edit.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
                let at = lane.map_or(ctx.content_rect().center_bottom() - vec2(0.0, 120.0), |r| {
                    r.center_bottom() + vec2(0.0, 12.0)
                });
                egui::Area::new(egui::Id::new("lyrics-toast"))
                    .order(egui::Order::Foreground)
                    .pivot(egui::Align2::CENTER_TOP)
                    .fixed_pos(at)
                    .show(&ctx, |ui| {
                        style::popup_frame(ui).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(l.text("edit.deleted"));
                                if style::button(ui, l.text("edit.undo"), true, false, 0.0)
                                    .clicked()
                                {
                                    self.undo();
                                    self.lyrics_edit.toast = None;
                                }
                            });
                        });
                    });
            }
        }
        let (Some(i), Some(track), Some(cue_rect)) =
            (self.lyrics_edit.selected, self.lyrics_track(), cue_rect)
        else {
            return;
        };
        let Some(cue) = track.cues.get(i).cloned() else {
            self.lyrics_edit.selected = None;
            return;
        };
        let width = 560.0_f32.min(ctx.content_rect().width() - 32.0);
        let x = (cue_rect.center().x - 60.0)
            .clamp(16.0, (ctx.content_rect().width() - width - 16.0).max(16.0));
        let mut close = false;
        let mut escape = false;
        let mut commit: Option<String> = None;
        let mut action: Option<&str> = None;
        let words_mode = self.lyrics_edit.words.as_ref().is_some_and(|w| w.cue == i);
        let mut word_action = None;
        let mut bubble_focus = false;
        egui::Area::new(egui::Id::new("lyrics-bubble"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(egui::pos2(x, cue_rect.top() - 8.0))
            .show(&ctx, |ui| {
                style::popup_frame(ui).show(ui, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = space::S;
                    if words_mode {
                        (word_action, bubble_focus) = self.words_body(ui, &cue);
                        return;
                    }
                    let draft = match &mut self.lyrics_edit.draft {
                        Some((k, text)) if *k == i => text,
                        _ => {
                            self.lyrics_edit.draft = Some((i, cue.text.clone()));
                            &mut self.lyrics_edit.draft.as_mut().unwrap().1
                        }
                    };
                    let edit_id = egui::Id::new(("lyrics-text", i));
                    // Enter validates, Shift+Enter breaks the line, Échap closes
                    // without saving what was typed.
                    let enter =
                        ui.input(|inp| inp.key_pressed(egui::Key::Enter) && !inp.modifiers.shift);
                    if ui.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                    {
                        escape = true;
                    }
                    let response = ui.add(
                        egui::TextEdit::multiline(draft)
                            .id(edit_id)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text(l.text("edit.text_label"))
                            .return_key(Some(egui::KeyboardShortcut::new(
                                egui::Modifiers::SHIFT,
                                egui::Key::Enter,
                            ))),
                    );
                    if response.has_focus() && enter {
                        commit = Some(draft.clone());
                    }
                    if response.lost_focus() && *draft != cue.text && !escape {
                        commit = Some(draft.clone());
                    }
                    let clock = |ms| app_timeline::clock(ms, self.project.duration_ms, l);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {}   {} {}",
                            l.text("edit.start"),
                            clock(cue.start_ms),
                            l.text("edit.end"),
                            clock(cue.end_ms)
                        ))
                        .size(style::text::SMALL)
                        .color(t.text_2),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if style::button(ui, l.text("edit.play"), true, false, 0.0).clicked() {
                            action = Some("play");
                        }
                        let ph = self.mini_preview.cursor_ms;
                        let inside = ph > cue.start_ms + edit::MIN_CUE_MS
                            && ph + edit::MIN_CUE_MS < cue.end_ms;
                        let words = cue.text.split_whitespace().count() > 1;
                        let can_cut = inside && words;
                        let cut = style::button(ui, l.text("edit.cut"), can_cut, false, 0.0);
                        if !can_cut {
                            cut.on_disabled_hover_text(l.text(if words {
                                "edit.cut_disabled"
                            } else {
                                "edit.cut_one_word"
                            }));
                        } else if cut.clicked() {
                            action = Some("cut");
                        }
                        if style::button(
                            ui,
                            l.text("edit.join"),
                            i + 1 < track.cues.len(),
                            false,
                            0.0,
                        )
                        .clicked()
                        {
                            action = Some("join");
                        }
                        if style::button(ui, l.text("words.title"), true, false, 0.0).clicked() {
                            action = Some("words");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if style::button_kind(
                                ui,
                                l.text("edit.delete"),
                                true,
                                style::Kind::Quiet,
                                0.0,
                                style::CONTROL_H,
                            )
                            .clicked()
                            {
                                action = Some("delete");
                            }
                        });
                    });
                    if self
                        .project
                        .edit
                        .as_ref()
                        .is_some_and(|s| s.outside_changed)
                    {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(l.text("edit.outside"))
                                    .size(style::text::SMALL)
                                    .color(t.warn),
                            )
                            .wrap(),
                        );
                    }
                    // Where the word marks come from.
                    let note = match &cue.words {
                        None => Some("words.note_none"),
                        Some(w) if w.iter().any(|w| w.source == WordSource::Whisper) => {
                            Some("words.note_whisper")
                        }
                        Some(_) => None,
                    };
                    if let Some(note) = note {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(l.text(note))
                                    .size(style::text::SMALL)
                                    .color(t.text_2),
                            )
                            .wrap(),
                        );
                    }
                    ui.label(
                        egui::RichText::new(l.text("edit.hint"))
                            .size(style::text::SMALL)
                            .color(t.text_3),
                    );
                });
            });
        self.lyrics_edit.bubble_focus = bubble_focus;
        match word_action {
            Some(WordAction::Tap) => self.start_tap(&ctx),
            Some(WordAction::Play) => self.mini_play_from(cue.start_ms, &ctx),
            Some(WordAction::Details) => {
                if let Some(w) = &mut self.lyrics_edit.words {
                    w.details = !w.details;
                }
            }
            Some(WordAction::Done) => self.end_words(),
            Some(WordAction::Word(wi)) => {
                if let Some(w) = &mut self.lyrics_edit.words {
                    w.word = wi;
                }
            }
            Some(WordAction::Nudge(k, delta)) => self.nudge_unit(k, delta),
            Some(WordAction::Hit) => self.tap(),
            Some(WordAction::Stop) => self.stop_tap(),
            Some(WordAction::SylType(wi, text)) => {
                if let Some(w) = &mut self.lyrics_edit.words {
                    // An empty text drops the draft (Échap).
                    w.syl_draft = (!text.is_empty()).then_some((wi, text));
                    w.syl_error = None;
                }
            }
            Some(WordAction::SylApply(wi, text)) => self.apply_syllables(i, wi, &text),
            Some(WordAction::SylJoin(wi)) => self.apply_syllables(i, wi, ""),
            Some(WordAction::SylPropose(wi)) => self.propose_syllables(i, wi),
            None => {}
        }
        if escape {
            close = true;
        }
        if let Some(text) = commit {
            let text = text.trim().to_string();
            if !text.is_empty() && text != cue.text {
                self.edit_track(|t| edit::set_text(t, i, &text));
            }
            self.lyrics_edit.draft = None;
            if ctx.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                close = true;
            }
        }
        match action {
            // From the cue's start, playing or not (mini_play would pause).
            Some("play") => self.mini_play_from(cue.start_ms, &ctx),
            Some("cut") => {
                let ph = self.mini_preview.cursor_ms;
                self.edit_track(|t| edit::split_at(t, i, ph));
            }
            Some("join") => {
                self.edit_track(|t| edit::join_next(t, i));
            }
            Some("delete") => self.delete_cue(i),
            Some("words") => self.start_words(i),
            _ => {}
        }
        if close {
            self.lyrics_edit.selected = None;
            self.lyrics_edit.draft = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn style_changes_share_the_undo_history_with_lane_edits() {
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
    fn text_typed_in_the_bubble_is_kept_when_another_cue_is_chosen() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.lane_event(app_timeline::CueEvent::Select(0));
        app.lyrics_edit.draft = Some((0, "Something has to give".into()));
        app.lane_event(app_timeline::CueEvent::Select(1));
        assert_eq!(
            app.lyrics_track().unwrap().cues[0].text,
            "Something has to give"
        );
        app.lyrics_edit.draft = Some((1, "and all disappears".into()));
        app.lane_event(app_timeline::CueEvent::Clear);
        assert_eq!(
            app.lyrics_track().unwrap().cues[1].text,
            "and all disappears"
        );
    }

    #[test]
    fn suppr_leaves_the_cue_alone_while_another_widget_has_the_focus() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.lyrics_edit.selected = Some(0);
        let ctx = egui::Context::default();
        let delete = |pressed| egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        let frame = |app: &mut NohApp, events: Vec<egui::Event>, focus: bool| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    if focus {
                        ui.memory_mut(|m| m.request_focus(egui::Id::new("clip-tile")));
                    }
                    app.lyrics_edit_frame(ui.ctx());
                },
            )
            .textures_delta
            .clear();
        };
        frame(&mut app, vec![], true);
        frame(&mut app, vec![delete(true), delete(false)], true);
        assert_eq!(
            app.lyrics_track().unwrap().cues.len(),
            2,
            "a focused tile keeps its Delete key"
        );
        frame(&mut app, vec![], false);
        ctx.run_ui(Default::default(), |ui| {
            ui.memory_mut(|m| m.surrender_focus(egui::Id::new("clip-tile")))
        })
        .textures_delta
        .clear();
        frame(&mut app, vec![delete(true), delete(false)], false);
        assert_eq!(
            app.lyrics_track().unwrap().cues.len(),
            1,
            "with no focus, Suppr deletes the chosen cue"
        );
    }

    #[test]
    fn history_keys_and_suppr_wait_while_the_lane_is_locked() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.edit_track(|t| edit::move_cue(t, 0, 100) != 0);
        app.lyrics_edit.selected = Some(1);
        app.cancelling = true;
        assert!(app.lyrics_locked());
        let file = app.project.edit.as_ref().unwrap().file.clone();
        let ctx = egui::Context::default();
        let key = |key, modifiers| {
            [true, false].map(|pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            })
        };
        let mut events: Vec<egui::Event> = key(egui::Key::Z, egui::Modifiers::COMMAND).into();
        events.extend(key(egui::Key::Delete, egui::Modifiers::NONE));
        ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| app.lyrics_edit_frame(ui.ctx()),
        )
        .textures_delta
        .clear();
        assert_eq!(
            app.lyrics_track().unwrap().cues.len(),
            2,
            "no delete while locked"
        );
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

    #[test]
    fn word_mode_zooms_onto_the_line_and_gives_the_view_back() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.project.duration_ms = 20_000;
        let before = app.project.viewport;
        app.lane_event(app_timeline::CueEvent::Select(0));
        app.start_words(0);
        let cue = app.lyrics_track().unwrap().cues[0].clone();
        let view = app.project.viewport.unwrap().0;
        assert_eq!(
            (view.start_ms, view.end_ms),
            (cue.start_ms.saturating_sub(600), cue.end_ms + 600)
        );
        frame(
            &mut app,
            key(egui::Key::Escape, egui::Modifiers::NONE).into(),
        );
        assert!(
            app.lyrics_edit.words.is_none() && app.lyrics_edit.selected == Some(0),
            "Échap ends word mode, the bubble stays"
        );
        assert_eq!(app.project.viewport, before);
        // Choosing another cue ends it too.
        app.start_words(0);
        app.lane_event(app_timeline::CueEvent::Select(1));
        assert!(app.lyrics_edit.words.is_none());
        assert_eq!(app.project.viewport, before);
    }

    #[test]
    fn arrows_choose_a_word_and_alt_arrows_move_its_start_by_hand() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        // An allowed file: no first-edit question holds the keys.
        app.trust_lyrics(folder.path().join("song.srt"));
        app.lane_event(app_timeline::CueEvent::Select(0));
        app.start_words(0);
        let units = |app: &NohApp| cue_units(&app.lyrics_track().unwrap().cues[0]);
        let had = units(&app)[1].2;
        frame(
            &mut app,
            key(egui::Key::ArrowRight, egui::Modifiers::NONE).into(),
        );
        assert_eq!(app.lyrics_edit.words.as_ref().unwrap().word, 1);
        frame(
            &mut app,
            key(egui::Key::ArrowRight, egui::Modifiers::ALT).into(),
        );
        assert_eq!(
            (units(&app)[1].2, units(&app)[1].4),
            (had + 50, WordSource::Hand)
        );
        frame(&mut app, key(egui::Key::Z, egui::Modifiers::COMMAND).into());
        assert_eq!(units(&app)[1].2, had, "Ctrl+Z gives the start back");
        // A dragged start, from the lane.
        app.lane_event(app_timeline::CueEvent::UnitStart {
            cue: 0,
            unit: 2,
            to_ms: units(&app)[2].2 + 120,
        });
        assert_eq!(units(&app)[2].4, WordSource::Hand);
        assert_eq!(app.lyrics_edit.words.as_ref().unwrap().word, 2);
    }

    #[test]
    fn rhythm_taps_apply_together_at_the_last_one_and_echap_applies_nothing() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.lane_event(app_timeline::CueEvent::Select(0));
        app.start_words(0);
        let ctx = egui::Context::default();
        let cue = app.lyrics_track().unwrap().cues[0].clone();
        let n = cue_units(&cue).len();
        // Before playback runs, Space places nothing.
        app.start_tap(&ctx);
        app.mini_preview.cursor_ms = cue.start_ms.saturating_sub(1000);
        frame(
            &mut app,
            key(egui::Key::Space, egui::Modifiers::NONE).into(),
        );
        assert_eq!(
            app.lyrics_edit
                .words
                .as_ref()
                .unwrap()
                .taps
                .as_ref()
                .unwrap()
                .len(),
            0,
            "no tap at the seek point"
        );
        // Échap mid-way: nothing changes.
        app.lyrics_edit.test_clock = Some(cue.start_ms + 100);
        frame(&mut app, Vec::new());
        frame(
            &mut app,
            key(egui::Key::Space, egui::Modifiers::NONE).into(),
        );
        assert_eq!(
            app.lyrics_edit
                .words
                .as_ref()
                .unwrap()
                .taps
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        frame(
            &mut app,
            key(egui::Key::Escape, egui::Modifiers::NONE).into(),
        );
        assert!(
            app.lyrics_edit
                .words
                .as_ref()
                .is_some_and(|w| w.taps.is_none()),
            "Échap stops tapping, word mode stays"
        );
        assert!(app.project.edit.is_none(), "nothing applied");
        // A held Space is one tap (egui marks the presses after the first as repeats).
        app.start_tap(&ctx);
        let mut times = vec![cue.start_ms + 200];
        app.lyrics_edit.test_clock = Some(times[0]);
        frame(&mut app, Vec::new());
        let held = egui::Context::default();
        let space = |pressed| egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![space(true)],
            vec![space(true)],
            vec![space(true)],
            vec![space(false)],
        ] {
            held.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| app.lyrics_edit_frame(ui.ctx()),
            )
            .textures_delta
            .clear();
        }
        assert_eq!(
            app.lyrics_edit
                .words
                .as_ref()
                .unwrap()
                .taps
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        for k in 1..n {
            app.lyrics_edit.test_clock = Some(cue.start_ms + 200 + 400 * k as u64);
            times.push(cue.start_ms + 200 + 400 * k as u64);
            frame(
                &mut app,
                key(egui::Key::Space, egui::Modifiers::NONE).into(),
            );
        }
        assert!(app.lyrics_edit.words.as_ref().unwrap().taps.is_none());
        let placed: Vec<_> = cue_units(&app.lyrics_track().unwrap().cues[0])
            .iter()
            .map(|u| (u.2, u.4))
            .collect();
        assert_eq!(
            placed,
            times
                .iter()
                .map(|&t| (t, WordSource::Hand))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            app.lyrics_edit.history.len(),
            1,
            "one undo step for every tap"
        );
        app.undo();
        assert!(app.project.edit.is_none());
    }

    #[test]
    fn a_stale_time_from_before_the_seek_neither_starts_nor_stops_tapping() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.project.duration_ms = 20_000;
        app.lane_event(app_timeline::CueEvent::Select(0));
        app.start_words(0);
        app.start_tap(&egui::Context::default());
        // The song was playing at 1:12; the seek went to 0:00 (1 s before the line).
        app.lyrics_edit.test_clock = Some(72_000);
        frame(
            &mut app,
            key(egui::Key::Space, egui::Modifiers::NONE).into(),
        );
        let w = app.lyrics_edit.words.as_ref().unwrap();
        assert!(
            w.tap_clock.is_none() && w.taps.as_ref().unwrap().is_empty(),
            "no start, no tap at the old time"
        );
        for at in [16, 400] {
            app.lyrics_edit.test_clock = Some(at);
            frame(&mut app, Vec::new());
        }
        app.lyrics_edit.test_clock = Some(1_200);
        frame(
            &mut app,
            key(egui::Key::Space, egui::Modifiers::NONE).into(),
        );
        let w = app.lyrics_edit.words.as_ref().unwrap();
        assert_eq!(
            w.taps.as_deref(),
            Some(&[1_200][..]),
            "tapping still runs; the first tap is after the seek"
        );
        assert!(w.notice.is_none());
    }

    #[test]
    fn syllables_typed_with_hyphens_become_units_and_join_back() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.lane_event(app_timeline::CueEvent::Select(0));
        app.start_words(0);
        let units = |app: &NohApp| cue_units(&app.lyrics_track().unwrap().cues[0]);
        assert_eq!(units(&app).len(), 4);
        app.apply_syllables(0, 0, "Some-thing");
        assert_eq!(
            units(&app).iter().map(|u| u.3.as_str()).collect::<Vec<_>>(),
            ["Some", "thing", "had", "to", "give"]
        );
        assert_eq!(app.lyrics_edit.words.as_ref().unwrap().syl_error, None);
        // Letters lost: refused, with the reason; nothing changes.
        let before = app.lyrics_track();
        app.apply_syllables(0, 0, "Some-thin");
        assert_eq!(
            app.lyrics_edit.words.as_ref().unwrap().syl_error,
            Some("words.syl_letters")
        );
        assert_eq!(app.lyrics_track(), before);
        // « Réunir en un mot », then Ctrl+Z gives the syllables back.
        app.apply_syllables(0, 0, "");
        assert_eq!(units(&app).len(), 4);
        app.undo();
        assert_eq!(units(&app).len(), 5);
    }

    #[test]
    fn couper_en_syllabes_proposes_from_the_language_pack() {
        let folder = tempfile::tempdir().unwrap();
        let packs = folder.path().join("languages");
        for (id, extra, dictionary) in [
            (
                "en",
                r#", "dictionary": {"file": "dictionary.txt", "name": "Moby", "source": "test", "licence": "public domain"}"#,
                Some("something\tsome-thing\n"),
            ),
            ("fr", "", None),
        ] {
            let dir = packs.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            let rule = if id == "fr" {
                r#"{"vowels": "aeiouyéèêàâ", "elisions": ["l'", "qu'"]}"#
            } else {
                r#"{"vowels": "aeiouy"}"#
            };
            std::fs::write(
                dir.join("language.json"),
                format!(
                    r#"{{"id": "{id}", "name": {{}}, "version": "1", "syllables": {rule}{extra}}}"#
                ),
            )
            .unwrap();
            if let Some(d) = dictionary {
                std::fs::write(dir.join("dictionary.txt"), d).unwrap();
            }
        }
        let mut app = app_with_track(folder.path());
        app.locale.language = noh::i18n::Language::En;
        app.lyrics_edit.packs = Some(Arc::new(noh::languages::scan(&[packs])));
        app.lane_event(app_timeline::CueEvent::Select(0));
        app.start_words(0);
        // The track has no language (an imported .srt): « Langue chantée » decides.
        app.subtitles.language = "en".into();
        app.propose_syllables(0, 0);
        let units = |app: &NohApp| {
            cue_units(&app.lyrics_track().unwrap().cues[0])
                .into_iter()
                .map(|u| u.3)
                .collect::<Vec<_>>()
        };
        assert_eq!(units(&app), ["Some", "thing", "had", "to", "give"]);
        let note = |app: &NohApp| {
            let word = edit::words_or_estimate(&app.lyrics_track().unwrap().cues[0])[0].clone();
            syl_note(app.lyrics_edit.words.as_ref().unwrap(), 0, &word).map(str::to_string)
        };
        assert_eq!(
            note(&app).as_deref(),
            Some("Proposed from the Moby dictionary.")
        );
        // Undone, the word is whole: no note; redone, it is back.
        app.undo();
        assert_eq!(note(&app), None);
        app.redo();
        assert_eq!(
            note(&app).as_deref(),
            Some("Proposed from the Moby dictionary.")
        );
        // One syllable: said, and nothing changes.
        app.propose_syllables(0, 1);
        assert_eq!(
            app.lyrics_edit.words.as_ref().unwrap().syl_error,
            Some("words.syl_one")
        );
        app.undo();
        assert_eq!(units(&app).len(), 4, "one undo step");
        // Typed over: the note goes with the proposal.
        app.propose_syllables(0, 0);
        app.apply_syllables(0, 0, "So-mething");
        assert_eq!(note(&app), None);
        app.undo();
        app.undo();
        // French, by its rule with the elision.
        app.edit_track(|t| edit::set_text(t, 0, "l'amour toujours"));
        app.subtitles.language = "fr".into();
        app.propose_syllables(0, 0);
        assert_eq!(units(&app)[..2], ["l'a".to_string(), "mour".to_string()]);
    }

    #[test]
    fn tapping_ends_without_change_when_playback_overruns_jumps_back_or_stops() {
        assert!(
            !tap_overrun(Some(5000), 4900, 3000),
            "within 2 s after the end"
        );
        assert!(tap_overrun(Some(5001), 5000, 3000), "past end + 2 s");
        assert!(
            tap_overrun(Some(1000), 2900, 3000),
            "the excerpt looped back"
        );
        assert!(tap_overrun(None, 2000, 3000), "the player stopped");
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.lane_event(app_timeline::CueEvent::Select(0));
        app.start_words(0);
        let end = app.lyrics_track().unwrap().cues[0].end_ms;
        for stop in [Some(end + 2001), Some(0), None] {
            app.start_tap(&egui::Context::default());
            app.lyrics_edit.test_clock = Some(100);
            frame(&mut app, Vec::new());
            app.lyrics_edit.test_clock = Some(end - 500);
            frame(
                &mut app,
                key(egui::Key::Space, egui::Modifiers::NONE).into(),
            );
            assert_eq!(
                app.lyrics_edit
                    .words
                    .as_ref()
                    .unwrap()
                    .taps
                    .as_ref()
                    .unwrap()
                    .len(),
                1
            );
            app.lyrics_edit.test_clock = stop;
            frame(&mut app, Vec::new());
            let w = app.lyrics_edit.words.as_ref().unwrap();
            assert!(
                w.taps.is_none() && w.notice == Some("words.tap_dropped"),
                "{stop:?}"
            );
            assert!(app.project.edit.is_none(), "nothing applied");
        }
    }

    #[test]
    fn delete_offers_undo_and_lane_events_edit_the_track() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.lane_event(app_timeline::CueEvent::Select(1));
        app.delete_cue(1);
        assert!(app.lyrics_edit.toast.is_some() && app.lyrics_edit.selected.is_none());
        assert_eq!(app.lyrics_track().unwrap().cues.len(), 1);
        app.undo();
        assert_eq!(app.lyrics_track().unwrap().cues.len(), 2);
        app.lane_event(app_timeline::CueEvent::Drag {
            cue: 0,
            part: app_timeline::CuePart::End,
            delta_ms: 0,
            to_ms: 9000,
        });
        assert_eq!(
            app.lyrics_track().unwrap().cues[0].end_ms,
            4000,
            "stops at the neighbour"
        );
        app.lane_event(app_timeline::CueEvent::Add(3500));
        assert!(
            app.lyrics_track().unwrap().cues.len() == 2,
            "no 0.5 s gap there anymore"
        );
        app.lane_event(app_timeline::CueEvent::Add(8000));
        assert_eq!(app.lyrics_track().unwrap().cues.len(), 3);
        assert_eq!(app.lyrics_edit.selected, Some(2));
    }
}
