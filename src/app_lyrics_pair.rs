//! The .srt kept in step with the lyrics file (lyrics plan 4–6; maintainer, 11
//! November: « les fichiers diffèrent ? la réponse est obligatoire pour
//! continuer, on unify »).
//!
//! Each load of the lyrics (an attach, or a change on disk to either file)
//! says how the two files stand (`app_track::Pairing`). When they differ, NOH
//! asks which one is the source of truth; the answer rewrites the other, so
//! the two are identical again. Until it is answered, nothing writes either
//! file and the lane, its keys and the export wait. Closed at load, the lyrics
//! are not opened; closed later, it stays one click away on the lyrics chip.
//! The other situations are handled without a question: a missing .srt or a
//! deleted lyrics file is rewritten at once, a lyrics file changed on disk is
//! reloaded as one undo step.
use super::*;
use app_style::{self as style, space};
use app_track::Pairing;
use noh::subtitle_track::SubtitleTrack;
use std::{path::PathBuf, sync::Arc};

/// The source-of-truth question, while it is unanswered.
pub struct Question {
    /// The .srt it is about.
    pub path: PathBuf,
    pub kind: Kind,
    /// Asked when the lyrics were opened: closing it does not open them.
    pub at_load: bool,
    /// Shown now (closed later, it waits on the lyrics chip).
    pub open: bool,
    /// Reopened by a close request: « Quitter sans enregistrer » replaces
    /// « Plus tard », so quitting never needs an answer that writes.
    pub quitting: bool,
}

pub enum Kind {
    /// Both readable: « Les paroles » or « Le .srt ».
    Differ {
        srt: Arc<SubtitleTrack>,
        lines: usize,
        lose: usize,
        hand: bool,
        /// How many lines would lose their voices by taking the .srt.
        voices_lost: usize,
        /// When each file was last changed, as HH:MM local time, if known.
        srt_time: Option<String>,
        lyrics_time: Option<String>,
    },
    /// The .srt cannot be these lyrics: « Les paroles » only.
    SrtUnreadable { reason: String },
}

#[derive(Default)]
pub struct State {
    pub question: Option<Question>,
    /// The last load looked at (`Attachment::loads`).
    seen: u64,
    /// With no .srt on disk: the copy of the loaded lyrics that exports read,
    /// made once per load (`loads`), so every caller gets the same file.
    pub export_copy: Option<(u64, PathBuf)>,
}

/// When a file was last changed, as HH:MM local time.
fn hhmm(stamp: Option<&noh::inspection::FileStamp>) -> Option<String> {
    stamp.map(|s| {
        chrono::DateTime::<chrono::Local>::from(s.modified)
            .format("%H:%M")
            .to_string()
    })
}

/// A free `<stem>.unreadable.srt` beside `srt`.
fn aside_srt(srt: &std::path::Path) -> PathBuf {
    let stem = srt
        .file_stem()
        .map_or("lyrics".into(), |s| s.to_string_lossy().into_owned());
    (1..)
        .map(|n| {
            srt.with_file_name(if n == 1 {
                format!("{stem}.unreadable.srt")
            } else {
                format!("{stem}.unreadable-{n}.srt")
            })
        })
        .find(|p| {
            matches!(std::fs::symlink_metadata(p),
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound)
        })
        .expect("a free name")
}

impl NohApp {
    /// An unanswered difference: nothing writes, the lane and export wait.
    pub(super) fn lyrics_question_pending(&self) -> bool {
        self.lyrics_pair.question.is_some()
    }

