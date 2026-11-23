//! The lyrics file beside a subtitle file.
//!
//! Version 2, `<name>.noh-lyrics.json`, holds every cue (start, end, text, word
//! and syllable starts when known, and their ends when known), the language and
//! the Whisper model: the lyrics themselves, which the .srt only mirrors. No
//! style, no media, no project. Unknown fields are ignored within a version; a
//! higher version is « newer » and never read as lyrics.
//!
//! Version 3 is version 2 plus who sings: the declared voices (`voices`, with
//! optional names) and each cue's set of voices, and word ends that run past
//! the backing vocals of their cue (`SubtitleCue::fit_ends`). It is written only
//! when the lyrics hold one of these, so a NOH that reads version 2 alone never
//! drops them when it writes the file back: to it the file is « newer ». Lyrics
//! without them are written as version 2, byte for byte as before.
//!
//! Version 1, `<name>.noh-words.json`, held only the cues with word times and
//! is still read: its words go to the identical cues of the .srt.
//!
//! Readers that merge into an .srt (`merge`, `read_beside`) give a cue its words
//! only when the .srt still has that exact cue (same start, end and text), so a
//! file edited outside NOH loses nothing else.
use crate::subtitle_track::{SubtitleCue, SubtitleTrack, SyllableTime, WordSource, WordTime};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Larger files are ignored (a 10 000-cue track with words is far below this).
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
const VERSION: u32 = 1;
/// The highest lyrics file version this NOH reads and writes (`to_lyrics_json`
/// writes the lowest that holds the lyrics: 2 or 3).
pub const LYRICS_VERSION: u32 = 3;
/// The version of lyrics that hold no voices and no backing-vocal ends.
pub const LYRICS_VERSION_PLAIN: u32 = 2;
/// The full suffix that names a lyrics file (never any `.json`).
pub const LYRICS_SUFFIX: &str = ".noh-lyrics.json";
/// The version-1 name, still read.
pub const OLD_SUFFIX: &str = ".noh-words.json";

/// `song.srt` → `song.noh-lyrics.json`, same folder.
pub fn lyrics_path(srt: &Path) -> PathBuf {
    srt.with_extension(&LYRICS_SUFFIX[1..])
}

/// Whether `path` is named as a lyrics file, new or old (full suffix, any case).
pub fn is_lyrics_file(path: &Path) -> bool {
    suffix_len(path).is_some()
}

fn suffix_len(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?;
    [LYRICS_SUFFIX, OLD_SUFFIX].into_iter().find_map(|suffix| {
        (name.len() > suffix.len()
            && name.is_char_boundary(name.len() - suffix.len())
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix))
        .then_some(suffix.len())
    })
}

/// The .srt a lyrics file belongs to: `song.noh-lyrics.json` → `song.srt`.
pub fn srt_beside(lyrics: &Path) -> Option<PathBuf> {
    let cut = suffix_len(lyrics)?;
    let name = lyrics.file_name()?.to_str()?;
    Some(lyrics.with_file_name(format!("{}.srt", &name[..name.len() - cut])))
}

/// Lyrics read from a version-2 file, checked on their own terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lyrics {
    /// `duration_ms` is the last cue's end (1 when there is none): the file is
    /// bound to no media; fitting a song or a video is the reader's check.
    pub track: SubtitleTrack,
    pub model: Option<String>,
}

/// Why a lyrics file cannot be used as lyrics.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LyricsError {
    #[error("The lyrics file is larger than {} MiB", MAX_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("The lyrics file comes from a newer NOH (version {0})")]
    Newer(u32),
    /// A version-1 file: word times only, read with its .srt (`merge`).
    #[error("The lyrics file is version 1 and is read with its .srt")]
    Version1,
    #[error("The lyrics file is damaged: {0}")]
    Damaged(String),
}

#[derive(Deserialize)]
struct VersionOnly {
    version: u32,
}

#[derive(Serialize, Deserialize)]
struct LyricsFile {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// Version 3: the declared voices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    voices: Vec<LyricsVoice>,
    cues: Vec<LyricsCue>,
}

#[derive(Serialize, Deserialize)]
struct LyricsVoice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LyricsCue {
    start_ms: u64,
    end_ms: u64,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    words: Option<Vec<LyricsWord>>,
    /// Version 3: the voices that sing the cue (empty or absent: unknown).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    voices: Vec<u8>,
}

// Mirrors of WordTime and SyllableTime that ignore unknown fields, as the rest
// of a version-2 file does.
#[derive(Serialize, Deserialize)]
struct LyricsWord {
    text: String,
    start_ms: u64,
    // Added within version 2: a reader that predates it ignores it (and loses it
    // when it writes the file back), so no version change is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end_ms: Option<u64>,
    source: WordSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    syllables: Option<Vec<LyricsSyllable>>,
}

#[derive(Serialize, Deserialize)]
struct LyricsSyllable {
    text: String,
    start_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end_ms: Option<u64>,
    source: WordSource,
}

impl From<&WordTime> for LyricsWord {
    fn from(w: &WordTime) -> Self {
        Self {
            text: w.text.clone(),
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            source: w.source,
            syllables: w.syllables.as_ref().map(|s| {
                s.iter()
                    .map(|s| LyricsSyllable {
                        text: s.text.clone(),
                        start_ms: s.start_ms,
                        end_ms: s.end_ms,
                        source: s.source,
                    })
                    .collect()
            }),
        }
    }
}

impl From<LyricsWord> for WordTime {
    fn from(w: LyricsWord) -> Self {
        Self {
            text: w.text,
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            source: w.source,
            syllables: w.syllables.map(|s| {
                s.into_iter()
                    .map(|s| SyllableTime {
                        text: s.text,
                        start_ms: s.start_ms,
                        end_ms: s.end_ms,
                        source: s.source,
                    })
                    .collect()
            }),
        }
    }
}

/// Whether `track` holds what only version 3 keeps: declared voices, a cue's
/// voices, or a word end past the backing vocals of its cue.
pub fn needs_version_3(track: &SubtitleTrack) -> bool {
    !track.voices.is_empty()
        || track.cues.iter().any(|c| {
            !c.voices.is_empty()
                || c.words
                    .as_ref()
                    .is_some_and(|w| c.words_fit(w) && c.has_backing_overlap(w))
        })
}

