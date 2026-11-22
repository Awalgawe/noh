//! Writing lyrics edits as they are made (lyrics plan 3; maintainer, 11 November):
//! about 1 s after the last change, NOH writes the lyrics file
//! (`<name>.noh-lyrics.json`, the reference) and the subtitle file from it,
//! nothing else. There is no Save button; Ctrl+S writes at once.
//!
//! The first edit of a source asks once whether to modify that .srt, which is
//! destructive (the original is not kept), or to work on a copy; the answer is
//! remembered while that file is the one NOH last read or wrote. Lyrics NOH
//! generated in this session are its own files and are not asked about. A
//! change outside NOH, or a missing file, asks again with a warning. A write
//! that fails says so at once, with « Réessayer », and leaves a mark on the
//! lyrics until a write succeeds; each later change tries again.
use super::*;
use app_style::{self as style, space};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// How long after the last change the lyrics are written.
pub const WRITE_DELAY: Duration = Duration::from_secs(1);

/// What is missing on disk after a failed write, shown on the lyrics chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// The lyrics file is not written: the lyrics live in memory.
    Unsaved,
    /// The lyrics file is written, the .srt beside it is not.
    SrtStale,
}

impl Mark {
    pub fn key(self) -> &'static str {
        match self {
            Self::Unsaved => "lyrics.mark_unsaved",
            Self::SrtStale => "lyrics.mark_srt",
        }
    }
}

/// The .srt write failed after the lyrics file was written.
#[derive(Debug)]
pub struct SrtNotWritten(String);

impl std::fmt::Display for SrtNotWritten {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SrtNotWritten {}

#[derive(Default)]
pub struct State {
    /// The subtitle file NOH may replace without asking (answer 2).
    remembered: Option<PathBuf>,
    /// The first-edit question is open.
    asking: bool,
    /// The question was asked by an edit: « Annuler » takes that edit back.
    cancel_edit: bool,
    /// The question offers only « Travailler sur une copie » (a newer lyrics
    /// file, or one that could not be created).
    copy_only: bool,
    /// When the pending change is due to be written.
    pub write_at: Option<Instant>,
    /// A failed write's mark, until a write succeeds.
    pub mark: Option<Mark>,
    /// The track revision whose creation failure was already shown.
    unwritten_seen: Option<u64>,
    /// The lyrics the mark is about: other lyrics attached drop it.
    marked_for: Option<PathBuf>,
    /// Close the app once the edits are saved (from the close question).
    close_after: bool,
    /// The app is closing with unsaved edits: Save, Quit without saving, Cancel.
    pub closing: bool,
    /// A lyrics action that would drop unsaved edits, waiting for an answer.
    pub pending: Option<super::app_lyrics::PendingLyrics>,
    /// The last save failed: (message key, detail).
    pub failed: Option<String>,
}

/// Write `track` as the lyrics file beside `path` (`<name>.noh-lyrics.json`,
/// the reference), then the .srt at `path` from it: each through a temporary
/// file and a rename, so a failed write leaves the previous file whole.
/// A lyrics file NOH cannot use is never overwritten: a newer or locked one
/// stops the save before anything is written, a damaged one is first renamed
/// to a free `<name>.unreadable.json` (named in the error if the save then
/// fails). With `replace_old` (a save that replaces the original, decision C),
/// a version-1 `<name>.noh-words.json` is removed once both files are written,
/// only when this save created the lyrics file (the migration): when both
/// names were already there, the old one is left in place. A copy leaves the
/// original pair alone.
pub fn write_subtitles(
    path: &Path,
    track: &noh::subtitle_track::SubtitleTrack,
    model: Option<&str>,
    replace_old: bool,
) -> std::io::Result<Saved> {
    use noh::subtitle_words as words;
    // Checked against the song before anything is written.
    let srt = track.to_srt().map_err(std::io::Error::other)?;
    let lyrics = words::lyrics_path(path);
    let migrating = !lyrics.exists();
    let set_aside = words::set_aside_unusable(&lyrics)?;
    let kept_as = |e: std::io::Error| match &set_aside {
        Some(aside) => std::io::Error::new(
            e.kind(),
            format!(
                "{e} (the unreadable lyrics file was kept as {})",
                aside.display()
            ),
        ),
        None => e,
    };
    words::write_lyrics(&lyrics, track, model).map_err(kept_as)?;
    let mut srt_temp = path.as_os_str().to_owned();
    srt_temp.push(".noh-saving");
    let srt_temp = PathBuf::from(srt_temp);
    if let Err(e) = words::ensure_writable(path)
        .and_then(|()| std::fs::write(&srt_temp, srt.as_bytes()))
        .and_then(|()| std::fs::rename(&srt_temp, path))
    {
        let _ = std::fs::remove_file(&srt_temp);
        let kind = e.kind();
        let text = kept_as(std::io::Error::new(
            kind,
            format!("the lyrics file was written, but the .srt was not: {e}"),
        ))
        .to_string();
        return Err(std::io::Error::new(kind, SrtNotWritten(text)));
    }
    let old = words::path_for(path);
    // A read-only old working file is kept, as Windows already did, on every system.
    let old_kept = (replace_old
        && migrating
        && old.exists()
        && words::ensure_writable(&old)
            .and_then(|()| std::fs::remove_file(&old))
            .is_err())
    .then_some(old);
    let stamp =
        noh::inspection::FileStamp::read(path).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(Saved {
        snapshot: noh::inspection::Snapshot(vec![stamp]),
        lyrics_stamp: noh::inspection::FileStamp::read(&lyrics).ok(),
        set_aside,
        old_kept,
    })
}

/// A save that wrote both files.
pub struct Saved {
    /// The .srt as NOH wrote it (its own write is not an outside change).
    pub snapshot: noh::inspection::Snapshot,
    /// The lyrics file as NOH wrote it.
    pub lyrics_stamp: Option<noh::inspection::FileStamp>,
    /// Where a damaged lyrics file was set aside, if one was.
    pub set_aside: Option<PathBuf>,
    /// The version-1 file that should have been removed and was not.
    pub old_kept: Option<PathBuf>,
}

impl Saved {
    /// What the status line says after the save, if anything.
    fn notice(&self) -> Option<Message> {
        let name = |p: &PathBuf| {
            p.file_name()
                .unwrap_or(p.as_os_str())
                .to_string_lossy()
                .into_owned()
        };
        self.set_aside
            .as_ref()
            .map(|p| Message::new("save.set_aside", &[name(p)]))
            .or_else(|| {
                self.old_kept
                    .as_ref()
                    .map(|p| Message::new("save.old_kept", &[name(p)]))
            })
    }
}

impl NohApp {
    pub(super) fn has_unsaved_lyrics(&self) -> bool {
        self.project.edit.is_some()
    }

