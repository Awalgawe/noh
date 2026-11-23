//! Subtitles given to a job (CLI `--srt`, MCP `subtitles`, the app's caption
//! burn), lyrics plan 10: an .srt or a NOH lyrics file, kept in step with each
//! other before the job reads them, by the same rule as the app.
//!
//! - A lyrics file given: its .srt beside it is used (rebuilt from it when
//!   missing).
//! - An .srt given alone: its lyrics file is created beside it (never over an
//!   existing file; a folder that cannot be written goes on from the .srt).
//! - The two differ: the answer is required and unifies them. It comes from
//!   `truth` (CLI `--truth`, MCP `truth`) or from the terminal question; without
//!   it the job is refused (`error.lyrics_truth_required`) before anything is
//!   written. The file the answer replaces is first kept beside it under a
//!   free `.replaced` name, which the notes name.
//! - A lyrics file NOH cannot use (damaged, newer, locked): the .srt is used.
//!
//! The job then reads the .srt, which says what the lyrics say, and merges the
//! word times from the lyrics file beside it as before.
use crate::{engine::EngineError, subtitle_track::SubtitleTrack, subtitle_words as words};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Which file is the source of truth when the .srt and the lyrics differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Truth {
    /// The lyrics file: the .srt is rewritten from it.
    Lyrics,
    /// The .srt: the lyrics file takes its lines (identical lines keep their
    /// word times).
    Srt,
}

impl std::str::FromStr for Truth {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "lyrics" => Ok(Self::Lyrics),
            "srt" => Ok(Self::Srt),
            other => Err(format!("{other:?} is not lyrics or srt")),
        }
    }
}

/// The .srt and the lyrics file differ: what the question shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    pub srt: PathBuf,
    pub lyrics: PathBuf,
    /// Differing lines (0 when the .srt cannot be read as these lyrics).
    pub lines: usize,
    /// The .srt can be chosen (it reads as SRT).
    pub srt_usable: bool,
}

/// What resolving did, for the job's output (a locale code and its arguments).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub code: &'static str,
    pub args: Vec<String>,
}

impl Note {
    fn new(code: &'static str, path: &Path) -> Self {
        Self {
            code,
            args: vec![path.display().to_string()],
        }
    }
}

/// The .srt the job reads, and what was done to get it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub srt: PathBuf,
    pub notes: Vec<Note>,
}

enum Lyrics {
    Absent,
    Usable(words::Lyrics),
    Unusable(String),
}

enum Srt {
    Missing,
    Unreadable(String),
    Text(SubtitleTrack),
}

fn error(code: &str, path: &Path, detail: impl ToString) -> EngineError {
    EngineError::new(code, "resolve_lyrics", Some(path), detail)
}

fn read_srt(path: &Path) -> Srt {
    let Ok(file) = std::fs::File::open(path) else {
        return if path.exists() {
            Srt::Unreadable("the .srt cannot be opened".into())
        } else {
            Srt::Missing
        };
    };
    let mut bytes = Vec::new();
    if let Err(e) = file
        .take(crate::subtitle_srt::MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
    {
        return Srt::Unreadable(e.to_string());
    }
    if bytes.len() > crate::subtitle_srt::MAX_INPUT_BYTES {
        return Srt::Unreadable("SRT input exceeds one MiB.".into());
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Srt::Unreadable("the .srt is not UTF-8".into());
    };
    match words::parse_srt_unbound(&text) {
        Ok(track) => Srt::Text(track),
        Err(e) => Srt::Unreadable(e.to_string()),
    }
}

fn read_lyrics(path: &Path) -> Lyrics {
    match words::read_lyrics(path) {
        Ok(Ok(lyrics)) => Lyrics::Usable(lyrics),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Lyrics::Absent,
        Err(e) => Lyrics::Unusable(e.to_string()),
        Ok(Err(e)) => Lyrics::Unusable(e.to_string()),
    }
}

/// A free `<stem><suffix>`, `<stem><suffix-2>`… beside `path`.
fn free_name(path: &Path, stem: &str, ext: &str) -> PathBuf {
    (1..)
        .map(|n| {
            path.with_file_name(if n == 1 {
                format!("{stem}.replaced{ext}")
            } else {
                format!("{stem}.replaced-{n}{ext}")
            })
        })
        .find(|p| {
            matches!(std::fs::symlink_metadata(p),
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound)
        })
        .expect("a free name")
}

/// Create `path` with `bytes` only if nothing is there; a failed write removes
/// what it began.
fn create_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if written.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    written
}

