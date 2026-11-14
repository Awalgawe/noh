//! Video encoder for the composed project pass (`project.rs`, final render) and
//! the standalone caption burn (`captions.rs`, its own NVENC and x264 settings).
//!
//! On Windows, NVENC is used when a real one-frame encode with the exact settings
//! of that use succeeds; otherwise the unchanged x264 path is used. The probe result
//! lives in memory only, keyed by FFmpeg path and use; nothing persists across launches.
//! Other encode sites (plain montage, hybrid copy seams, standalone short,
//! preview proxies) are deliberately out of scope and keep x264.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// Test-only override: when set to any non-empty value, always choose x264.
pub(crate) const FORCE_X264_ENV: &str = "NOH_TEST_FORCE_X264";
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoEncoder {
    Nvenc,
    X264,
}

impl VideoEncoder {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Nvenc => "h264_nvenc",
            Self::X264 => "libx264",
        }
    }

    /// Video codec arguments for the final project pass.
    pub(crate) fn args(self, rate_num: i64, rate_den: i64, preview: bool) -> Vec<String> {
        match self {
            // Measured as candidate C3 in M0b (2026-11-06): valid at level 4.0,
            // VMAF 99.3 against lossless, deterministic output.
            Self::Nvenc => [
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p5",
                "-tune",
                "hq",
                "-rc",
                "vbr",
                "-cq",
                "19",
                "-b:v",
                "0",
                "-bf",
                "0",
                "-pix_fmt",
                "yuv420p",
            ]
            .map(String::from)
            .to_vec(),
            // Unchanged 0.1.5 arguments. Concatenated FFV1/NUT pieces do not
            // provide a nominal picture rate: keep their precise timestamps,
            // but prevent x264 from treating the time-base frequency as fps
            // (level 6.2 and hardware decode artifacts).
            Self::X264 => vec![
                "-c:v".into(),
                "libx264".into(),
                "-x264-params".into(),
                format!("fps={rate_num}/{rate_den}"),
                "-bf".into(),
                "0".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-crf".into(),
                if preview { "24" } else { "16" }.into(),
                "-preset".into(),
                if preview { "ultrafast" } else { "medium" }.into(),
            ],
        }
    }
}

/// The caption burn's NVENC settings: the picture of its own x264 encode for
/// about the same size. Measured 2026-11-10 against the source on a 30 s 1080p
/// clip: x264 (crf 16) SSIM 0.99896, XPSNR 51.6 dB, 30.8 MB, 12.1 s; these
/// settings SSIM 0.99896, XPSNR 52.7 dB, 32.6 MB (+6 %), about 3 s. The export's
/// settings (`VideoEncoder::args`) gave 45.8 MB (+48 %) for a higher fidelity.
pub(crate) const BURN_NVENC: [&str; 20] = [
    "-c:v",
    "h264_nvenc",
    "-preset",
    "p7",
    "-tune",
    "hq",
    "-rc",
    "vbr",
    "-cq",
    "23",
    "-b:v",
    "0",
    // No B-frames, as with x264 (see captions::encode_args).
    "-bf",
    "0",
    "-spatial-aq",
    "1",
    "-temporal-aq",
    "1",
    "-pix_fmt",
    "yuv420p",
];

/// What an encode is for: each use has its NVENC settings, probed as such.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Use {
    Export,
    Burn,
}

impl Use {
    fn nvenc_args(self) -> Vec<String> {
        match self {
            Self::Export => VideoEncoder::Nvenc.args(24, 1, false),
            Self::Burn => BURN_NVENC.map(String::from).to_vec(),
        }
    }
}

/// Choice for one export, and the probe time when a probe actually ran.
pub(crate) struct Selection {
    pub encoder: VideoEncoder,
    pub probe: Option<Duration>,
}

fn forced_x264() -> bool {
    std::env::var_os(FORCE_X264_ENV).is_some_and(|v| !v.is_empty())
}

pub(crate) fn select(ffmpeg: &Path, preview: bool) -> Selection {
    select_for(ffmpeg, preview, Use::Export)
}

/// The choice for `usage`, probed with that use's own NVENC settings (a GPU
/// without temporal AQ, for instance, passes the export's and fails the burn's).
pub(crate) fn select_for(ffmpeg: &Path, preview: bool, usage: Use) -> Selection {
    select_with(
        ffmpeg,
        usage,
        preview,
        cfg!(windows),
        forced_x264(),
        |ffmpeg| probe(ffmpeg, &usage.nvenc_args()),
    )
}

