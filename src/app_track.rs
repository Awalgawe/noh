//! One imported/generated subtitle attachment. Reads and identity checks stay off the UI thread.
use noh::{
    engine::EngineError,
    i18n::Message,
    inspection::{FileStamp, Snapshot},
    subtitle_track::SubtitleTrack,
};
use std::{
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Request {
    generation: u64,
    path: PathBuf,
    wav: PathBuf,
    /// The FFmpeg chosen for this project (empty: NOH's own).
    ffmpeg: PathBuf,
    duration_ms: u64,
    /// An attach: an .srt alone gets its lyrics file (decision B). A reload
    /// after a change on disk does not: a deleted lyrics file is rewritten from
    /// the lyrics in memory instead.
    create: bool,
}

/// How the .srt and the lyrics file beside it stand, as a load read them
/// (lyrics plan 6). The lyrics file is the reference whenever it is usable.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Pairing {
    /// They say the same, or there is only the .srt (its lyrics file was just
    /// created, or only a version-1 file is beside it).
    #[default]
    Same,
    /// A usable lyrics file and no .srt: the .srt is rewritten from it.
    SrtMissing,
    /// An .srt and no lyrics file under either name, on a reload: the lyrics
    /// file was deleted; it is rewritten from the lyrics in memory.
    LyricsMissing,
    /// Both readable, and different: the question. `srt` is the .srt's cues
    /// with the word times of the lyrics' identical cues; `lines` how many
    /// lines differ; `lose` how many lines of the lyrics would lose their word
    /// times by taking the .srt, `hand` whether some of those were set by hand,
    /// `voices_lost` how many lines would lose their voices (`carry_voices`).
    Differ {
        srt: Arc<SubtitleTrack>,
        lines: usize,
        lose: usize,
        hand: bool,
        voices_lost: usize,
    },
    /// A usable lyrics file and an .srt that cannot be used as these lyrics
    /// (not SRT, or not fitting the song): the question, « Les paroles » only.
    SrtUnreadable { reason: String },
    /// A lyrics file NOH cannot use (damaged, newer, locked): the .srt is used.
    LyricsUnusable { reason: String },
}
pub struct Loaded {
    pub track: Arc<SubtitleTrack>,
    pub snapshot: Snapshot,
    /// The Whisper model that generated these lyrics, from the words file.
    pub model: Option<String>,
    /// Why the lyrics file could not be created beside an .srt opened alone
    /// (decision B): the lyrics then live in memory until a write succeeds.
    pub lyrics_unwritten: Option<String>,
    /// The .srt and the lyrics file as this load read them (None: absent).
    /// A write goes ahead only while both are still so (plan 5).
    pub srt_stamp: Option<FileStamp>,
    pub lyrics_stamp: Option<FileStamp>,
    pub pairing: Pairing,
}
enum Reply {
    Loaded(Request, Result<Loaded, EngineError>),
    Checked {
        generation: u64,
        stamps: (Option<FileStamp>, Option<FileStamp>),
        /// `own_writes` when the check began.
        own_writes: u64,
    },
}

#[derive(Default)]
pub struct Attachment {
    /// Last successfully attached track (or the initial candidate when none is loaded).
    pub path: Option<PathBuf>,
    /// The FFmpeg the project uses (empty: NOH's own), set with its inputs. A
    /// song whose copy left the cache is decoded again with it to check its length.
    pub ffmpeg: PathBuf,
    pub loaded: Option<Loaded>,
    pub error: Option<EngineError>,
    /// A read failed since the app last looked: `error` is new (taken by the
    /// app to say a refused .srt even while other lyrics stay attached).
    pub fresh_error: bool,
    /// Only committed changes invalidate an otherwise current rendered preview.
    pub revision: u64,
    generation: u64,
    pending: Option<Request>,
    work: Option<(u64, Receiver<Reply>)>,
    reading: bool,
    source: Option<(PathBuf, u64)>,
    /// The .srt and the lyrics file as last read or written by NOH.
    observed: (Option<FileStamp>, Option<FileStamp>),
    checked: Option<Instant>,
    /// Counts NOH's own writes that change `observed` without a new
    /// generation: a check begun before one of them is ignored.
    own_writes: u64,
    /// Counts the loads that arrived, so the app looks at each pairing once.
    pub loads: u64,
    /// The lyrics before a change on disk replaced them (which file changed,
    /// and the lyrics a deleted lyrics file is rewritten from).
    pub replaced: Option<Loaded>,
}