    /// Lyrics NOH may write over without asking: the remembered file, unchanged
    /// outside NOH and still there.
    fn may_write(&self) -> bool {
        self.project.edit.as_ref().is_some_and(|s| {
            self.lyrics_save.remembered.as_ref() == Some(&s.path)
                && !s.outside_changed
                && s.path.exists()
        })
    }

    /// Lyrics NOH generated (and attached) are its own file: no question.
    pub(super) fn trust_lyrics(&mut self, path: PathBuf) {
        self.lyrics_save.remembered = Some(path);
    }

    /// Open the first-edit question; `cancel_edit`: « Annuler » takes back the
    /// edit that asked.
    fn ask_first_edit(&mut self, cancel_edit: bool) {
        let s = &mut self.lyrics_save;
        if s.asking {
            return;
        }
        s.asking = true;
        s.cancel_edit = cancel_edit;
        let source = self.project.edit.as_ref().map(|e| e.path.clone());
        let newer = source.is_some_and(|p| {
            matches!(
                noh::subtitle_words::read_lyrics(&noh::subtitle_words::lyrics_path(&p)),
                Ok(Err(noh::subtitle_words::LyricsError::Newer(_)))
            )
        });
        let unwritten = self
            .project
            .track
            .loaded
            .as_ref()
            .is_some_and(|l| l.lyrics_unwritten.is_some());
        self.lyrics_save.copy_only = newer || unwritten;
    }

    /// The shown lyrics changed (an edit, an undo, a redo): written about 1 s
    /// later. Only a new edit asks, through `lyrics_edited`; a due write that
    /// may not happen asks without taking anything back.
    pub(super) fn lyrics_changed(&mut self) {
        self.lyrics_save.write_at = self
            .project
            .edit
            .is_some()
            .then(|| Instant::now() + WRITE_DELAY);
    }

    /// A new lyrics edit (edit_track): the first edit of this source asks, and
    /// « Annuler » takes back that edit alone.
    pub(super) fn lyrics_edited(&mut self) {
        if self.project.edit.is_some() && !self.may_write() {
            self.ask_first_edit(true);
        }
    }

    /// A lyrics question or message is open: lane keys wait.
    pub(super) fn lyrics_question_open(&self) -> bool {
        self.lyrics_question_kind().is_some()
    }

    /// Tests: the first-edit question closed without an answer, the edit kept
    /// and still unsaved.
    #[cfg(test)]
    pub(super) fn set_first_edit_aside(&mut self) {
        self.lyrics_save.asking = false;
        self.lyrics_save.cancel_edit = false;
    }

    /// Which lyrics question or message is open, for a driven window's agent.
    pub(super) fn lyrics_question_kind(&self) -> Option<&'static str> {
        let s = &self.lyrics_save;
        if s.asking {
            Some("first_edit")
        } else if s.closing {
            Some("unsaved_on_close")
        } else if s.pending.is_some() {
            Some("lyrics_action")
        } else if s.failed.is_some() {
            Some("save_failed")
        } else {
            None
        }
    }

    /// Set a failed write's mark for the lyrics attached now.
    fn set_mark(&mut self, mark: Mark) {
        self.lyrics_save.mark = Some(mark);
        self.lyrics_save.marked_for = self.project.track.path.clone();
    }

    /// Once per frame: write the pending change when it is due, not while a
    /// question is open, the lane is locked (an export reads its copy) or a
    /// drag is under way; and show once that the lyrics file could not be
    /// created at load.
    pub(super) fn lyrics_autosave(&mut self, ctx: &egui::Context) {
        if self.lyrics_save.mark.is_some() && self.lyrics_save.marked_for != self.project.track.path
        {
            self.lyrics_save.mark = None;
            self.lyrics_save.copy_only = false;
        }
        if self.project.edit.is_none() {
            self.lyrics_save.write_at = None;
        }
        if let Some(loaded) = &self.project.track.loaded
            && let Some(reason) = &loaded.lyrics_unwritten
            && self.lyrics_save.unwritten_seen != Some(self.project.track.revision)
        {
            self.lyrics_save.unwritten_seen = Some(self.project.track.revision);
            self.notice = Some(Message::new("lyrics.unwritten", &[reason.clone()]));
            self.set_mark(Mark::Unsaved);
        }
        let Some(at) = self.lyrics_save.write_at else {
            return;
        };
        let now = Instant::now();
        let held = ctx.input(|i| i.pointer.any_down());
        // Not during a sync game either: its takes are one edit, written when
        // it ends, and nothing reads the disk while a key is being timed.
        if now < at
            || self.lyrics_save.asking
            || self.lyrics_frozen()
            || held
            || self.lyrics_game_running()
        {
            ctx.request_repaint_after(at.saturating_duration_since(now).max(WRITE_DELAY / 4));
            return;
        }
        self.lyrics_save.write_at = None;
        self.save_lyrics();
    }