/// The lyrics file for `track` (made by `model`, if known): every cue, as
/// version 2, or version 3 when `needs_version_3`.
pub fn to_lyrics_json(track: &SubtitleTrack, model: Option<&str>) -> Vec<u8> {
    serde_json::to_vec_pretty(&LyricsFile {
        version: if needs_version_3(track) {
            LYRICS_VERSION
        } else {
            LYRICS_VERSION_PLAIN
        },
        language: track.language.clone(),
        model: model.filter(|m| valid_model(m)).map(str::to_string),
        voices: track
            .voices
            .iter()
            .map(|v| LyricsVoice {
                name: v.name.clone(),
            })
            .collect(),
        cues: track
            .cues
            .iter()
            .map(|c| LyricsCue {
                start_ms: c.start_ms,
                end_ms: c.end_ms,
                text: c.text.clone(),
                words: c
                    .words
                    .as_ref()
                    .filter(|w| c.words_fit(w))
                    .map(|w| w.iter().map(LyricsWord::from).collect()),
                voices: c.voices.clone(),
            })
            .collect(),
    })
    .expect("serializable")
}

/// Read a lyrics file's bytes. Version 2 is checked on its own terms, with no
/// media: valid JSON, the track validation an .srt gets (order, overlaps, sizes,
/// text limits, language), word times that fit their cue, and every cue the
/// same after the SRT writer and parser. Anything else is « damaged ».
pub fn parse_lyrics(bytes: &[u8]) -> Result<Lyrics, LyricsError> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(LyricsError::TooLarge);
    }
    let damaged = |e: &dyn std::fmt::Display| LyricsError::Damaged(e.to_string());
    let version = serde_json::from_slice::<VersionOnly>(bytes)
        .map_err(|e| damaged(&e))?
        .version;
    match version {
        1 => return Err(LyricsError::Version1),
        LYRICS_VERSION_PLAIN..=LYRICS_VERSION => {}
        v if v > LYRICS_VERSION => return Err(LyricsError::Newer(v)),
        v => return Err(LyricsError::Damaged(format!("unknown version {v}"))),
    }
    let file = serde_json::from_slice::<LyricsFile>(bytes).map_err(|e| damaged(&e))?;
    let cues: Vec<SubtitleCue> = file
        .cues
        .into_iter()
        .map(|c| {
            let mut cue = SubtitleCue {
                // Read in either version: a version-2 file given voices by hand
                // or by an agent keeps them (and is written back as version 3).
                voices: c.voices,
                start_ms: c.start_ms,
                end_ms: c.end_ms,
                // One form after loading, as the SRT parser gives (merge compares
                // texts as they are).
                text: c.text.replace("\r\n", "\n"),
                words: None,
            };
            // Ends are a hint: one that does not fit is dropped, never a reason
            // to call the file damaged.
            cue.words = c.words.map(|w| {
                let mut w: Vec<WordTime> = w.into_iter().map(WordTime::from).collect();
                cue.fit_ends(&mut w);
                w
            });
            cue
        })
        .collect();
    let track = unbound(SubtitleTrack {
        voices: file
            .voices
            .into_iter()
            .map(|v| crate::subtitle_track::Voice { name: v.name })
            .collect(),
        language: file.language,
        duration_ms: 1,
        cues,
    });
    track.validate().map_err(|e| damaged(&e))?;
    let srt = track.to_srt().map_err(|e| damaged(&e))?;
    let back = crate::subtitle_srt::parse(&srt, track.duration_ms).map_err(|e| damaged(&e))?;
    if let Some(number) = first_difference(&back, &track) {
        return Err(LyricsError::Damaged(format!(
            "cue {number} changes when written as SRT"
        )));
    }
    let model = file.model.filter(|m| valid_model(m));
    Ok(Lyrics { track, model })
}

/// Read and check the lyrics file at `path` (see `parse_lyrics`).
pub fn read_lyrics(path: &Path) -> std::io::Result<Result<Lyrics, LyricsError>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(parse_lyrics(&bytes))
}

/// Create the version-2 lyrics file at `path` only if no file is there
/// (decision B: an .srt opened alone gets its lyrics file; an existing file,
/// usable or not, is never replaced). A failed write removes the file it began.
pub fn create_lyrics(
    path: &Path,
    track: &SubtitleTrack,
    model: Option<&str>,
) -> std::io::Result<()> {
    unbound(track.clone())
        .validate()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let bytes = to_lyrics_json(track, model);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let written = file.write_all(&bytes).and_then(|()| file.sync_all());
    if written.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    written
}

/// Before NOH replaces the lyrics file at `path` (plan 7: a file NOH cannot use
/// is never deleted or overwritten). Absent or usable → Ok(None). Newer, or
/// unreadable from disk (locked) → an error, and nothing may be written.
/// Damaged (or a version-1 file under the new name) → renamed to a free
/// `<name>.unreadable.json`, whose path is returned.
pub fn set_aside_unusable(path: &Path) -> std::io::Result<Option<PathBuf>> {
    match read_lyrics(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
        Ok(Ok(_)) => Ok(None),
        Ok(Err(e @ LyricsError::Newer(_))) => Err(std::io::Error::other(e)),
        Ok(Err(LyricsError::TooLarge | LyricsError::Damaged(_) | LyricsError::Version1)) => {
            ensure_writable(path)?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("lyrics");
            let stem = &name[..name.len() - suffix_len(path).unwrap_or(0)];
            let aside = (1..)
                .map(|n| {
                    path.with_file_name(if n == 1 {
                        format!("{stem}.unreadable.json")
                    } else {
                        format!("{stem}.unreadable-{n}.json")
                    })
                })
                .find(|p| {
                    matches!(std::fs::symlink_metadata(p),
                        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound)
                })
                .expect("a free name");
            std::fs::rename(path, &aside)?;
            Ok(Some(aside))
        }
    }
}

/// The .srt beside a lyrics file, as it is named on disk: `srt_beside`, or a
/// file in the same folder whose name differs from it only in ASCII case.
/// The name as the folder lists it is preferred, so a case-insensitive volume
/// gives the same spelling as a drop of that .srt.
pub fn find_srt_beside(lyrics: &Path) -> Option<PathBuf> {
    let srt = srt_beside(lyrics)?;
    let wanted = srt.file_name()?.to_str()?.to_owned();
    let listed: Vec<PathBuf> = std::fs::read_dir(srt.parent()?)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default();
    let named = |exact: bool| {
        listed.iter().find(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                if exact {
                    n == wanted
                } else {
                    n.eq_ignore_ascii_case(&wanted)
                }
            })
        })
    };
    named(true)
        .or_else(|| named(false))
        .cloned()
        .or_else(|| srt.is_file().then_some(srt))
}