fn srt_stem(srt: &Path) -> String {
    srt.file_stem()
        .map_or("lyrics".into(), |s| s.to_string_lossy().into_owned())
}

/// Resolve `input` (an .srt or a lyrics file) before a job starts. `ask` is
/// the terminal question, called only when the two differ and `truth` is
/// None; it returns None when no answer can be asked (not a terminal).
pub fn resolve(
    input: &Path,
    truth: Option<Truth>,
    ask: impl FnOnce(&Difference) -> Option<Truth>,
) -> Result<Resolved, EngineError> {
    let lyrics_input = words::is_lyrics_file(input)
        && input
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(words::LYRICS_SUFFIX));
    let srt = if words::is_lyrics_file(input) {
        words::find_srt_beside(input)
            .or_else(|| words::srt_beside(input))
            .ok_or_else(|| error("error.caption_input", input, "No .srt name for this file"))?
    } else {
        input.to_path_buf()
    };
    let lyrics_path = if lyrics_input {
        input.to_path_buf()
    } else {
        words::lyrics_path(&srt)
    };
    resolve_pair(&srt, &lyrics_path, truth, ask, true)
}

fn resolve_pair(
    srt: &Path,
    lyrics_path: &Path,
    truth: Option<Truth>,
    ask: impl FnOnce(&Difference) -> Option<Truth>,
    retry: bool,
) -> Result<Resolved, EngineError> {
    let mut notes = Vec::new();
    let done = |notes| Resolved {
        srt: srt.to_path_buf(),
        notes,
    };
    match (read_srt(srt), read_lyrics(lyrics_path)) {
        (Srt::Missing, Lyrics::Usable(lyrics)) => {
            // Rebuilt from the lyrics (never over an .srt made meanwhile).
            let text = lyrics
                .track
                .to_srt()
                .map_err(|e| error("error.caption_track", lyrics_path, e))?;
            match create_new(srt, text.as_bytes()) {
                Ok(()) => notes.push(Note::new("lyrics_input.srt_rewritten", srt)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && retry => {
                    return resolve_pair(srt, lyrics_path, truth, ask, false);
                }
                Err(e) => return Err(error("error.caption_input", srt, e)),
            }
            Ok(done(notes))
        }
        (Srt::Text(srt_track), Lyrics::Usable(lyrics)) => {
            let lines = words::differing_cues(&srt_track, &lyrics.track);
            if lines == 0 {
                return Ok(done(notes));
            }
            let difference = Difference {
                srt: srt.to_path_buf(),
                lyrics: lyrics_path.to_path_buf(),
                lines,
                srt_usable: true,
            };
            let truth = truth.or_else(|| ask(&difference));
            unify(
                srt,
                lyrics_path,
                &difference,
                truth,
                Some(srt_track),
                &lyrics,
                notes,
            )
        }
        (Srt::Unreadable(reason), Lyrics::Usable(lyrics)) => {
            let difference = Difference {
                srt: srt.to_path_buf(),
                lyrics: lyrics_path.to_path_buf(),
                lines: 0,
                srt_usable: false,
            };
            let truth = truth.or_else(|| ask(&difference));
            if truth == Some(Truth::Srt) {
                return Err(error(
                    "error.caption_track",
                    srt,
                    format!("The .srt cannot be the source of truth: {reason}"),
                ));
            }
            unify(srt, lyrics_path, &difference, truth, None, &lyrics, notes)
        }
        (Srt::Text(srt_track), Lyrics::Absent) => {
            // A version-1 file beside it: read as before (word times merged).
            if words::path_for(srt).exists() {
                return Ok(done(notes));
            }
            match words::create_lyrics(lyrics_path, &srt_track, None) {
                Ok(()) => notes.push(Note::new("lyrics_input.created", lyrics_path)),
                // Another job created it meanwhile: compared as it is now.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && retry => {
                    return resolve_pair(srt, lyrics_path, truth, ask, false);
                }
                // A folder that cannot be written: the job goes on from the .srt.
                Err(e) => notes.push(Note {
                    code: "lyrics_input.not_created",
                    args: vec![lyrics_path.display().to_string(), e.to_string()],
                }),
            }
            Ok(done(notes))
        }
        (Srt::Text(_), Lyrics::Unusable(reason)) => {
            notes.push(Note {
                code: "lyrics_input.unusable",
                args: vec![lyrics_path.display().to_string(), reason],
            });
            Ok(done(notes))
        }
        // No usable lyrics and no readable .srt: the job reports the .srt.
        (Srt::Missing | Srt::Unreadable(_), _) => Ok(done(notes)),
    }
}