    /// Once per frame, after the session follows its file: look at a new load.
    pub(super) fn lyrics_pair_frame(&mut self) {
        // A question about lyrics that are no longer attached ends with them.
        if self
            .lyrics_pair
            .question
            .as_ref()
            .is_some_and(|q| self.project.track.path.as_ref() != Some(&q.path))
        {
            self.lyrics_pair.question = None;
        }
        let track = &mut self.project.track;
        if track.loads == self.lyrics_pair.seen {
            return;
        }
        self.lyrics_pair.seen = track.loads;
        let Some(path) = track.path.clone() else {
            return;
        };
        let replaced = track.replaced.take();
        let Some(loaded) = &track.loaded else {
            return;
        };
        let pairing = loaded.pairing.clone();
        let (srt_stamp, lyrics_stamp) = (loaded.srt_stamp.clone(), loaded.lyrics_stamp.clone());
        // Each load says anew how the two stand: an earlier question ends.
        self.lyrics_pair.question = None;
        let at_load = replaced.is_none() && self.project.edit.is_none();
        // The lyrics file changed on disk and is usable: it is reloaded, as one
        // undo step (Ctrl+Z gives back what was shown, a pending change too),
        // before anything else, so an answer never writes the old lyrics over it.
        let lyrics_changed = replaced
            .as_ref()
            .is_some_and(|old| old.lyrics_stamp.is_some() && old.lyrics_stamp != lyrics_stamp);
        if lyrics_changed
            && matches!(
                pairing,
                Pairing::Same | Pairing::Differ { .. } | Pairing::SrtUnreadable { .. }
            )
            && let Some(old) = &replaced
        {
            let shown = self
                .project
                .edit
                .as_ref()
                .map(|s| s.track.clone())
                .unwrap_or_else(|| old.track.clone());
            self.keep_as_undo_step(shown);
            self.notice = Some("lyrics.reloaded".into());
        }
        match pairing {
            Pairing::Same => {
                if replaced.is_some() && !lyrics_changed {
                    if let Some(session) = &mut self.project.edit {
                        // The .srt changed in its formatting only: the next
                        // write rewrites it.
                        session.outside_changed = false;
                        self.notice = Some("lyrics.srt_reformatted".into());
                    }
                }
            }
            Pairing::SrtMissing => {
                // Rewritten from the lyrics at once.
                if let Some(track) = self.lyrics_track() {
                    self.rewrite_pair(path, track, false);
                    self.notice = Some("lyrics.srt_rewritten".into());
                }
            }
            Pairing::LyricsMissing => {
                // The lyrics file was deleted: the lyrics in memory (word times
                // included) are the lyrics. They are rewritten at once when the
                // .srt read now says the same; when the .srt was changed too, the
                // question decides (both changed on disk).
                let memory = self
                    .project
                    .edit
                    .as_ref()
                    .map(|s| s.track.clone())
                    .or_else(|| replaced.as_ref().map(|l| l.track.clone()));
                let srt = self.project.track.loaded.as_ref().map(|l| l.track.clone());
                if let (Some(memory), Some(srt)) = (memory, srt) {
                    let lines = noh::subtitle_words::differing_cues(&srt, &memory);
                    if lines == 0 {
                        self.rewrite_pair(path, memory, false);
                        self.notice = Some("lyrics.file_rewritten".into());
                    } else {
                        // The memory is shown as the lyrics the question offers.
                        if self.project.edit.is_none() {
                            self.replace_lyrics(memory.clone());
                        }
                        let mut choice = (*srt).clone();
                        noh::subtitle_words::merge(
                            &mut choice,
                            &noh::subtitle_words::to_lyrics_json(&memory, None),
                        );
                        choice.language = memory.language.clone();
                        let voices_lost = noh::subtitle_words::carry_voices(&memory, &mut choice);
                        let (lose, hand) = app_track::lost_words(&memory, &choice);
                        self.lyrics_pair.question = Some(Question {
                            path,
                            kind: Kind::Differ {
                                srt: Arc::new(choice),
                                lines,
                                lose,
                                hand,
                                voices_lost,
                                srt_time: hhmm(srt_stamp.as_ref()),
                                lyrics_time: None,
                            },
                            at_load: false,
                            open: true,
                            quitting: false,
                        });
                    }
                }
            }
            Pairing::Differ {
                srt,
                lines,
                lose,
                hand,
                voices_lost,
            } => {
                self.lyrics_pair.question = Some(Question {
                    path,
                    kind: Kind::Differ {
                        srt,
                        lines,
                        lose,
                        hand,
                        voices_lost,
                        srt_time: hhmm(srt_stamp.as_ref()),
                        lyrics_time: hhmm(lyrics_stamp.as_ref()),
                    },
                    at_load,
                    open: true,
                    quitting: false,
                });
            }
            Pairing::SrtUnreadable { reason } => {
                self.lyrics_pair.question = Some(Question {
                    path,
                    kind: Kind::SrtUnreadable { reason },
                    at_load,
                    open: true,
                    quitting: false,
                });
            }
            Pairing::LyricsUnusable { reason } => {
                self.notice = Some(Message::new("lyrics.unusable", &[reason]));
            }
        }
        self.ensure_export_copy();
    }

