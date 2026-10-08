//! Prepare each clip once, then repeat the sequence with FFmpeg.
use crate::progress::ProgressRange;
use crate::{Result, hybrid, media};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

fn words(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn file(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    args.push(flag.into());
    args.push(path.as_os_str().into());
}
fn concat(folder: &Path, name: &str, entries: &[(PathBuf, f64)]) -> Result<PathBuf> {
    let mut text = String::from("ffconcat version 1.0\n");
    for (path, seconds) in entries {
        // All names are generated in ASCII inside the temporary folder.
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("Invalid temporary filename")?;
        text.push_str(&format!("file '{name}'\nduration {seconds:.9}\n"));
    }
    let path = folder.join(name);
    fs::write(&path, text)?;
    Ok(path)
}

pub(crate) struct Prepared {
    pub video: PathBuf,
    pub wav: PathBuf,
    pub encoded: bool,
    pub parameter_sets: u32,
}

pub(crate) struct Preparation<'a> {
    pub ffmpeg: &'a Path,
    pub items: &'a [crate::engine::MediaItem],
    pub wav: &'a Path,
    pub folder: &'a Path,
    pub duration: f64,
    pub encode: bool,
    pub preview: bool,
    pub audio: bool,
}
pub(crate) fn prepare(
    request: Preparation<'_>,
    inspection: Inspection,
    report: &mut crate::engine::Reporter,
) -> Result<Prepared> {
    let Preparation {
        ffmpeg,
        items,
        wav,
        folder,
        duration,
        encode,
        preview,
        audio,
    } = request;
    let progress = ProgressRange::new(0, 40);
    let Inspection { info, decision } = inspection;
    report.plan(decision.public.clone());
    let target = &decision.public.target;
    let (w, h, rate_num, rate_den) = (
        target.width,
        target.height,
        target.rate_num,
        target.rate_den,
    );
    let fps = rate_num as f64 / rate_den as f64;
    let crate::plan::Decision {
        profile,
        level,
        parameter_sets,
        encoded_sps,
        decode_delay,
        normalize,
        heterogeneous,
        ..
    } = decision;
    let copy: Vec<_> = decision
        .public
        .clips
        .iter()
        .map(|c| c.treatment == crate::plan::Treatment::Copy)
        .collect();
    let mut video_entries = Vec::new();
    let mut audio_entries = Vec::new();
    if heterogeneous {
        report.warning(&format!("warning.harmonize|{w}|{h}|{fps:.3}"));
    }
    // Keep the target/normalization decision from the entire ordered input.
    // Only a prefix is rendered when the soundtrack ends in its first cycle.
    // Packet copies stay complete: a partial B-frame GOP can depend on packets
    // beyond the presentation cut. Final rendering already handles that tail.
    let sources = crate::inspection::SourceInspections::new_items(ffmpeg, items)?;
    let mut video_cache: HashMap<(usize, bool, u64), PathBuf> = HashMap::new();
    let mut audio_cache: HashMap<(usize, u64), PathBuf> = HashMap::new();
    let mut prepared_seconds = 0.0;
    let mut encoded = false;
    for (index, (item, meta)) in items.iter().zip(&info).enumerate() {
        let remaining = duration - prepared_seconds;
        if remaining <= 1e-9 {
            break;
        }
        sources.verify_entry(index)?;
        let path = item.path();
        let clip_progress = progress.subrange(0, 75).partition(index, items.len());
        let output = folder.join(format!("source-{index}.mp4"));
        let frames = if item.is_image() {
            crate::images::preparation_frames(meta.frames as u64, remaining, target, path)?
        } else {
            (meta.seconds * fps)
                .round()
                .max(1.0)
                .min((remaining * fps).ceil().max(1.0)) as u64
        };
        let seconds = if copy[index] {
            meta.seconds
        } else {
            crate::images::seconds(frames, target)
        };
        let key = (sources.source(index), copy[index], seconds.to_bits());
        encoded |= !copy[index];
        let video_output = if let Some(cached) = video_cache.get(&key) {
            report.progress(
                clip_progress.percent(1.0),
                &format!("progress.prepare_clip|{}|{}", index + 1, items.len()),
            );
            cached.clone()
        } else {
            let mut args = if item.is_image() {
                crate::images::input_args(path, rate_num, rate_den)?
            } else {
                let mut args = words(&["-i"]);
                args.push(path.as_os_str().into());
                args
            };
            if item.is_image() {
                args.extend(words(&[
                    "-filter_complex",
                    &crate::images::filter(target, meta.image_orientation),
                    "-map",
                    "[v]",
                    "-an",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-crf",
                    if preview { "28" } else { "16" },
                    "-preset",
                    if preview { "ultrafast" } else { "medium" },
                    "-bf",
                    "0",
                    "-video_track_timescale",
                    &rate_num.to_string(),
                    "-profile:v",
                    profile.as_deref().unwrap_or("high"),
                    "-x264-params",
                    &format!("sps-id={encoded_sps}:open-gop=0"),
                    "-frames:v",
                    &frames.to_string(),
                ]));
                if let Some(level) = &level {
                    args.extend(words(&["-level:v", level]));
                }
            } else if !copy[index] {
                args.extend(words(&["-map", "0:v:0", "-an"]));
                if !encode {
                    report.warning(&format!(
                        "warning.convert_clip|{}|{}|{}",
                        index + 1,
                        items.len(),
                        meta.codec
                    ));
                }
                // Fit the displayed aspect ratio, including SAR and autorotation,
                // into a square-pixel canvas. FFmpeg 7.1 has no scale reset_sar option.
                args.extend(words(&["-vf",&format!("setpts=PTS-STARTPTS,scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)',setsar=1,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={rate_num}/{rate_den}"),
                "-c:v","libx264","-pix_fmt","yuv420p","-crf",if preview{"28"}else{"16"},"-preset",if preview{"ultrafast"}else{"medium"},"-bf","0","-video_track_timescale",&rate_num.to_string()]));
                args.extend(words(&[
                    "-profile:v",
                    profile.as_deref().unwrap_or("high"),
                    "-x264-params",
                    &format!("sps-id={encoded_sps}:open-gop=0"),
                ]));
                if let Some(level) = &level {
                    args.extend(words(&["-level:v", level]));
                }
                // Some MP4 files hide their last frame through an edit list.
                // Padding to the calculated frame count avoids a gap between clips.
                let filter = args
                    .iter()
                    .position(|a| a == "-vf")
                    .ok_or("Missing video filter")?
                    + 1;
                let mut value = args[filter].to_string_lossy().into_owned();
                value.push_str(",tpad=stop_mode=clone:stop_duration=1");
                args[filter] = value.into();
                args.extend(words(&["-frames:v", &frames.to_string()]));
            } else {
                args.extend(words(&["-map", "0:v:0", "-an", "-c:v", "copy"]));
            }
            if normalize {
                args.extend(words(&["-bsf:v", &format!("setts=time_base=1/{rate_num}:pts=(PTS-STARTPTS)*TB*{rate_num}:dts=N*{rate_den}-{decode_delay}:duration={rate_den}"), "-video_track_timescale", &rate_num.to_string()]));
            }
            args.push(output.as_os_str().into());
            hybrid::stage(
                ffmpeg,
                args,
                seconds,
                if audio {
                    clip_progress.subrange(0, 80)
                } else {
                    clip_progress
                },
                &format!("progress.prepare_clip|{}|{}", index + 1, items.len()),
                report,
            )?;
            sources.verify_entry(index)?;
            video_cache.insert(key, output.clone());
            output
        };
        video_entries.push((video_output, seconds));
        if audio {
            let audio_seconds = seconds.min(remaining);
            let key = (sources.source(index), audio_seconds.to_bits());
            let audio_output = if let Some(cached) = audio_cache.get(&key) {
                cached.clone()
            } else {
                let output = folder.join(format!("audio-{index}.wav"));
                let mut args = Vec::new();
                if meta.audio {
                    file(&mut args, "-i", path);
                    args.extend(words(&["-map", "0:a:0"]));
                } else {
                    args.extend(words(&["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"]));
                }
                args.extend(words(&[
                    "-vn",
                    "-af",
                    "asetpts=PTS-STARTPTS,aresample=48000:async=1,apad",
                    "-t",
                    &format!("{audio_seconds:.9}"),
                    "-ar",
                    "48000",
                    "-ac",
                    "2",
                    "-c:a",
                    "pcm_s16le",
                ]));
                args.push(output.as_os_str().into());
                hybrid::stage(
                    ffmpeg,
                    args,
                    audio_seconds,
                    clip_progress.subrange(80, 100),
                    &format!("progress.clip_audio|{}", index + 1),
                    report,
                )?;
                sources.verify_entry(index)?;
                audio_cache.insert(key, output.clone());
                output
            };
            audio_entries.push((audio_output, audio_seconds));
        }
        prepared_seconds += seconds;
    }
    sources.verify()?;
    let total = video_entries.iter().map(|e| e.1).sum();
    let video_list = concat(folder, "sequence-video.ffconcat", &video_entries)?;
    let video = folder.join("sequence.mp4");
    let mut args = words(&["-f", "concat", "-safe", "0"]);
    file(&mut args, "-i", &video_list);
    args.extend(words(&["-map", "0:v:0", "-an", "-c:v", "copy"]));
    if normalize {
        args.extend(words(&[
            "-tag:v",
            "avc3",
            "-bsf:v",
            &format!("setts=pts=PTS:dts=N*{rate_den}-{decode_delay}:duration={rate_den}"),
            "-video_track_timescale",
            &rate_num.to_string(),
        ]));
    }
    args.push(video.as_os_str().into());
    hybrid::stage(
        ffmpeg,
        args,
        total,
        progress.subrange(75, if audio { 85 } else { 100 }),
        "progress.sequence",
        report,
    )?;
    let mut mixed = wav.to_path_buf();
    if audio {
        let audio_list = concat(folder, "sequence-audio.ffconcat", &audio_entries)?;
        let sequence_audio = folder.join("sequence.wav");
        let mut args = words(&["-f", "concat", "-safe", "0"]);
        file(&mut args, "-i", &audio_list);
        args.extend(words(&["-c:a", "copy"]));
        args.push(sequence_audio.as_os_str().into());
        hybrid::stage(
            ffmpeg,
            args,
            total,
            progress.subrange(85, 88),
            "progress.sequence_audio",
            report,
        )?;
        mixed = folder.join("mix.wav");
        let mut args = Vec::new();
        file(&mut args, "-i", wav);
        args.extend(words(&["-stream_loop", "-1"]));
        file(&mut args, "-i", &sequence_audio);
        args.extend(words(&[
            "-filter_complex",
            "[0:a:0][1:a:0]amix=inputs=2:duration=first:dropout_transition=0[a]",
            "-map",
            "[a]",
            "-t",
            &format!("{duration:.9}"),
            "-c:a",
            "pcm_s24le",
        ]));
        args.push(mixed.as_os_str().into());
        hybrid::stage(
            ffmpeg,
            args,
            duration,
            progress.subrange(88, 100),
            "progress.mix",
            report,
        )?;
    }
    Ok(Prepared {
        video,
        wav: mixed,
        encoded,
        parameter_sets,
    })
}

