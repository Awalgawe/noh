//! Streaming media inspection shared by the engine and GUI.
use crate::{Result, command};
use std::path::Path;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct MediaInfo {
    pub seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub audio: bool,
    pub(crate) display_width: f64,
    pub(crate) display_height: f64,
    pub(crate) signature: String,
    pub(crate) frames: usize,
    pub(crate) step: i64,
    pub(crate) timescale: i64,
    pub(crate) fixed: bool,
    pub(crate) reordered: bool,
    pub(crate) decode_delay: i64,
    pub(crate) codec: String,
    pub(crate) square_pixels: bool,
    pub(crate) rotation: f64,
    pub(crate) description: String,
    /// Explicit PNG EXIF transform; JPEG transforms are applied by FFmpeg.
    #[serde(default)]
    pub(crate) image_orientation: u8,
}

impl MediaInfo {
    /// Source codec reported by media inspection, for adapter presentation.
    pub fn codec(&self) -> &str {
        &self.codec
    }
}

/// Header-only inspection used by a GUI background thread.
pub fn preview_info(ffmpeg: &Path, path: &Path) -> Result<MediaInfo> {
    quick_info(ffmpeg, path, true)
}

/// Decode exactly one still frame; duration belongs to an ordered occurrence.
pub(crate) fn image_info(ffmpeg: &Path, path: &Path) -> Result<MediaInfo> {
    image_info_inner(ffmpeg, path, None)
}

pub(crate) fn image_info_interruptible(
    ffmpeg: &Path,
    path: &Path,
    cancel: &std::sync::atomic::AtomicBool,
    deadline: std::time::Duration,
) -> Result<MediaInfo> {
    image_info_inner(ffmpeg, path, Some((cancel, deadline)))
}

fn image_info_inner(
    ffmpeg: &Path,
    path: &Path,
    interrupt: Option<(&std::sync::atomic::AtomicBool, std::time::Duration)>,
) -> Result<MediaInfo> {
    let probe = || -> Result<MediaInfo> {
        let codec = crate::images::codec(path)?;
        let orientation = if codec == "png" {
            crate::images::png_orientation(path)?
        } else {
            0
        };
        let mut cmd = command(ffmpeg);
        cmd.args([
            "-hide_banner",
            "-nostdin",
            "-xerror",
            "-err_detect",
            "explode",
        ])
        .args(crate::images::input_args(path, 25, 1)?)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-vf",
            &format!("{}showinfo", crate::images::orientation_filter(orientation)),
            "-frames:v",
            "1",
            "-f",
            "null",
            "-",
        ]);
        let (status, text) = if let Some((cancel, deadline)) = interrupt {
            crate::process::stream_interruptible(cmd, deadline, cancel, |_| Ok(()))?
        } else {
            crate::process::capture(cmd, std::time::Duration::from_secs(15), false)?
        };
        let geometry = text
            .lines()
            .filter(|line| line.contains("showinfo") && line.contains(" n:"))
            .find_map(|line| {
                let size = line
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("s:"))?;
                let (w, h) = size.split_once('x')?;
                Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
            });
        let Some((width, height)) = geometry.filter(|(w, h)| status.success() && *w > 0 && *h > 0)
        else {
            let mut error = crate::engine::EngineError::new(
                "error.image",
                "inspect_image",
                Some(path),
                "Cannot decode a PNG or JPEG image.",
            );
            error.technical = crate::engine::bounded(&text, 8192).into();
            return Err(error.into());
        };
        Ok(MediaInfo {
            width,
            height,
            display_width: f64::from(width),
            display_height: f64::from(height),
            fps: 25.0,
            step: 1,
            timescale: 25,
            fixed: true,
            square_pixels: true,
            codec: codec.into(),
            signature: format!("#image {codec} {width}x{height} orientation={orientation}\n"),
            image_orientation: orientation,
            ..Default::default()
        })
    };
    probe().map_err(|e| {
        e.downcast_ref::<crate::engine::EngineError>()
            .cloned()
            .unwrap_or_else(|| {
                crate::engine::EngineError::new("error.image", "inspect_image", Some(path), e)
            })
            .into()
    })
}
pub(crate) fn quick_info(ffmpeg: &Path, path: &Path, isolate: bool) -> Result<MediaInfo> {
    let mut cmd = command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", "0:v:0?", "-frames:v", "0", "-an", "-f", "null", "-"]);
    let (status, text) = crate::process::capture(cmd, std::time::Duration::from_secs(15), isolate)?;
    let duration = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Duration: "))
        .and_then(|s| s.split(',').next())
        .and_then(parse_time)
        .ok_or("Video duration unavailable")?;
    if !status.success() || !text.contains("Video:") {
        return Err("This file contains no readable video".into());
    }
    Ok(MediaInfo {
        seconds: duration,
        audio: text.contains("Audio:"),
        ..Default::default()
    })
}

