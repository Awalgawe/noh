//! A song that is not a WAV is read through one WAV copy (`noh::song`), with
//! real FFmpeg. Masters are synthetic: a quiet noise floor and one click.
//!
//! The budget, measured before this path was chosen: the copy keeps the
//! master's clock to the sample (an MP3 with its LAME tag, FLAC, OGG), and an
//! M4A only keeps up to one AAC frame of end padding.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Once,
};

const RATE: u32 = 44_100;
const FRAMES: usize = 88_200;
const CLICK: usize = 30_000;
const AAC_FRAME: usize = 1024;

/// Song copies go to a cache folder of this test run, never the user's.
fn private_cache() {
    static CACHE: Once = Once::new();
    CACHE.call_once(|| {
        let folder = tempfile::Builder::new()
            .prefix("noh-song-cache-")
            .tempdir()
            .unwrap()
            .keep();
        // SAFETY: every test calls this first and `Once` makes the others wait,
        // so no thread of this binary reads the environment meanwhile.
        unsafe { std::env::set_var("NOH_CACHE_DIR", folder) };
    });
}

fn ffmpeg() -> PathBuf {
    noh::find_ffmpeg(None).unwrap()
}

/// Two seconds of stereo 16-bit: a deterministic noise floor and one click.
fn master(folder: &Path) -> PathBuf {
    let path = folder.join("master.wav");
    let data = (FRAMES * 4) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x02\0");
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(b"\x04\0\x10\0data");
    bytes.extend_from_slice(&data.to_le_bytes());
    let mut seed = 7_u32;
    for index in 0..FRAMES {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 16) as i32 - 32_768) / 300;
        let value = if index == CLICK { 26_000 } else { noise } as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

fn encode(master: &Path, output: &Path, codec: &[&str]) {
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-nostdin", "-v", "error", "-y", "-i"])
        .arg(master)
        .args(codec)
        .arg(output)
        .status()
        .unwrap();
    assert!(status.success(), "encoding {}", output.display());
}