/// `track` bound to no media: its duration is its last cue's end (1 without cues).
fn unbound(mut track: SubtitleTrack) -> SubtitleTrack {
    track.duration_ms = track
        .cues
        .iter()
        .map(|c| c.end_ms)
        .max()
        .unwrap_or(0)
        .max(1);
    track
}

/// Write the version-2 lyrics file at `path` (temporary file, then rename).
/// Lyrics that `parse_lyrics` would call damaged are refused (InvalidData) and
/// nothing is written.
pub fn write_lyrics(
    path: &Path,
    track: &SubtitleTrack,
    model: Option<&str>,
) -> std::io::Result<()> {
    unbound(track.clone())
        .validate()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_replacing(path, &to_lyrics_json(track, model))
}

/// What an .srt holds of a cue, once through the SRT writer: start, end and
/// text with CRLF as LF (the writer's only change).
fn srt_key(cue: &SubtitleCue) -> (u64, u64, String) {
    (cue.start_ms, cue.end_ms, cue.text.replace("\r\n", "\n"))
}

/// The first cue (one-based) where two tracks hold different SRT content.
fn first_difference(a: &SubtitleTrack, b: &SubtitleTrack) -> Option<usize> {
    let n = a.cues.len().max(b.cues.len());
    (0..n)
        .find(|&i| a.cues.get(i).map(srt_key) != b.cues.get(i).map(srt_key))
        .map(|i| i + 1)
}

/// « Raccord »: how many lines differ between an .srt and the lyrics, compared
/// on what an .srt holds (start, end in ms, text; SRT formatting never counts).
/// Neither track's duration matters, so two files never differ because of the
/// media. Word times are not compared. A cue matches an identical cue wherever
/// it sits (as `merge` does), so one inserted line counts 1, not every line
/// after it; the count is the larger of the two sides' unmatched cues, so a
/// changed line counts 1 too.
pub fn differing_cues(srt: &SubtitleTrack, lyrics: &SubtitleTrack) -> usize {
    // Both are ordered and do not overlap, so starts strictly increase: one walk.
    let (a, b) = (&srt.cues, &lyrics.cues);
    let (mut i, mut j, mut only_a, mut only_b) = (0, 0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].start_ms.cmp(&b[j].start_ms) {
            std::cmp::Ordering::Less => (i, only_a) = (i + 1, only_a + 1),
            std::cmp::Ordering::Greater => (j, only_b) = (j + 1, only_b + 1),
            std::cmp::Ordering::Equal => {
                if srt_key(&a[i]) != srt_key(&b[j]) {
                    (only_a, only_b) = (only_a + 1, only_b + 1);
                }
                (i, j) = (i + 1, j + 1);
            }
        }
    }
    (only_a + a.len() - i).max(only_b + b.len() - j)
}