fn parse_time(text: &str) -> Option<f64> {
    let mut parts = text.split(':');
    let mut seconds = 0.0;
    for multiplier in [3600.0, 60.0, 1.0] {
        let value = parts.next()?.parse::<f64>().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        seconds += value * multiplier;
    }
    (parts.next().is_none() && seconds.is_finite()).then_some(seconds)
}

struct PacketTimes {
    dts: i64,
    pts: i64,
    duration: i64,
    end: i64,
}

fn framehash_packet(line: &str) -> Result<(usize, PacketTimes)> {
    let mut fields = line.split(',').map(str::trim);
    let stream = fields.next().ok_or("Invalid framehash packet")?.parse()?;
    let dts = fields.next().ok_or("Invalid framehash packet")?.parse()?;
    let pts: i64 = fields.next().ok_or("Invalid framehash packet")?.parse()?;
    let duration = fields.next().ok_or("Invalid framehash packet")?.parse()?;
    let end = pts
        .checked_add(duration)
        .ok_or("Framehash packet timestamp exceeds the supported range")?;
    Ok((
        stream,
        PacketTimes {
            dts,
            pts,
            duration,
            end,
        },
    ))
}

#[cfg(test)]
fn packet_times(line: &str) -> Result<PacketTimes> {
    framehash_packet(line).map(|(_, packet)| packet)
}

#[derive(Clone, Debug)]
pub(crate) struct TimedMediaInfo {
    pub video: MediaInfo,
    /// First presented video PTS in the inspection's chosen time origin.
    pub video_start: f64,
    /// Greatest known video packet end. A zero final packet duration is not guessed.
    pub video_end: f64,
    /// First audio stream's first presented PTS in the same origin, if present.
    pub audio_start: Option<f64>,
    /// First audio stream's greatest known packet end, if present.
    pub audio_end: Option<f64>,
    pub audio_codec: Option<String>,
    /// Number of source audio streams reported in FFmpeg's input header.
    pub audio_streams: usize,
    /// Greatest known stream end measured from playback time zero.
    pub duration: f64,
    /// True when the last video presentation packet supplies no positive duration.
    pub unknown_final_duration: bool,
}

#[derive(Default)]
struct StreamTiming {
    time_base: Option<f64>,
    media_type: Option<String>,
    codec: Option<String>,
    first: i64,
    end: i64,
    last_pts: i64,
    last_duration: i64,
    packets: u64,
}

impl StreamTiming {
    fn push(&mut self, packet: PacketTimes) {
        if self.packets == 0 {
            self.first = packet.pts;
            self.end = packet.end;
            self.last_pts = packet.pts;
            self.last_duration = packet.duration;
        } else {
            self.first = self.first.min(packet.pts);
            self.end = self.end.max(packet.end);
            if packet.pts > self.last_pts {
                self.last_pts = packet.pts;
                self.last_duration = packet.duration;
            }
        }
        self.packets += 1;
    }
}

fn header_field(line: &str) -> Option<(usize, &str, &str)> {
    let rest = line.strip_prefix('#')?;
    let (key, rest) = rest.split_once(' ')?;
    let (index, value) = rest.split_once(':')?;
    Some((index.parse().ok()?, key, value.trim()))
}

fn parse_time_base(value: &str) -> Result<(f64, Option<i64>)> {
    let (numerator, denominator) = value.split_once('/').ok_or("Invalid stream time base")?;
    let numerator = numerator.parse::<i64>()?;
    let denominator = denominator.parse::<i64>()?;
    if numerator <= 0 || denominator <= 0 {
        return Err("Invalid stream time base".into());
    }
    Ok((
        numerator as f64 / denominator as f64,
        (numerator == 1).then_some(denominator),
    ))
}