    /// Write `track` as both files at `path` now, as NOH's own repair or an
    /// answer: the consent to rewrite them (no first-edit question after).
    fn rewrite_pair(&mut self, path: PathBuf, track: Arc<SubtitleTrack>, set_aside_srt: bool) {
        if set_aside_srt && path.exists() {
            let aside = aside_srt(&path);
            if let Err(e) = noh::subtitle_words::ensure_writable(&path)
                .and_then(|()| std::fs::rename(&path, &aside))
            {
                self.lyrics_save.failed = Some(e.to_string());
                return;
            }
            // NOH moved it: not a change made outside NOH.
            if let Some(loaded) = &mut self.project.track.loaded {
                loaded.srt_stamp = None;
            }
            self.notice = Some(Message::new(
                "lyrics.srt_set_aside",
                &[aside
                    .file_name()
                    .map_or(String::new(), |n| n.to_string_lossy().into_owned())],
            ));
        }
        self.trust_lyrics(path.clone());
        self.save_track_to(path, false, track);
    }

    /// The answer « Les paroles »: the .srt is rewritten from the lyrics (with
    /// a change not yet written, which is kept).
    pub(super) fn answer_lyrics(&mut self) {
        let Some(question) = self.lyrics_pair.question.take() else {
            return;
        };
        let Some(track) = self.lyrics_track() else {
            return;
        };
        let unreadable = matches!(question.kind, Kind::SrtUnreadable { .. });
        self.rewrite_pair(question.path, track, unreadable);
    }

    /// The answer « Le .srt »: the lyrics take the .srt's lines (identical lines
    /// keep their word times), as one undo step, and both files are written.
    pub(super) fn answer_srt(&mut self) {
        let Some(question) = self.lyrics_pair.question.take() else {
            return;
        };
        let Kind::Differ { srt, .. } = question.kind else {
            self.lyrics_pair.question = Some(question);
            return;
        };
        self.replace_lyrics(srt.clone());
        self.rewrite_pair(question.path, srt, false);
    }

    /// Closing the question: at load the lyrics are not opened and nothing is
    /// written; later it waits on the lyrics chip, one click away.
    pub(super) fn close_lyrics_question(&mut self) {
        let Some(question) = &mut self.lyrics_pair.question else {
            return;
        };
        if question.at_load {
            self.lyrics_pair.question = None;
            self.project.edit = None;
            self.project.track.clear();
            if self.project.apply_subtitles {
                self.project.apply_subtitles = false;
                self.invalidated();
            }
        } else {
            question.open = false;
        }
    }

    /// The window asks to close: a pending change is written first; while a
    /// difference waits, its question comes back with a way to quit that
    /// writes nothing; otherwise the close question.
    pub(super) fn lyrics_close_request(&mut self) {
        if self.flush_lyrics() {
            self.close_now = true;
        } else if let Some(question) = &mut self.lyrics_pair.question {
            question.open = true;
            question.quitting = true;
        } else {
            self.lyrics_save.closing = true;
        }
    }

    /// « Quitter sans enregistrer » in the question: neither file is written.
    pub(super) fn quit_without_writing(&mut self) {
        self.lyrics_pair.question = None;
        self.project.edit = None;
        self.close_now = true;
    }

    pub(super) fn reopen_lyrics_question(&mut self) {
        if let Some(question) = &mut self.lyrics_pair.question {
            question.open = true;
        }
    }

