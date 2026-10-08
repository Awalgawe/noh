//! Trim only inaudible cue edges on the prepared PCM clock. This is not a voice
//! detector: music and quiet singing remain active, and no samples are removed.
use crate::subtitle_track::SubtitleTrack;
use std::{
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const BIN_MS: u64 = 10;
const BIN_BYTES: usize = 320; // 160 samples of 16 kHz mono PCM16.
const FLOOR: u16 = 4; // -78 dBFS; allows codec rounding in digital silence.

pub(super) fn trim_silence(path: &Path, track: &mut SubtitleTrack) -> io::Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let active = activity(&mut reader, track.duration_ms)?;
    trim(track, &active);
    track.validate().map_err(io::Error::other)
}

fn invalid() -> io::Error {
    io::Error::other("Invalid prepared 16 kHz mono PCM16 WAV")
}

// Parse only FFmpeg's private prepared WAV, already duration-validated by wav.rs.
// Unknown header chunks are skipped, not allocated; memory is <=720,000 bins.
fn activity(reader: &mut (impl Read + Seek), duration_ms: u64) -> io::Result<Vec<bool>> {
    let mut header = [0; 12];
    reader.read_exact(&mut header)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err(invalid());
    }
    let mut format = false;
    let bytes = loop {
        if reader.stream_position()? > 65_536 {
            return Err(invalid());
        }
        let mut chunk = [0; 8];
        reader.read_exact(&mut chunk)?;
        let size = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
        if &chunk[..4] == b"data" {
            if !format || size == 0 || size % 2 != 0 || size > super::MAX_AUDIO_MS * 32 {
                return Err(invalid());
            }
            break size;
        }
        let next = reader.stream_position()? + size + (size & 1);
        if next > 65_536 {
            return Err(invalid());
        }
        if &chunk[..4] == b"fmt " {
            if format || size < 16 {
                return Err(invalid());
            }
            let mut fmt = [0; 16];
            reader.read_exact(&mut fmt)?;
            if fmt != [1, 0, 1, 0, 128, 62, 0, 0, 0, 125, 0, 0, 2, 0, 16, 0] {
                return Err(invalid());
            }
            format = true;
        }
        reader.seek(SeekFrom::Start(next))?;
    };
    if bytes.div_ceil(BIN_BYTES as u64) != duration_ms.div_ceil(BIN_MS) {
        // Millisecond duration rounds to nearest; the last bin may straddle it.
        if bytes
            .div_ceil(BIN_BYTES as u64)
            .abs_diff(duration_ms.div_ceil(BIN_MS))
            > 1
        {
            return Err(invalid());
        }
    }
    let mut active = Vec::with_capacity(bytes.div_ceil(BIN_BYTES as u64) as usize);
    let mut buffer = [0; BIN_BYTES];
    let mut remaining = bytes;
    while remaining != 0 {
        let count = remaining.min(BIN_BYTES as u64) as usize;
        reader.read_exact(&mut buffer[..count])?;
        active.push(
            buffer[..count]
                .chunks_exact(2)
                .any(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > FLOOR),
        );
        remaining -= count as u64;
    }
    Ok(active)
}

fn trim(track: &mut SubtitleTrack, active: &[bool]) {
    track.cues.retain_mut(|cue| {
        let first = (cue.start_ms / BIN_MS) as usize;
        let end = (cue.end_ms.div_ceil(BIN_MS) as usize).min(active.len());
        let Some(bins) = active.get(first..end) else {
            return false;
        };
        let Some(start) = bins.iter().position(|&v| v) else {
            return false;
        };
        let last = bins.iter().rposition(|&v| v).unwrap();
        cue.start_ms = cue.start_ms.max((first + start) as u64 * BIN_MS);
        cue.end_ms = cue.end_ms.min((first + last + 1) as u64 * BIN_MS);
        cue.start_ms < cue.end_ms
    });
    if track.cues.is_empty() {
        track.language = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle_track::SubtitleCue;

    #[test]
    fn silence_edges_preserve_clock_quiet_audio_internal_gaps_and_final_word() {
        let mut track = SubtitleTrack {
            language: Some("en".into()),
            duration_ms: 90000,
            cues: vec![
                SubtitleCue {
                    start_ms: 0,
                    end_ms: 10000,
                    text: "first".into(),
                },
                SubtitleCue {
                    start_ms: 20000,
                    end_ms: 25000,
                    text: "silence".into(),
                },
                SubtitleCue {
                    start_ms: 71000,
                    end_ms: 90000,
                    text: "last".into(),
                },
            ],
        };
        let mut bins = vec![false; 9000];
        bins[300..500].fill(true);
        bins[600..800].fill(true);
        bins[7300..8900].fill(true);
        trim(&mut track, &bins);
        assert_eq!(track.cues.len(), 2);
        assert_eq!((track.cues[0].start_ms, track.cues[0].end_ms), (3000, 8000));
        assert_eq!(
            (track.cues[1].start_ms, track.cues[1].end_ms),
            (73000, 89000)
        );
        assert_eq!(track.duration_ms, 90000);
        track.validate().unwrap();
        trim(&mut track, &vec![false; 9000]);
        assert!(track.cues.is_empty());
        assert_eq!(track.language, None);
    }

    #[test]
    fn prepared_pcm_reader_keeps_quiet_signed_samples_and_bounds_headers() {
        let mut wav = b"RIFF".to_vec();
        wav.extend(996u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend([1, 0, 1, 0, 128, 62, 0, 0, 0, 125, 0, 0, 2, 0, 16, 0]);
        wav.extend(b"data");
        wav.extend(960u32.to_le_bytes());
        for sample in [0i16, -5, i16::MIN] {
            for _ in 0..160 {
                wav.extend(sample.to_le_bytes());
            }
        }
        assert_eq!(
            activity(&mut io::Cursor::new(&wav), 30).unwrap(),
            [false, true, true]
        );
        assert!(activity(&mut io::Cursor::new(&wav[..wav.len() - 1]), 30).is_err());
        wav[24] = 0;
        assert!(activity(&mut io::Cursor::new(&wav), 30).is_err());
    }
}