    /// Write the pending change now (Ctrl+S, the close question's « Enregistrer »,
    /// a due write): quietly when allowed, else through the first-edit question.
    pub(super) fn save_lyrics(&mut self) {
        let Some(session) = &self.project.edit else {
            return;
        };
        let source = session.path.clone();
        if self.may_write() {
            self.save_lyrics_to(source, false);
        } else {
            self.ask_first_edit(false);
        }
    }

    /// « Modifier le .srt »: remembered for this file (a write that fails then
    /// retries on the next change without asking again), and written now.
    pub(super) fn consent_first_edit(&mut self, source: PathBuf) {
        self.lyrics_save.asking = false;
        self.lyrics_save.cancel_edit = false;
        self.lyrics_save.remembered = Some(source.clone());
        self.save_lyrics_to(source, false);
    }

    /// « Annuler » in the first-edit question: nothing is written, and the edit
    /// that asked is taken back.
    pub(super) fn cancel_first_edit(&mut self) {
        let take_back = self.lyrics_save.cancel_edit;
        let s = &mut self.lyrics_save;
        s.asking = false;
        s.close_after = false;
        s.cancel_edit = false;
        s.write_at = None;
        if take_back {
            self.take_back_edit();
        }
    }

    /// Before an action that would drop the edits (close, another file, remove):
    /// write them if NOH may. True when nothing unsaved is left.
    pub(super) fn flush_lyrics(&mut self) -> bool {
        if self.project.edit.is_some() && self.may_write() && !self.lyrics_save.asking {
            self.save_lyrics();
        }
        !self.has_unsaved_lyrics()
    }

    /// « Enregistrer une copie… »: the copy becomes the attached subtitle file.
    pub(super) fn save_lyrics_copy(&mut self) {
        let Some(track) = self.lyrics_track() else {
            return;
        };
        let source = self.project.track.path.clone().unwrap_or_default();
        let l = self.locale.language;
        let stem = source
            .file_stem()
            .map_or("lyrics".into(), |s| s.to_string_lossy().into_owned());
        let mut dialog = rfd::FileDialog::new()
            .set_title(l.text("save.copy"))
            .add_filter("SRT", &["srt"])
            .set_file_name(l.format("save.copy_name", &[stem]) + ".srt");
        if let Some(folder) = source.parent() {
            dialog = dialog.set_directory(folder);
        }
        if let Some(path) = dialog.save_file() {
            let path = if path.extension().is_none() {
                path.with_extension("srt")
            } else {
                path
            };
            let _ = track;
            self.save_lyrics_to(path, true);
        } else {
            // No copy written: a close that waited on this save is abandoned.
            self.lyrics_save.close_after = false;
        }
    }

    /// « Annuler » in the close question: stay open, and forget any close that
    /// was waiting on a save.
    fn cancel_closing(&mut self) {
        self.lyrics_save.closing = false;
        self.lyrics_save.close_after = false;
    }

    fn save_lyrics_to(&mut self, path: PathBuf, copy: bool) {
        let Some(track) = self.lyrics_track() else {
            return;
        };
        self.save_track_to(path, copy, track);
    }

    /// Both files as NOH last read or wrote them (plan 5): a write over the
    /// attached lyrics goes ahead only then.
    pub(super) fn pair_unchanged(&self, path: &Path) -> bool {
        let Some(loaded) = &self.project.track.loaded else {
            return false;
        };
        let now = |p: &Path| noh::inspection::FileStamp::read(p).ok();
        now(path) == loaded.srt_stamp
            && now(&noh::subtitle_words::lyrics_path(path)) == loaded.lyrics_stamp
    }

    /// Write `track` as both files at `path`. Over the attached lyrics, never
    /// while a difference waits for its answer, and only while both files are
    /// as NOH last read or wrote them; otherwise nothing is written and both
    /// are read again (the edits stay).
    pub(super) fn save_track_to(
        &mut self,
        path: PathBuf,
        copy: bool,
        track: std::sync::Arc<noh::subtitle_track::SubtitleTrack>,
    ) {
        if !copy && self.project.track.path.as_ref() == Some(&path) {
            if self.lyrics_question_pending() {
                return;
            }
            if !self.pair_unchanged(&path) {
                self.lyrics_save.write_at = None;
                if self.project.track.loaded.is_some() {
                    self.project.track.recheck();
                }
                return;
            }
        }
        let model = self
            .project
            .track
            .loaded
            .as_ref()
            .and_then(|l| l.model.clone());
        match write_subtitles(&path, &track, model.as_deref(), !copy) {
            Ok(saved) => {
                if let Some(notice) = saved.notice() {
                    self.notice = Some(notice);
                }
                let snapshot = saved.snapshot;
                // Undo after a save must not land on the saved version: the steps
                // that meant "the file" keep the file as it was before.
                let before = self.project.track.loaded.as_ref().map(|l| l.track.clone());
                self.pin_file_version(before);
                // NOH's own write is not a change made outside NOH (plan v3 (b)).
                self.project
                    .track
                    .adopt(path.clone(), track, snapshot, saved.lyrics_stamp, copy);
                self.project.edit = None;
                self.lyrics_save.remembered = Some(path);
                self.lyrics_save.failed = None;
                self.lyrics_save.asking = false;
                self.lyrics_save.cancel_edit = false;
                self.lyrics_save.write_at = None;
                self.lyrics_save.mark = None;
                if self.lyrics_save.close_after {
                    self.lyrics_save.close_after = false;
                    self.lyrics_save.closing = false;
                    self.close_now = true;
                }
            }
            Err(e) => {
                // The edits stay; the next change tries again (no retry loop).
                let srt_only = e.get_ref().is_some_and(|r| r.is::<SrtNotWritten>());
                if srt_only && !copy && self.project.track.path.as_ref() == Some(&path) {
                    let lyrics = noh::subtitle_words::lyrics_path(&path);
                    self.project
                        .track
                        .own_lyrics_write(noh::inspection::FileStamp::read(&lyrics).ok());
                }
                self.set_mark(if srt_only {
                    Mark::SrtStale
                } else {
                    Mark::Unsaved
                });
                self.lyrics_save.failed = Some(e.to_string());
                self.lyrics_save.close_after = false;
                self.lyrics_save.asking = false;
                self.lyrics_save.cancel_edit = false;
            }
        }
    }

