//! One imported/generated subtitle attachment. Reads and identity checks stay off the UI thread.
use noh::{
    engine::EngineError,
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
    duration_ms: u64,
}
pub struct Loaded {
    pub track: Arc<SubtitleTrack>,
    pub snapshot: Snapshot,
}
enum Reply {
    Loaded(Request, Result<Loaded, EngineError>),
    Checked {
        generation: u64,
        stamp: Option<FileStamp>,
    },
}

#[derive(Default)]
pub struct Attachment {
    /// Last successfully attached track (or the initial candidate when none is loaded).
    pub path: Option<PathBuf>,
    pub loaded: Option<Loaded>,
    pub error: Option<EngineError>,
    /// Only committed changes invalidate an otherwise current rendered preview.
    pub revision: u64,
    generation: u64,
    pending: Option<Request>,
    work: Option<(u64, Receiver<Reply>)>,
    reading: bool,
    source: Option<(PathBuf, u64)>,
    observed: Option<FileStamp>,
    checked: Option<Instant>,
}

impl Attachment {
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
        self.observed = None;
        self.checked = None;
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
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
        if self.path.is_none() {
            self.path = Some(path.clone());
        }
        self.pending = Some(Request {
            generation: self.generation,
            path,
            wav,
            duration_ms,
        });
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
                                    self.observed = Some(loaded.snapshot.0[0].clone());
                                    self.loaded = Some(loaded);
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
                                }
                            },
                            Reply::Checked { generation, stamp }
                                if generation == self.generation =>
                            {
                                if stamp != self.observed {
                                    self.observed = stamp;
                                    self.loaded = None;
                                    self.revision = self.revision.wrapping_add(1);
                                    changed = true;
                                    if let (Some(path), Some((wav, duration))) =
                                        (self.path.clone(), self.source.clone())
                                    {
                                        self.request(path, wav, duration);
                                    }
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
            let (tx, rx) = mpsc::sync_channel(1);
            let ctx = ctx.clone();
            self.checked = Some(Instant::now());
            self.work = Some((generation, rx));
            self.reading = false;
            std::thread::spawn(move || {
                let stamp = FileStamp::read(&path).ok();
                let _ = tx.send(Reply::Checked { generation, stamp });
                ctx.request_repaint();
            });
        }
        if self.path.is_some() {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        changed
    }
}

fn load(request: &Request) -> Result<Loaded, EngineError> {
    let error = |detail: String| {
        EngineError::new(
            "error.caption_track",
            "read_subtitles",
            Some(&request.path),
            detail,
        )
    };
    let snapshot = Snapshot(vec![
        FileStamp::read(&request.path)?,
        FileStamp::read(&request.wav)?,
    ]);
    let actual_duration = noh::wav_duration(&request.wav).map_err(|e| error(e.to_string()))?;
    if (actual_duration * 1_000.0).round() as u64 != request.duration_ms {
        return Err(EngineError::new(
            "error.stale_inspection",
            "read_subtitles",
            Some(&request.wav),
            "WAV duration changed; refresh the project before attaching its subtitles.",
        ));
    }
    if snapshot.0[0].bytes > noh::subtitle_srt::MAX_INPUT_BYTES as u64 {
        return Err(error("SRT input exceeds one MiB.".into()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&request.path)
        .map_err(|e| error(e.to_string()))?
        .take(noh::subtitle_srt::MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| error(e.to_string()))?;
    if bytes.len() > noh::subtitle_srt::MAX_INPUT_BYTES {
        return Err(error("SRT input exceeds one MiB.".into()));
    }
    let text = std::str::from_utf8(&bytes).map_err(|e| error(e.to_string()))?;
    let track = match noh::subtitle_srt::parse(text, request.duration_ms) {
        Ok(track) => track,
        Err(duration_error) => {
            match noh::subtitle_srt::parse(text, noh::subtitle_track::MAX_DURATION_MS) {
                Ok(_) => {
                    return Err(EngineError::new(
                        "lyrics.after_end",
                        "read_subtitles",
                        Some(&request.path),
                        "Some subtitle lines end after the song.",
                    ));
                }
                Err(_) => return Err(error(duration_error.to_string())),
            }
        }
    };
    snapshot.verify()?;
    Ok(Loaded {
        track: Arc::new(track),
        snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
                duration_ms: 20_000,
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
        assert!(load(&request).is_err());
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