/// The first channel of a float WAV, and its rate.
fn read_float(path: &Path) -> (Vec<f32>, u32) {
    let bytes = std::fs::read(path).unwrap();
    let mut at = 12;
    let (mut channels, mut rate, mut format) = (0usize, 0u32, 0u16);
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        match &bytes[at..at + 4] {
            b"fmt " => {
                format = u16::from_le_bytes([body[0], body[1]]);
                channels = u16::from_le_bytes([body[2], body[3]]) as usize;
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            }
            b"data" => {
                assert!(
                    format == 3 || format == 0xFFFE,
                    "float WAV, format {format}"
                );
                let samples = body
                    .chunks_exact(4 * channels)
                    .map(|frame| f32::from_le_bytes(frame[..4].try_into().unwrap()))
                    .collect();
                return (samples, rate);
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    panic!("no data chunk in {}", path.display());
}

fn loudest(samples: &[f32]) -> usize {
    (0..samples.len())
        .max_by(|&a, &b| samples[a].abs().total_cmp(&samples[b].abs()))
        .unwrap()
}

#[test]
fn an_mp3_is_read_through_a_wav_copy_on_the_masters_clock() {
    private_cache();
    let folder = tempfile::tempdir().unwrap();
    let mp3 = folder.path().join("song.mp3");
    encode(
        &master(folder.path()),
        &mp3,
        &["-c:a", "libmp3lame", "-b:a", "192k"],
    );
    let audio = noh::song::audio(&mp3, &ffmpeg(), &|| false).unwrap();
    assert_ne!(audio.path(), mp3.as_path());
    let (samples, rate) = read_float(audio.path());
    assert_eq!(rate, RATE);
    // The LAME tag's start and end padding are removed: not one sample moves.
    assert_eq!(samples.len(), FRAMES);
    assert_eq!(loudest(&samples), CLICK);
    let seconds = noh::song::duration(&mp3, &ffmpeg(), &|| false).unwrap();
    assert_eq!(seconds, FRAMES as f64 / f64::from(RATE));
    // Nothing is written beside the song: its lyrics and scene files are the
    // only things NOH ever puts there.
    let beside: Vec<_> = std::fs::read_dir(folder.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(beside.len(), 2, "{beside:?}");
}

#[test]
fn m4a_flac_and_ogg_songs_take_the_same_path() {
    private_cache();
    let folder = tempfile::tempdir().unwrap();
    let master = master(folder.path());
    for (name, codec, padding) in [
        ("song.m4a", &["-c:a", "aac", "-b:a", "192k"][..], AAC_FRAME),
        ("song.flac", &["-c:a", "flac"][..], 0),
        ("song.ogg", &["-c:a", "libvorbis", "-q:a", "5"][..], 0),
    ] {
        let song = folder.path().join(name);
        encode(&master, &song, codec);
        let audio = noh::song::audio(&song, &ffmpeg(), &|| false).unwrap();
        let (samples, rate) = read_float(audio.path());
        assert_eq!(rate, RATE, "{name}");
        assert_eq!(loudest(&samples), CLICK, "{name}");
        assert!(
            (FRAMES..=FRAMES + padding).contains(&samples.len()),
            "{name}: {} frames",
            samples.len()
        );
    }
}

#[test]
fn song_metadata_of_an_mp3_is_its_exact_length() {
    private_cache();
    let folder = tempfile::tempdir().unwrap();
    let mp3 = folder.path().join("song.MP3");
    encode(
        &master(folder.path()),
        &mp3,
        &["-c:a", "libmp3lame", "-b:a", "128k"],
    );
    let result = noh::inspection::inspect(
        &noh::inspection::InspectionRequest::Metadata {
            path: mp3.clone(),
            wav: true,
            ffmpeg: Some(ffmpeg()),
        },
        &mut |_| {},
    )
    .unwrap();
    let noh::inspection::InspectionResult::Metadata { info, .. } = result else {
        panic!("metadata expected");
    };
    assert_eq!(info.seconds, FRAMES as f64 / f64::from(RATE));
}

#[test]
fn a_file_that_is_not_a_song_fails_as_an_unreadable_song() {
    private_cache();
    let folder = tempfile::tempdir().unwrap();
    let fake = folder.path().join("song.mp3");
    std::fs::write(&fake, "not audio at all").unwrap();
    let error = noh::inspection::inspect(
        &noh::inspection::InspectionRequest::Metadata {
            path: fake,
            wav: true,
            ffmpeg: Some(ffmpeg()),
        },
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(error.code, "error.wav", "{error}");
}

#[test]
fn the_command_line_exports_an_mp3_song_on_its_own_clock() {
    private_cache();
    let folder = tempfile::tempdir().unwrap();
    let mp3 = folder.path().join("song.mp3");
    encode(
        &master(folder.path()),
        &mp3,
        &["-c:a", "libmp3lame", "-b:a", "192k"],
    );
    let video = folder.path().join("clip.mp4");
    let status = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("testsrc2=s=160x90:r=24:d=1")
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());
    let output = folder.path().join("video.mp4");
    let noh = std::env::var_os("NOH_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_noh")));
    let run = Command::new(noh)
        .arg(&video)
        .arg(&mp3)
        .arg("--output")
        .arg(&output)
        .env("NOH_FFMPEG", ffmpeg())
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let heard = folder.path().join("heard.wav");
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-nostdin", "-v", "error", "-y", "-i"])
        .arg(&output)
        .args(["-map", "0:a:0", "-c:a", "pcm_f32le", "-f", "wav"])
        .arg(&heard)
        .status()
        .unwrap();
    assert!(status.success());
    let (samples, rate) = read_float(&heard);
    let scale = f64::from(rate) / f64::from(RATE);
    // AAC in MP4 may keep up to one frame of end padding; the click stays put
    // within half a millisecond (an MP3 read without its LAME tag would be
    // 25 ms late).
    let frames = (FRAMES as f64 * scale) as usize;
    assert!(
        (frames..=frames + AAC_FRAME).contains(&samples.len()),
        "{} frames at {rate} Hz",
        samples.len()
    );
    let click = CLICK as f64 * scale;
    let found = loudest(&samples) as f64;
    assert!(
        (found - click).abs() <= f64::from(rate) / 2000.0,
        "click at {found}, expected {click}"
    );
}