/// Parse an .srt's text for the raccord: no media bound (the longest an SRT can
/// say), so only the file itself can make it unreadable.
pub fn parse_srt_unbound(text: &str) -> Result<SubtitleTrack, crate::subtitle_srt::SrtError> {
    crate::subtitle_srt::parse(text, crate::subtitle_track::MAX_DURATION_MS)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkingFile {
    version: u32,
    /// The Whisper model that generated these lyrics (`large-v3`), if known.
    /// Optional, so files written before it still read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    cues: Vec<WorkingCue>,
}

/// A Whisper model's name from its file (`ggml-large-v3.bin` → `large-v3`).
pub fn model_name(model: &Path) -> Option<String> {
    let stem = model.file_stem()?.to_string_lossy().into_owned();
    let name = stem.strip_prefix("ggml-").unwrap_or(&stem).trim();
    valid_model(name).then(|| name.to_string())
}

/// A short printable name: what the interface shows, nothing else.
fn valid_model(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkingCue {
    start_ms: u64,
    end_ms: u64,
    text: String,
    words: Vec<WordTime>,
}

/// `song.srt` → `song.noh-words.json`, same folder (a name that does not depend
/// on the app's language).
pub fn path_for(srt: &Path) -> PathBuf {
    srt.with_extension("noh-words.json")
}

/// The working file for `track` (made by `model`, if known), or None when no
/// cue has word times and no model is known.
pub fn to_json(track: &SubtitleTrack, model: Option<&str>) -> Option<Vec<u8>> {
    let cues: Vec<WorkingCue> = track
        .cues
        .iter()
        .filter_map(|c| {
            c.words
                .as_ref()
                .filter(|w| c.words_fit(w))
                .map(|w| WorkingCue {
                    start_ms: c.start_ms,
                    end_ms: c.end_ms,
                    text: c.text.clone(),
                    words: w.clone(),
                })
        })
        .collect();
    // Kept for the word times, and for the model alone: lyrics generated without
    // word times still say which model made them, and replace a stale file.
    let model = model.filter(|m| valid_model(m)).map(str::to_string);
    (!cues.is_empty() || model.is_some()).then(|| {
        serde_json::to_vec_pretty(&WorkingFile {
            version: VERSION,
            model,
            cues,
        })
        .expect("serializable")
    })
}

/// The cues with word times and the model of a version-1 or version-2 file.
fn worded_cues(json: &[u8]) -> Option<(Vec<WorkingCue>, Option<String>)> {
    match parse_lyrics(json) {
        Ok(lyrics) => Some((
            lyrics
                .track
                .cues
                .into_iter()
                .filter_map(|c| {
                    Some(WorkingCue {
                        words: c.words?,
                        start_ms: c.start_ms,
                        end_ms: c.end_ms,
                        text: c.text,
                    })
                })
                .collect(),
            lyrics.model,
        )),
        Err(LyricsError::Version1) => {
            let file = serde_json::from_slice::<WorkingFile>(json).ok()?;
            (file.version == VERSION).then_some((file.cues, file.model))
        }
        Err(_) => None,
    }
}

/// Give `track`'s cues the word times of the identical cues in `json` (version 1
/// or 2); returns how many cues got them. An unreadable, newer or foreign file
/// changes nothing.
pub fn merge(track: &mut SubtitleTrack, json: &[u8]) -> usize {
    let Some((cues, _)) = worded_cues(json) else {
        return 0;
    };
    let mut merged = 0;
    for wc in cues {
        let at = track.cues.partition_point(|c| c.start_ms < wc.start_ms);
        if let Some(cue) = track.cues.get_mut(at)
            && cue.start_ms == wc.start_ms
            && cue.end_ms == wc.end_ms
            && cue.text == wc.text
            && cue.words_fit(&wc.words)
        {
            cue.words = Some(wc.words);
            merged += 1;
        }
    }
    merged
}

/// Give the cues of `to` (lyrics taken from an .srt, which holds no voices) the
/// voices of `from` (the lyrics it replaces), and its declared voices. A cue of
/// `to` takes the voices of the `from` cue with the same start and end, or else
/// of the first unused `from` cue with the same text, so a typo fixed in the
/// .srt keeps its voice. Returns how many `from` cues with voices found no cue:
/// the lines that lose their voices.
pub fn carry_voices(from: &SubtitleTrack, to: &mut SubtitleTrack) -> usize {
    to.voices = from.voices.clone();
    let mut taken = vec![false; to.cues.len()];
    let mut lost = Vec::new();
    for cue in from.cues.iter().filter(|c| !c.voices.is_empty()) {
        match (0..to.cues.len()).find(|&j| {
            !taken[j] && to.cues[j].start_ms == cue.start_ms && to.cues[j].end_ms == cue.end_ms
        }) {
            Some(j) => {
                taken[j] = true;
                to.cues[j].voices = cue.voices.clone();
            }
            None => lost.push(cue),
        }
    }
    let mut count = 0;
    for cue in lost {
        match (0..to.cues.len()).find(|&j| !taken[j] && to.cues[j].text == cue.text) {
            Some(j) => {
                taken[j] = true;
                to.cues[j].voices = cue.voices.clone();
            }
            None => count += 1,
        }
    }
    count
}

/// The bytes of the lyrics file beside `srt`: the new name when it exists,
/// otherwise the old one.
fn bytes_beside(srt: &Path) -> Option<Vec<u8>> {
    let new = lyrics_path(srt);
    let path = if new.exists() { new } else { path_for(srt) };
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    if file.take(MAX_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    Some(bytes)
}

/// Read the lyrics file beside `srt` (new name, then old), if any, into `track`.
pub fn read_beside(srt: &Path, track: &mut SubtitleTrack) -> usize {
    bytes_beside(srt).map_or(0, |bytes| merge(track, &bytes))
}

/// The model named in the lyrics file beside `srt` (new name, then old), if any.
pub fn model_beside(srt: &Path) -> Option<String> {
    worded_cues(&bytes_beside(srt)?)?
        .1
        .filter(|m| valid_model(m))
}

/// Write the working file beside `srt` (temporary file, then rename). Without
/// word times there is nothing to keep: an existing working file is left alone.
pub fn write_beside(
    srt: &Path,
    track: &SubtitleTrack,
    model: Option<&str>,
) -> std::io::Result<Option<PathBuf>> {
    let Some(json) = to_json(track, model) else {
        return Ok(None);
    };
    let path = path_for(srt);
    write_replacing(&path, &json)?;
    Ok(Some(path))
}

/// Refuse to replace, rewrite or set aside a file the user made read-only, on
/// every system. Windows already refuses to replace it; Linux and macOS would
/// let a rename overwrite it, so NOH checks the flag itself and leaves the file.
pub fn ensure_writable(path: &Path) -> std::io::Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.permissions().readonly() => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is read-only: NOH leaves it as it is", path.display()),
        )),
        _ => Ok(()),
    }
}