fn select_with(
    ffmpeg: &Path,
    usage: Use,
    preview: bool,
    windows: bool,
    forced: bool,
    probe: impl FnOnce(&Path) -> bool,
) -> Selection {
    let x264 = Selection {
        encoder: VideoEncoder::X264,
        probe: None,
    };
    if preview || !windows || forced {
        return x264;
    }
    static CACHE: OnceLock<Mutex<HashMap<(PathBuf, Use), bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (ffmpeg.to_path_buf(), usage);
    if let Some(&ok) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Selection {
            encoder: if ok {
                VideoEncoder::Nvenc
            } else {
                VideoEncoder::X264
            },
            probe: None,
        };
    }
    let started = Instant::now();
    let ok = probe(ffmpeg);
    let elapsed = started.elapsed();
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, ok);
    Selection {
        encoder: if ok {
            VideoEncoder::Nvenc
        } else {
            VideoEncoder::X264
        },
        probe: Some(elapsed),
    }
}

/// One real 256×256 encode with the exact settings of the use. Any error,
/// including a missing lavfi device or a timeout, means "use x264".
fn probe(ffmpeg: &Path, settings: &[String]) -> bool {
    let mut cmd = crate::command(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=256x256:r=24",
        "-frames:v",
        "1",
    ])
    .args(settings)
    .args(["-f", "null", "-"]);
    matches!(
        crate::process::stream(cmd, Some(PROBE_TIMEOUT), false, |_| Ok(())),
        Ok((status, _)) if status.success()
    )
}

/// True when FFmpeg's log shows that the NVENC encoder itself failed, as
/// opposed to a decode, filter, caption renderer or cancellation failure.
/// Markers were checked against the shipped FFmpeg 7.1.1 `avcodec-61.dll`
/// (2026-11-06): session/initialisation failures and per-frame failures.
pub(crate) fn nvenc_failure(log: &str) -> bool {
    const MARKERS: [&str; 9] = [
        // Initialisation.
        "OpenEncodeSessionEx failed",
        "No capable devices found",
        "Cannot load nvcuda.dll",
        "Cannot load nvEncodeAPI",
        "does not support the required nvenc API version",
        "InitializeEncoder failed",
        "CreateInputBuffer failed",
        // While encoding.
        "EncodePicture failed",
        "Failed locking bitstream buffer",
    ];
    log.lines()
        .any(|line| line.contains("h264_nvenc") && MARKERS.iter().any(|m| line.contains(m)))
}

/// Encode with `encoder`; when NVENC itself fails, remove the partial output and
/// retry once with x264. Every other failure is returned unchanged. When the
/// retry fails too, its error keeps the NVENC failure log as prior context.
pub(crate) fn render_with_fallback<E: Technical>(
    encoder: VideoEncoder,
    output: &Path,
    mut render: impl FnMut(VideoEncoder) -> Result<(), E>,
    mut on_fallback: impl FnMut(&E),
) -> Result<VideoEncoder, E> {
    match render(encoder) {
        Ok(()) => Ok(encoder),
        Err(e) if encoder == VideoEncoder::Nvenc && nvenc_failure(e.technical()) => {
            on_fallback(&e);
            // The final pass refuses to overwrite (-n): a partial file would make
            // the retry fail immediately.
            if output.exists() {
                std::fs::remove_file(output).map_err(|_| e.clone_for_report())?;
            }
            render(VideoEncoder::X264)
                .map(|()| VideoEncoder::X264)
                .map_err(|mut retry| {
                    retry.add_prior(e.technical());
                    retry
                })
        }
        Err(e) => Err(e),
    }
}

/// Per-attempt log limit when both attempts are kept (run_command uses 8192).
const PART_LIMIT: usize = 8192;

pub(crate) trait Technical {
    fn technical(&self) -> &str;
    /// Prefix the technical log with an earlier, failed NVENC attempt.
    fn add_prior(&mut self, prior: &str);
    fn clone_for_report(&self) -> Self;
}

