//! What a scene reads of a song, gathered from the files of a job: the lyrics
//! (cues and their word times, from the reviewed SRT and the lyrics file beside
//! it, as caption effects read them), the sound's energy and onsets (decoded from
//! the source's first audio stream by FFmpeg), the canvas and its safe area.
use super::audio::Audio;
use super::{Cue, Inputs};
use crate::safe_area::SafeArea;
use crate::subtitle_track::SubtitleTrack;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Samples per second the sound is decoded at for the analysis: enough for an
/// envelope at 100 per second and onsets 30 ms apart.
const ANALYSIS_HZ: u32 = 8000;
/// Longest decode of the sound (a two-hour source decodes in well under this).
const DECODE_DEADLINE: Duration = Duration::from_secs(120);

/// A scene's inputs with what was said about them.
pub struct Song {
    pub inputs: Inputs,
    pub audio: Audio,
    /// Notes for the report (no sound, estimated word times).
    pub notes: Vec<String>,
}

/// The cues as a scene reads them, from a track whose word times were read
/// beside the subtitles where they exist (`words` cues), split by letters
/// elsewhere.
pub fn cues(track: &SubtitleTrack) -> Vec<Cue> {
    crate::fx::line::from_track(track)
        .into_iter()
        .map(|l| Cue {
            start: l.start,
            end: l.end,
            text: l
                .words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(&l.sep),
            words: l
                .words
                .iter()
                .map(|w| (w.text.clone(), w.start, w.end))
                .collect(),
        })
        .collect()
}

/// The safe area as fractions of the frame: x0, y0, x1, y1.
pub fn safe_fractions(safe: SafeArea, w: u32, h: u32) -> [f32; 4] {
    match safe.insets(w, h) {
        Some([l, t, r, b]) => [
            l as f32 / w as f32,
            t as f32 / h as f32,
            1.0 - r as f32 / w as f32,
            1.0 - b as f32 / h as f32,
        ],
        None => [0.0, 0.0, 1.0, 1.0],
    }
}

/// Run FFmpeg to its end within `deadline` (killed past it, and with this
/// process), keeping its output and the start of its error output.
pub(crate) fn capture(
    mut cmd: std::process::Command,
    deadline: Duration,
) -> Result<(std::process::ExitStatus, Vec<u8>, String), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = crate::process::Tree::spawn(cmd, true).map_err(|e| format!("FFmpeg: {e}"))?;
    let mut out = tree.0.stdout().take().ok_or("FFmpeg: no output")?;
    let mut err = tree.0.stderr().take().ok_or("FFmpeg: no error output")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        out.read_to_end(&mut bytes).map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = (&mut err).take(16 * 1024).read_to_string(&mut text);
        let _ = std::io::copy(&mut err, &mut std::io::sink());
        text
    });
    let until = Instant::now() + deadline;
    let status = loop {
        if let Some(s) = tree.0.try_wait().map_err(|e| format!("FFmpeg: {e}"))? {
            break s;
        }
        if Instant::now() > until {
            let _ = tree.0.start_kill();
            return Err("FFmpeg took too long".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let bytes = reader
        .join()
        .map_err(|_| "FFmpeg output reader failed".to_string())?
        .map_err(|e| format!("FFmpeg output: {e}"))?;
    Ok((status, bytes, errors.join().unwrap_or_default()))
}

/// The source's first audio stream as mono samples at `ANALYSIS_HZ`, or None
/// when it has none.
pub fn decode_sound(ffmpeg: &Path, source: &Path) -> Result<Option<Vec<f32>>, String> {
    let mut cmd = crate::command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(source)
        .args([
            "-map",
            "0:a:0?",
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "1",
            "-ar",
            &ANALYSIS_HZ.to_string(),
            "-f",
            "f32le",
            "pipe:1",
        ]);
    let (status, bytes, log) = capture(cmd, DECODE_DEADLINE)?;
    if !status.success() {
        return Err(format!("FFmpeg could not decode the sound: {}", log.trim()));
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
    ))
}

/// The inputs of a scene drawn over `source` (`w` x `h`, `duration` seconds;
/// `sound`: whether it has an audio stream) with `track` (word times read
/// beside it for `words` of its cues).
#[allow(clippy::too_many_arguments)]
pub fn gather(
    ffmpeg: &Path,
    source: &Path,
    sound: bool,
    (w, h): (u32, u32),
    duration: f64,
    track: &SubtitleTrack,
    words: usize,
    safe: SafeArea,
) -> Result<Song, String> {
    let mut notes = Vec::new();
    // FFmpeg refuses an output with no stream: a video without sound is not
    // decoded at all.
    let samples = if sound {
        decode_sound(ffmpeg, source)?
    } else {
        None
    };
    let audio = match samples {
        Some(samples) => Audio::analyse(&samples, ANALYSIS_HZ),
        None => {
            notes.push("the video has no sound: energy is 0 and there is no onset".into());
            Audio::silent()
        }
    };
    let cues = cues(track);
    let words_estimated = words < cues.len();
    if words_estimated {
        notes.push(format!(
            "{} of {} lines have no measured word times (no lyrics file beside the subtitles, or lines without them): their words are spread by letter count, too loose for word-by-word effects",
            cues.len() - words.min(cues.len()),
            cues.len()
        ));
    }
    Ok(Song {
        inputs: Inputs {
            w,
            h,
            src_w: w,
            src_h: h,
            duration,
            cues,
            words_estimated,
            safe: safe_fractions(safe, w, h),
            shaders: Vec::new(),
        },
        audio,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_safe_area_becomes_fractions_of_the_frame() {
        assert_eq!(
            safe_fractions(SafeArea::None, 1920, 1080),
            [0.0, 0.0, 1.0, 1.0]
        );
        let s = safe_fractions(SafeArea::YoutubeShorts, 1080, 1920);
        let [l, t, r, b] = SafeArea::YoutubeShorts.insets(1080, 1920).unwrap();
        assert_eq!(s[0], l as f32 / 1080.0);
        assert_eq!(s[1], t as f32 / 1920.0);
        assert_eq!(s[2], 1.0 - r as f32 / 1080.0);
        assert_eq!(s[3], 1.0 - b as f32 / 1920.0);
    }
}