impl Attachment {
    /// The FFmpeg a requested read will use, before it starts.
    #[cfg(test)]
    pub fn pending_ffmpeg(&self) -> Option<&std::path::Path> {
        self.pending.as_ref().map(|r| r.ffmpeg.as_path())
    }
    pub fn validating(&self) -> bool {
        self.pending.is_some()
            || self.reading
                && self
                    .work
                    .as_ref()
                    .is_some_and(|(generation, _)| *generation == self.generation)
    }
    pub fn loading(&self) -> bool {
        self.validating()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn clear(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.generation = self.generation.wrapping_add(1);
        self.path = None;
        self.loaded = None;
        self.error = None;
        self.pending = None;
        self.source = None;
        self.observed = Default::default();
        self.checked = None;
        self.replaced = None;
    }
    /// NOH just wrote `track` to `path` (a save): that file is now the attached
    /// one, its stamp observed, so the 2 s check does not take NOH's own write
    /// for a change made outside NOH. A late read of the old file is ignored.
    pub fn adopt(
        &mut self,
        path: PathBuf,
        track: Arc<SubtitleTrack>,
        snapshot: Snapshot,
        lyrics_stamp: Option<FileStamp>,
        new_source: bool,
    ) {
        if new_source || self.path.as_ref() != Some(&path) {
            self.revision = self.revision.wrapping_add(1);
        }
        self.generation = self.generation.wrapping_add(1);
        let srt_stamp = snapshot.0.first().cloned();
        self.observed = (srt_stamp.clone(), lyrics_stamp.clone());
        self.path = Some(path);
        // NOH's own save: the lyrics keep the model that generated them.
        let model = self
            .loaded
            .as_ref()
            .or(self.replaced.as_ref())
            .and_then(|l| l.model.clone());
        self.replaced = None;
        self.loaded = Some(Loaded {
            track,
            snapshot,
            model,
            lyrics_unwritten: None,
            srt_stamp,
            lyrics_stamp,
            pairing: Pairing::Same,
        });
        self.error = None;
        self.pending = None;
        self.checked = Some(Instant::now());
    }
    /// Keep the filename, but never use validation from a different WAV clock.
    pub fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.generation = self.generation.wrapping_add(1);
        self.loaded = None;
        self.error = None;
        self.pending = None;
        self.source = None;
        self.checked = None;
    }
    /// A malformed replacement leaves the existing valid track in place. Its
    /// generation still changes, preventing a late transcription from replacing it.
    pub fn request(&mut self, path: PathBuf, wav: PathBuf, duration_ms: u64) {
        // An attach: the lyrics replaced by a change on disk belong to the
        // previous request (recheck sets them again after this).
        self.replaced = None;
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
        if self.path.is_none() {
            self.path = Some(path.clone());
        }
        self.pending = Some(Request {
            generation: self.generation,
            path,
            wav,
            ffmpeg: self.ffmpeg.clone(),
            duration_ms,
            create: true,
        });
    }
    /// Read both files again (a change on disk, or a write that found them
    /// changed): the lyrics shown so far are kept in `replaced`.
    pub fn recheck(&mut self) {
        let (Some(path), Some((wav, duration))) = (self.path.clone(), self.source.clone()) else {
            return;
        };
        let replaced = self.loaded.take().or_else(|| self.replaced.take());
        self.revision = self.revision.wrapping_add(1);
        self.request(path, wav, duration);
        self.replaced = replaced;
        if let Some(request) = &mut self.pending {
            request.create = false;
        }
    }
    /// NOH wrote the lyrics file and then failed to write the .srt: that
    /// lyrics file is NOH's own write, not a change made outside NOH (plan 5).
    pub fn own_lyrics_write(&mut self, stamp: Option<FileStamp>) {
        self.observed.1 = stamp.clone();
        // A check already under way read the files before this write: ignored.
        self.own_writes = self.own_writes.wrapping_add(1);
        if let Some(loaded) = &mut self.loaded {
            loaded.lyrics_stamp = stamp;
        }
    }
    /// One coalesced bounded read/check at a time. Metadata-only checks run every
    /// two seconds, including after an attached file becomes malformed or missing.
    pub fn poll(&mut self, ctx: &eframe::egui::Context) -> bool {
        let mut changed = false;
        if let Some((generation, work)) = &self.work {
            let generation = *generation;
            match work.try_recv() {
                Ok(reply) => {
                    self.work = None;
                    if generation == self.generation {
                        match reply {
                            Reply::Loaded(request, result) => match result {
                                Ok(loaded) => {
                                    self.path = Some(request.path);
                                    self.source = Some((request.wav, request.duration_ms));
                                    self.observed =
                                        (loaded.srt_stamp.clone(), loaded.lyrics_stamp.clone());
                                    self.loaded = Some(loaded);
                                    self.loads = self.loads.wrapping_add(1);
                                    self.error = None;
                                    self.revision = self.revision.wrapping_add(1);
                                    changed = true;
                                }
                                Err(error) => {
                                    if self.loaded.is_none()
                                        && self.path.as_ref() == Some(&request.path)
                                    {
                                        self.source = Some((request.wav, request.duration_ms));
                                    }
                                    self.error = Some(error);
                                    self.fresh_error = true;
                                }
                            },
                            Reply::Checked {
                                generation,
                                stamps,
                                own_writes,
                            } if generation == self.generation => {
                                // Either file changed on disk: both are read again.
                                if own_writes == self.own_writes && stamps != self.observed {
                                    self.observed = stamps;
                                    self.recheck();
                                    changed = true;
                                }
                            }
                            Reply::Checked { .. } => {}
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.work = None;
                    if generation == self.generation {
                        self.error = Some(EngineError::new(
                            "error.caption_track",
                            "read_subtitles",
                            self.path.as_deref(),
                            "Subtitle reader stopped unexpectedly.",
                        ));
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.work.is_none()
            && let Some(request) = self.pending.take()
        {
            let (tx, rx) = mpsc::sync_channel(1);
            let ctx = ctx.clone();
            self.work = Some((request.generation, rx));
            self.reading = true;
            std::thread::spawn(move || {
                let result = load(&request);
                let _ = tx.send(Reply::Loaded(request, result));
                ctx.request_repaint();
            });
        } else if self.work.is_none()
            && self.source.is_some()
            && self
                .checked
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
            && let Some(path) = self.path.clone()
        {
            let generation = self.generation;
            let own_writes = self.own_writes;
            let (tx, rx) = mpsc::sync_channel(1);
            let ctx = ctx.clone();
            self.checked = Some(Instant::now());
            self.work = Some((generation, rx));
            self.reading = false;
            std::thread::spawn(move || {
                let stamps = (
                    FileStamp::read(&path).ok(),
                    FileStamp::read(&noh::subtitle_words::lyrics_path(&path)).ok(),
                );
                let _ = tx.send(Reply::Checked {
                    generation,
                    stamps,
                    own_writes,
                });
                ctx.request_repaint();
            });
        }
        if self.path.is_some() {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        changed
    }
}

/// The read error of an .srt whose line starts before the previous one ends;
/// its detail is that line's number (from 1).
pub const OVERLAP: &str = "lyrics.srt_overlap";

/// What to tell of a refused .srt: which line starts before the previous ends.
pub fn overlap_message(error: &EngineError) -> Option<Message> {
    let cue: usize = (error.code == OVERLAP).then(|| error.detail.parse().ok())??;
    Some(Message::new(
        OVERLAP,
        &[cue.saturating_sub(1).to_string(), cue.to_string()],
    ))
}

fn load(request: &Request) -> Result<Loaded, EngineError> {
    use noh::subtitle_words as words;
    let error = |detail: String| {
        EngineError::new(
            "error.caption_track",
            "read_subtitles",
            Some(&request.path),
            detail,
        )
    };
    let after_end = || {
        EngineError::new(
            "lyrics.after_end",
            "read_subtitles",
            Some(&request.path),
            "Some subtitle lines end after the song.",
        )
    };
    let wav_stamp = FileStamp::read(&request.wav)?;
    // The song's length as its WAV gives it: an MP3's copy, made when it was
    // dropped, is found again here, or made again with the project's FFmpeg.
    let actual_duration = noh::song::duration(&request.wav, &request.ffmpeg, &|| false)
        .map_err(|e| error(e.to_string()))?;
    if (actual_duration * 1_000.0).round() as u64 != request.duration_ms {
        return Err(EngineError::new(
            "error.stale_inspection",
            "read_subtitles",
            Some(&request.wav),
            "WAV duration changed; refresh the project before attaching its subtitles.",
        ));
    }
    // The lyrics file: usable lyrics, absent, or unusable (with the reason).
    let lyrics_path = words::lyrics_path(&request.path);
    let lyrics_stamp = FileStamp::read(&lyrics_path).ok();
    let lyrics = match words::read_lyrics(&lyrics_path) {
        Ok(Ok(lyrics)) => Ok(Some(lyrics)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(Err(e)) => Err(e.to_string()),
    };
    // Usable lyrics must fit the song, as an .srt must.
    let fitted = |lyrics: &words::Lyrics| -> Result<SubtitleTrack, EngineError> {
        let mut track = lyrics.track.clone();
        track.duration_ms = request.duration_ms;
        track.validate().map_err(|_| after_end())?;
        Ok(track)
    };
    // The .srt: absent, unreadable as text (with the reason), or its text.
    let srt_stamp = if request.path.exists() {
        Some(FileStamp::read(&request.path)?)
    } else {
        None
    };
    let srt_text = srt_stamp.as_ref().map(|stamp| {
        if stamp.bytes > noh::subtitle_srt::MAX_INPUT_BYTES as u64 {
            return Err("SRT input exceeds one MiB.".to_owned());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&request.path)
            .and_then(|f| {
                f.take(noh::subtitle_srt::MAX_INPUT_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|e| e.to_string())?;
        if bytes.len() > noh::subtitle_srt::MAX_INPUT_BYTES {
            return Err("SRT input exceeds one MiB.".to_owned());
        }
        String::from_utf8(bytes).map_err(|e| e.to_string())
    });
    let mut lyrics_unwritten = None;
    let mut lyrics_stamp = lyrics_stamp;
    let (track, model, pairing) = match (srt_text, lyrics) {
        (None, Ok(Some(lyrics))) => (fitted(&lyrics)?, lyrics.model, Pairing::SrtMissing),
        (None, _) => {
            // No .srt and no lyrics to rebuild it from: the error an .srt gets.
            return Err(FileStamp::read(&request.path)
                .err()
                .unwrap_or_else(|| error("The subtitle file is missing.".to_owned())));
        }
        (Some(Err(reason)), Ok(Some(lyrics))) => (
            fitted(&lyrics)?,
            lyrics.model,
            Pairing::SrtUnreadable { reason },
        ),
        (Some(Err(reason)), _) => return Err(error(reason)),
        (Some(Ok(text)), Ok(Some(lyrics))) => {
            let shown = fitted(&lyrics)?;
            let pairing = match words::parse_srt_unbound(&text) {
                Err(e) => Pairing::SrtUnreadable {
                    reason: e.to_string(),
                },
                Ok(unbound) => match words::differing_cues(&unbound, &lyrics.track) {
                    0 => Pairing::Same,
                    lines => match noh::subtitle_srt::parse(&text, request.duration_ms) {
                        // An .srt that does not fit the song cannot be chosen.
                        Err(_) => Pairing::SrtUnreadable {
                            reason: "Some subtitle lines end after the song.".to_owned(),
                        },
                        Ok(mut srt) => {
                            words::merge(&mut srt, &words::to_lyrics_json(&lyrics.track, None));
                            srt.language = lyrics.track.language.clone();
                            let voices_lost = words::carry_voices(&lyrics.track, &mut srt);
                            let (lose, hand) = lost_words(&lyrics.track, &srt);
                            Pairing::Differ {
                                srt: Arc::new(srt),
                                lines,
                                lose,
                                hand,
                                voices_lost,
                            }
                        }
                    },
                },
            };
            (shown, lyrics.model, pairing)
        }
        (Some(Ok(text)), lyrics) => {
            let mut track = match noh::subtitle_srt::parse(&text, request.duration_ms) {
                Ok(track) => track,
                // A line that starts before the previous one ends: this NOH
                // reads no lines sung at once yet. Refused, saying which lines.
                Err(noh::subtitle_srt::SrtError::Validation {
                    source: noh::subtitle_track::ValidationError::Order { cue },
                    ..
                }) => {
                    return Err(EngineError::new(
                        OVERLAP,
                        "read_subtitles",
                        Some(&request.path),
                        cue,
                    ));
                }
                Err(duration_error) => {
                    return Err(match words::parse_srt_unbound(&text) {
                        Ok(_) => after_end(),
                        Err(_) => error(duration_error.to_string()),
                    });
                }
            };
            // Word times kept in a version-1 file, for the cues that still match.
            let old = words::path_for(&request.path);
            if lyrics.is_ok() && old.exists() {
                words::read_beside(&request.path, &mut track);
            }
            let model = if lyrics.is_ok() {
                words::model_beside(&request.path)
            } else {
                None
            };
            let pairing = match lyrics {
                Err(reason) => Pairing::LyricsUnusable { reason },
                Ok(_) if old.exists() => Pairing::Same,
                Ok(_) if request.create => {
                    // Decision B: an .srt opened alone gets its lyrics file.
                    lyrics_unwritten = words::create_lyrics(&lyrics_path, &track, None)
                        .err()
                        .map(|e| e.to_string());
                    lyrics_stamp = FileStamp::read(&lyrics_path).ok();
                    Pairing::Same
                }
                Ok(_) => Pairing::LyricsMissing,
            };
            (track, model, pairing)
        }
    };
    let snapshot = Snapshot(srt_stamp.iter().cloned().chain([wav_stamp]).collect());
    snapshot.verify()?;
    Ok(Loaded {
        track: Arc::new(track),
        snapshot,
        model,
        lyrics_unwritten,
        srt_stamp,
        lyrics_stamp,
        pairing,
    })
}

/// How many lines of `lyrics` with word times have no identical line in `srt`
/// (they lose their word times if the .srt is chosen), and whether some of
/// those times were set by hand.
pub fn lost_words(lyrics: &SubtitleTrack, srt: &SubtitleTrack) -> (usize, bool) {
    let mut lost = (0, false);
    for cue in &lyrics.cues {
        let Some(words) = &cue.words else { continue };
        let kept = srt
            .cues
            .iter()
            .any(|c| c.start_ms == cue.start_ms && c.end_ms == cue.end_ms && c.text == cue.text);
        if !kept {
            lost.0 += 1;
            lost.1 |= words.iter().any(|w| {
                w.source == noh::subtitle_track::WordSource::Hand
                    || w.syllables.as_ref().is_some_and(|s| {
                        s.iter()
                            .any(|s| s.source == noh::subtitle_track::WordSource::Hand)
                    })
            });
        }
    }
    lost
}

#[cfg(test)]
mod tests {
    use super::*;
    use noh::i18n::Language;
    #[test]
    fn an_attach_drops_the_lyrics_a_change_on_disk_replaced() {
        let (_dir, request) = fixture();
        let mut attachment = Attachment {
            path: Some(request.path.clone()),
            loaded: Some(load(&request).unwrap()),
            source: Some((request.wav.clone(), request.duration_ms)),
            ..Default::default()
        };
        attachment.recheck();
        assert!(attachment.replaced.is_some() && attachment.loaded.is_none());
        assert!(
            !attachment.pending.as_ref().unwrap().create,
            "a reload creates nothing"
        );
        attachment.request(
            request.path.clone(),
            request.wav.clone(),
            request.duration_ms,
        );
        assert!(attachment.replaced.is_none());
        assert!(attachment.pending.as_ref().unwrap().create);
    }

    #[test]
    fn an_srt_opened_alone_gets_its_lyrics_file_and_an_existing_one_is_kept() {
        use noh::subtitle_words as words;
        let (_dir, request) = fixture();
        let lyrics = words::lyrics_path(&request.path);
        let loaded = load(&request).unwrap();
        assert!(loaded.lyrics_unwritten.is_none());
        let file = words::read_lyrics(&lyrics).unwrap().unwrap();
        assert_eq!(file.track.cues, loaded.track.cues);
        // A file already there is never replaced, usable or not.
        std::fs::write(&lyrics, b"{ not json").unwrap();
        load(&request).unwrap();
        assert_eq!(std::fs::read(&lyrics).unwrap(), b"{ not json");
        // A version-1 file alone is left for the next save (decision C).
        std::fs::remove_file(&lyrics).unwrap();
        let mut worded = (*loaded.track).clone();
        worded.cues[0].words = Some(noh::subtitle_edit::words_or_estimate(&worded.cues[0]));
        words::write_beside(&request.path, &worded, None).unwrap();
        let again = load(&request).unwrap();
        assert!(!lyrics.exists());
        assert!(again.track.cues[0].words.is_some());
    }

    #[test]
    fn the_lyrics_file_gives_its_language_when_it_matches_the_srt() {
        use noh::subtitle_words as words;
        let (_dir, request) = fixture();
        let mut track = (*load(&request).unwrap().track).clone();
        track.language = Some("fr".into());
        words::write_lyrics(&words::lyrics_path(&request.path), &track, None).unwrap();
        assert_eq!(
            load(&request).unwrap().track.language.as_deref(),
            Some("fr")
        );
        // Different text: the lyrics file is shown (the reference) and the
        // difference waits for its question, with the .srt's lines ready.
        track.cues[0].text = "Autre".into();
        words::write_lyrics(&words::lyrics_path(&request.path), &track, None).unwrap();
        let loaded = load(&request).unwrap();
        assert_eq!(loaded.track.cues[0].text, "Autre");
        let Pairing::Differ { srt, lines, .. } = &loaded.pairing else {
            panic!("{:?}", loaded.pairing);
        };
        assert_eq!((srt.cues[0].text.as_str(), *lines), ("Text", 1));
    }

    fn fixture() -> (tempfile::TempDir, Request) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("字幕.srt");
        let wav = dir.path().join("music.wav");
        let mut header = b"RIFF".to_vec();
        let data_bytes = 20_u32 * 8_000 * 2;
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
        std::fs::write(&path, "1\n00:00:14,000 --> 00:00:16,000\nText\n").unwrap();
        (
            dir,
            Request {
                generation: 1,
                path,
                wav,
                ffmpeg: PathBuf::new(),
                duration_ms: 20_000,
                create: true,
            },
        )
    }
    #[test]
    fn external_changes_and_bounds_require_fresh_validation() {
        let (_dir, request) = fixture();
        let loaded = load(&request).unwrap();
        assert_eq!(
            loaded.track.trim(13_000, 18_000).unwrap().cues[0].start_ms,
            1_000
        );
        assert!(
            load(&Request {
                duration_ms: 15_000,
                ..request.clone()
            })
            .is_err()
        );
        std::fs::write(&request.path, "malformed replacement").unwrap();
        assert!(loaded.snapshot.verify().is_err());
        // The lyrics file made at the first load is the reference: the .srt
        // that is no longer SRT waits for the question.
        assert!(matches!(
            load(&request).unwrap().pairing,
            Pairing::SrtUnreadable { .. }
        ));
        std::fs::remove_file(noh::subtitle_words::lyrics_path(&request.path)).unwrap();
        assert!(load(&request).is_err(), "without lyrics, as before");
    }
    #[test]
    fn cues_after_song_return_a_readable_after_end_error() {
        let (_dir, request) = fixture();
        std::fs::write(
            &request.path,
            "1\n00:00:14,000 --> 00:00:16,000\nText\n\n2\n00:00:21,000 --> 00:00:22,000\nLater\n",
        )
        .unwrap();

        let error = match load(&request) {
            Ok(_) => panic!("out-of-duration lyrics unexpectedly loaded"),
            Err(error) => error,
        };

        assert_eq!(error.code, "lyrics.after_end");
    }

    #[test]
    fn lines_sung_at_once_are_refused_saying_which_lines() {
        let (_dir, request) = fixture();
        // Line 2 starts before line 1 ends.
        std::fs::write(
            &request.path,
            "1
00:00:01,000 --> 00:00:04,000
Une

2
00:00:03,000 --> 00:00:05,000
Deux
",
        )
        .unwrap();
        let error = match load(&request) {
            Ok(_) => panic!("overlapping lines unexpectedly loaded"),
            Err(error) => error,
        };
        assert_eq!(error.code, OVERLAP);
        let message = overlap_message(&error).unwrap();
        assert_eq!(message.args, ["1", "2"]);
        for language in Language::ALL {
            let text = message.render(language);
            assert!(
                text.contains('1') && text.contains('2') && !text.contains('{'),
                "{text}"
            );
        }
        // Any other refusal keeps its own words.
        assert!(
            overlap_message(&EngineError::new("error.caption_track", "x", None, "2")).is_none()
        );
    }

    #[test]
    fn malformed_srt_does_not_become_an_after_end_error() {
        let (_dir, request) = fixture();
        std::fs::write(&request.path, "not an SRT file").unwrap();

        let error = match load(&request) {
            Ok(_) => panic!("malformed SRT unexpectedly loaded"),
            Err(error) => error,
        };

        assert_eq!(error.code, "error.caption_track");
    }
    #[test]
    fn malformed_replacement_keeps_the_valid_track_and_late_replies_stay_removed() {
        let (dir, request) = fixture();
        let mut attachment = Attachment {
            path: Some(request.path.clone()),
            loaded: Some(load(&request).unwrap()),
            ..Default::default()
        };
        let bad = dir.path().join("bad.srt");
        std::fs::write(&bad, "bad").unwrap();
        attachment.request(bad, request.wav.clone(), request.duration_ms);
        assert!(attachment.validating());
        let pending = attachment.pending.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        attachment.work = Some((pending.generation, rx));
        attachment.reading = true;
        assert!(attachment.validating());
        tx.send(Reply::Loaded(pending.clone(), load(&pending)))
            .unwrap();
        assert!(!attachment.poll(&eframe::egui::Context::default()));
        assert_eq!(attachment.path, Some(request.path));
        assert!(attachment.loaded.is_some() && attachment.error.is_some());
        assert_eq!(attachment.revision, 0);
        assert!(!attachment.validating());
        let (tx, rx) = mpsc::sync_channel(1);
        attachment.work = Some((attachment.generation, rx));
        attachment.clear();
        tx.send(Reply::Loaded(pending.clone(), load(&pending)))
            .unwrap();
        assert!(!attachment.poll(&eframe::egui::Context::default()));
        assert!(
            attachment.path.is_none() && attachment.loaded.is_none() && attachment.error.is_none()
        );
    }
}