impl Technical for crate::engine::EngineError {
    fn technical(&self) -> &str {
        &self.technical
    }
    fn add_prior(&mut self, prior: &str) {
        // Bound each part separately, then join: the labels always survive and
        // each log keeps its own tail (where FFmpeg prints the failing line).
        self.technical = format!(
            "NVENC attempt failed first:\n{}\n\nlibx264 retry failed:\n{}",
            crate::engine::bounded(prior, PART_LIMIT),
            crate::engine::bounded(&self.technical, PART_LIMIT)
        )
        .into();
    }
    fn clone_for_report(&self) -> Self {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Failure(String);
    impl Technical for Failure {
        fn technical(&self) -> &str {
            &self.0
        }
        fn add_prior(&mut self, prior: &str) {
            self.0 = format!("{prior}\n--\n{}", self.0);
        }
        fn clone_for_report(&self) -> Self {
            Failure(self.0.clone())
        }
    }
    const NVENC_LOG: &str = "[h264_nvenc @ 0000] OpenEncodeSessionEx failed: out of memory (10): (no details)\nError while opening encoder";

    #[test]
    fn mid_encode_nvenc_failures_are_recognised() {
        // Exact per-frame strings present in the shipped avcodec-61.dll.
        assert!(nvenc_failure(
            "[h264_nvenc @ 000001] EncodePicture failed!: generic error (20): (no details)"
        ));
        assert!(nvenc_failure(
            "[h264_nvenc @ 000001] Failed locking bitstream buffer: generic error (20)"
        ));
    }

    #[test]
    fn a_failed_retry_keeps_the_nvenc_failure_as_prior_context() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("export.mp4");
        let result = render_with_fallback(
            VideoEncoder::Nvenc,
            &output,
            |encoder| match encoder {
                VideoEncoder::Nvenc => Err(Failure(NVENC_LOG.into())),
                VideoEncoder::X264 => Err(Failure("x264 retry log".into())),
            },
            |_| {},
        );
        let error = result.unwrap_err();
        assert!(error.0.contains("OpenEncodeSessionEx failed"), "{error:?}");
        assert!(error.0.contains("x264 retry log"), "{error:?}");
    }

    #[test]
    fn engine_error_prior_context_survives_bounding() {
        let mut error =
            crate::engine::EngineError::new("error.short_backend", "render_project", None, "x");
        // Worst case: both logs already truncated by run_command (8192 bytes plus
        // the "[truncated] " prefix), with the failing line at each tail.
        let truncated = |body: &str, tail: &str| {
            crate::engine::bounded(&format!("{}{tail}", body.repeat(20_000)), 8192)
        };
        let prior = truncated("p", NVENC_LOG);
        error.technical = truncated("r", "\nx264 failing line").into();
        assert!(prior.starts_with("[truncated] ") && prior.len() > 8192);
        assert!(error.technical.starts_with("[truncated] ") && error.technical.len() > 8192);
        error.add_prior(&prior);
        assert!(error.technical.starts_with("NVENC attempt failed first:\n"));
        assert!(error.technical.contains("\n\nlibx264 retry failed:\n"));
        assert!(error.technical.contains("OpenEncodeSessionEx failed"));
        assert!(error.technical.ends_with("x264 failing line"));
    }

    #[test]
    fn x264_arguments_are_the_unchanged_release_arguments() {
        assert_eq!(
            VideoEncoder::X264.args(12288, 512, false).join(" "),
            "-c:v libx264 -x264-params fps=12288/512 -bf 0 -pix_fmt yuv420p -crf 16 -preset medium"
        );
        assert_eq!(
            VideoEncoder::X264.args(24, 1, true).join(" "),
            "-c:v libx264 -x264-params fps=24/1 -bf 0 -pix_fmt yuv420p -crf 24 -preset ultrafast"
        );
    }

    #[test]
    fn nvenc_arguments_match_the_measured_candidate() {
        assert_eq!(
            VideoEncoder::Nvenc.args(12288, 512, false).join(" "),
            "-c:v h264_nvenc -preset p5 -tune hq -rc vbr -cq 19 -b:v 0 -bf 0 -pix_fmt yuv420p"
        );
    }

