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
    crate::style::line::from_track(track)
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
    cmd: std::process::Command,
    deadline: Duration,
) -> Result<(std::process::ExitStatus, Vec<u8>, String), String> {
    capture_with(cmd, deadline, |mut out| {
        let mut bytes = Vec::new();
        out.read_to_end(&mut bytes).map(|_| bytes)
    })
}

/// `capture`, with FFmpeg's output read as it comes by `read`.
pub(crate) fn capture_with<T: Send + 'static>(
    mut cmd: std::process::Command,
    deadline: Duration,
    read: impl FnOnce(Box<dyn Read + Send>) -> std::io::Result<T> + Send + 'static,
) -> Result<(std::process::ExitStatus, T, String), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = crate::process::Tree::spawn(cmd, true).map_err(|e| format!("FFmpeg: {e}"))?;
    let out = tree.0.stdout().take().ok_or("FFmpeg: no output")?;
    let mut err = tree.0.stderr().take().ok_or("FFmpeg: no error output")?;
    let reader = std::thread::spawn(move || {
        let out: Box<dyn Read + Send> = Box::new(out);
        read(out)
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

/// The loudness of the source's first audio stream, mono at its own sample
/// rate (`audio::Loudness`), or None when it has none. FFmpeg writes it as a
/// WAV of 32-bit floats, whose header gives the rate; it is read as it comes.
pub fn sound_loudness(ffmpeg: &Path, source: &Path) -> Result<Option<Vec<f32>>, String> {
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
            "-c:a",
            "pcm_f32le",
            "-f",
            "wav",
            "pipe:1",
        ]);
    let (status, loudness, log) = capture_with(cmd, DECODE_DEADLINE, read_wav_loudness)?;
    if !status.success() {
        return Err(format!("FFmpeg could not decode the sound: {}", log.trim()));
    }
    Ok(loudness)
}

/// A mono WAV of 32-bit floats, streamed: its loudness per hundredth of a
/// second, or None when FFmpeg wrote nothing (no audio stream).
fn read_wav_loudness(mut r: Box<dyn Read + Send>) -> std::io::Result<Option<Vec<f32>>> {
    use std::io::{Error, ErrorKind};
    let bad = |what: &str| Error::new(ErrorKind::InvalidData, format!("sound: {what}"));
    let mut head = [0u8; 12];
    match r.read_exact(&mut head) {
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        other => other?,
    }
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Err(bad("not a WAV"));
    }
    let mut hz = 0u32;
    loop {
        let mut chunk = [0u8; 8];
        r.read_exact(&mut chunk)?;
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        if &chunk[0..4] == b"data" {
            break;
        }
        // Headers are small; a size past this is not one FFmpeg writes.
        if size > 1 << 20 {
            return Err(bad("header chunk too large"));
        }
        let mut body = vec![0u8; size as usize + (size as usize & 1)];
        r.read_exact(&mut body)?;
        if &chunk[0..4] == b"fmt " {
            if body.len() < 16 {
                return Err(bad("short format chunk"));
            }
            let format = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let bits = u16::from_le_bytes([body[14], body[15]]);
            // 3: IEEE float; 0xFFFE: the extensible form FFmpeg may write.
            if !matches!(format, 3 | 0xFFFE) || channels != 1 || bits != 32 {
                return Err(bad("not mono 32-bit float"));
            }
            hz = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
        }
    }
    if hz == 0 {
        return Err(bad("no format before the samples"));
    }
    let mut loudness = super::audio::Loudness::new(hz);
    let mut buf = vec![0u8; 64 * 1024];
    let mut carry = 0usize;
    let mut samples = Vec::with_capacity(buf.len() / 4);
    loop {
        let n = r.read(&mut buf[carry..])?;
        if n == 0 {
            break;
        }
        let have = carry + n;
        let whole = have / 4 * 4;
        samples.clear();
        samples.extend(
            buf[..whole]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c)),
        );
        loudness.push(&samples);
        buf.copy_within(whole..have, 0);
        carry = have - whole;
    }
    Ok(Some(loudness.finish()))
}

/// The inputs of a scene drawn on a frame of `w` x `h` over `source` (its
/// picture `src` pixels, `duration` seconds; `sound`: whether it has an audio
/// stream) with `track` (word times read beside it for `words` of its cues).
#[allow(clippy::too_many_arguments)]
pub fn gather(
    ffmpeg: &Path,
    source: &Path,
    sound: bool,
    (w, h): (u32, u32),
    src: (u32, u32),
    duration: f64,
    track: &SubtitleTrack,
    words: usize,
    safe: SafeArea,
) -> Result<Song, String> {
    let mut notes = Vec::new();
    // FFmpeg refuses an output with no stream: a video without sound is not
    // decoded at all.
    let loudness = if sound {
        sound_loudness(ffmpeg, source)?
    } else {
        None
    };
    let audio = match loudness {
        Some(raw) => Audio::from_loudness(raw),
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
            src_w: src.0,
            src_h: src.1,
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