fn header_audio_streams(log: &str) -> usize {
    let (mut input, mut count) = (false, 0);
    for line in log.lines() {
        if line.starts_with("Input #") {
            input = true;
        } else if line.starts_with("Output #") {
            input = false;
        } else if input && line.trim_start().starts_with("Stream #") && line.contains("Audio:") {
            count += 1;
        }
    }
    count
}

/// Exact duration from video packets; the frame list is never stored.
pub(crate) fn inspect(ffmpeg: &Path, path: &Path) -> Result<MediaInfo> {
    Ok(inspect_streams(ffmpeg, path, false, false)?.video)
}

/// Stream video and the optional first audio timeline in FFmpeg's default
/// demux start-time origin. No timestamp reset is applied to either stream.
pub(crate) fn inspect_timed(ffmpeg: &Path, path: &Path) -> Result<TimedMediaInfo> {
    inspect_streams(ffmpeg, path, true, false)
}

pub(crate) fn inspect_timed_interruptible(
    ffmpeg: &Path,
    path: &Path,
    cancel: &std::sync::atomic::AtomicBool,
    deadline: std::time::Duration,
) -> Result<TimedMediaInfo> {
    inspect_streams_inner(ffmpeg, path, true, false, Some((cancel, deadline)))
}

/// Preserve the output container origin when measuring a selected interval.
/// Ordinary demuxing subtracts its first timestamp, hiding a leading cut gap.
pub(crate) fn inspect_timed_raw(ffmpeg: &Path, path: &Path) -> Result<TimedMediaInfo> {
    inspect_streams(ffmpeg, path, true, true)
}

fn inspect_streams(
    ffmpeg: &Path,
    path: &Path,
    with_audio: bool,
    raw_timestamps: bool,
) -> Result<TimedMediaInfo> {
    inspect_streams_inner(ffmpeg, path, with_audio, raw_timestamps, None)
}