    #[test]
    fn selection_requires_windows_a_final_export_no_override_and_a_passing_probe() {
        let ff = |n: &str| PathBuf::from(format!("C:/probe-test/{n}/ffmpeg.exe"));
        let never = |_: &Path| panic!("probe must not run");
        assert_eq!(
            select_with(&ff("a"), Use::Export, true, true, false, never).encoder,
            VideoEncoder::X264
        );
        assert_eq!(
            select_with(&ff("b"), Use::Export, false, false, false, never).encoder,
            VideoEncoder::X264
        );
        assert_eq!(
            select_with(&ff("c"), Use::Export, false, true, true, never).encoder,
            VideoEncoder::X264
        );
        let failed = select_with(&ff("d"), Use::Export, false, true, false, |_| false);
        assert_eq!(failed.encoder, VideoEncoder::X264);
        assert!(failed.probe.is_some());
        let passed = select_with(&ff("e"), Use::Export, false, true, false, |_| true);
        assert_eq!(passed.encoder, VideoEncoder::Nvenc);
        assert!(passed.probe.is_some());
        // Cached per FFmpeg path for the session: no second probe.
        let cached = select_with(&ff("e"), Use::Export, false, true, false, never);
        assert_eq!(cached.encoder, VideoEncoder::Nvenc);
        assert!(cached.probe.is_none());
        // The burn's settings are probed on their own: the export's pass says
        // nothing about them, and each result is cached apart.
        let burn = select_with(&ff("e"), Use::Burn, false, true, false, |_| false);
        assert_eq!(burn.encoder, VideoEncoder::X264);
        assert!(burn.probe.is_some());
        let burn = select_with(&ff("e"), Use::Burn, false, true, false, never);
        assert_eq!(burn.encoder, VideoEncoder::X264);
        assert!(burn.probe.is_none());
        let export = select_with(&ff("e"), Use::Export, false, true, false, never);
        assert_eq!(export.encoder, VideoEncoder::Nvenc);
        // A burn preview stays on x264, unprobed.
        let preview = select_with(&ff("f"), Use::Burn, true, true, false, never);
        assert_eq!(preview.encoder, VideoEncoder::X264);
    }

    #[test]
    fn burn_nvenc_arguments_match_the_measured_setting() {
        assert_eq!(
            BURN_NVENC.join(" "),
            "-c:v h264_nvenc -preset p7 -tune hq -rc vbr -cq 23 -b:v 0 -bf 0 -spatial-aq 1 -temporal-aq 1 -pix_fmt yuv420p"
        );
        assert_eq!(Use::Burn.nvenc_args().join(" "), BURN_NVENC.join(" "));
        assert_eq!(
            Use::Export.nvenc_args(),
            VideoEncoder::Nvenc.args(24, 1, false)
        );
    }

    #[test]
    fn only_encoder_failures_are_recognised() {
        assert!(nvenc_failure(NVENC_LOG));
        assert!(nvenc_failure("[h264_nvenc @ 01] No capable devices found"));
        for other in [
            "[h264 @ 01] decode_slice_header error",
            "[Parsed_subtitles_0 @ 01] Unable to open captions.ass",
            "libass wasn't built with ASS_FEATURE_WRAP_UNICODE support",
            "Conversion failed!",
            "", // killed by cancellation: no encoder message
        ] {
            assert!(!nvenc_failure(other), "{other}");
        }
    }

    #[test]
    fn nvenc_failure_removes_the_partial_output_and_retries_with_x264_once() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("export.mp4");
        let mut calls = Vec::new();
        let mut fallbacks = 0;
        let used = render_with_fallback(
            VideoEncoder::Nvenc,
            &output,
            |encoder| {
                calls.push(encoder);
                if encoder == VideoEncoder::Nvenc {
                    std::fs::write(&output, b"partial").unwrap();
                    Err(Failure(NVENC_LOG.into()))
                } else {
                    assert!(!output.exists(), "partial output must be removed first");
                    std::fs::write(&output, b"complete").unwrap();
                    Ok(())
                }
            },
            |_| fallbacks += 1,
        );
        assert!(matches!(used, Ok(VideoEncoder::X264)));
        assert_eq!(calls, [VideoEncoder::Nvenc, VideoEncoder::X264]);
        assert_eq!(fallbacks, 1);
        assert_eq!(std::fs::read(&output).unwrap(), b"complete");
    }

    #[test]
    fn other_failures_and_x264_failures_are_never_retried() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("export.mp4");
        for (encoder, log) in [
            (
                VideoEncoder::Nvenc,
                "[Parsed_subtitles_0 @ 01] Unable to open captions.ass",
            ),
            (VideoEncoder::Nvenc, ""),
            (VideoEncoder::X264, NVENC_LOG),
        ] {
            let mut calls = 0;
            let result = render_with_fallback(
                encoder,
                &output,
                |_| {
                    calls += 1;
                    Err(Failure(log.into()))
                },
                |_| panic!("no fallback expected"),
            );
            assert!(result.is_err());
            assert_eq!(calls, 1, "{encoder:?} {log}");
        }
    }
}