pub(crate) struct Inspection {
    pub info: Vec<media::MediaInfo>,
    pub decision: crate::plan::Decision,
}
pub(crate) fn inspect(
    ffmpeg: &Path,
    items: &[crate::engine::MediaItem],
    encode: bool,
    preview: bool,
    for_fades: bool,
    report: &mut crate::engine::Reporter,
) -> Result<Inspection> {
    inspect_with(
        ffmpeg,
        items,
        encode,
        preview,
        for_fades,
        report,
        (
            |ffmpeg, path, is_image| {
                if is_image {
                    media::image_info(ffmpeg, path)
                } else {
                    media::inspect(ffmpeg, path)
                }
            },
            crate::h264::inspect,
        ),
    )
}

pub(crate) fn inspect_with(
    ffmpeg: &Path,
    items: &[crate::engine::MediaItem],
    encode: bool,
    preview: bool,
    for_fades: bool,
    report: &mut crate::engine::Reporter,
    (mut inspect_media, mut inspect_headers): (
        impl FnMut(&Path, &Path, bool) -> Result<media::MediaInfo>,
        impl FnMut(&Path, &Path, &media::MediaInfo) -> Result<crate::h264::Support>,
    ),
) -> Result<Inspection> {
    let mut sources = crate::inspection::SourceInspections::new_items(ffmpeg, items)?;
    let mut info = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let path = item.path();
        report.progress(
            0,
            &format!("progress.analyze_clip|{}|{}", index + 1, items.len()),
        );
        info.push(sources.media(index, || inspect_media(ffmpeg, path, item.is_image()))?);
    }
    let target = crate::plan::item_target(items, &info, preview);
    for (item, meta) in items.iter().zip(&mut info) {
        if let crate::engine::MediaItem::Image { path, duration } = item {
            let frames = crate::images::frames(*duration, &target, Some(path))?;
            meta.frames = frames.try_into().map_err(|_| {
                crate::engine::EngineError::new(
                    "error.image_duration",
                    "image_timing",
                    Some(path),
                    "Image frame count exceeds the supported range.",
                )
            })?;
            meta.seconds = crate::images::seconds(frames, &target);
            meta.fps = target.rate_num as f64 / target.rate_den as f64;
            meta.step = target.rate_den;
            meta.timescale = target.rate_num;
        }
    }
    let mut support = Vec::with_capacity(info.len());
    let normalize = encode
        || for_fades
        || items.iter().any(|item| item.is_image())
        || info
            .iter()
            .skip(1)
            .any(|i| i.signature != info[0].signature);
    let mut headers = vec![None; items.len()];
    for (index, (item, meta)) in items.iter().zip(&info).enumerate() {
        let path = item.path();
        support.push(if normalize && !encode && !item.is_image() {
            sources.verify_entry(index)?;
            let source = sources.source(index);
            if headers[source].is_none() {
                let inspected = inspect_headers(ffmpeg, path, meta)?;
                sources.verify_entry(index)?;
                headers[source] = Some(inspected);
            }
            headers[source].clone()
        } else {
            None
        });
    }
    sources.verify()?;
    let decision = crate::plan::decide(items, &info, support, encode, preview, for_fades);

    Ok(Inspection { info, decision })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineError, Event, Reporter};
    use crate::h264::{Support, Video};

    #[test]
    fn preparation_renders_only_needed_occurrences_and_reuses_equivalent_work() {
        if std::env::var_os("NOH_MEDIA_TESTS").is_none() {
            return;
        }
        let ffmpeg = crate::find_ffmpeg(None).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("source.mp4");
        let wav = temp.path().join("song.wav");
        for args in [
            vec![
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=64x48:r=25",
                "-t",
                "4",
                "-c:v",
                "libx264",
                "-bf",
                "2",
            ],
            vec![
                "-f",
                "lavfi",
                "-i",
                "sine=r=48000",
                "-t",
                "9",
                "-c:a",
                "pcm_s16le",
            ],
        ]
        .into_iter()
        .zip([&video, &wav])
        {
            let status = crate::command(&ffmpeg)
                .args(["-v", "error", "-nostdin"])
                .args(args.0)
                .arg(args.1)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let items: Vec<_> = (0..4).map(|_| video.clone().into()).collect();
        for (duration, encode, videos, audios, sequence_seconds) in [
            (1.0, false, 1, 1, 4.0),
            (9.0, false, 1, 2, 12.0),
            (1.0, true, 1, 1, 1.0),
        ] {
            let folder = tempfile::tempdir().unwrap();
            let mut emit = |_| {};
            let mut report = Reporter::new(&mut emit);
            let inspection = inspect(&ffmpeg, &items, encode, false, false, &mut report).unwrap();
            let result = prepare(
                Preparation {
                    ffmpeg: &ffmpeg,
                    items: &items,
                    wav: &wav,
                    folder: folder.path(),
                    duration,
                    encode,
                    preview: false,
                    audio: true,
                },
                inspection,
                &mut report,
            )
            .unwrap();
            let names: Vec<_> = fs::read_dir(folder.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                names.iter().filter(|n| n.starts_with("source-")).count(),
                videos
            );
            assert_eq!(
                names.iter().filter(|n| n.starts_with("audio-")).count(),
                audios
            );
            assert!(
                (crate::wav_duration(&folder.path().join("sequence.wav")).unwrap() - duration)
                    .abs()
                    < 1e-6
            );
            assert!(
                (media::inspect(&ffmpeg, &result.video).unwrap().seconds - sequence_seconds).abs()
                    < 1e-6
            );
            assert_eq!(result.encoded, encode);
        }
    }

    fn inputs() -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let folder = tempfile::tempdir().unwrap();
        let ffmpeg = folder.path().join("ffmpeg");
        let a = folder.path().join("a");
        let b = folder.path().join("b");
        for path in [&ffmpeg, &a, &b] {
            fs::write(path, b"synthetic").unwrap();
        }
        fs::create_dir(folder.path().join("alias")).unwrap();
        let alias = folder.path().join("alias").join("..").join("a");
        (folder, ffmpeg, vec![a.clone(), b.clone(), alias, b, a])
    }
    fn metadata(path: &Path) -> media::MediaInfo {
        let a = path.file_name().unwrap() == "a";
        media::MediaInfo {
            seconds: if a { 1.0 } else { 2.0 },
            width: 320,
            height: 180,
            display_width: 320.0,
            display_height: 180.0,
            fps: 25.0,
            frames: if a { 25 } else { 50 },
            step: 1,
            timescale: 25,
            fixed: true,
            square_pixels: true,
            codec: if a { "h264" } else { "mpeg4" }.into(),
            signature: if a { "a" } else { "b" }.into(),
            ..Default::default()
        }
    }
    fn support(meta: &media::MediaInfo) -> Support {
        if meta.codec == "h264" {
            Support::Copy(Video {
                frames: meta.frames,
                step: meta.step,
                timescale: meta.timescale,
                profile: "high".into(),
                level: "3.1".into(),
                sps: 1,
                parameter_sets: 1,
                decode_delay: 0,
            })
        } else {
            Support::Convert("warning.partial_codec")
        }
    }

    #[test]
    fn repeated_sources_inspect_media_and_headers_once_with_all_progress_and_decisions() {
        let (_folder, ffmpeg, paths) = inputs();
        let items: Vec<_> = paths.iter().cloned().map(Into::into).collect();
        let (mut media_calls, mut header_calls) = (Vec::new(), Vec::new());
        let mut progress = Vec::new();
        let result = inspect_with(
            &ffmpeg,
            &items,
            false,
            false,
            false,
            &mut Reporter::new(&mut |event| {
                if let Event::Progress { phase, .. } = event {
                    progress.push(phase);
                }
            }),
            (
                |_, path, _| {
                    media_calls.push(path.to_owned());
                    Ok(metadata(path))
                },
                |_, path, meta| {
                    header_calls.push(path.to_owned());
                    Ok(support(meta))
                },
            ),
        )
        .unwrap();
        assert_eq!(media_calls, paths[..2]);
        assert_eq!(header_calls, paths[..2]);
        assert_eq!(
            result.info.iter().map(|i| i.seconds).collect::<Vec<_>>(),
            [1.0, 2.0, 1.0, 2.0, 1.0]
        );
        assert_eq!(
            result
                .decision
                .public
                .clips
                .iter()
                .map(|c| c.path.clone())
                .collect::<Vec<_>>(),
            paths
        );
        assert_eq!(
            result
                .decision
                .public
                .clips
                .iter()
                .map(|c| c.treatment)
                .collect::<Vec<_>>(),
            [
                crate::plan::Treatment::Copy,
                crate::plan::Treatment::Convert,
                crate::plan::Treatment::Copy,
                crate::plan::Treatment::Convert,
                crate::plan::Treatment::Copy
            ]
        );
        assert_eq!(progress.len(), paths.len());
        for (index, phase) in progress.iter().enumerate() {
            assert_eq!(phase.code, "progress.analyze_clip");
            assert_eq!(
                phase.args,
                [(index + 1).to_string(), paths.len().to_string()]
            );
        }
    }

    #[test]
    fn header_failure_propagates_original_context_and_stops_later_sources() {
        let (_folder, ffmpeg, paths) = inputs();
        let items: Vec<_> = paths.iter().cloned().map(Into::into).collect();
        let mut calls = 0;
        let error = inspect_with(
            &ffmpeg,
            &items,
            false,
            false,
            false,
            &mut Reporter::new(&mut |_| {}),
            (
                |_, path, _| Ok(metadata(path)),
                |_, path, _| {
                    calls += 1;
                    Err(EngineError::new(
                        "error.h264_headers",
                        "inspect_headers",
                        Some(path),
                        "broken headers",
                    )
                    .into())
                },
            ),
        )
        .err()
        .unwrap();
        let error = error.downcast_ref::<EngineError>().unwrap();
        assert_eq!(calls, 1);
        assert_eq!(error.code, "error.h264_headers");
        assert_eq!(error.path.as_ref(), Some(&paths[0]));
    }

    #[test]
    fn header_inspection_cannot_return_a_plan_after_engine_changes() {
        let (_folder, ffmpeg, paths) = inputs();
        let items: Vec<_> = paths.iter().cloned().map(Into::into).collect();
        let error = inspect_with(
            &ffmpeg,
            &items,
            false,
            false,
            false,
            &mut Reporter::new(&mut |_| {}),
            (
                |_, path, _| Ok(metadata(path)),
                |engine, _, meta| {
                    fs::write(engine, b"changed engine contents").unwrap();
                    Ok(support(meta))
                },
            ),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<EngineError>().unwrap().code,
            "error.stale_inspection"
        );
    }

    #[test]
    fn image_cache_is_intrinsic_and_first_video_controls_fractional_frame_grid() {
        use crate::engine::MediaItem;
        let (_folder, ffmpeg, paths) = inputs();
        let items = vec![
            MediaItem::Image {
                path: paths[0].clone(),
                duration: 0.019,
            },
            paths[1].clone().into(),
            MediaItem::Image {
                path: paths[2].clone(),
                duration: 0.066,
            },
        ];
        let (mut images, mut videos, mut headers) = (0, 0, 0);
        let inspection = inspect_with(
            &ffmpeg,
            &items,
            false,
            false,
            false,
            &mut Reporter::new(&mut |_| {}),
            (
                |_, path, is_image| {
                    let mut meta = metadata(path);
                    if is_image {
                        images += 1;
                        meta.width = 48;
                        meta.height = 96;
                        meta.display_width = 48.0;
                        meta.display_height = 96.0;
                        meta.codec = "png".into();
                        meta.seconds = 0.0;
                    } else {
                        videos += 1;
                        meta.codec = "h264".into();
                        meta.fps = 30000.0 / 1001.0;
                        meta.step = 1001;
                        meta.timescale = 30000;
                    }
                    Ok(meta)
                },
                |_, _, meta| {
                    headers += 1;
                    Ok(support(meta))
                },
            ),
        )
        .unwrap();
        assert_eq!((images, videos, headers), (1, 1, 1));
        let target = &inspection.decision.public.target;
        assert_eq!(
            (
                target.width,
                target.height,
                target.rate_num,
                target.rate_den
            ),
            (320, 180, 30000, 1001)
        );
        assert_eq!(
            (inspection.info[0].frames, inspection.info[2].frames),
            (1, 2)
        );
        assert!((inspection.info[0].seconds - 1001.0 / 30000.0).abs() < 1e-12);
        assert!((inspection.info[2].seconds - 2002.0 / 30000.0).abs() < 1e-12);
        assert_eq!(inspection.decision.public.clips[0].reasons, ["image"]);
        assert_eq!(
            inspection.decision.public.clips[1].treatment,
            crate::plan::Treatment::Copy
        );
    }

    #[test]
    fn identical_path_with_different_kinds_does_not_share_probe_results() {
        use crate::engine::MediaItem;
        let (_folder, ffmpeg, paths) = inputs();
        let items = vec![
            MediaItem::Image {
                path: paths[0].clone(),
                duration: 0.04,
            },
            paths[0].clone().into(),
            MediaItem::Image {
                path: paths[0].clone(),
                duration: 0.08,
            },
        ];
        let mut kinds = Vec::new();
        let inspection = inspect_with(
            &ffmpeg,
            &items,
            false,
            false,
            false,
            &mut Reporter::new(&mut |_| {}),
            (
                |_, path, is_image| {
                    kinds.push(is_image);
                    Ok(metadata(path))
                },
                |_, _, meta| Ok(support(meta)),
            ),
        )
        .unwrap();
        assert_eq!(kinds, [true, false]);
        assert_eq!(
            inspection
                .info
                .iter()
                .map(|meta| meta.seconds)
                .collect::<Vec<_>>(),
            [0.04, 1.0, 0.08]
        );
    }
}
