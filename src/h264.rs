//! H.264 copy capabilities shared by sequence preparation and partial fades.
use crate::Result;
use std::path::Path;

#[derive(Clone)]
pub(crate) struct Video {
    pub frames: usize,
    pub step: i64,
    pub timescale: i64,
    pub profile: String,
    pub level: String,
    pub sps: u32,
    pub parameter_sets: u32,
    pub decode_delay: i64,
}

#[derive(Clone)]
pub(crate) enum Support {
    Copy(Video),
    Convert(&'static str),
}

fn fallback(reason: &'static str) -> Result<Support> {
    Ok(Support::Convert(reason))
}

// Incompatible copy parameters are distinct from media/FFmpeg failures.
pub(crate) fn inspect(
    ffmpeg: &Path,
    source: &Path,
    info: &crate::media::MediaInfo,
) -> Result<Support> {
    if !info.signature.contains("#codec_id 0: h264") {
        return fallback("warning.partial_codec");
    }
    let stream = &info.description;
    if !(stream.contains(" yuv420p(") || stream.contains(" yuv420p,"))
        || stream.contains("interlaced")
    {
        return fallback("warning.partial_pixels");
    }
    let profile = if stream.contains("(Main)") {
        "main"
    } else if stream.contains("(High)") {
        "high"
    } else if stream.contains("(Constrained Baseline)") || stream.contains("(Baseline)") {
        "baseline"
    } else {
        return fallback("warning.partial_profile");
    };
    if !info.fixed || info.timescale <= 0 {
        return fallback("warning.partial_timing");
    }
    let step = info.step;
    let timescale = info.timescale;
    let mut command = crate::command(ffmpeg);
    command
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(source)
        .args([
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-bsf:v",
            "trace_headers",
            "-frames:v",
            "1",
            "-f",
            "null",
            "-",
        ]);
    let (status, headers) =
        crate::process::capture(command, std::time::Duration::from_secs(30), false)?;
    if !status.success() {
        let mut error = crate::engine::EngineError::new(
            "error.h264_headers",
            "inspect_headers",
            Some(source),
            "Cannot read H.264 parameters.",
        );
        error.technical = crate::engine::bounded(&headers, 8192).into();
        return Err(Box::new(error));
    }
    let value = |line: &str| {
        line.rsplit('=')
            .next()
            .and_then(|n| n.trim().parse::<u32>().ok())
    };
    if !headers
        .lines()
        .any(|l| l.contains("nal_unit_type") && value(l) == Some(5))
    {
        return fallback("warning.partial_idr");
    }
    if headers
        .lines()
        .any(|l| l.contains("frame_mbs_only_flag") && value(l) == Some(0))
    {
        return fallback("warning.partial_pixels");
    }
    let Some(level) = headers
        .lines()
        .find(|l| l.contains("level_idc"))
        .and_then(value)
    else {
        return fallback("warning.partial_parameters");
    };
    let used: Vec<_> = headers
        .lines()
        .filter(|l| l.contains("seq_parameter_set_id"))
        .filter_map(value)
        .collect();
    let Some(sps) = (0..32).find(|id| !used.contains(id)) else {
        return fallback("warning.partial_parameters");
    };
    Ok(Support::Copy(Video {
        frames: info.frames,
        step,
        timescale,
        profile: profile.into(),
        level: format!("{}.{:01}", level / 10, level % 10),
        sps,
        parameter_sets: used
            .iter()
            .filter(|id| **id < 32)
            .fold(0, |mask, id| mask | (1 << id)),
        decode_delay: info.decode_delay,
    }))
}
