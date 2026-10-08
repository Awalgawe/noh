//! Deterministic synthetic media for GUI layout and playback checks.
use super::{Result, process};
use noh::{
    engine::ExportRequest,
    input::MediaItem,
    project::{ProjectRequest, ProjectShort},
    shorts::ShortCaptions,
};
use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const DEADLINE: Duration = Duration::from_secs(120);
/// Sample soundtrack length in seconds.
pub const SONG_SECONDS: f64 = 182.4;
const TRUNCATED_BYTES: usize = 16 * 1024;

#[derive(clap::Args)]
pub struct Options {
    /// Fixture set to write.
    #[arg(value_parser = ["layout"])]
    pub set: String,
    /// New or empty folder receiving the files; existing files are never replaced.
    pub output: PathBuf,
    #[arg(long)]
    pub ffmpeg: Option<PathBuf>,
}

const FILES: [&str; 7] = [
    "ma-chanson.wav",
    "vagues.mp4",
    "ville-nuit.jpg",
    "concert.mov",
    "ma-chanson.srt",
    "project.json",
    "project-short.json",
];

/// Deterministic pseudo-random helper for repeatable cue timing.
fn hash(n: f64) -> f64 {
    let x = (n * 12.9898 + 78.233).sin() * 43758.5453;
    x - x.floor()
}

/// Synthetic lyric cue intervals (seconds): 12.4–169 s with a gap at 88–104 s.
pub fn cues() -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let (mut t, mut k) = (12.4_f64, 0.0_f64);
    while t < 169.0 {
        if t > 88.0 && t < 104.0 {
            t = 104.6;
            continue;
        }
        let d = 2.1 + 1.5 * hash(k + 7.0);
        out.push((t, (t + d).min(170.0)));
        t += d + 0.45 + 0.7 * hash(k + 31.0);
        k += 1.0;
    }
    out
}

fn srt_time(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn srt() -> String {
    let mut text = String::new();
    for (index, (start, end)) in cues().into_iter().enumerate() {
        let _ = write!(
            text,
            "{}\n{} --> {}\nLigne {}\n\n",
            index + 1,
            srt_time(start),
            srt_time(end),
            index + 1
        );
    }
    text
}

fn encode(ffmpeg: &Path, args: &[&str], output: &Path) -> Result<()> {
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner", "-loglevel", "error", "-n"]);
    command.args(args).arg(output);
    let result = process::run(command, DEADLINE)?;
    if !result.status.success() {
        return Err(format!(
            "FFmpeg failed for {}: {}",
            output.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        )
        .into());
    }
    Ok(())
}

pub fn run(options: Options) -> Result<()> {
    let ffmpeg = noh::find_ffmpeg(options.ffmpeg.map(Into::into))?;
    let dir = &options.output;
    fs::create_dir_all(dir)?;
    for name in FILES {
        if dir.join(name).exists() {
            return Err(format!("Refusing to replace {}", dir.join(name).display()).into());
        }
    }
    // Loud and quiet passages give the waveform a varied, repeatable waveform.
    // A 2 Hz beat varies the peaks column by column, like music.
    let voice = "(0.12+0.7*abs(sin(PI*t/45)))*(0.3+0.7*abs(sin(2*PI*t)))*(0.5*sin(2*PI*220*t)+0.3*(2*random(0)-1))";
    encode(
        &ffmpeg,
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("aevalsrc=exprs={voice}|{voice}:s=48000:d={SONG_SECONDS}"),
            "-c:a",
            "pcm_s16le",
        ],
        &dir.join("ma-chanson.wav"),
    )?;
    let video = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=1280x720:r=25:d=4",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
    ];
    encode(&ffmpeg, &video, &dir.join("vagues.mp4"))?;
    encode(
        &ffmpeg,
        &[
            "-f",
            "lavfi",
            "-i",
            "smptehdbars=s=1280x720",
            "-frames:v",
            "1",
        ],
        &dir.join("ville-nuit.jpg"),
    )?;
    // A real MOV cut before its index cannot be read: the unreadable-media state.
    let complete = tempfile::Builder::new()
        .prefix(".concert-")
        .suffix(".mov")
        .tempfile_in(dir)?
        .into_temp_path();
    fs::remove_file(&complete)?;
    encode(&ffmpeg, &video, &complete)?;
    let bytes = fs::read(&complete)?;
    fs::write(
        dir.join("concert.mov"),
        &bytes[..bytes.len().min(TRUNCATED_BYTES)],
    )?;
    complete.close()?;
    fs::write(dir.join("ma-chanson.srt"), srt())?;
    let request = ExportRequest {
        items: vec![
            MediaItem::Video {
                path: dir.join("vagues.mp4"),
            },
            MediaItem::Image {
                path: dir.join("ville-nuit.jpg"),
                duration: 6.0,
            },
        ],
        wav: dir.join("ma-chanson.wav"),
        output: dir.join("ma-chanson_noh.mp4"),
        ffmpeg: PathBuf::new(),
        fade_in: 1.0,
        fade_out: 2.0,
        partial_fades: true,
        preview: false,
        clip_audio: false,
        force_encode: false,
    };
    fs::write(
        dir.join("project.json"),
        serde_json::to_vec_pretty(&request)?,
    )?;
    // The selected short, 0:13,0–0:18,0 with its lyrics, for live short playback.
    let short = ProjectRequest {
        montage: request,
        short: Some(ProjectShort {
            start_ms: 13_000,
            end_ms: 18_000,
            restart_loops: false,
            framing: Default::default(),
        }),
        captions: Some(ShortCaptions {
            subtitles: dir.join("ma-chanson.srt"),
            style: Default::default(),
        }),
    };
    fs::write(
        dir.join("project-short.json"),
        serde_json::to_vec_pretty(&short)?,
    )?;
    eprintln!("Layout fixtures written to {}", dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_have_repeatable_timing_and_gap() {
        let cues = cues();
        assert_eq!(cues.len(), 39, "fixture contains 39 lyric lines");
        assert!((cues[0].0 - 12.4).abs() < 1e-9);
        assert!(cues.iter().all(|(a, b)| a < b && *b <= 170.0));
        assert!(cues.iter().all(|(a, _)| !(*a > 88.0 && *a < 104.0)));
        assert!(cues.windows(2).all(|w| w[0].1 < w[1].0));
        let text = srt();
        assert!(text.starts_with("1\n00:00:12,400 --> "));
        assert_eq!(text.matches(" --> ").count(), 39);
    }
}
