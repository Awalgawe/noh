//! H.264 assembly: encode endpoint loops and copy central loops.
//! AVC3 retains each segment's parameter sets in the stream.
use crate::Result;
use crate::progress::ProgressRange;
use std::{
    ffi::OsString,
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

pub(crate) fn stage(
    ffmpeg: &Path,
    args: Vec<OsString>,
    duration: f64,
    progress: ProgressRange,
    name: &str,
    report: &mut crate::engine::Reporter,
) -> Result<()> {
    report.progress(progress.percent(0.0), name);
    let mut command = crate::command(ffmpeg);
    command
        .args([
            "-hide_banner",
            "-nostdin",
            "-n",
            "-nostats",
            "-stats_period",
            "0.5",
            "-progress",
            "pipe:1",
        ])
        .args(args);
    let mut last = progress.percent(0.0);
    let (status, log) = crate::process::stream(command, None, false, |line| {
        if let Some(microseconds) = line
            .strip_prefix("out_time_us=")
            .and_then(|v| v.parse::<f64>().ok())
        {
            let percent = progress.percent((microseconds / 1_000_000.0 / duration).clamp(0.0, 1.0));
            if percent > last {
                report.progress(percent, name);
                last = percent;
            }
        }
        Ok(())
    })?;
    report.diagnostic(&log);
    if !status.success() {
        let mut error = crate::engine::EngineError::new(
            "error.engine",
            name,
            None,
            format!("Stage failed: {name}. No final export was created."),
        );
        error.technical = crate::engine::bounded(&log, 8192).into();
        return Err(Box::new(error));
    }
    report.progress(progress.percent(1.0), name);
    Ok(())
}

fn words(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn add_file(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    args.push(flag.into());
    args.push(path.as_os_str().into());
}

pub(crate) struct FadeRender<'a> {
    pub ffmpeg: &'a Path,
    pub source: &'a Path,
    pub wav: &'a Path,
    pub output: &'a Path,
    pub duration: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    pub preview: bool,
    pub progress: ProgressRange,
    pub reserved_parameter_sets: u32,
}
pub(crate) fn render(
    request: FadeRender<'_>,
    report: &mut crate::engine::Reporter,
) -> Result<bool> {
    let FadeRender {
        ffmpeg,
        source,
        wav,
        output,
        duration,
        fade_in,
        fade_out,
        preview,
        progress,
        reserved_parameter_sets,
    } = request;
    report.progress(progress.percent(0.0), "progress.analyze");
    let info = crate::media::inspect(ffmpeg, source)?;
    let loop_duration = info.seconds;
    let segments = crate::plan::fade_segments(
        duration,
        loop_duration,
        fade_in,
        fade_out,
        info.decode_delay > 0,
    );
    let copied = segments
        .iter()
        .find(|s| s.treatment == crate::plan::Treatment::Copy);
    let head_loops = copied.map_or(0, |s| (s.start / loop_duration).round() as usize);
    let tail_loop = copied.map_or(0, |s| (s.end / loop_duration + 1e-9).floor() as usize);
    let mut plan = crate::plan::RenderPlan {
        exact_inspection: true,
        target: crate::plan::target(&info, preview),
        clips: Vec::new(),
        stage: "final_fades".into(),
        segments,
    };
    if tail_loop <= head_loops {
        report.plan(plan);
        // The WAV may be shorter than a clip: encode only the required portion.
        let mut filters = vec!["setpts=PTS-STARTPTS".to_owned()];
        if fade_in > 0.0 {
            filters.push(format!("fade=t=in:st=0:d={fade_in:.9}"));
        }
        if fade_out > 0.0 {
            filters.push(format!(
                "fade=t=out:st={:.9}:d={fade_out:.9}",
                duration - fade_out
            ));
        }
        let mut args = words(&["-stream_loop", "-1"]);
        add_file(&mut args, "-i", source);
        add_file(&mut args, "-i", wav);
        args.extend(words(&[
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-vf",
            &filters.join(","),
            "-c:v",
            "libx264",
            "-crf",
            if preview { "28" } else { "16" },
            "-preset",
            if preview { "ultrafast" } else { "medium" },
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            if preview { "160k" } else { "320k" },
            "-t",
            &format!("{duration:.9}"),
            "-movflags",
            "+faststart",
        ]));
        args.push(output.as_os_str().into());
        stage(
            ffmpeg,
            args,
            duration,
            progress.subrange(5, 100),
            "progress.fades",
            report,
        )?;
        return Ok(true);
    }
    let mut video = match crate::h264::inspect(ffmpeg, source, &info)? {
        crate::h264::Support::Copy(video) => video,
        crate::h264::Support::Convert(reason) => {
            report.warning(reason);
            plan.segments = vec![crate::plan::Segment {
                start: 0.0,
                end: duration,
                treatment: crate::plan::Treatment::Convert,
            }];
            report.plan(plan);
            return Ok(false);
        }
    };
    let used = video.parameter_sets | reserved_parameter_sets;
    let Some(sps) = (0..32).find(|id| used & (1u32 << id) == 0) else {
        report.warning("warning.partial_parameters");
        plan.segments = vec![crate::plan::Segment {
            start: 0.0,
            end: duration,
            treatment: crate::plan::Treatment::Convert,
        }];
        report.plan(plan);
        return Ok(false);
    };
    video.sps = sps;
    report.plan(plan);
    let fps = video.timescale as f64 / video.step as f64;
    let head_duration = head_loops as f64 * loop_duration;
    let tail_duration = (duration - tail_loop as f64 * loop_duration).max(0.0);
    let middle_loops = tail_loop - head_loops;
    let encode_tail = fade_out > 0.0 || (video.decode_delay > 0 && tail_duration > 1e-9);
    report.diagnostic(format!(
        "Encoding: start {head_duration:.3} s, end {:.3} s. {middle_loops} central loops copied ({:.3} s).",
        if encode_tail { tail_duration } else { 0.0 },
        middle_loops as f64 * loop_duration
    ));
    let folder = output.parent().ok_or("Missing temporary folder")?;
    let base = folder.join("loop.mp4");
    let head = folder.join("head.mp4");
    let tail = folder.join("tail.mp4");
    // Preserve presentation order; B-frames must never have PTS replaced with DTS.
    let mut normalize = words(&["-i"]);
    normalize.push(source.as_os_str().into());
    normalize.extend(words(&["-map", "0:v:0", "-c:v", "copy", "-bsf:v"]));
    normalize.push(
        format!(
            "setts=pts=PTS-STARTPTS:dts=N*{}-{}:duration={}",
            video.step, video.decode_delay, video.step
        )
        .into(),
    );
    normalize.extend(words(&[
        "-video_track_timescale",
        &video.timescale.to_string(),
    ]));
    normalize.push(base.as_os_str().into());
    stage(
        ffmpeg,
        normalize,
        loop_duration,
        progress.subrange(5, 10),
        "progress.copy_prepare",
        report,
    )?;

    let encoding = words(&[
        "-c:v",
        "libx264",
        "-profile:v",
        &video.profile,
        "-level:v",
        &video.level,
        "-pix_fmt",
        "yuv420p",
        "-crf",
        if preview { "28" } else { "16" },
        "-preset",
        if preview { "ultrafast" } else { "medium" },
        "-x264-params",
        &format!("bframes=0:ref=1:sps-id={}:force-cfr=1", video.sps),
        "-video_track_timescale",
        &video.timescale.to_string(),
    ]);
    let mut entries: Vec<(PathBuf, f64, usize)> = Vec::with_capacity(3);
    if head_loops > 0 {
        let mut args = words(&["-stream_loop", "-1"]);
        add_file(&mut args, "-i", &base);
        args.extend(words(&[
            "-map",
            "0:v:0",
            "-vf",
            &format!("fade=t=in:st=0:d={fade_in:.9}"),
            "-frames:v",
            &(head_loops * video.frames).to_string(),
        ]));
        args.extend(encoding.clone());
        args.push(head.as_os_str().into());
        stage(
            ffmpeg,
            args,
            head_duration,
            progress.subrange(10, 40),
            "progress.fade_in",
            report,
        )?;
        entries.push((head, head_duration, 1));
    }
    entries.push((base.clone(), loop_duration, middle_loops));
    if encode_tail {
        let mut args = words(&["-stream_loop", "-1"]);
        add_file(&mut args, "-i", &base);
        args.extend(words(&["-map", "0:v:0"]));
        if fade_out > 0.0 {
            args.extend(words(&[
                "-vf",
                &format!(
                    "fade=t=out:st={:.9}:d={fade_out:.9}",
                    (tail_duration - fade_out).max(0.0)
                ),
            ]));
        }
        args.extend(words(&[
            "-frames:v",
            &((tail_duration * fps - 1e-9).ceil() as u64).to_string(),
        ]));
        args.extend(encoding);
        args.push(tail.as_os_str().into());
        stage(
            ffmpeg,
            args,
            tail_duration,
            progress.subrange(40, 65),
            "progress.fade_out",
            report,
        )?;
        entries.push((tail, tail_duration, 1));
    } else if tail_duration > 1e-9 {
        entries.push((base, tail_duration, 1));
    }
    let concat = folder.join("concat.ffconcat");
    let mut list = BufWriter::new(fs::File::create(&concat)?);
    writeln!(list, "ffconcat version 1.0")?;
    for (path, length, count) in &entries {
        let name = path.file_name().unwrap().to_str().unwrap(); // Noms internes ASCII fixes.
        for _ in 0..*count {
            writeln!(list, "file '{name}'\nduration {length:.9}")?;
        }
    }
    list.flush()?;
    drop(list);
    let mut args = words(&["-f", "concat", "-safe", "0"]);
    add_file(&mut args, "-i", &concat);
    add_file(&mut args, "-i", wav);
    // setts defaults to decode timestamps: preserve PTS explicitly for B-frames.
    args.extend(words(&[
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "copy",
        "-tag:v",
        "avc3",
        "-bsf:v",
        &format!(
            "setts=pts=PTS:dts=N*{}-{}:duration='min({},max(1,{duration:.9}/TB-PTS))'",
            video.step, video.decode_delay, video.step
        ),
        "-c:a",
        "aac",
        "-b:a",
        if preview { "160k" } else { "320k" },
        "-t",
        &format!("{duration:.9}"),
        "-movflags",
        "+faststart",
    ]));
    args.push(output.as_os_str().into());
    stage(
        ffmpeg,
        args,
        duration,
        progress.subrange(65, 100),
        "progress.assemble",
        report,
    )?;
    if fs::metadata(output)?.len() == 0 {
        return Err("Empty export".into());
    }
    Ok(true)
}
