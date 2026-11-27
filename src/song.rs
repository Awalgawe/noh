//! The song as NOH reads it: one WAV.
//!
//! A `.wav` song is read as it is. Any other song (MP3, M4A, FLAC, OGG) is
//! decoded once by FFmpeg into a WAV copy, and every reader of its sound (the
//! players, the waveform, Whisper, the scene, the export) reads that copy: one
//! decoder, one clock, sample-exact durations and seeks. Decoders may treat the
//! silence an MP3 starts with differently; readers that share a copy cannot.
//!
//! What belongs to the song itself (its lyrics file, its scene, the "lyrics
//! from another song" check, the output name) follows the ORIGINAL path. Only
//! reading its sound goes through here.
//!
//! Where the preview-copy store exists (the app), a copy is kept in the user's
//! cache folder under the song's content signature, as the preview copies are
//! (`copies.rs`): made when the song is dropped (its metadata inspection) and
//! found again by every later reader, in this session or the next. Elsewhere,
//! or when that folder cannot be used, a copy lives in a temporary folder for
//! as long as the [`Audio`] that holds it.
use crate::inspection::FileStamp;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The song files NOH takes, by extension (any case).
pub const EXTENSIONS: [&str; 5] = ["wav", "mp3", "m4a", "flac", "ogg"];
/// Names the decoding below. Change it with the decoding: older copies are
/// then no longer found, and leave with the least recently used.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
const RECIPE: &str = "song 1 pcm_f32le";
/// One decode may take this long (a two-hour song included).
const DECODE_TIME: Duration = Duration::from_secs(15 * 60);
const CANCELLED: &str = "Song preparation cancelled";

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}
/// Whether NOH takes this file as a song, from its name only.
pub fn is_song(path: &Path) -> bool {
    extension(path).is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}
fn is_wav(path: &Path) -> bool {
    extension(path).as_deref() == Some("wav")
}
/// Whether reading this song's sound takes a decode (and so FFmpeg): any
/// song but a `.wav`.
pub fn needs_decoding(path: &Path) -> bool {
    !is_wav(path)
}

/// The WAV that holds a song's sound. A temporary copy is deleted with it.
#[derive(Debug)]
pub struct Audio {
    path: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}
impl Audio {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The WAV to read for `song`: the song itself when it is a `.wav` (FFmpeg is
/// then neither needed nor touched), its decoded copy otherwise. `ffmpeg` is
/// resolved as `inspection::resolve_ffmpeg` does (empty: NOH's own), and only
/// when a decode is needed.
pub fn audio(song: &Path, ffmpeg: &Path, stop: &dyn Fn() -> bool) -> Result<Audio> {
    if is_wav(song) {
        return Ok(Audio {
            path: song.to_owned(),
            _temporary: None,
        });
    }
    #[cfg(feature = "gui")]
    if let (Some(store), Some(signatures)) = (Store::shared(), crate::copies::Store::shared()) {
        return store.audio(signatures, song, ffmpeg, stop);
    }
    temporary(song, ffmpeg, stop)
}

/// The song's length in seconds, from the WAV [`audio`] gives.
pub fn duration(song: &Path, ffmpeg: &Path, stop: &dyn Fn() -> bool) -> Result<f64> {
    let audio = audio(song, ffmpeg, stop)?;
    crate::wav_duration(audio.path()).map_err(|e| e.to_string().into())
}

/// A song reading error as the engine reports it: under `code` with the
/// reason, except a missing FFmpeg, which keeps its own `error.ffmpeg`.
pub fn engine_error(
    code: &str,
    operation: &str,
    song: &Path,
    error: Box<dyn std::error::Error + Send + Sync>,
) -> crate::engine::EngineError {
    match error.downcast::<crate::engine::EngineError>() {
        Ok(error) if error.code == "error.ffmpeg" => *error,
        Ok(error) => crate::engine::EngineError::new(code, operation, Some(song), error.detail),
        Err(error) => crate::engine::EngineError::new(code, operation, Some(song), error),
    }
}

fn temporary(song: &Path, ffmpeg: &Path, stop: &dyn Fn() -> bool) -> Result<Audio> {
    let source = FileStamp::read(song)?;
    let folder = tempfile::Builder::new().prefix("noh-song-").tempdir()?;
    let path = folder.path().join("song.wav");
    decode(&source, ffmpeg, &path, stop)?;
    Ok(Audio {
        path,
        _temporary: Some(folder),
    })
}

/// Decodes the first sound stream of `source` into a float WAV at `output`,
/// at the source's own rate and channels. MP3 and AAC start padding is removed
/// as FFmpeg's demuxers know it (LAME tag, edit list); see the module notes.
fn decode(source: &FileStamp, ffmpeg: &Path, output: &Path, stop: &dyn Fn() -> bool) -> Result<()> {
    // As given by the caller, or empty for NOH's own FFmpeg.
    let ffmpeg = crate::inspection::resolve_ffmpeg(ffmpeg)?;
    let log_path = output.with_extension("log");
    let log = File::create(&log_path)?;
    let mut command = crate::command(&ffmpeg);
    command
        .args(["-hide_banner", "-nostdin", "-v", "error", "-n", "-i"])
        .arg(&source.resolved)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-map_metadata",
            "-1",
            "-c:a",
            "pcm_f32le",
            "-fflags",
            "+bitexact",
            "-flags:a",
            "+bitexact",
            "-f",
            "wav",
            "-rf64",
            "auto",
        ])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    let outcome = (|| {
        let mut child = command
            .spawn()
            .map_err(|e| format!("FFmpeg could not start: {e}"))?;
        // Closing NOH stops the decoder; a NOH that is killed takes it along.
        crate::process::adopt(&child);
        let began = Instant::now();
        let status = loop {
            let late = began.elapsed() > DECODE_TIME;
            if stop() || late {
                let _ = child.kill();
                let _ = child.wait();
                return Err(if late {
                    "Song preparation took too long".into()
                } else {
                    CANCELLED.to_owned()
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e.to_string());
                }
            }
        };
        if !status.success() {
            let mut text = String::new();
            let _ = File::open(&log_path).and_then(|f| f.take(4096).read_to_string(&mut text));
            let reason = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            return Err(format!(
                "FFmpeg could not decode this song: {}",
                reason.trim()
            ));
        }
        Ok(())
    })();
    // The command holds the log open until it is dropped.
    drop(command);
    let _ = std::fs::remove_file(&log_path);
    if let Err(error) = outcome {
        let _ = std::fs::remove_file(output);
        return Err(error.into());
    }
    if FileStamp::read(&source.path).ok().as_ref() != Some(source) {
        let _ = std::fs::remove_file(output);
        return Err("The song changed while it was read".into());
    }
    Ok(())
}