fn inspect_streams_inner(
    ffmpeg: &Path,
    path: &Path,
    with_audio: bool,
    raw_timestamps: bool,
    interrupt: Option<(&std::sync::atomic::AtomicBool, std::time::Duration)>,
) -> Result<TimedMediaInfo> {
    let mut cmd = command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-nostats"]);
    if raw_timestamps {
        cmd.arg("-copyts");
    }
    cmd.arg("-i").arg(path).args(["-map", "0:v:0"]);
    if with_audio {
        cmd.args(["-map", "0:a:0?"]);
    }
    cmd.args(["-c", "copy", "-f", "framehash", "-hash", "sha256", "-"]);
    let mut info = MediaInfo::default();
    let mut timing = crate::timing::PacketTiming::default();
    let mut pixel_aspect = 1.0;
    let mut streams = std::collections::HashMap::<usize, StreamTiming>::new();
    let (mut first, mut end, mut frames) = (f64::INFINITY, f64::NEG_INFINITY, 0u64);
    let mut read_line = |line: &str| {
        if let Some(value) = line.strip_prefix("#dimensions 0: ") {
            let (w, h) = value.split_once('x').ok_or("Invalid dimensions")?;
            info.width = w.parse()?;
            info.height = h.parse()?;
        }
        if let Some(value) = line.strip_prefix("#codec_id 0: ") {
            info.codec = value.to_owned();
        }
        if let Some(value) = line.strip_prefix("#sar 0: ") {
            let (a, b) = value.split_once('/').ok_or("Invalid pixel aspect ratio")?;
            let (a, b) = (a.parse::<u32>()?, b.parse::<u32>()?);
            // 0/1 means unspecified; FFmpeg treats these as square pixels.
            if a > 0 && b > 0 {
                pixel_aspect = f64::from(a) / f64::from(b);
            }
        }
        if line.starts_with("#extradata 0,")
            || line.starts_with("#codec_id 0:")
            || line.starts_with("#dimensions 0:")
            || line.starts_with("#sar 0:")
            || line.starts_with("#tb 0:")
        {
            if info.signature.len() + line.len() > 8192 {
                return Err("Video metadata exceeds its size limit".into());
            }
            info.signature.push_str(line);
            info.signature.push('\n');
        }
        if line.starts_with('#') || line.trim().is_empty() {
            if let Some((index, key, value)) = header_field(line) {
                let stream = streams.entry(index).or_default();
                match key {
                    "tb" => {
                        let (time_base, timescale) = parse_time_base(value)?;
                        stream.time_base = Some(time_base);
                        if index == 0 {
                            info.timescale = timescale.unwrap_or_default();
                        }
                    }
                    "media_type" => stream.media_type = Some(value.to_owned()),
                    "codec_id" => stream.codec = Some(value.to_owned()),
                    _ => {}
                }
            }
            return Ok(());
        }
        let (index, packet) = framehash_packet(line)?;
        let stream = streams.entry(index).or_default();
        match stream.media_type.as_deref() {
            Some("video") => {
                info.reordered |= packet.dts != packet.pts;
                timing.push(packet.pts, packet.duration);
                first = first.min(packet.pts as f64);
                end = end.max(packet.end as f64);
                frames += 1;
            }
            Some("audio") => {}
            _ => return Err("Framehash packet has no known media type".into()),
        }
        stream.push(packet);
        Ok(())
    };
    let (status, log) = if let Some((cancel, deadline)) = interrupt {
        crate::process::stream_interruptible(cmd, deadline, cancel, &mut read_line)?
    } else {
        crate::process::stream(
            cmd,
            Some(std::time::Duration::from_secs(120)),
            false,
            &mut read_line,
        )?
    };
    if !status.success() {
        let mut error = crate::engine::EngineError::new(
            "error.video",
            "inspect_video",
            Some(path),
            format!("Cannot inspect {}", path.display()),
        );
        error.technical = crate::engine::bounded(&log, 8192).into();
        return Err(Box::new(error));
    }
    let video = streams
        .get(&0)
        .filter(|stream| stream.media_type.as_deref() == Some("video"))
        .ok_or_else(|| {
            crate::engine::EngineError::new(
                "error.video",
                "inspect_video",
                Some(path),
                "No video packets were inspected.",
            )
        })?;
    let video_time_base = video.time_base.ok_or("Missing video time base")?;
    info.seconds = (end - first) * video_time_base;
    info.fixed = timing.regular();
    info.step = timing.step;
    info.decode_delay = timing.decode_delay;
    info.fps = frames as f64 / info.seconds;
    info.audio = log.contains("Audio:");
    info.frames = frames as usize;
    info.description = log
        .lines()
        .find(|l| l.contains("Video:"))
        .unwrap_or("")
        .into();
    info.display_width = f64::from(info.width) * pixel_aspect;
    info.display_height = f64::from(info.height);
    info.square_pixels = (pixel_aspect - 1.0).abs() < 1e-9;
    let rotation = log
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("displaymatrix: rotation of ")?
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
        })
        .unwrap_or(0.0);
    // FFmpeg autorotates decoded frames. Match the canvas to portrait sources.
    if (rotation.rem_euclid(180.0) - 90.0).abs() < 0.01 {
        std::mem::swap(&mut info.display_width, &mut info.display_height);
    }
    info.signature
        .push_str(&format!("#display_rotation: {rotation}\n"));
    info.rotation = rotation;
    if !info.seconds.is_finite() || info.seconds <= 0.0 || info.width == 0 || info.height == 0 {
        return Err("Invalid video duration or dimensions".into());
    }
    let video_start = video.first as f64 * video_time_base;
    let video_end = video.end as f64 * video_time_base;
    let first_audio = streams
        .values()
        .find(|stream| stream.media_type.as_deref() == Some("audio") && stream.packets > 0);
    let audio_start =
        first_audio.map(|stream| stream.first as f64 * stream.time_base.unwrap_or(0.0));
    let audio_end = first_audio.map(|stream| stream.end as f64 * stream.time_base.unwrap_or(0.0));
    let audio_codec = streams
        .values()
        .find(|stream| stream.media_type.as_deref() == Some("audio"))
        .and_then(|stream| stream.codec.clone());
    let duration = audio_end.map_or(video_end, |audio| video_end.max(audio));
    let unknown_final_duration = video.last_duration <= 0;
    if !video_start.is_finite()
        || !video_end.is_finite()
        || audio_start.is_some_and(|value| !value.is_finite())
        || audio_end.is_some_and(|value| !value.is_finite())
        || !duration.is_finite()
    {
        return Err("Invalid media presentation timestamps".into());
    }
    Ok(TimedMediaInfo {
        video: info,
        video_start,
        video_end,
        audio_start,
        audio_end,
        audio_codec,
        audio_streams: header_audio_streams(&log),
        duration,
        unknown_final_duration,
    })
}