/// The required answer, then both files made identical: the file it replaces
/// is kept beside under a free `.replaced` name first.
fn unify(
    srt: &Path,
    lyrics_path: &Path,
    difference: &Difference,
    truth: Option<Truth>,
    srt_track: Option<SubtitleTrack>,
    lyrics: &words::Lyrics,
    mut notes: Vec<Note>,
) -> Result<Resolved, EngineError> {
    let Some(truth) = truth else {
        // Only what can be chosen: an .srt that does not read cannot be.
        let accepted: &[&str] = if difference.srt_usable {
            &["lyrics", "srt"]
        } else {
            &["lyrics"]
        };
        let detail = if difference.srt_usable {
            format!(
                "The .srt and the lyrics file differ ({}). Say which one is the source of truth: truth \"lyrics\" (rewrites the .srt) or \"srt\" (replaces the lyrics).",
                lines_text(difference.lines)
            )
        } else {
            "The .srt cannot be read as these lyrics. Only truth \"lyrics\" (rewrites the .srt from the lyrics file) is accepted.".to_owned()
        };
        let mut refused = error("error.lyrics_truth_required", srt, detail);
        refused.technical = serde_json::json!({
            "srt": difference.srt,
            "lyrics": difference.lyrics,
            "differing_lines": difference.lines,
            "srt_usable": difference.srt_usable,
            "accepted": accepted,
        })
        .to_string()
        .into();
        return Err(refused);
    };
    let stem = srt_stem(srt);
    match truth {
        Truth::Lyrics => {
            let text = lyrics
                .track
                .to_srt()
                .map_err(|e| error("error.caption_track", lyrics_path, e))?;
            if srt.exists() {
                words::ensure_writable(srt).map_err(|e| error("error.caption_input", srt, e))?;
                let aside = free_name(srt, &stem, ".srt");
                std::fs::rename(srt, &aside).map_err(|e| error("error.caption_input", srt, e))?;
                notes.push(Note::new("lyrics_input.replaced", &aside));
            }
            create_new(srt, text.as_bytes()).map_err(|e| error("error.caption_input", srt, e))?;
        }
        Truth::Srt => {
            let mut chosen = srt_track.expect("an .srt that reads");
            words::merge(&mut chosen, &words::to_lyrics_json(&lyrics.track, None));
            chosen.language = lyrics.track.language.clone();
            let voices_lost = words::carry_voices(&lyrics.track, &mut chosen);
            if voices_lost > 0 {
                notes.push(Note {
                    code: "lyrics_input.voices_lost",
                    args: vec![voices_lost.to_string()],
                });
            }
            // Not `<stem>.replaced.noh-lyrics.json`: that would read as the
            // lyrics file of a `<stem>.replaced.srt`.
            words::ensure_writable(lyrics_path)
                .map_err(|e| error("error.caption_input", lyrics_path, e))?;
            let aside = free_name(lyrics_path, &format!("{stem}.noh-lyrics"), ".json");
            std::fs::rename(lyrics_path, &aside)
                .map_err(|e| error("error.caption_input", lyrics_path, e))?;
            notes.push(Note::new("lyrics_input.replaced", &aside));
            words::write_lyrics(lyrics_path, &chosen, lyrics.model.as_deref())
                .map_err(|e| error("error.caption_input", lyrics_path, e))?;
        }
    }
    notes.push(Note {
        code: match truth {
            Truth::Lyrics => "lyrics_input.unified_lyrics",
            Truth::Srt => "lyrics_input.unified_srt",
        },
        args: Vec::new(),
    });
    Ok(Resolved {
        srt: srt.to_path_buf(),
        notes,
    })
}