    /// The question, while it is shown.
    pub(super) fn lyrics_pair_dialog(&mut self, ctx: &egui::Context) {
        let Some(question) = &self.lyrics_pair.question else {
            return;
        };
        if !question.open {
            return;
        }
        let quitting = question.quitting;
        let l = self.locale.language;
        let unsaved = self.has_unsaved_lyrics();
        let (body, srt_button) = match &question.kind {
            Kind::Differ {
                lines,
                lose,
                hand,
                voices_lost,
                srt_time,
                lyrics_time,
                ..
            } => (
                // « (.srt modifié à HH:MM, paroles à HH:MM) » when both are known,
                // then « N lignes y perdent leurs voix. » when the .srt would cost some.
                match (srt_time, lyrics_time) {
                    (Some(s), Some(t)) => format!(
                        "{} {}",
                        l.plural("pair.body", *lines),
                        l.format("pair.times", &[s.clone(), t.clone()])
                    ),
                    (Some(s), None) => format!(
                        "{} {}",
                        l.plural("pair.body", *lines),
                        l.format("pair.srt_time", &[s.clone()])
                    ),
                    _ => format!("{}.", l.plural("pair.body", *lines)),
                } + &if *voices_lost > 0 {
                    format!(" {}", l.plural("pair.voices_lost", *voices_lost))
                } else {
                    String::new()
                },
                Some(if unsaved {
                    l.text("pair.srt_unsaved").to_owned()
                } else if *lose == 0 {
                    l.text("pair.srt").to_owned()
                } else if *hand {
                    l.plural("pair.srt_lose_hand", *lose)
                } else {
                    l.plural("pair.srt_lose", *lose)
                }),
            ),
            Kind::SrtUnreadable { reason } => {
                (l.format("pair.unreadable", &[reason.clone()]), None)
            }
        };
        let mut choice = None;
        egui::Modal::new(egui::Id::new("lyrics-pair")).show(ctx, |ui| {
            ui.set_max_width(460.0_f32.min(ctx.content_rect().width() - 64.0));
            ui.spacing_mut().item_spacing.y = space::M;
            ui.label(style::strong(l.text("pair.title"), style::text::HEADING));
            ui.add(egui::Label::new(body).wrap());
            ui.vertical(|ui| {
                if style::button(ui, l.text("pair.lyrics"), true, true, 0.0).clicked() {
                    choice = Some(1);
                }
                if let Some(text) = &srt_button
                    && style::button(ui, text, true, false, 0.0).clicked()
                {
                    choice = Some(2);
                }
                let last = if quitting {
                    l.text("save.discard")
                } else {
                    l.text("pair.later")
                };
                if style::button(ui, last, true, false, 0.0).clicked() {
                    choice = Some(if quitting { 3 } else { 0 });
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(0);
        }
        match choice {
            Some(0) => {
                if let Some(question) = &mut self.lyrics_pair.question {
                    question.quitting = false;
                }
                self.close_lyrics_question();
            }
            Some(3) => self.quit_without_writing(),
            Some(1) => self.answer_lyrics(),
            Some(2) => self.answer_srt(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noh::subtitle_words as words;

    /// A song beside an .srt and its lyrics file, attached and loaded for real.
    fn attached(folder: &std::path::Path, srt: &str, lyrics: Option<&str>) -> NohApp {
        let wav = folder.join("song.wav");
        let data_bytes = 20_u32 * 8_000 * 2;
        let mut header = b"RIFF".to_vec();
        header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
        header.extend_from_slice(&8_000_u32.to_le_bytes());
        header.extend_from_slice(&16_000_u32.to_le_bytes());
        header.extend_from_slice(b"\x02\0\x10\0data");
        header.extend_from_slice(&data_bytes.to_le_bytes());
        std::fs::write(&wav, header).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&wav)
            .unwrap()
            .set_len(44 + u64::from(data_bytes))
            .unwrap();
        let path = folder.join("song.srt");
        if !srt.is_empty() {
            std::fs::write(&path, srt).unwrap();
        }
        if let Some(text) = lyrics {
            let track = noh::subtitle_srt::parse(text, 20_000).unwrap();
            words::write_lyrics(&words::lyrics_path(&path), &track, None).unwrap();
        }
        let mut app = NohApp::default();
        app.project.duration_ms = 20_000;
        app.wav = Some(wav.clone());
        app.project.attach(path, Some(&wav));
        settle(&mut app);
        app
    }

    /// Run the reader until the load arrives, then the frame that looks at it.
    fn settle(app: &mut NohApp) {
        let ctx = egui::Context::default();
        for _ in 0..400 {
            app.project.track.poll(&ctx);
            if !app.project.track.validating() && app.project.track.loads != app.lyrics_pair.seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.lyrics_pair_frame();
    }

    /// A change on disk, as the 2 s check would find it.
    fn changed_on_disk(app: &mut NohApp) {
        app.project.track.recheck();
        settle(app);
    }

    const A: &str =
        "1\n00:00:01,000 --> 00:00:03,000\nla la la\n\n2\n00:00:04,000 --> 00:00:05,000\nlo lo\n\n";
    const B: &str =
        "1\n00:00:01,000 --> 00:00:03,000\nla la la\n\n2\n00:00:04,000 --> 00:00:05,000\nlo li\n\n";

    fn srt_of(folder: &std::path::Path) -> String {
        std::fs::read_to_string(folder.join("song.srt")).unwrap()
    }

    fn lyrics_of(folder: &std::path::Path) -> SubtitleTrack {
        words::read_lyrics(&words::lyrics_path(&folder.join("song.srt")))
            .unwrap()
            .unwrap()
            .track
    }

    #[test]
    fn a_difference_at_load_asks_and_nothing_is_written_until_answered() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), B, Some(A));
        let question = app.lyrics_pair.question.as_ref().expect("asked");
        assert!(question.at_load && question.open);
        let Kind::Differ { lines, lose, .. } = &question.kind else {
            panic!("differ");
        };
        assert_eq!((*lines, *lose), (1, 0));
        // The lane, a write and the export wait.
        assert!(app.lyrics_frozen());
        assert!(!app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "x")));
        app.save_lyrics();
        assert!(!app.lyrics_ready());
        assert_eq!(srt_of(folder.path()), B);
        // Closed at load: the lyrics are not opened, nothing written.
        app.close_lyrics_question();
        assert!(app.project.track.path.is_none() && app.lyrics_pair.question.is_none());
        assert_eq!(srt_of(folder.path()), B);
        assert_eq!(lyrics_of(folder.path()).cues[1].text, "lo lo");
    }

    #[test]
    fn the_lyrics_answer_rewrites_the_srt_and_the_srt_answer_replaces_the_lyrics() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), B, Some(A));
        app.answer_lyrics();
        assert!(app.lyrics_pair.question.is_none());
        assert!(srt_of(folder.path()).contains("lo lo"));
        assert_eq!(lyrics_of(folder.path()).cues[1].text, "lo lo");
        assert!(!app.lyrics_frozen());

        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), B, Some(A));
        app.answer_srt();
        assert!(srt_of(folder.path()).contains("lo li"));
        assert_eq!(lyrics_of(folder.path()).cues[1].text, "lo li");
        // One undo step: Ctrl+Z gives the lyrics back (and writes them).
        assert!(app.undo());
        assert_eq!(app.lyrics_track().unwrap().cues[1].text, "lo lo");
    }

    #[test]
    fn the_srt_answer_names_the_word_times_it_loses() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.srt");
        let mut lyrics = noh::subtitle_srt::parse(A, 20_000).unwrap();
        lyrics.cues[1].words = Some(noh::subtitle_edit::words_or_estimate(&lyrics.cues[1]));
        lyrics.cues[1].words.as_mut().unwrap()[1].source = noh::subtitle_track::WordSource::Hand;
        std::fs::write(&path, B).unwrap();
        words::write_lyrics(&words::lyrics_path(&path), &lyrics, None).unwrap();
        let app = attached(folder.path(), B, None);
        let Some(Question {
            kind: Kind::Differ { lose, hand, .. },
            ..
        }) = &app.lyrics_pair.question
        else {
            panic!("differ");
        };
        assert_eq!((*lose, *hand), (1, true));
    }

    #[test]
    fn the_srt_answer_says_how_many_lines_lose_their_voices() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.srt");
        let mut lyrics = noh::subtitle_srt::parse(A, 20_000).unwrap();
        lyrics.voices = vec![noh::subtitle_track::Voice::default(); 2];
        lyrics.cues[0].voices = vec![1];
        lyrics.cues[1].voices = vec![2];
        std::fs::write(&path, B).unwrap();
        words::write_lyrics(&words::lyrics_path(&path), &lyrics, None).unwrap();
        let app = attached(folder.path(), B, None);
        let Some(Question {
            kind: Kind::Differ {
                srt, voices_lost, ..
            },
            ..
        }) = &app.lyrics_pair.question
        else {
            panic!("differ");
        };
        // B changes only the second line's text, at the same times: no voice is lost.
        assert_eq!(*voices_lost, 0);
        assert_eq!(srt.cues[1].voices, [2]);
    }

    #[test]
    fn a_missing_srt_is_rewritten_from_the_lyrics_at_once() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), "", Some(A));
        assert!(app.lyrics_pair.question.is_none());
        assert_eq!(app.notice.as_ref().unwrap().key, "lyrics.srt_rewritten");
        assert!(srt_of(folder.path()).contains("lo lo"));
        // Deleted while open: the same.
        std::fs::remove_file(folder.path().join("song.srt")).unwrap();
        changed_on_disk(&mut app);
        assert!(srt_of(folder.path()).contains("lo lo"));
    }

    #[test]
    fn an_unreadable_srt_offers_only_the_lyrics_and_is_kept_aside() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), "not an srt", Some(A));
        let question = app.lyrics_pair.question.as_ref().unwrap();
        assert!(matches!(question.kind, Kind::SrtUnreadable { .. }));
        app.answer_srt();
        assert!(app.lyrics_pair.question.is_some(), "no .srt answer for it");
        app.answer_lyrics();
        assert!(srt_of(folder.path()).contains("lo lo"));
        assert_eq!(
            std::fs::read_to_string(folder.path().join("song.unreadable.srt")).unwrap(),
            "not an srt"
        );
    }

    #[test]
    fn a_lyrics_file_changed_on_disk_is_reloaded_as_one_undo_step() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        let path = folder.path().join("song.srt");
        app.trust_lyrics(path.clone());
        app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "pending"));
        // Both files changed outside NOH, to the same new lyrics.
        let b = noh::subtitle_srt::parse(B, 20_000).unwrap();
        words::write_lyrics(&words::lyrics_path(&path), &b, None).unwrap();
        std::fs::write(&path, B).unwrap();
        changed_on_disk(&mut app);
        assert_eq!(app.notice.as_ref().unwrap().key, "lyrics.reloaded");
        assert_eq!(app.lyrics_track().unwrap().cues[1].text, "lo li");
        assert!(app.lyrics_pair.question.is_none());
        assert!(app.undo(), "Ctrl+Z gives back the pending change");
        assert_eq!(app.lyrics_track().unwrap().cues[0].text, "pending");
    }

    #[test]
    fn a_lyrics_file_changed_with_a_pending_change_is_reloaded_before_the_question() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        let path = folder.path().join("song.srt");
        app.trust_lyrics(path.clone());
        app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "pending"));
        let b = noh::subtitle_srt::parse(B, 20_000).unwrap();
        words::write_lyrics(&words::lyrics_path(&path), &b, None).unwrap();
        changed_on_disk(&mut app);
        assert!(app.lyrics_question_pending());
        assert_eq!(app.lyrics_track().unwrap().cues[1].text, "lo li");
        // « Les paroles » writes the lyrics file as changed outside NOH.
        app.answer_lyrics();
        assert!(srt_of(folder.path()).contains("lo li"));
        assert_eq!(lyrics_of(folder.path()).cues[0].text, "la la la");
        // Ctrl+Z gives the pending change back.
        assert!(app.undo());
        assert_eq!(app.lyrics_track().unwrap().cues[0].text, "pending");
    }

    #[test]
    fn a_deleted_lyrics_file_with_an_srt_changed_too_asks() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        let path = folder.path().join("song.srt");
        std::fs::remove_file(words::lyrics_path(&path)).unwrap();
        std::fs::write(&path, B).unwrap();
        changed_on_disk(&mut app);
        assert!(app.lyrics_question_pending(), "both changed: the question");
        assert_eq!(srt_of(folder.path()), B, "nothing written yet");
        assert!(!words::lyrics_path(&path).exists());
        // The lyrics offered are the memory; « Le .srt » takes the edit.
        assert_eq!(app.lyrics_track().unwrap().cues[1].text, "lo lo");
        app.answer_srt();
        assert_eq!(lyrics_of(folder.path()).cues[1].text, "lo li");
        assert!(srt_of(folder.path()).contains("lo li"));
    }

    #[test]
    fn closing_while_a_difference_waits_can_quit_without_writing() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        let path = folder.path().join("song.srt");
        // The lyrics file deleted and the .srt changed: the question, with the
        // lyrics in memory as a pending change.
        std::fs::remove_file(words::lyrics_path(&path)).unwrap();
        std::fs::write(&path, B).unwrap();
        changed_on_disk(&mut app);
        app.close_lyrics_question();
        assert!(app.lyrics_question_pending() && app.has_unsaved_lyrics());
        app.lyrics_close_request();
        let question = app.lyrics_pair.question.as_ref().unwrap();
        assert!(
            question.open && question.quitting,
            "back, with a way to quit"
        );
        assert!(!app.close_now);
        app.quit_without_writing();
        assert!(app.close_now);
        assert_eq!(srt_of(folder.path()), B, "neither file written");
        assert!(!words::lyrics_path(&path).exists());
    }

    /// The cues an export would read from `captions_file`.
    fn exported(app: &NohApp) -> SubtitleTrack {
        let (path, snapshot) = app.captions_file().unwrap();
        snapshot.verify().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        noh::subtitle_srt::parse(&text, 20_000).unwrap()
    }

    #[test]
    fn the_export_writes_a_pending_change_first_and_reads_the_lyrics() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        app.project.apply_subtitles = true;
        app.trust_lyrics(folder.path().join("song.srt"));
        app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "pending"));
        assert!(
            app.prepare_lyrics_export().unwrap().is_empty(),
            "written: nothing to say"
        );
        assert!(!app.has_unsaved_lyrics());
        assert_eq!(lyrics_of(folder.path()).cues[0].text, "pending");
        assert_eq!(exported(&app).cues[0].text, "pending");
    }

    #[test]
    fn an_export_from_unsaved_lyrics_reads_memory_and_says_so() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        app.project.apply_subtitles = true;
        // Not allowed to write (the first-edit question was never answered).
        app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "memory"));
        let warnings = app.prepare_lyrics_export().unwrap();
        assert_eq!(warnings, [Message::from("lyrics.export_unsaved")]);
        assert_eq!(exported(&app).cues[0].text, "memory");
        assert_eq!(srt_of(folder.path()), A, "nothing written");
    }

    #[test]
    fn an_export_with_a_stale_srt_reads_the_lyrics_and_says_so() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        app.project.apply_subtitles = true;
        let path = folder.path().join("song.srt");
        app.trust_lyrics(path.clone());
        let read_only = |on: bool| {
            let mut p = std::fs::metadata(&path).unwrap().permissions();
            p.set_readonly(on);
            std::fs::set_permissions(&path, p).unwrap();
        };
        read_only(true);
        app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "lyrics"));
        let warnings = app.prepare_lyrics_export().unwrap();
        read_only(false);
        // The lyrics file is written, the .srt is not: the export reads the
        // lyrics (in memory, equal to the file) and the report says so.
        assert_eq!(warnings, [Message::from("lyrics.export_srt_stale")]);
        assert_eq!(lyrics_of(folder.path()).cues[0].text, "lyrics");
        assert_eq!(
            app.lyrics_save.mark,
            Some(super::super::app_lyrics_save::Mark::SrtStale)
        );
        assert_eq!(exported(&app).cues[0].text, "lyrics");
        assert_eq!(srt_of(folder.path()), A);
    }

    #[test]
    fn a_missing_srt_that_cannot_be_rewritten_exports_from_memory() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        app.project.apply_subtitles = true;
        // As after a failed rewrite: no .srt on disk, the lyrics loaded.
        std::fs::remove_file(folder.path().join("song.srt")).unwrap();
        app.project.track.loaded.as_mut().unwrap().srt_stamp = None;
        app.ensure_export_copy();
        assert!(app.lyrics_ready());
        assert_eq!(exported(&app).cues[1].text, "lo lo");
        // One copy per load: every caller reads the file its snapshot guards.
        let (first, snapshot) = app.captions_file().unwrap();
        let (second, _) = app.captions_file().unwrap();
        assert_eq!(first, second);
        assert_eq!(snapshot.0[0].path, std::path::absolute(&first).unwrap());
        snapshot.verify().unwrap();
    }

    #[test]
    fn a_file_changed_on_disk_since_it_was_read_stops_the_export_and_is_read_again() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        app.project.apply_subtitles = true;
        let path = folder.path().join("song.srt");
        let b = noh::subtitle_srt::parse(B, 20_000).unwrap();
        words::write_lyrics(&words::lyrics_path(&path), &b, None).unwrap();
        assert!(app.prepare_lyrics_export().is_err());
        settle(&mut app);
        assert!(
            app.lyrics_question_pending(),
            "the question comes before the export"
        );
    }

    #[test]
    fn an_unusable_lyrics_file_exports_from_the_srt_and_says_so() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.srt");
        std::fs::write(&path, B).unwrap();
        std::fs::write(words::lyrics_path(&path), br#"{"version":4,"cues":[]}"#).unwrap();
        let mut app = attached(folder.path(), B, None);
        app.project.apply_subtitles = true;
        let warnings = app.prepare_lyrics_export().unwrap();
        assert_eq!(warnings, [Message::from("lyrics.export_unusable")]);
        assert_eq!(exported(&app).cues[1].text, "lo li");
    }

    #[test]
    fn a_failed_srt_write_is_retried_without_the_question() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        let path = folder.path().join("song.srt");
        app.trust_lyrics(path.clone());
        let read_only = |on: bool| {
            let mut p = std::fs::metadata(&path).unwrap().permissions();
            p.set_readonly(on);
            std::fs::set_permissions(&path, p).unwrap();
        };
        read_only(true);
        app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "pending"));
        app.save_lyrics();
        assert_eq!(
            app.lyrics_save.mark,
            Some(super::super::app_lyrics_save::Mark::SrtStale)
        );
        assert_eq!(lyrics_of(folder.path()).cues[0].text, "pending");
        // The 2 s check sees NOH's own lyrics file: no reload, no question.
        let loads = app.project.track.loads;
        let ctx = egui::Context::default();
        for _ in 0..40 {
            app.project.track.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.lyrics_pair_frame();
        assert_eq!(app.project.track.loads, loads);
        assert!(!app.lyrics_question_pending());
        // The retry writes the .srt.
        read_only(false);
        app.lyrics_save.failed = None;
        app.save_lyrics();
        assert!(srt_of(folder.path()).contains("pending"));
        assert_eq!(app.lyrics_save.mark, None);
    }

    #[test]
    fn a_lyrics_file_changed_alone_then_differs_and_asks() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        let path = folder.path().join("song.srt");
        let b = noh::subtitle_srt::parse(B, 20_000).unwrap();
        words::write_lyrics(&words::lyrics_path(&path), &b, None).unwrap();
        changed_on_disk(&mut app);
        let question = app.lyrics_pair.question.as_ref().expect("asked");
        assert!(!question.at_load, "while open: closing keeps it pending");
        app.close_lyrics_question();
        assert!(app.lyrics_question_pending() && app.project.track.path.is_some());
        app.reopen_lyrics_question();
        assert!(app.lyrics_pair.question.as_ref().unwrap().open);
    }

    #[test]
    fn an_srt_changed_on_disk_with_a_pending_change_asks_and_keeps_it() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        let path = folder.path().join("song.srt");
        app.trust_lyrics(path.clone());
        app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "pending"));
        std::fs::write(&path, B).unwrap();
        // The write finds the .srt changed: nothing written, both read again.
        app.save_lyrics();
        assert_eq!(srt_of(folder.path()), B);
        settle(&mut app);
        assert!(app.lyrics_question_pending());
        // « Les paroles » keeps the pending change and rewrites the .srt.
        app.answer_lyrics();
        assert!(srt_of(folder.path()).contains("pending"));
        assert_eq!(lyrics_of(folder.path()).cues[0].text, "pending");
    }

    #[test]
    fn a_deleted_lyrics_file_is_rewritten_from_memory_with_its_word_times() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.srt");
        let mut lyrics = noh::subtitle_srt::parse(A, 20_000).unwrap();
        lyrics.cues[0].words = Some(noh::subtitle_edit::words_or_estimate(&lyrics.cues[0]));
        std::fs::write(&path, A).unwrap();
        words::write_lyrics(&words::lyrics_path(&path), &lyrics, None).unwrap();
        let mut app = attached(folder.path(), A, None);
        std::fs::remove_file(words::lyrics_path(&path)).unwrap();
        changed_on_disk(&mut app);
        assert_eq!(app.notice.as_ref().unwrap().key, "lyrics.file_rewritten");
        assert!(lyrics_of(folder.path()).cues[0].words.is_some());
    }

    #[test]
    fn an_unusable_lyrics_file_uses_the_srt_and_says_why() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.srt");
        std::fs::write(&path, A).unwrap();
        std::fs::write(words::lyrics_path(&path), b"{ not json").unwrap();
        let app = attached(folder.path(), A, None);
        assert_eq!(app.notice.as_ref().unwrap().key, "lyrics.unusable");
        assert!(app.lyrics_pair.question.is_none());
        assert_eq!(app.lyrics_track().unwrap().cues.len(), 2);
    }

    #[test]
    fn an_srt_reformatted_on_disk_keeps_the_pending_change() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = attached(folder.path(), A, Some(A));
        let path = folder.path().join("song.srt");
        app.trust_lyrics(path.clone());
        app.edit_track(|t| noh::subtitle_edit::set_text(t, 0, "pending"));
        std::fs::write(&path, A.replace('\n', "\r\n")).unwrap();
        changed_on_disk(&mut app);
        assert_eq!(app.notice.as_ref().unwrap().key, "lyrics.srt_reformatted");
        assert_eq!(app.lyrics_track().unwrap().cues[0].text, "pending");
        app.save_lyrics();
        assert!(srt_of(folder.path()).contains("pending"));
    }
}