#[cfg(test)]
mod timed_tests {
    use super::*;

    #[test]
    fn framehash_bounds_preserve_signed_demux_origin_and_unknown_final_duration() {
        let (video_tb, _) = parse_time_base("1/1000").unwrap();
        let (audio_tb, _) = parse_time_base("1/48000").unwrap();
        let mut video = StreamTiming {
            time_base: Some(video_tb),
            media_type: Some("video".into()),
            ..Default::default()
        };
        for row in [
            "0, -140, -100, 40, 1024, abc",
            "0, -100, -60, 40, 1024, def",
            "0, -20, 20, 0, 1024, ghi",
        ] {
            video.push(framehash_packet(row).unwrap().1);
        }
        let mut audio = StreamTiming {
            time_base: Some(audio_tb),
            media_type: Some("audio".into()),
            ..Default::default()
        };
        audio.push(framehash_packet("1, 0, 4800, 4800, 256, jkl").unwrap().1);
        audio.push(framehash_packet("1, 4800, 9600, 4800, 256, mno").unwrap().1);

        let video_start = video.first as f64 * video.time_base.unwrap();
        let video_end = video.end as f64 * video.time_base.unwrap();
        let audio_start = audio.first as f64 * audio.time_base.unwrap();
        let audio_end = audio.end as f64 * audio.time_base.unwrap();
        assert_eq!((video_start, video_end), (-0.1, 0.02));
        assert!((audio_start - 0.1).abs() < 1e-12);
        assert!((audio_end - 0.3).abs() < 1e-12);
        assert_eq!(video_end.max(audio_end), 0.3);
        assert!(video.last_duration <= 0);
    }

    #[test]
    fn framehash_audio_is_optional_and_input_header_counts_only_source_streams() {
        assert_eq!(
            header_field("#media_type 1: audio"),
            Some((1, "media_type", "audio"))
        );
        assert_eq!(parse_time_base("1/1000").unwrap(), (0.001, Some(1000)));
        assert!(parse_time_base("1/0").is_err());
        let log = "Input #0, matroska, from 'source.mkv':\n  Stream #0:0: Video: h264\n  Stream #0:1: Audio: aac\n  Stream #0:2: Audio: ac3\nOutput #0, framehash, to 'pipe:':\n  Stream #0:1: Audio: aac\n";
        assert_eq!(header_audio_streams(log), 2);
        let no_audio = StreamTiming::default();
        assert_eq!(no_audio.packets, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_duration_requires_three_finite_nonnegative_components() {
        assert_eq!(parse_time("01:02:03.44"), Some(3723.44));
        assert_eq!(parse_time("100:00:00.00"), Some(360000.0));
        assert_eq!(parse_time("00:00:00.00"), Some(0.0));
        for text in [
            "N/A",
            "00:01",
            "00:00:01:02",
            "00:00:NaN",
            "inf:00:00",
            "1e308:00:00",
            "00:-1:00",
            "00:00:-1",
        ] {
            assert_eq!(parse_time(text), None, "{text}");
        }
    }

    #[test]
    fn packet_timestamps_allow_negative_decode_times_but_reject_overflow() {
        let packet = packet_times("0, -80, 0, 40, 1024, abcdef").unwrap();
        assert_eq!(
            (packet.dts, packet.pts, packet.duration, packet.end),
            (-80, 0, 40, 40)
        );
        for row in [
            format!("0, 0, {}, 1", i64::MAX),
            format!("0, 0, {}, -1", i64::MIN),
            "0, 0, N/A, 40".into(),
            "0, 0, 0".into(),
            "".into(),
        ] {
            assert!(packet_times(&row).is_err(), "{row}");
        }
        assert_eq!(
            packet_times(&format!("0, 0, {}, 1", i64::MAX - 1))
                .unwrap()
                .end,
            i64::MAX
        );
    }
}