    /// The first-save question, the close question and the "edits will be lost" question.
    pub(super) fn lyrics_save_dialogs(&mut self, ctx: &egui::Context) {
        let l = self.locale.language;
        if self.lyrics_save.asking {
            let session = self.project.edit.as_ref();
            let source = session.map(|s| s.path.clone()).unwrap_or_default();
            let warn = match session {
                Some(_) if !source.exists() => Some(l.text("save.missing")),
                Some(s) if s.outside_changed => Some(l.text("save.outside")),
                _ => None,
            };
            let name = source
                .file_name()
                .map_or(String::new(), |n| n.to_string_lossy().into_owned());
            let copy_only = self.lyrics_save.copy_only;
            let mut choice = None;
            modal(ctx, "lyrics-save", l.text("save.title"), |ui| {
                ui.add(egui::Label::new(l.format("save.body", &[name.clone()])).wrap());
                let warn = warn.or(copy_only.then(|| l.text("save.copy_only")));
                if let Some(w) = warn {
                    ui.add(
                        egui::Label::new(egui::RichText::new(w).color(style::tokens(ui).warn))
                            .wrap(),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if style::button(ui, l.text("save.cancel"), true, false, 0.0).clicked() {
                        choice = Some(0);
                    }
                    if !copy_only
                        && style::button(ui, l.text("save.replace"), true, false, 0.0).clicked()
                    {
                        choice = Some(1);
                    }
                    if style::button(ui, l.text("save.work_copy"), true, true, 0.0).clicked() {
                        choice = Some(2);
                    }
                });
            });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                choice = Some(0);
            }
            let take_back = self.lyrics_save.cancel_edit;
            match choice {
                Some(0) => self.cancel_first_edit(),
                Some(1) => self.consent_first_edit(source),
                Some(2) => {
                    self.lyrics_save.asking = false;
                    self.save_lyrics_copy();
                    // No copy chosen (the file dialog cancelled): as « Annuler ».
                    if self.has_unsaved_lyrics() && self.lyrics_save.failed.is_none() {
                        self.lyrics_save.cancel_edit = take_back;
                        self.cancel_first_edit();
                    }
                }
                _ => {}
            }
        }
        if self.lyrics_save.closing && !self.lyrics_save.asking && !self.has_unsaved_lyrics() {
            // The edit that kept the app open was taken back: nothing to lose.
            self.lyrics_save.closing = false;
            self.lyrics_save.close_after = false;
            self.close_now = true;
        }
        if self.lyrics_save.closing && !self.lyrics_save.asking {
            let file = self
                .project
                .edit
                .as_ref()
                .map(|s| noh::subtitle_words::lyrics_path(&s.path))
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let mut choice = None;
            modal(ctx, "lyrics-unsaved", l.text("save.unsaved_title"), |ui| {
                ui.add(egui::Label::new(l.format("save.unsaved_body", &[file.clone()])).wrap());
                ui.horizontal_wrapped(|ui| {
                    if style::button(ui, l.text("save.cancel"), true, false, 0.0).clicked() {
                        choice = Some(0);
                    }
                    if style::button(ui, l.text("save.discard"), true, false, 0.0).clicked() {
                        choice = Some(1);
                    }
                    if style::button(ui, l.text("save.menu"), true, true, 0.0).clicked() {
                        choice = Some(2);
                    }
                });
            });
            match choice {
                Some(0) => self.cancel_closing(),
                Some(1) => {
                    self.project.edit = None;
                    self.lyrics_save.closing = false;
                    self.close_now = true;
                }
                Some(2) => {
                    self.lyrics_save.close_after = true;
                    self.save_lyrics();
                }
                _ => {}
            }
        }
        if let Some(pending) = self.lyrics_save.pending.clone() {
            let mut choice = None;
            modal(ctx, "lyrics-lose", l.text("save.lose_title"), |ui| {
                ui.add(egui::Label::new(l.text("save.lose_body")).wrap());
                ui.horizontal_wrapped(|ui| {
                    if style::button(ui, l.text("save.cancel"), true, false, 0.0).clicked() {
                        choice = Some(false);
                    }
                    if style::button(ui, l.text("save.continue"), true, true, 0.0).clicked() {
                        choice = Some(true);
                    }
                });
            });
            if let Some(go) = choice {
                self.lyrics_save.pending = None;
                if go {
                    self.project.edit = None;
                    self.run_pending_lyrics(pending, ctx);
                }
            }
        }
        if let Some(detail) = self.lyrics_save.failed.clone() {
            let (mut close, mut retry) = (false, false);
            modal(
                ctx,
                "lyrics-save-failed",
                l.text("save.failed_title"),
                |ui| {
                    ui.add(egui::Label::new(l.format("save.failed", &[detail.clone()])).wrap());
                    ui.horizontal_wrapped(|ui| {
                        close = style::button(ui, l.text("save.close"), true, false, 0.0).clicked();
                        retry = style::button(ui, l.text("save.retry"), true, true, 0.0).clicked();
                    });
                },
            );
            if close || retry {
                self.lyrics_save.failed = None;
            }
            if retry {
                self.save_lyrics();
            }
        }
    }
}