/// Write `bytes` to `path` through a temporary file of its own and a rename:
/// `<path>.<process>-<n>.tmp`, so two jobs writing the same file never share one.
fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    ensure_writable(path)?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{}-{n}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle_track::{SubtitleCue, WordSource};

    fn track() -> SubtitleTrack {
        let mut a = SubtitleCue::new(1000, 3000, "la la la");
        a.words = Some(
            [(1000, "la"), (1400, "la"), (2100, "la")]
                .iter()
                .map(|(t, w)| WordTime {
                    text: (*w).into(),
                    start_ms: *t,
                    end_ms: None,
                    source: WordSource::Whisper,
                    syllables: None,
                })
                .collect(),
        );
        SubtitleTrack {
            voices: Vec::new(),
            language: Some("fr".into()),
            duration_ms: 10_000,
            cues: vec![a, SubtitleCue::new(4000, 5000, "lo lo")],
        }
    }

    /// A read-only lyrics file is neither rewritten nor set aside, on any system;
    /// once writable again it is written normally.
    #[test]
    fn a_read_only_lyrics_file_is_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.noh-lyrics.json");
        let set_read_only = |on: bool| {
            let mut p = std::fs::metadata(&path).unwrap().permissions();
            p.set_readonly(on);
            std::fs::set_permissions(&path, p).unwrap();
        };
        // A missing file is no obstacle.
        assert!(ensure_writable(&path).is_ok());
        write_lyrics(&path, &track(), None).unwrap();
        let before = std::fs::read(&path).unwrap();
        set_read_only(true);
        let mut other = track();
        other.cues[1].text = "li li".into();
        let refused = write_lyrics(&path, &other, None).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(refused.to_string().contains("read-only"), "{refused}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // An unusable file is not set aside either while read-only.
        set_read_only(false);
        std::fs::write(&path, b"damaged").unwrap();
        set_read_only(true);
        assert!(set_aside_unusable(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"damaged");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        set_read_only(false);
        write_lyrics(&path, &other, None).unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn the_working_file_round_trips_only_cues_with_words() {
        let t = track();
        let json = to_json(&t, None).unwrap();
        let mut bare = t.clone();
        for c in &mut bare.cues {
            c.words = None;
        }
        assert_eq!(merge(&mut bare, &json), 1);
        assert_eq!(bare, t);
        let mut none = SubtitleTrack {
            voices: Vec::new(),
            language: None,
            duration_ms: 10_000,
            cues: vec![SubtitleCue::new(0, 500, "x")],
        };
        assert!(to_json(&none, None).is_none());
        assert_eq!(merge(&mut none, b"not json"), 0);
    }

    #[test]
    fn a_cue_changed_outside_noh_loses_only_its_own_words() {
        let json = to_json(&track(), None).unwrap();
        for change in [
            |c: &mut SubtitleCue| c.text = "la la li".into(),
            |c: &mut SubtitleCue| c.start_ms = 1100,
            |c: &mut SubtitleCue| c.end_ms = 2900,
        ] {
            let mut t = track();
            t.cues[0].words = None;
            change(&mut t.cues[0]);
            assert_eq!(merge(&mut t, &json), 0);
            assert!(t.cues[0].words.is_none());
        }
    }

    #[test]
    fn the_file_sits_beside_the_srt_and_replaces_its_previous_version() {
        let folder = tempfile::tempdir().unwrap();
        let srt = folder.path().join("song.srt");
        assert_eq!(path_for(&srt), folder.path().join("song.noh-words.json"));
        let mut t = track();
        assert_eq!(write_beside(&srt, &t, None).unwrap(), Some(path_for(&srt)));
        t.cues[0].words.as_mut().unwrap()[1].start_ms = 1500;
        write_beside(&srt, &t, None).unwrap();
        let mut read = track();
        read.cues[0].words = None;
        assert_eq!(read_beside(&srt, &mut read), 1);
        assert_eq!(read.cues[0].words.as_ref().unwrap()[1].start_ms, 1500);
        assert!(no_temp_left(folder.path()));
    }

    #[test]
    fn the_working_file_names_the_model_that_made_the_lyrics() {
        let folder = tempfile::tempdir().unwrap();
        let srt = folder.path().join("song.srt");
        assert_eq!(
            model_name(Path::new("C:/x/bin/speech/ggml-large-v3.bin")).as_deref(),
            Some("large-v3")
        );
        assert_eq!(
            model_name(Path::new("custom.bin")).as_deref(),
            Some("custom")
        );
        assert_eq!(model_name(Path::new("")), None);
        write_beside(&srt, &track(), Some("large-v3")).unwrap();
        assert_eq!(model_beside(&srt).as_deref(), Some("large-v3"));
        // The words still merge, and a file without a model (written before) reads.
        let mut read = track();
        read.cues[0].words = None;
        assert_eq!(read_beside(&srt, &mut read), 1);
        write_beside(&srt, &track(), None).unwrap();
        assert_eq!(model_beside(&srt), None);
        assert!(
            !String::from_utf8(std::fs::read(path_for(&srt)).unwrap())
                .unwrap()
                .contains("model")
        );
        // Lyrics without any word times still record their model, and replace a
        // leftover file that named another one.
        let mut bare = track();
        bare.cues[0].words = None;
        write_beside(&srt, &track(), Some("small")).unwrap();
        assert_eq!(
            write_beside(&srt, &bare, Some("large-v3")).unwrap(),
            Some(path_for(&srt))
        );
        assert_eq!(model_beside(&srt).as_deref(), Some("large-v3"));
        let mut read = bare.clone();
        assert_eq!(read_beside(&srt, &mut read), 0, "no word times to merge");
        assert!(
            to_json(&bare, None).is_none(),
            "neither words nor model: no file"
        );
        // A control character is not a model name.
        write_beside(&srt, &track(), Some("bad\nname")).unwrap();
        assert_eq!(model_beside(&srt), None);
    }

    fn no_temp_left(folder: &Path) -> bool {
        std::fs::read_dir(folder)
            .unwrap()
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp"))
    }

    /// `track()` with syllables on its first word, bound to no media.
    fn lyrics_track() -> SubtitleTrack {
        let mut t = track();
        t.cues[0].words.as_mut().unwrap()[1].syllables = Some(vec![
            SyllableTime {
                text: "l".into(),
                start_ms: 1400,
                end_ms: None,
                source: WordSource::Hand,
            },
            SyllableTime {
                text: "a".into(),
                start_ms: 1700,
                end_ms: None,
                source: WordSource::Hand,
            },
        ]);
        t.duration_ms = 5000;
        t
    }

    #[test]
    fn the_lyrics_file_holds_every_cue_and_round_trips() {
        let t = lyrics_track();
        let json = to_lyrics_json(&t, Some("large-v3"));
        let lyrics = parse_lyrics(&json).unwrap();
        assert_eq!(lyrics.track, t, "every cue, words, syllables and language");
        assert_eq!(lyrics.model.as_deref(), Some("large-v3"));
        // Bound to no media: the duration is the last cue's end.
        let mut long = t.clone();
        long.duration_ms = 99_000;
        assert_eq!(parse_lyrics(&to_lyrics_json(&long, None)).unwrap().track, t);
        // No cues at all is valid lyrics.
        let empty = SubtitleTrack {
            voices: Vec::new(),
            language: None,
            duration_ms: 1,
            cues: vec![],
        };
        assert_eq!(
            parse_lyrics(&to_lyrics_json(&empty, None)).unwrap().track,
            empty
        );
        // Word times that do not fit their cue are not written.
        let mut stale = t.clone();
        stale.cues[1].words = stale.cues[0].words.clone();
        let read = parse_lyrics(&to_lyrics_json(&stale, None)).unwrap();
        assert!(read.track.cues[1].words.is_none());
    }

    /// Word and syllable ends live inside version 2. A NOH that predates them
    /// (0.2.0's mirror shape, no end field) reads such a file, and its rewrite
    /// reads back here without the ends, with no error and no « damaged ».
    #[test]
    fn word_ends_stay_in_version_2_and_older_readers_ignore_them() {
        #[derive(Serialize, Deserialize)]
        struct OldFile {
            version: u32,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            language: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            model: Option<String>,
            cues: Vec<OldCue>,
        }
        #[derive(Serialize, Deserialize)]
        struct OldCue {
            start_ms: u64,
            end_ms: u64,
            text: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            words: Option<Vec<OldWord>>,
        }
        #[derive(Serialize, Deserialize)]
        struct OldWord {
            text: String,
            start_ms: u64,
            source: WordSource,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            syllables: Option<Vec<OldSyllable>>,
        }
        #[derive(Serialize, Deserialize)]
        struct OldSyllable {
            text: String,
            start_ms: u64,
            source: WordSource,
        }

        // Without ends, the file is byte for byte what 0.2.0's shape writes.
        let plain = lyrics_track();
        let old_shape = OldFile {
            version: LYRICS_VERSION_PLAIN,
            language: plain.language.clone(),
            model: None,
            cues: plain
                .cues
                .iter()
                .map(|c| OldCue {
                    start_ms: c.start_ms,
                    end_ms: c.end_ms,
                    text: c.text.clone(),
                    words: c.words.as_ref().map(|w| {
                        w.iter()
                            .map(|w| OldWord {
                                text: w.text.clone(),
                                start_ms: w.start_ms,
                                source: w.source,
                                syllables: w.syllables.as_ref().map(|s| {
                                    s.iter()
                                        .map(|s| OldSyllable {
                                            text: s.text.clone(),
                                            start_ms: s.start_ms,
                                            source: s.source,
                                        })
                                        .collect()
                                }),
                            })
                            .collect()
                    }),
                })
                .collect(),
        };
        assert_eq!(
            to_lyrics_json(&plain, None),
            serde_json::to_vec_pretty(&old_shape).unwrap()
        );

        let mut t = lyrics_track();
        {
            let w = t.cues[0].words.as_mut().unwrap();
            w[0].end_ms = Some(1300);
            w[1].syllables.as_mut().unwrap()[0].end_ms = Some(1600);
        }
        assert!(t.validate().is_ok());
        let json = to_lyrics_json(&t, None);
        assert!(String::from_utf8_lossy(&json).contains("\"version\": 2"));
        assert_eq!(parse_lyrics(&json).unwrap().track, t, "ends round-trip");

        // 0.2.0 reads it (the ends ignored) and writes it back without them.
        let old: OldFile = serde_json::from_slice(&json).expect("0.2.0 reads it");
        let rewritten = serde_json::to_vec_pretty(&old).unwrap();
        let back = parse_lyrics(&rewritten)
            .expect("no error, not damaged")
            .track;
        assert_eq!(back, plain, "the ends are gone, the starts are kept");

        // An end that does not fit is dropped, never a reason to call the file
        // damaged.
        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value["cues"][0]["words"][0]["end_ms"] = 9_000.into();
        let read = parse_lyrics(&serde_json::to_vec(&value).unwrap())
            .unwrap()
            .track;
        assert_eq!(read.cues[0].words.as_ref().unwrap()[0].end_ms, Some(1400));
        value["cues"][0]["words"][0]["end_ms"] = 900.into();
        let read = parse_lyrics(&serde_json::to_vec(&value).unwrap())
            .unwrap()
            .track;
        assert_eq!(read.cues[0].words.as_ref().unwrap()[0].end_ms, None);
    }

    #[test]
    fn lyrics_that_would_read_back_damaged_are_never_written() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.noh-lyrics.json");
        let mut bad = lyrics_track();
        bad.cues[1].start_ms = 2000;
        let error = write_lyrics(&path, &bad, None).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(!path.exists());
        bad = lyrics_track();
        bad.language = Some("f".into());
        assert!(write_lyrics(&path, &bad, None).is_err());
        // The media bound plays no part: a track longer than its duration is fine.
        let mut long = lyrics_track();
        long.duration_ms = 10;
        write_lyrics(&path, &long, None).unwrap();
        // A failed write leaves no temporary file behind.
        let missing = folder.path().join("no-folder").join("song.noh-lyrics.json");
        assert!(write_lyrics(&missing, &lyrics_track(), None).is_err());
        assert!(!folder.path().join("no-folder").exists());
    }

    #[test]
    fn create_lyrics_never_replaces_an_existing_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.noh-lyrics.json");
        for existing in [&br#"{"version":4,"cues":[]}"#[..], b"{ not json"] {
            std::fs::write(&path, existing).unwrap();
            let error = create_lyrics(&path, &lyrics_track(), None).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
            assert_eq!(std::fs::read(&path).unwrap(), existing);
        }
        let usable = to_lyrics_json(&lyrics_track(), Some("small"));
        std::fs::write(&path, &usable).unwrap();
        assert!(create_lyrics(&path, &track(), None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), usable);
        std::fs::remove_file(&path).unwrap();
        create_lyrics(&path, &lyrics_track(), None).unwrap();
        assert_eq!(read_lyrics(&path).unwrap().unwrap().track, lyrics_track());
    }

    #[test]
    fn crlf_in_a_cue_reads_as_lf_and_keeps_its_words() {
        let mut v: serde_json::Value =
            serde_json::from_slice(&to_lyrics_json(&lyrics_track(), None)).unwrap();
        v["cues"][1]["text"] = "lo\r\nlo".into();
        v["cues"][1]["words"] = serde_json::json!([
            {"text": "lo", "start_ms": 4000, "source": "hand"},
            {"text": "lo", "start_ms": 4500, "source": "hand"}
        ]);
        let bytes = serde_json::to_vec(&v).unwrap();
        assert_eq!(parse_lyrics(&bytes).unwrap().track.cues[1].text, "lo\nlo");
        let mut srt = parse_srt_unbound(
            "1\n00:00:01,000 --> 00:00:03,000\nla la la\n\n2\n00:00:04,000 --> 00:00:05,000\nlo\nlo\n",
        )
        .unwrap();
        assert_eq!(
            merge(&mut srt, &bytes),
            2,
            "the CRLF cue gets its words too"
        );
    }

    #[test]
    fn unknown_fields_are_ignored_within_version_2() {
        let mut v: serde_json::Value =
            serde_json::from_slice(&to_lyrics_json(&lyrics_track(), None)).unwrap();
        v["future"] = 1.into();
        v["cues"][0]["future"] = "x".into();
        v["cues"][0]["words"][1]["future"] = true.into();
        v["cues"][0]["words"][1]["syllables"][0]["future"] = 2.into();
        let bytes = serde_json::to_vec(&v).unwrap();
        assert_eq!(parse_lyrics(&bytes).unwrap().track, lyrics_track());
    }

    #[test]
    fn newer_old_and_damaged_files_are_told_apart() {
        let mut v: serde_json::Value =
            serde_json::from_slice(&to_lyrics_json(&lyrics_track(), None)).unwrap();
        v["version"] = 4.into();
        assert_eq!(
            parse_lyrics(&serde_json::to_vec(&v).unwrap()),
            Err(LyricsError::Newer(4))
        );
        assert_eq!(
            parse_lyrics(&to_json(&track(), None).unwrap()),
            Err(LyricsError::Version1)
        );
        v["version"] = 0.into();
        assert!(matches!(
            parse_lyrics(&serde_json::to_vec(&v).unwrap()),
            Err(LyricsError::Damaged(_))
        ));
        for bad in [
            &b"not json"[..],
            b"{}",
            b"{\"version\":2}",
            b"{\"version\":2,\"cues\":3}",
        ] {
            assert!(
                matches!(parse_lyrics(bad), Err(LyricsError::Damaged(_))),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        assert_eq!(
            parse_lyrics(&vec![b' '; MAX_BYTES as usize + 1]),
            Err(LyricsError::TooLarge)
        );
    }

    #[test]
    fn lyrics_that_an_srt_could_not_hold_are_damaged() {
        let damaged = |edit: fn(&mut serde_json::Value)| {
            let mut v: serde_json::Value =
                serde_json::from_slice(&to_lyrics_json(&lyrics_track(), None)).unwrap();
            edit(&mut v);
            parse_lyrics(&serde_json::to_vec(&v).unwrap())
        };
        // Overlapping cues, a cue ending before it starts, an empty line, a
        // control character, a bad language tag, words that do not spell the cue.
        for edit in [
            (|v: &mut serde_json::Value| v["cues"][1]["start_ms"] = 2000.into())
                as fn(&mut serde_json::Value),
            |v| v["cues"][1]["end_ms"] = 4000.into(),
            |v| v["cues"][1]["text"] = "lo\n\nlo".into(),
            |v| v["cues"][1]["text"] = "lo\u{7}lo".into(),
            |v| v["language"] = "f".into(),
            |v| v["cues"][0]["words"][0]["text"] = "li".into(),
        ] {
            assert!(matches!(damaged(edit), Err(LyricsError::Damaged(_))));
        }
    }

    #[test]
    fn raccord_compares_what_an_srt_holds_and_nothing_else() {
        let lyrics = lyrics_track();
        let srt = parse_srt_unbound(
            "1\r\n00:00:01,000 --> 00:00:03,000\r\nla la la\r\n\r\n\r\n7\n00:00:04,000 --> 00:00:05,000\nlo lo\n",
        )
        .unwrap();
        // Formatting, numbering, durations and word times never count.
        assert_eq!(differing_cues(&srt, &lyrics), 0);
        let mut changed = lyrics.clone();
        changed.cues[1].text = "lo li".into();
        assert_eq!(differing_cues(&srt, &changed), 1);
        changed.cues[0].end_ms = 2900;
        assert_eq!(differing_cues(&srt, &changed), 2);
        let mut more = lyrics.clone();
        more.cues.push(SubtitleCue::new(6000, 7000, "lu"));
        assert_eq!(differing_cues(&srt, &more), 1, "a cue on one side only");
        // One line inserted at the start counts 1, not every line after it.
        let mut first = lyrics.clone();
        first.cues.insert(0, SubtitleCue::new(0, 500, "oh"));
        assert_eq!(differing_cues(&srt, &first), 1);
        assert_eq!(differing_cues(&first, &srt), 1);
        let mut removed = lyrics.clone();
        removed.cues.remove(0);
        assert_eq!(differing_cues(&srt, &removed), 1);
        // CRLF inside a cue is what the SRT writer turns into LF.
        let mut crlf = lyrics.clone();
        crlf.cues[1].text = "lo\r\nlo".into();
        let mut lf = lyrics;
        lf.cues[1].text = "lo\nlo".into();
        assert_eq!(differing_cues(&crlf, &lf), 0);
        // An .srt read for the raccord is bound to no song.
        assert!(parse_srt_unbound("1\n90:00:00,000 --> 90:00:01,000\nx\n").is_ok());
    }

    #[test]
    fn the_lyrics_file_is_named_by_its_full_suffix() {
        let folder = Path::new("C:/x");
        let srt = folder.join("my.song.srt");
        let lyrics = lyrics_path(&srt);
        assert_eq!(lyrics, folder.join("my.song.noh-lyrics.json"));
        assert!(is_lyrics_file(&lyrics));
        assert!(is_lyrics_file(&path_for(&srt)), "the old name too");
        assert!(is_lyrics_file(Path::new("C:/x/Song.NOH-Lyrics.JSON")));
        for other in [
            "song.json",
            "song.lyrics.json",
            ".noh-lyrics.json",
            "song.srt",
        ] {
            assert!(!is_lyrics_file(&folder.join(other)), "{other}");
        }
        assert_eq!(srt_beside(&lyrics), Some(srt.clone()));
        assert_eq!(srt_beside(&path_for(&srt)), Some(srt));
        assert_eq!(srt_beside(&folder.join("song.json")), None);
    }

    #[test]
    fn readers_take_the_new_file_first_then_the_old_one() {
        let folder = tempfile::tempdir().unwrap();
        let srt = folder.path().join("song.srt");
        // The old file alone is read, as before.
        write_beside(&srt, &track(), Some("small")).unwrap();
        let mut read = track();
        read.cues[0].words = None;
        assert_eq!(read_beside(&srt, &mut read), 1);
        assert_eq!(model_beside(&srt).as_deref(), Some("small"));
        // The new file wins when both exist, and only identical cues get words.
        let mut newer = lyrics_track();
        newer.cues[0].words.as_mut().unwrap()[2].start_ms = 2200;
        write_lyrics(&lyrics_path(&srt), &newer, Some("large-v3")).unwrap();
        assert!(no_temp_left(folder.path()));
        let mut read = track();
        read.cues[0].words = None;
        assert_eq!(read_beside(&srt, &mut read), 1);
        assert_eq!(read.cues[0].words.as_ref().unwrap()[2].start_ms, 2200);
        assert_eq!(model_beside(&srt).as_deref(), Some("large-v3"));
        let mut edited = track();
        edited.cues[0].words = None;
        edited.cues[0].text = "la la lu".into();
        assert_eq!(read_beside(&srt, &mut edited), 0);
        // read_lyrics gives the whole file.
        assert_eq!(
            read_lyrics(&lyrics_path(&srt)).unwrap().unwrap().track,
            newer_bound(&newer)
        );
        // A newer new file is not lyrics: nothing merges (the old one is not used
        // in its place).
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(lyrics_path(&srt)).unwrap()).unwrap();
        v["version"] = 4.into();
        std::fs::write(lyrics_path(&srt), serde_json::to_vec(&v).unwrap()).unwrap();
        let mut read = track();
        read.cues[0].words = None;
        assert_eq!(read_beside(&srt, &mut read), 0);
        assert_eq!(model_beside(&srt), None);
    }

    fn newer_bound(t: &SubtitleTrack) -> SubtitleTrack {
        let mut t = t.clone();
        t.duration_ms = t.cues.last().map_or(1, |c| c.end_ms);
        t
    }

    fn voiced() -> SubtitleTrack {
        let mut t = lyrics_track();
        t.voices = vec![
            crate::subtitle_track::Voice {
                name: Some("Léa".into()),
            },
            crate::subtitle_track::Voice { name: None },
        ];
        t.cues[0].voices = vec![1];
        t.cues[1].voices = vec![1, 2];
        t
    }

    #[test]
    fn lyrics_without_voices_stay_version_2_and_voices_make_version_3() {
        let plain = lyrics_track();
        let json: serde_json::Value =
            serde_json::from_slice(&to_lyrics_json(&plain, None)).unwrap();
        assert_eq!(json["version"], 2);
        assert!(json.get("voices").is_none() && json["cues"][0].get("voices").is_none());

        let t = voiced();
        let bytes = to_lyrics_json(&t, None);
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["version"], 3);
        assert_eq!(json["voices"][0]["name"], "Léa");
        assert!(json["voices"][1].get("name").is_none());
        assert_eq!(json["cues"][1]["voices"], serde_json::json!([1, 2]));
        assert_eq!(parse_lyrics(&bytes).unwrap().track, t, "voices round-trip");

        // Declared voices alone (no line uses them yet) also need version 3.
        let mut declared = lyrics_track();
        declared.voices = vec![crate::subtitle_track::Voice::default(); 2];
        assert!(needs_version_3(&declared));
        // Taking every voice away writes version 2 again, byte for byte.
        let mut back = t.clone();
        back.voices.clear();
        back.cues.iter_mut().for_each(|c| c.voices.clear());
        assert_eq!(to_lyrics_json(&back, None), to_lyrics_json(&plain, None));
    }

    #[test]
    fn a_version_2_file_given_voices_from_outside_keeps_them() {
        let mut v: serde_json::Value =
            serde_json::from_slice(&to_lyrics_json(&voiced(), None)).unwrap();
        v["version"] = 2.into();
        let read = parse_lyrics(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(read.track, voiced());
    }

    #[test]
    fn undeclared_or_unordered_voices_make_the_file_damaged() {
        for (voices, cue) in [
            (serde_json::json!([{}]), serde_json::json!([2])),
            (serde_json::json!([{}, {}]), serde_json::json!([2, 1])),
            (serde_json::json!([{}, {}]), serde_json::json!([1, 1])),
            (serde_json::json!([{}]), serde_json::json!([0])),
            (
                serde_json::json!([{}, {}, {}, {}, {}]),
                serde_json::json!([1]),
            ),
            (serde_json::json!([{ "name": " " }]), serde_json::json!([1])),
        ] {
            let mut v: serde_json::Value =
                serde_json::from_slice(&to_lyrics_json(&voiced(), None)).unwrap();
            v["voices"] = voices.clone();
            v["cues"][1]["voices"] = cue.clone();
            assert!(
                matches!(
                    parse_lyrics(&serde_json::to_vec(&v).unwrap()),
                    Err(LyricsError::Damaged(_))
                ),
                "{voices} {cue}"
            );
        }
    }

    #[test]
    fn a_word_held_past_backing_vocals_needs_version_3() {
        let mut t = lyrics_track();
        let mut cue = SubtitleCue::new(4000, 6000, "matin (au matin)");
        let w = |text: &str, start_ms, end_ms| WordTime {
            text: text.into(),
            start_ms,
            end_ms,
            source: WordSource::Hand,
            syllables: None,
        };
        // « matin » is held until 5600, while « (au matin) » starts at 4800.
        cue.words = Some(vec![
            w("matin", 4000, Some(5600)),
            w("(au", 4800, None),
            w("matin)", 5200, None),
        ]);
        t.cues[1] = cue;
        t.duration_ms = 6000;
        assert!(needs_version_3(&t));
        let bytes = to_lyrics_json(&t, None);
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["version"], 3);
        assert_eq!(
            json["cues"][1]["words"][0]["end_ms"], 5600,
            "the held end is kept"
        );
        assert_eq!(parse_lyrics(&bytes).unwrap().track, t);
    }

    #[test]
    fn voices_follow_the_srt_by_times_then_by_text_and_the_lost_are_counted() {
        let from = {
            let mut t = SubtitleTrack {
                voices: vec![crate::subtitle_track::Voice::default(); 2],
                language: None,
                duration_ms: 10_000,
                cues: vec![
                    SubtitleCue::new(1000, 2000, "one"),
                    SubtitleCue::new(3000, 4000, "two"),
                    SubtitleCue::new(5000, 6000, "three"),
                    SubtitleCue::new(7000, 8000, "four"),
                ],
            };
            t.cues[0].voices = vec![1];
            t.cues[1].voices = vec![2];
            t.cues[2].voices = vec![1, 2];
            t.cues[3].voices = vec![2];
            t
        };
        let mut to = SubtitleTrack {
            voices: Vec::new(),
            language: None,
            duration_ms: 10_000,
            cues: vec![
                // Same times, typo fixed: keeps voice 1.
                SubtitleCue::new(1000, 2000, "One"),
                // Moved, same text: keeps voice 2.
                SubtitleCue::new(3200, 4200, "two"),
                // « three » is gone; « four » changed time and text: both lost.
                SubtitleCue::new(7500, 8500, "for"),
            ],
        };
        assert_eq!(carry_voices(&from, &mut to), 2);
        assert_eq!(to.voices, from.voices);
        let sets: Vec<Vec<u8>> = to.cues.iter().map(|c| c.voices.clone()).collect();
        assert_eq!(sets, [vec![1], vec![2], vec![]]);
        assert!(to.validate().is_ok());
    }
}