#[cfg(feature = "gui")]
use cache::Store;
#[cfg(feature = "gui")]
mod cache {
    use super::{Audio, FileStamp, RECIPE, Result, decode};
    use std::{
        collections::HashSet,
        path::{Path, PathBuf},
        sync::{
            Mutex, OnceLock,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    /// Beyond this size the folder deletes the copies used least recently.
    const STORE_BYTES: u64 = 1024 * 1024 * 1024;
    /// A partial file this old is the leftover of an interrupted decode.
    const PARTIAL_AGE: Duration = Duration::from_secs(86_400);

    /// The folder of song copies. Several processes may share it: a copy
    /// appears by rename, whole, and the first one there is kept.
    pub(super) struct Store {
        folder: PathBuf,
        limit: u64,
        /// Copies handed out in this session and its partial files, by name:
        /// never deleted by it.
        used: Mutex<HashSet<String>>,
        /// One decode at a time in this process: a second reader of the same
        /// song then finds the first one's copy.
        making: Mutex<()>,
        partials: AtomicU64,
    }
    impl Store {
        pub(super) fn at(folder: PathBuf, limit: u64) -> std::io::Result<Self> {
            std::fs::create_dir_all(&folder)?;
            Ok(Self {
                folder,
                limit,
                used: Mutex::new(HashSet::new()),
                making: Mutex::new(()),
                partials: AtomicU64::new(0),
            })
        }
        /// `songs` in the user's cache folder (`NOH_CACHE_DIR` replaces it).
        /// None when there is none or it cannot be created, and in this
        /// library's own tests.
        pub(super) fn shared() -> Option<&'static Store> {
            static STORE: OnceLock<Option<Store>> = OnceLock::new();
            STORE
                .get_or_init(|| {
                    if cfg!(test) {
                        return None;
                    }
                    let folder = crate::copies::cache_folder()?.join("songs");
                    match Store::at(folder, STORE_BYTES) {
                        Ok(store) => Some(store),
                        Err(error) => {
                            tracing::warn!("Song copies stay temporary: {error}");
                            None
                        }
                    }
                })
                .as_ref()
        }

        fn used(&self, name: &str) {
            self.used
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name.to_owned());
        }

        pub(super) fn audio(
            &self,
            signatures: &crate::copies::Store,
            song: &Path,
            ffmpeg: &Path,
            stop: &dyn Fn() -> bool,
        ) -> Result<Audio> {
            let source = FileStamp::read(song)?;
            let signature = signatures.signature(&source, stop)?;
            let name = format!("{}.wav", crate::copies::key(&signature, RECIPE));
            let path = self.folder.join(&name);
            self.used(&name);
            let making = self.making.lock().unwrap_or_else(|e| e.into_inner());
            if readable(&path) {
                crate::profile!("song: copy found for {}", song.display());
                // Best effort: the date only orders the trimming.
                let _ = std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .and_then(|f| f.set_modified(SystemTime::now()));
                return Ok(Audio {
                    path,
                    _temporary: None,
                });
            }
            let partial = format!(
                "partial-{}-{}.wav",
                std::process::id(),
                self.partials.fetch_add(1, Ordering::Relaxed)
            );
            self.used(&partial);
            let partial = self.folder.join(partial);
            let began = std::time::Instant::now();
            decode(&source, ffmpeg, &partial, stop)?;
            crate::profile!(
                "song: copy made in {:.0} ms for {}",
                began.elapsed().as_secs_f64() * 1000.0,
                song.display()
            );
            if std::fs::rename(&partial, &path).is_err() {
                // Another process filed the same copy meanwhile: that one stays.
                let _ = std::fs::remove_file(&partial);
                if !readable(&path) {
                    return Err("The song copy could not be filed".into());
                }
            }
            drop(making);
            self.trim();
            Ok(Audio {
                path,
                _temporary: None,
            })
        }

        /// Deletes the copies used least recently beyond the limit, never one
        /// this session handed out, and the leftovers of interrupted decodes.
        pub(super) fn trim(&self) {
            let Ok(entries) = std::fs::read_dir(&self.folder) else {
                return;
            };
            let used = self.used.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let mut copies = Vec::new();
            let mut total = 0u64;
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                if name.starts_with("partial-") {
                    // Another process may still be writing a recent one.
                    if !used.contains(&name)
                        && modified.elapsed().is_ok_and(|age| age > PARTIAL_AGE)
                    {
                        let _ = std::fs::remove_file(entry.path());
                    }
                    continue;
                }
                if name.ends_with(".wav") {
                    total += meta.len();
                    copies.push((modified, name, meta.len()));
                }
            }
            copies.sort();
            for (_, name, bytes) in copies {
                if total <= self.limit {
                    break;
                }
                if used.contains(&name) {
                    continue;
                }
                // A copy another process reads cannot be deleted on Windows: it stays.
                if std::fs::remove_file(self.folder.join(&name)).is_ok() {
                    total -= bytes;
                }
            }
        }
    }

    /// A complete copy: a WAV with sound frames.
    fn readable(path: &Path) -> bool {
        crate::wav_duration(path).is_ok_and(|seconds| seconds > 0.0)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;

        /// A WAV header with `frames` mono 16-bit frames at 8 kHz.
        fn wav(path: &Path, frames: u32) {
            let data = frames * 2;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&(36 + data).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
            bytes.extend_from_slice(&8000u32.to_le_bytes());
            bytes.extend_from_slice(&16000u32.to_le_bytes());
            bytes.extend_from_slice(b"\x02\0\x10\0data");
            bytes.extend_from_slice(&data.to_le_bytes());
            bytes.resize(bytes.len() + data as usize, 0);
            std::fs::File::create(path)
                .unwrap()
                .write_all(&bytes)
                .unwrap();
        }

        fn stores(folder: &Path, limit: u64) -> (Store, crate::copies::Store) {
            (
                Store::at(folder.join("songs"), limit).unwrap(),
                crate::copies::Store::at(folder.join("copies"), 0).unwrap(),
            )
        }

        /// Files a copy for `song` as a decode would have, without FFmpeg.
        fn file(store: &Store, signatures: &crate::copies::Store, song: &Path, frames: u32) {
            let stamp = FileStamp::read(song).unwrap();
            let signature = signatures.signature(&stamp, &|| false).unwrap();
            let name = format!("{}.wav", crate::copies::key(&signature, RECIPE));
            wav(&store.folder.join(name), frames);
        }

        #[test]
        fn a_filed_copy_is_found_under_any_name_of_its_song_without_ffmpeg() {
            let folder = tempfile::tempdir().unwrap();
            let (store, signatures) = stores(folder.path(), u64::MAX);
            let song = folder.path().join("song.mp3");
            std::fs::write(&song, b"same bytes").unwrap();
            file(&store, &signatures, &song, 8000);
            let missing = folder.path().join("no-ffmpeg");
            let found = store
                .audio(&signatures, &song, &missing, &|| false)
                .unwrap();
            let renamed = folder.path().join("renamed.mp3");
            std::fs::copy(&song, &renamed).unwrap();
            let again = store
                .audio(&signatures, &renamed, &missing, &|| false)
                .unwrap();
            assert_eq!(found.path(), again.path());
            assert_eq!(crate::wav_duration(found.path()).unwrap(), 1.0);
        }

        #[test]
        fn changed_content_is_another_copy_and_a_broken_one_is_made_again() {
            let folder = tempfile::tempdir().unwrap();
            let (store, signatures) = stores(folder.path(), u64::MAX);
            let song = folder.path().join("song.mp3");
            std::fs::write(&song, b"first").unwrap();
            file(&store, &signatures, &song, 8000);
            std::fs::write(&song, b"second, longer").unwrap();
            let missing = folder.path().join("no-ffmpeg");
            assert!(
                store
                    .audio(&signatures, &song, &missing, &|| false)
                    .is_err()
            );
            // A truncated copy under the right name is not trusted either.
            let stamp = FileStamp::read(&song).unwrap();
            let signature = signatures.signature(&stamp, &|| false).unwrap();
            let name = format!("{}.wav", crate::copies::key(&signature, RECIPE));
            std::fs::write(store.folder.join(&name), b"RIFF").unwrap();
            assert!(
                store
                    .audio(&signatures, &song, &missing, &|| false)
                    .is_err()
            );
            // Nothing is left behind by the failed decodes.
            let left: Vec<_> = std::fs::read_dir(&store.folder)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("partial-"))
                .collect();
            assert!(left.is_empty(), "{left:?}");
        }

        #[test]
        fn trimming_keeps_copies_used_by_this_session_and_drops_the_oldest_others() {
            let folder = tempfile::tempdir().unwrap();
            let (store, signatures) = stores(folder.path(), 3 * 16_044);
            let song = folder.path().join("song.mp3");
            std::fs::write(&song, b"kept").unwrap();
            file(&store, &signatures, &song, 8000);
            let kept = store
                .audio(&signatures, &song, Path::new("no-ffmpeg"), &|| false)
                .unwrap();
            let old = SystemTime::now() - Duration::from_secs(3600);
            let mut others = Vec::new();
            for index in 0..4 {
                let path = store.folder.join(format!("other-{index}.wav"));
                wav(&path, 8000);
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(old + Duration::from_secs(index))
                    .unwrap();
                others.push(path);
            }
            // The kept copy is the oldest of all, yet used by this session.
            std::fs::File::options()
                .write(true)
                .open(kept.path())
                .unwrap()
                .set_modified(old - Duration::from_secs(60))
                .unwrap();
            let stale = store.folder.join("partial-1-1.wav");
            std::fs::write(&stale, b"left over").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&stale)
                .unwrap()
                .set_modified(old - PARTIAL_AGE)
                .unwrap();
            store.trim();
            assert!(kept.path().exists());
            assert!(!stale.exists());
            let left: Vec<bool> = others.iter().map(|p| p.exists()).collect();
            assert_eq!(left, [false, false, true, true]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songs_are_known_by_extension_in_any_case() {
        for name in ["a.wav", "a.MP3", "a.m4a", "a.Flac", "a.ogg"] {
            assert!(is_song(Path::new(name)), "{name}");
        }
        for name in ["a.mp4", "a.srt", "a", "a.aac", "a.wav.txt"] {
            assert!(!is_song(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn a_wav_is_read_as_it_is_without_ffmpeg() {
        let audio = audio(
            Path::new("missing/song.WAV"),
            Path::new("no-ffmpeg"),
            &|| false,
        )
        .unwrap();
        assert_eq!(audio.path(), Path::new("missing/song.WAV"));
    }

    #[test]
    fn another_song_without_a_decoder_fails_and_leaves_nothing() {
        let folder = tempfile::tempdir().unwrap();
        let song = folder.path().join("song.mp3");
        std::fs::write(&song, b"not decoded").unwrap();
        assert!(audio(&song, &folder.path().join("no-ffmpeg"), &|| false).is_err());
        let names: Vec<_> = std::fs::read_dir(folder.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let error = audio(&folder.path().join("missing.mp3"), Path::new("x"), &|| {
            false
        })
        .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}