/// The terminal's answer to `difference`, or None when it is not a terminal
/// (both stdin and stderr must be) or under `--json`.
pub fn ask_terminal(difference: &Difference, json: bool) -> Option<Truth> {
    use std::io::IsTerminal;
    if json || !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return None;
    }
    let prompt = if difference.srt_usable {
        format!(
            "The .srt and the lyrics file differ ({}). Which one is the source of truth? [l]yrics (rewrites the .srt) / [s]rt (replaces the lyrics): ",
            lines_text(difference.lines)
        )
    } else {
        "The .srt cannot be read as these lyrics. Rewrite it from the lyrics file? [l]yrics / anything else to stop: ".to_owned()
    };
    for _ in 0..3 {
        eprint!("{prompt}");
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).ok()? == 0 {
            return None;
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "l" | "lyrics" | "p" | "paroles" => return Some(Truth::Lyrics),
            "s" | "srt" if difference.srt_usable => return Some(Truth::Srt),
            _ if !difference.srt_usable => return None,
            _ => {}
        }
    }
    None
}

/// « 1 line », « 3 lines ».
fn lines_text(n: usize) -> String {
    if n == 1 {
        "1 line".to_owned()
    } else {
        format!("{n} lines")
    }
}

/// The notes as English lines (CLI stderr).
pub fn note_text(note: &Note) -> String {
    crate::i18n::Language::En.format(note.code, &note.args)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str =
        "1\n00:00:01,000 --> 00:00:03,000\nla la la\n\n2\n00:00:04,000 --> 00:00:05,000\nlo lo\n\n";
    const B: &str =
        "1\n00:00:01,000 --> 00:00:03,000\nla la la\n\n2\n00:00:04,000 --> 00:00:05,000\nlo li\n\n";

    fn lyrics(text: &str) -> SubtitleTrack {
        words::parse_srt_unbound(text).unwrap()
    }

    fn no_terminal(_: &Difference) -> Option<Truth> {
        None
    }

    #[test]
    fn an_srt_alone_gets_its_lyrics_file_and_nothing_else_changes() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        std::fs::write(&srt, A).unwrap();
        let resolved = resolve(&srt, None, no_terminal).unwrap();
        assert_eq!(resolved.srt, srt);
        assert_eq!(resolved.notes[0].code, "lyrics_input.created");
        let made = words::read_lyrics(&words::lyrics_path(&srt))
            .unwrap()
            .unwrap();
        assert_eq!(made.track.cues, lyrics(A).cues);
        assert_eq!(std::fs::read_to_string(&srt).unwrap(), A);
        // Again: the same, nothing to do.
        assert!(resolve(&srt, None, no_terminal).unwrap().notes.is_empty());
    }

    #[test]
    fn a_lyrics_file_given_uses_its_srt_and_rebuilds_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        let file = words::lyrics_path(&srt);
        words::write_lyrics(&file, &lyrics(A), None).unwrap();
        let resolved = resolve(&file, None, no_terminal).unwrap();
        assert_eq!(resolved.srt, srt);
        assert_eq!(resolved.notes[0].code, "lyrics_input.srt_rewritten");
        assert_eq!(
            lyrics(&std::fs::read_to_string(&srt).unwrap()).cues,
            lyrics(A).cues
        );
    }

    fn read_only(path: &Path, on: bool) {
        let mut p = std::fs::metadata(path).unwrap().permissions();
        p.set_readonly(on);
        std::fs::set_permissions(path, p).unwrap();
    }

    /// The truth answer never rewrites, replaces or sets aside a file the user
    /// made read-only, on any system (Linux would let a rename replace it).
    #[test]
    fn a_read_only_file_is_left_as_it_is_whatever_the_truth() {
        for (truth, protected_srt) in [(Truth::Lyrics, true), (Truth::Srt, false)] {
            let dir = tempfile::tempdir().unwrap();
            let srt = dir.path().join("song.srt");
            let file = words::lyrics_path(&srt);
            std::fs::write(&srt, B).unwrap();
            words::write_lyrics(&file, &lyrics(A), None).unwrap();
            let target = if protected_srt { &srt } else { &file };
            let before = (std::fs::read(&srt).unwrap(), std::fs::read(&file).unwrap());
            read_only(target, true);
            let refused = resolve(&srt, Some(truth), no_terminal).unwrap_err();
            read_only(target, false);
            assert_eq!(refused.code, "error.caption_input", "{truth:?}");
            assert!(refused.detail.contains("read-only"), "{}", refused.detail);
            assert_eq!(
                (std::fs::read(&srt).unwrap(), std::fs::read(&file).unwrap()),
                before,
                "{truth:?}: both files as they were"
            );
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                2,
                "{truth:?}: nothing set aside"
            );
        }
    }

    #[test]
    fn a_difference_without_an_answer_is_refused_before_any_write() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        std::fs::write(&srt, B).unwrap();
        words::write_lyrics(&words::lyrics_path(&srt), &lyrics(A), None).unwrap();
        let before = std::fs::read(words::lyrics_path(&srt)).unwrap();
        let mut asked = false;
        let refused = resolve(&srt, None, |d| {
            asked = true;
            assert_eq!((d.lines, d.srt_usable), (1, true));
            None
        })
        .unwrap_err();
        assert!(asked, "the terminal was asked");
        assert_eq!(refused.code, "error.lyrics_truth_required");
        let technical: serde_json::Value = serde_json::from_str(&refused.technical).unwrap();
        assert_eq!(technical["differing_lines"], 1);
        assert!(refused.detail.contains("(1 line)"), "{}", refused.detail);
        assert_eq!(technical["accepted"], serde_json::json!(["lyrics", "srt"]));
        assert_eq!(std::fs::read_to_string(&srt).unwrap(), B);
        assert_eq!(std::fs::read(words::lyrics_path(&srt)).unwrap(), before);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            2,
            "nothing else"
        );
    }

    #[test]
    fn truth_lyrics_rewrites_the_srt_and_keeps_the_old_one_beside() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        std::fs::write(&srt, B).unwrap();
        std::fs::write(dir.path().join("song.replaced.srt"), "taken").unwrap();
        words::write_lyrics(&words::lyrics_path(&srt), &lyrics(A), None).unwrap();
        let resolved = resolve(&srt, Some(Truth::Lyrics), no_terminal).unwrap();
        let aside = dir.path().join("song.replaced-2.srt");
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), B);
        assert_eq!(
            resolved.notes[0],
            Note::new("lyrics_input.replaced", &aside)
        );
        assert_eq!(resolved.notes[1].code, "lyrics_input.unified_lyrics");
        assert_eq!(
            lyrics(&std::fs::read_to_string(&srt).unwrap()).cues,
            lyrics(A).cues
        );
    }

    #[test]
    fn truth_srt_replaces_the_lyrics_keeping_identical_word_times() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        std::fs::write(&srt, B).unwrap();
        let mut worded = lyrics(A);
        worded.cues[0].words = Some(crate::subtitle_edit::words_or_estimate(&worded.cues[0]));
        worded.cues[1].words = Some(crate::subtitle_edit::words_or_estimate(&worded.cues[1]));
        words::write_lyrics(&words::lyrics_path(&srt), &worded, Some("small")).unwrap();
        let resolved = resolve(&srt, Some(Truth::Srt), no_terminal).unwrap();
        let aside = dir.path().join("song.noh-lyrics.replaced.json");
        assert!(
            !words::is_lyrics_file(&aside),
            "not read as another lyrics file"
        );
        assert_eq!(
            resolved.notes[0],
            Note::new("lyrics_input.replaced", &aside)
        );
        let now = words::read_lyrics(&words::lyrics_path(&srt))
            .unwrap()
            .unwrap();
        assert_eq!(now.track.cues[1].text, "lo li");
        assert!(
            now.track.cues[0].words.is_some(),
            "identical line keeps its words"
        );
        assert!(now.track.cues[1].words.is_none());
        assert_eq!(now.model.as_deref(), Some("small"));
        assert_eq!(
            words::read_lyrics(&aside).unwrap().unwrap().track.cues[1].text,
            "lo lo"
        );
    }

    #[test]
    fn truth_is_ignored_without_a_difference_and_an_unreadable_srt_offers_only_lyrics() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        std::fs::write(&srt, A).unwrap();
        words::write_lyrics(&words::lyrics_path(&srt), &lyrics(A), None).unwrap();
        assert!(
            resolve(&srt, Some(Truth::Srt), no_terminal)
                .unwrap()
                .notes
                .is_empty()
        );
        std::fs::write(&srt, "not an srt").unwrap();
        let unanswered = resolve(&srt, None, no_terminal).unwrap_err();
        let technical: serde_json::Value = serde_json::from_str(&unanswered.technical).unwrap();
        assert_eq!(technical["accepted"], serde_json::json!(["lyrics"]));
        let refused = resolve(&srt, Some(Truth::Srt), no_terminal).unwrap_err();
        assert_eq!(refused.code, "error.caption_track");
        assert_eq!(std::fs::read_to_string(&srt).unwrap(), "not an srt");
        resolve(&srt, Some(Truth::Lyrics), no_terminal).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("song.replaced.srt")).unwrap(),
            "not an srt"
        );
    }

    #[test]
    fn an_unusable_lyrics_file_leaves_the_srt_in_use_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        std::fs::write(&srt, A).unwrap();
        std::fs::write(words::lyrics_path(&srt), br#"{"version":4,"cues":[]}"#).unwrap();
        let resolved = resolve(&srt, None, no_terminal).unwrap();
        assert_eq!(resolved.notes[0].code, "lyrics_input.unusable");
        assert_eq!(
            std::fs::read(words::lyrics_path(&srt)).unwrap(),
            br#"{"version":4,"cues":[]}"#
        );
    }

    #[test]
    fn a_lyrics_path_that_is_not_a_file_leaves_the_srt_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        std::fs::write(&srt, A).unwrap();
        // A folder at the lyrics file's name: not lyrics, never replaced.
        std::fs::create_dir(words::lyrics_path(&srt)).unwrap();
        let resolved = resolve(&srt, None, no_terminal).unwrap();
        assert_eq!(resolved.notes[0].code, "lyrics_input.unusable");
        assert_eq!(resolved.srt, srt);
        assert!(words::lyrics_path(&srt).is_dir());
    }

    #[test]
    fn two_jobs_on_one_srt_alone_create_one_lyrics_file() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        std::fs::write(&srt, A).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let srt = srt.clone();
                std::thread::spawn(move || resolve(&srt, None, no_terminal))
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(Result::is_ok));
        let created = results
            .iter()
            .filter(|r| {
                r.as_ref()
                    .unwrap()
                    .notes
                    .iter()
                    .any(|n| n.code == "lyrics_input.created")
            })
            .count();
        assert_eq!(created, 1, "one job created it, the others read it");
        assert!(
            words::read_lyrics(&words::lyrics_path(&srt))
                .unwrap()
                .is_ok()
        );
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn truth_reads_from_the_command_line_words() {
        assert_eq!("lyrics".parse::<Truth>(), Ok(Truth::Lyrics));
        assert_eq!("srt".parse::<Truth>(), Ok(Truth::Srt));
        assert!("paroles".parse::<Truth>().is_err());
        assert_eq!(serde_json::to_string(&Truth::Srt).unwrap(), "\"srt\"");
    }

    #[test]
    fn truth_srt_keeps_the_voices_it_can_and_names_how_many_lines_lose_them() {
        let dir = tempfile::tempdir().unwrap();
        let srt = dir.path().join("song.srt");
        // Same times for the first line, new times and text for the second.
        std::fs::write(
            &srt,
            "1\n00:00:01,000 --> 00:00:03,000\nla la la\n\n2\n00:00:04,100 --> 00:00:05,000\nlo lu\n\n",
        )
        .unwrap();
        let mut voiced = lyrics(A);
        voiced.voices = vec![crate::subtitle_track::Voice::default(); 2];
        voiced.cues[0].voices = vec![1];
        voiced.cues[1].voices = vec![1, 2];
        words::write_lyrics(&words::lyrics_path(&srt), &voiced, None).unwrap();
        let resolved = resolve(&srt, Some(Truth::Srt), no_terminal).unwrap();
        assert!(resolved.notes.contains(&Note {
            code: "lyrics_input.voices_lost",
            args: vec!["1".into()],
        }));
        let now = words::read_lyrics(&words::lyrics_path(&srt))
            .unwrap()
            .unwrap()
            .track;
        assert_eq!(now.voices, voiced.voices, "the declared voices stay");
        assert_eq!(now.cues[0].voices, [1]);
        assert!(now.cues[1].voices.is_empty());
        assert_eq!(
            note_text(resolved.notes.last().unwrap()),
            "The lyrics file was replaced by the .srt."
        );
    }
}