fn modal(ctx: &egui::Context, id: &str, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        // Inside a compact window too (420 wide): its margins stay visible.
        ui.set_max_width(440.0_f32.min(ctx.content_rect().width() - 64.0));
        ui.spacing_mut().item_spacing.y = space::M;
        ui.label(style::strong(title, style::text::HEADING));
        body(ui);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use noh::subtitle_edit as edit;
    use std::sync::Arc;

    fn app_with_track(folder: &Path) -> NohApp {
        let mut app = NohApp::default();
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nSomething had to give\n\n";
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
        app
    }

    #[test]
    fn the_first_save_asks_then_replace_is_remembered_for_that_file() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        app.save_lyrics();
        assert!(app.lyrics_save.asking, "the first save asks");
        let source = folder.path().join("song.srt");
        app.lyrics_save.asking = false;
        app.save_lyrics_to(source.clone(), false);
        assert!(app.project.edit.is_none(), "saved: no unsaved edits");
        assert!(
            std::fs::read_to_string(&source)
                .unwrap()
                .contains("Something has to give")
        );
        assert!(
            app.project.track.loaded.as_ref().unwrap().track.cues[0]
                .text
                .contains("has")
        );
        // NOH's own write is what NOH last read: the next save does not ask.
        app.edit_track(|t| edit::set_text(t, 0, "Something had to give"));
        app.save_lyrics();
        assert!(!app.lyrics_save.asking && app.project.edit.is_none());
        assert!(
            std::fs::read_to_string(&source)
                .unwrap()
                .contains("had to give")
        );
    }

    #[test]
    fn a_change_outside_noh_or_a_missing_file_asks_again() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        app.lyrics_save.remembered = Some(source.clone());
        app.edit_track(|t| edit::move_cue(t, 0, 100) != 0);
        app.project.edit.as_mut().unwrap().outside_changed = true;
        app.save_lyrics();
        assert!(
            app.lyrics_save.asking,
            "outside change: asks even with Replace remembered"
        );
        app.lyrics_save.asking = false;
        app.project.edit.as_mut().unwrap().outside_changed = false;
        std::fs::remove_file(&source).unwrap();
        app.save_lyrics();
        assert!(app.lyrics_save.asking, "missing file: asks");
    }

    #[test]
    fn undo_after_a_save_goes_back_to_the_original_not_to_the_saved_file() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.edit_track(|t| edit::set_text(t, 0, "A"));
        app.edit_track(|t| edit::set_text(t, 0, "B"));
        app.save_lyrics_to(folder.path().join("song.srt"), false);
        assert!(app.undo());
        assert_eq!(app.lyrics_track().unwrap().cues[0].text, "A");
        assert!(app.undo());
        assert_eq!(
            app.lyrics_track().unwrap().cues[0].text,
            "Something had to give"
        );
        assert!(
            app.has_unsaved_lyrics(),
            "the original is now an unsaved change of the saved file"
        );
    }

    #[test]
    fn a_copy_becomes_the_attached_and_remembered_file_and_the_source_is_untouched() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        let before = std::fs::read(&source).unwrap();
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        let copy = folder.path().join("song (edited).srt");
        app.save_lyrics_to(copy.clone(), true);
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert_eq!(app.project.track.path.as_ref(), Some(&copy));
        assert_eq!(app.lyrics_save.remembered.as_ref(), Some(&copy));
        // A new source resets the remembered choice: editing the original again asks.
        app.project.track.path = Some(source.clone());
        app.edit_track(|t| edit::set_text(t, 0, "x"));
        app.project.edit.as_mut().unwrap().path = source;
        app.save_lyrics();
        assert!(app.lyrics_save.asking);
    }

    /// Lyrics generated by a model keep its name through NOH's own save: in the
    /// words file beside them, and in the loaded state the chip reads.
    #[test]
    fn a_save_keeps_the_model_that_generated_the_lyrics() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.project.track.loaded.as_mut().unwrap().model = Some("large-v3".into());
        // Word times, so the save writes a words file.
        app.edit_track(|t| {
            t.cues[0].words = Some(edit::words_or_estimate(&t.cues[0]));
            true
        });
        let source = folder.path().join("song.srt");
        app.save_lyrics_to(source.clone(), false);
        assert_eq!(
            noh::subtitle_words::model_beside(&source).as_deref(),
            Some("large-v3")
        );
        assert_eq!(
            app.project.track.loaded.as_ref().unwrap().model.as_deref(),
            Some("large-v3")
        );
        // Every word time removed, then saved: the model is still recorded.
        app.edit_track(|t| {
            t.cues[0].words = None;
            true
        });
        app.save_lyrics_to(source.clone(), false);
        assert_eq!(
            noh::subtitle_words::model_beside(&source).as_deref(),
            Some("large-v3")
        );
    }

    /// Close → Save → the first-save question → a cancelled copy → Cancel in the
    /// close question: the next successful save must not quit the app.
    #[test]
    fn cancelling_the_close_question_forgets_the_close_after_save() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        app.lyrics_save.closing = true;
        app.lyrics_save.close_after = true; // « Enregistrer » in the close question
        app.cancel_closing();
        assert!(!app.lyrics_save.closing && !app.lyrics_save.close_after);
        app.save_lyrics_to(folder.path().join("song.srt"), false);
        assert!(!app.close_now, "a later save does not close the app");
    }

    #[test]
    fn a_failed_write_keeps_the_edits_and_leaves_no_staging_file() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        let target = folder.path().join("missing-folder").join("song.srt");
        app.save_lyrics_to(target.clone(), false);
        assert!(app.lyrics_save.failed.is_some());
        assert!(app.has_unsaved_lyrics(), "the edits are kept");
        assert!(!folder.path().join("missing-folder").exists());
    }

    #[test]
    fn saving_writes_the_working_file_and_removes_a_stale_one() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a.srt");
        let mut cue = noh::subtitle_track::SubtitleCue::new(0, 2000, "la la");
        cue.words = Some(vec![
            noh::subtitle_track::WordTime {
                text: "la".into(),
                start_ms: 0,
                end_ms: None,
                source: noh::subtitle_track::WordSource::Hand,
                syllables: None,
            },
            noh::subtitle_track::WordTime {
                text: "la".into(),
                start_ms: 900,
                end_ms: None,
                source: noh::subtitle_track::WordSource::Hand,
                syllables: None,
            },
        ]);
        let mut track = noh::subtitle_track::SubtitleTrack {
            language: None,
            duration_ms: 5000,
            cues: vec![cue],
        };
        use noh::subtitle_words as words;
        // A version-1 file from before.
        words::write_beside(&path, &track, None).unwrap();
        let saved = write_subtitles(&path, &track, None, true).unwrap();
        saved.snapshot.verify().unwrap();
        assert!(saved.set_aside.is_none() && saved.old_kept.is_none());
        let lyrics = words::lyrics_path(&path);
        assert_eq!(
            words::read_lyrics(&lyrics).unwrap().unwrap().track.cues,
            track.cues
        );
        assert!(!words::path_for(&path).exists(), "the old file removed (C)");
        // Without word times the lyrics file is still written: it holds the cues.
        track.cues[0].words = None;
        write_subtitles(&path, &track, None, true).unwrap();
        let read = words::read_lyrics(&lyrics).unwrap().unwrap().track;
        assert_eq!(read.cues, track.cues);
        assert!(!folder.path().join("a.srt.noh-saving").exists());
        assert!(
            std::fs::read_dir(folder.path())
                .unwrap()
                .filter_map(Result::ok)
                .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")),
            "no temporary file left"
        );
    }

    #[test]
    fn both_names_already_there_keep_the_old_file() {
        use noh::subtitle_words as words;
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        let track = app.lyrics_track().unwrap();
        words::write_beside(&source, &track, Some("small")).unwrap();
        words::write_lyrics(&words::lyrics_path(&source), &track, None).unwrap();
        observe(&mut app);
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        app.save_lyrics_to(source.clone(), false);
        assert!(!app.has_unsaved_lyrics(), "written");
        assert!(
            words::path_for(&source).exists(),
            "decision C: left in place"
        );
    }

    #[test]
    fn an_srt_that_cannot_be_written_after_the_lyrics_file_says_so() {
        use noh::subtitle_words as words;
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        // A folder where the .srt should go: the rename onto it fails.
        let target = folder.path().join("blocked.srt");
        std::fs::create_dir(&target).unwrap();
        // A damaged lyrics file there too: its new name is in the message.
        std::fs::write(words::lyrics_path(&target), b"{ not json").unwrap();
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        app.save_lyrics_to(target.clone(), false);
        let failed = app.lyrics_save.failed.clone().unwrap();
        assert!(failed.contains("the lyrics file was written, but the .srt was not"));
        assert!(failed.contains("blocked.unreadable.json"), "{failed}");
        assert!(app.has_unsaved_lyrics(), "the edits are kept");
        assert!(
            words::read_lyrics(&words::lyrics_path(&target))
                .unwrap()
                .is_ok()
        );
        assert!(!folder.path().join("blocked.srt.noh-saving").exists());
    }

    /// The files as they are now, as the reader would record them after a
    /// change on disk (a test that writes beside the lyrics stands for it).
    fn observe(app: &mut NohApp) {
        let path = app.project.track.path.clone().unwrap();
        let loaded = app.project.track.loaded.as_mut().unwrap();
        loaded.srt_stamp = noh::inspection::FileStamp::read(&path).ok();
        loaded.lyrics_stamp =
            noh::inspection::FileStamp::read(&noh::subtitle_words::lyrics_path(&path)).ok();
    }

    /// Run the due write now, as the frame after the delay would.
    fn write_due(app: &mut NohApp) {
        app.lyrics_save.write_at = Some(Instant::now() - WRITE_DELAY);
        app.lyrics_autosave(&egui::Context::default());
    }

    #[test]
    fn an_edit_is_written_about_a_second_later_without_a_save() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        app.trust_lyrics(source.clone());
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        assert!(
            !app.lyrics_save.asking,
            "an allowed file is not asked about"
        );
        let at = app.lyrics_save.write_at.unwrap();
        assert!(at > Instant::now() + WRITE_DELAY / 2, "due about 1 s later");
        // Not yet due: nothing written.
        app.lyrics_autosave(&egui::Context::default());
        assert!(app.has_unsaved_lyrics());
        write_due(&mut app);
        assert!(!app.has_unsaved_lyrics() && app.lyrics_save.write_at.is_none());
        assert!(
            std::fs::read_to_string(&source)
                .unwrap()
                .contains("has to give")
        );
        let lyrics = noh::subtitle_words::lyrics_path(&source);
        let file = noh::subtitle_words::read_lyrics(&lyrics).unwrap().unwrap();
        assert_eq!(file.track.cues[0].text, "Something has to give");
        // Undo after the write brings the earlier version back, written in turn.
        assert!(app.undo());
        write_due(&mut app);
        assert!(
            std::fs::read_to_string(&source)
                .unwrap()
                .contains("had to give")
        );
        // Ctrl+S writes at once.
        app.edit_track(|t| edit::set_text(t, 0, "Now"));
        app.save_lyrics();
        assert!(!app.has_unsaved_lyrics());
        assert!(std::fs::read_to_string(&source).unwrap().contains("Now"));
    }

    #[test]
    fn a_minimized_window_still_writes_a_due_change() {
        // A minimized window gets only `App::logic`, no ui: the frame it runs
        // then writes the due change, as a drawn frame does.
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.resources_checked();
        let source = folder.path().join("song.srt");
        app.trust_lyrics(source.clone());
        app.edit_track(|t| edit::set_text(t, 0, "Edited while minimized"));
        app.lyrics_save.write_at = Some(Instant::now() - WRITE_DELAY);
        app.hidden_frame(&egui::Context::default());
        assert!(!app.has_unsaved_lyrics() && app.lyrics_save.write_at.is_none());
        assert!(
            std::fs::read_to_string(&source)
                .unwrap()
                .contains("Edited while minimized")
        );
    }

    #[test]
    fn the_first_edit_asks_and_cancelling_takes_the_edit_back() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        let before = std::fs::read(&source).unwrap();
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        assert!(app.lyrics_save.asking && !app.lyrics_save.copy_only);
        // While the question is open, nothing is written.
        write_due(&mut app);
        assert!(app.has_unsaved_lyrics());
        app.cancel_first_edit();
        assert!(!app.has_unsaved_lyrics(), "the edit is taken back");
        assert!(!app.redo(), "and not offered again by Ctrl+Y");
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert!(app.lyrics_save.write_at.is_none());
        // « Modifier le .srt »: written, then remembered (no second question).
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        app.lyrics_save.asking = false;
        app.save_lyrics_to(source.clone(), false);
        app.edit_track(|t| edit::set_text(t, 0, "Again"));
        assert!(!app.lyrics_save.asking);
    }

    #[test]
    fn cancelling_takes_back_only_the_edit_that_asked() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        app.trust_lyrics(source.clone());
        app.edit_track(|t| edit::set_text(t, 0, "First"));
        // Changed outside NOH with that edit pending: the next edit asks.
        app.project.edit.as_mut().unwrap().outside_changed = true;
        app.edit_track(|t| edit::set_text(t, 0, "Second"));
        assert!(app.lyrics_save.asking && app.lyrics_save.cancel_edit);
        app.cancel_first_edit();
        assert!(!app.lyrics_save.asking, "the undo does not ask again");
        assert_eq!(app.lyrics_track().unwrap().cues[0].text, "First");
        // Ctrl+Z in that state: scheduled, not a question that would take more.
        assert!(app.undo());
        assert!(!app.lyrics_save.asking);
        assert!(app.redo(), "the redo the user made is kept");
        assert_eq!(app.lyrics_track().unwrap().cues[0].text, "First");
    }

    #[test]
    fn consent_is_remembered_even_when_that_write_fails() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        // The lyrics file cannot be written (a folder at its name).
        let lyrics = noh::subtitle_words::lyrics_path(&source);
        std::fs::create_dir(&lyrics).unwrap();
        app.edit_track(|t| edit::set_text(t, 0, "One"));
        assert!(app.lyrics_save.asking);
        app.consent_first_edit(source.clone());
        assert_eq!(app.lyrics_save.mark, Some(Mark::Unsaved));
        app.lyrics_save.failed = None;
        // The next edit does not ask again; « Annuler » is not offered for it.
        app.edit_track(|t| edit::set_text(t, 0, "Two"));
        assert!(!app.lyrics_save.asking);
        assert_eq!(app.lyrics_track().unwrap().cues[0].text, "Two");
        // Once the file can be written, the next due write clears the mark.
        std::fs::remove_dir(&lyrics).unwrap();
        write_due(&mut app);
        assert_eq!(app.lyrics_save.mark, None);
        assert!(std::fs::read_to_string(&source).unwrap().contains("Two"));
    }

    #[test]
    fn other_lyrics_drop_the_mark() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.project.track.loaded.as_mut().unwrap().lyrics_unwritten = Some("denied".into());
        app.lyrics_autosave(&egui::Context::default());
        assert_eq!(app.lyrics_save.mark, Some(Mark::Unsaved));
        app.project.track.path = Some(folder.path().join("other.srt"));
        app.project.track.loaded = None;
        app.lyrics_autosave(&egui::Context::default());
        assert_eq!(app.lyrics_save.mark, None);
        assert!(!app.lyrics_save.copy_only);
    }

    #[test]
    fn a_held_button_or_a_locked_lane_holds_the_write_back() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.trust_lyrics(folder.path().join("song.srt"));
        app.edit_track(|t| edit::set_text(t, 0, "Dragged"));
        app.lyrics_save.write_at = Some(Instant::now() - WRITE_DELAY);
        let ctx = egui::Context::default();
        let pointer = |pressed: bool| {
            let mut input = egui::RawInput::default();
            input
                .events
                .push(egui::Event::PointerMoved(egui::pos2(10.0, 10.0)));
            input.events.push(egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
            input
        };
        ctx.run_ui(pointer(true), |ui| app.lyrics_autosave(ui.ctx()))
            .textures_delta
            .clear();
        assert!(app.has_unsaved_lyrics(), "not while the button is down");
        app.cancelling = true;
        ctx.run_ui(pointer(false), |ui| app.lyrics_autosave(ui.ctx()))
            .textures_delta
            .clear();
        assert!(app.has_unsaved_lyrics(), "not while the lane is locked");
        app.cancelling = false;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            app.lyrics_autosave(ui.ctx())
        })
        .textures_delta
        .clear();
        assert!(!app.has_unsaved_lyrics(), "written at the end of the drag");
    }

    #[test]
    fn closing_with_nothing_left_to_save_closes() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.lyrics_save.closing = true;
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            app.lyrics_save_dialogs(ui.ctx())
        })
        .textures_delta
        .clear();
        assert!(app.close_now && !app.lyrics_save.closing);
    }

    #[test]
    fn a_newer_lyrics_file_offers_only_a_copy() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        std::fs::write(
            noh::subtitle_words::lyrics_path(&source),
            br#"{"version":3,"cues":[]}"#,
        )
        .unwrap();
        app.edit_track(|t| edit::set_text(t, 0, "x"));
        assert!(app.lyrics_save.asking && app.lyrics_save.copy_only);
    }

    #[test]
    fn a_failed_write_leaves_its_mark_until_a_write_succeeds() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        // The lyrics file cannot be written: « pas enregistré ».
        let missing = folder.path().join("missing-folder").join("song.srt");
        app.edit_track(|t| edit::set_text(t, 0, "A"));
        app.save_lyrics_to(missing, false);
        assert_eq!(app.lyrics_save.mark, Some(Mark::Unsaved));
        assert!(app.has_unsaved_lyrics());
        // The .srt cannot be written after the lyrics file: « .srt pas à jour ».
        let blocked = folder.path().join("blocked.srt");
        std::fs::create_dir(&blocked).unwrap();
        app.save_lyrics_to(blocked, false);
        assert_eq!(app.lyrics_save.mark, Some(Mark::SrtStale));
        assert!(app.has_unsaved_lyrics(), "the next change tries again");
        // A write that succeeds clears the mark.
        app.save_lyrics_to(folder.path().join("song.srt"), false);
        assert_eq!(app.lyrics_save.mark, None);
        assert!(app.lyrics_save.failed.is_none());
    }

    #[test]
    fn closing_or_replacing_writes_first_and_asks_only_when_it_cannot() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        app.trust_lyrics(source.clone());
        app.edit_track(|t| edit::set_text(t, 0, "Pending"));
        assert!(app.flush_lyrics(), "written: nothing is left to ask about");
        assert!(
            std::fs::read_to_string(&source)
                .unwrap()
                .contains("Pending")
        );
        // Changed outside NOH: not written, so the caller asks.
        app.edit_track(|t| edit::set_text(t, 0, "Again"));
        app.project.edit.as_mut().unwrap().outside_changed = true;
        assert!(!app.flush_lyrics());
        assert!(
            std::fs::read_to_string(&source)
                .unwrap()
                .contains("Pending")
        );
    }

    #[test]
    fn a_lyrics_file_that_could_not_be_created_is_said_once_and_marked() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        app.project.track.loaded.as_mut().unwrap().lyrics_unwritten = Some("denied".into());
        app.lyrics_autosave(&egui::Context::default());
        assert_eq!(app.notice.as_ref().unwrap().key, "lyrics.unwritten");
        assert_eq!(app.lyrics_save.mark, Some(Mark::Unsaved));
        app.notice = None;
        app.lyrics_autosave(&egui::Context::default());
        assert!(app.notice.is_none(), "said once");
        app.edit_track(|t| edit::set_text(t, 0, "x"));
        assert!(
            app.lyrics_save.copy_only,
            "the first edit offers only a copy"
        );
    }

    #[test]
    fn a_copy_leaves_the_original_pair_alone() {
        use noh::subtitle_words as words;
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        let track = app.lyrics_track().unwrap();
        words::write_beside(&source, &track, Some("small")).unwrap();
        let old = std::fs::read(words::path_for(&source)).unwrap();
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        let copy = folder.path().join("song (edited).srt");
        app.save_lyrics_to(copy.clone(), true);
        assert_eq!(std::fs::read(words::path_for(&source)).unwrap(), old);
        assert!(!words::lyrics_path(&source).exists());
        assert!(words::lyrics_path(&copy).exists());
    }

    #[test]
    fn a_lyrics_file_noh_cannot_use_is_never_overwritten() {
        use noh::subtitle_words as words;
        let folder = tempfile::tempdir().unwrap();
        let mut app = app_with_track(folder.path());
        let source = folder.path().join("song.srt");
        let lyrics = words::lyrics_path(&source);
        let srt_before = std::fs::read(&source).unwrap();
        // Newer: the save stops before writing either file.
        let newer = br#"{"version":3,"cues":[]}"#;
        std::fs::write(&lyrics, newer).unwrap();
        observe(&mut app);
        app.edit_track(|t| edit::set_text(t, 0, "Something has to give"));
        app.save_lyrics_to(source.clone(), false);
        assert!(app.lyrics_save.failed.is_some());
        assert!(app.has_unsaved_lyrics());
        assert_eq!(std::fs::read(&lyrics).unwrap(), newer);
        assert_eq!(std::fs::read(&source).unwrap(), srt_before);
        // Damaged: set aside under a free name, then the save writes, and says so.
        std::fs::write(folder.path().join("song.unreadable.json"), b"taken").unwrap();
        std::fs::write(&lyrics, b"{ not json").unwrap();
        observe(&mut app);
        app.lyrics_save.failed = None;
        app.save_lyrics_to(source.clone(), false);
        assert!(app.lyrics_save.failed.is_none() && !app.has_unsaved_lyrics());
        let aside = folder.path().join("song.unreadable-2.json");
        assert_eq!(std::fs::read(&aside).unwrap(), b"{ not json");
        assert_eq!(app.notice.as_ref().unwrap().key, "save.set_aside");
        assert!(words::read_lyrics(&lyrics).unwrap().is_ok());
    }
}
