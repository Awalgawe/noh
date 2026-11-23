//! Task 43: one source whose captions Classique draws on the GPU, for the Short
//! alone (shorts.rs) and the caption burn (captions.rs), as the project export
//! does (project.rs `run_effect`): FFmpeg #1 decodes and frames the video without
//! captions, the engine draws Classique and mixes it in, FFmpeg #2 adds the sound
//! and encodes. Each path keeps its own input options, encoder, audio and checks;
//! libass stays their fallback when no OpenGL context exists.
//!
//! Clock: the scene is drawn at FFmpeg #1's frame times as given, the clock libass
//! reads in the single-FFmpeg command (same input options, passthrough timing).
//! `fx::export::pipe_input` keeps the picture's place against the sound.
use crate::captions::CaptionStyle;
use crate::engine::EngineError;
use crate::subtitle_track::SubtitleTrack;
use crate::{fx, style};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

pub(crate) struct Job {
    pub ffmpeg: PathBuf,
    pub source: PathBuf,
    pub workspace: PathBuf,
    /// Options before `-i source`, in both FFmpeg (the Short's `-ss start`).
    pub input: Vec<String>,
    /// Output length options (the Short's `-t span`), in both.
    pub limit: Vec<String>,
    /// FFmpeg #1's video filter: colour conversion and framing, no captions, no tags.
    pub filter: String,
    /// FFmpeg #2's one video filter chain: the tags first, then a preview scale.
    pub finish: Option<String>,
    /// The canvas the captions are drawn on (FFmpeg #1's output size).
    pub size: (u32, u32),
    pub matrix: crate::color::Matrix,
    /// Cues on FFmpeg #1's clock.
    pub track: SubtitleTrack,
    pub style: CaptionStyle,
    /// The effect plugin chosen instead of Classique (command line, MCP): read
    /// from its folder once, its id checked again. None draws the built-in
    /// Classique with `style`.
    pub effect: Option<crate::project::EffectChoice>,
    /// A motion scene drawn instead of the captions (no fallback).
    #[cfg_attr(not(feature = "motion"), allow(dead_code))]
    pub scene: Option<SceneJob>,
    /// FFmpeg #1's picture size when it differs from the canvas: a scene
    /// makes a frame of another shape (a vertical frame) from the source's
    /// picture. None: FFmpeg #1 outputs the canvas.
    #[cfg_attr(not(feature = "motion"), allow(dead_code))]
    pub source_size: Option<(u32, u32)>,
    /// FFmpeg #2's encoder, audio and bitstream arguments, before `-progress`.
    pub encode: Vec<String>,
    pub deadline: Duration,
    /// The video's known end on FFmpeg #1's clock (the last frame's duration).
    pub end: Option<f64>,
}

/// A scene to draw: validated at the output's frames before FFmpeg starts.
#[cfg_attr(not(feature = "motion"), allow(dead_code))]
pub(crate) struct SceneJob {
    pub scene: PathBuf,
    /// Where FFmpeg #1's clock starts on the source's (a clip), and how long it runs.
    pub start: f64,
    pub span: f64,
    /// The source's duration (the scene's song length) and nominal frame rate.
    pub duration: f64,
    pub fps: f64,
    /// Cues whose word times were read beside the subtitles.
    pub words: usize,
    /// Whether the source has sound (its energy and onsets).
    pub sound: bool,
}

/// Why the GPU did not export.
pub(crate) enum Failure {
    /// No OpenGL context while drawing Classique: nothing started, the caller
    /// draws with libass. A chosen effect has no fallback: it fails as `Engine`
    /// (`error.effect_gl_unavailable`), nothing published.
    Gl(String),
    /// FFmpeg #2 failed (its log in `technical`) or the engine failed.
    Engine(EngineError),
}

/// What a GPU export reports.
pub(crate) struct Done {
    /// Output time of the first frame whose letters left the safe area.
    pub overflow_at: Option<f64>,
    /// Output time of the first frame showing a word wider than its box.
    pub too_wide_at: Option<f64>,
    /// The installed family the effect names and this machine lacks.
    pub font_missing: Option<String>,
    pub diagnostic: String,
}

impl Job {
    /// FFmpeg #1, without its output (fx::export::run adds the raw pipe).
    pub(crate) fn first(&self) -> Command {
        let mut cmd = crate::command(&self.ffmpeg);
        cmd.current_dir(&self.workspace)
            .args([
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-xerror",
                "-err_detect",
                "explode",
            ])
            .args(&self.input)
            .arg("-i")
            .arg(&self.source)
            .args(&self.limit)
            .args([
                "-map",
                "0:v:0",
                "-vf",
                &self.filter,
                "-fps_mode",
                "passthrough",
                "-enc_time_base:v",
                "filter",
            ]);
        cmd
    }

    /// FFmpeg #2 for FFmpeg #1's time base and first frame time.
    pub(crate) fn second(&self, time_base: &str, first: f64) -> Command {
        let mut cmd = crate::command(&self.ffmpeg);
        cmd.current_dir(&self.workspace).args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-n",
            "-xerror",
        ]);
        fx::export::pipe_input(&mut cmd, first);
        cmd.args(&self.input)
            .arg("-i")
            .arg(&self.source)
            .args(["-map", "0:v", "-map", "1:a:0?"]);
        if let Some(finish) = &self.finish {
            cmd.args(["-filter:v", finish]);
        }
        cmd.args(&self.limit)
            .args(["-fps_mode", "passthrough", "-enc_time_base:v", time_base])
            .args(&self.encode)
            .args(["-progress", "pipe:1", "export.mp4"]);
        cmd
    }
}

/// Draw the captions with the built-in Classique, or the chosen effect, and
/// encode. `failed` is the path's backend error code and message; `progress`
/// gets FFmpeg #2's lines.
pub(crate) fn run(
    job: Job,
    failed: (&str, &str, &str),
    progress: impl FnMut(&str),
) -> Result<Done, Failure> {
    #[cfg(feature = "motion")]
    if job.scene.is_some() {
        return run_scene(job, failed, progress);
    }
    let (w, h) = job.size;
    let engine = |dir: Option<&std::path::Path>, e: String| {
        Failure::Engine(EngineError::new("error.effect", failed.1, dir, e))
    };
    let (effect, preset, values, dir) = match &job.effect {
        None => (
            style::catalog::builtin_classic(),
            None,
            crate::project::classic_values(&job.style),
            None,
        ),
        Some(choice) => (
            style::catalog::load_one(&choice.dir, &choice.id)
                .map_err(|e| engine(Some(&choice.dir), e))?,
            choice.preset.clone(),
            choice.values.clone(),
            Some(choice.dir.as_path()),
        ),
    };
    let name = effect.id.clone();
    if let Some(preset) = &preset
        && !effect.presets.iter().any(|(id, _)| id == preset)
    {
        return Err(engine(
            dir,
            format!("The effect has no preset \"{preset}\"."),
        ));
    }
    let scene = style::plugin::PluginScene::new(
        effect,
        w,
        h,
        style::line::from_track(&job.track),
        preset.as_deref(),
    )
    .with_values(&values)
    .map_err(|e| engine(dir, e))?
    .with_safe_insets(job.style.safe_area.insets(w, h));
    // Classique's letters never leave the picture by design; animated effects may.
    let scene = if job.effect.is_none() {
        scene.with_frame_check()
    } else {
        scene
    };
    let classic = job.effect.is_none();
    let first = job.first();
    let workspace = job.workspace.clone();
    let matrix = if job.matrix == crate::color::Matrix::Bt601 {
        fx::gl::Matrix::Bt601
    } else {
        fx::gl::Matrix::Bt709
    };
    let deadline = job.deadline;
    let end = job.end;
    let export = fx::export::Job {
        first,
        second: Box::new(move |time_base, first| job.second(time_base, first)),
        workspace,
        width: w,
        height: h,
        source: (w, h),
        matrix,
        scene: Box::new(crate::style::plugin::StyleLayer::new(scene)),
        deadline,
        end,
    };
    match fx::export::run(export, progress) {
        Ok((status, _, stats)) if status.success() => Ok(Done {
            overflow_at: stats.overflow_at,
            too_wide_at: stats.too_wide_at,
            font_missing: stats.font_missing,
            diagnostic: format!(
                "Caption effect ({name}): {} frames, engine set up in {:.0} ms on {}, time base {}, layer and mix p95 {:.2} ms per frame.",
                stats.frames, stats.setup_ms, stats.adapter, stats.time_base, stats.render_ms_p95
            ),
        }),
        Ok((_, log, _)) => {
            let mut e = EngineError::new(failed.0, failed.1, None, failed.2);
            e.technical = crate::engine::bounded(&log, 8192).into();
            Err(Failure::Engine(e))
        }
        Err(f) if f.gl_unavailable && classic => Err(Failure::Gl(f.message)),
        Err(f) if f.gl_unavailable => Err(Failure::Engine(EngineError::new(
            "error.effect_gl_unavailable",
            failed.1,
            None,
            format!("This computer cannot draw caption effects ({}).", f.message),
        ))),
        Err(f) => {
            let mut e = EngineError::new(
                "error.effect_render",
                failed.1,
                None,
                format!(
                    "The caption effect could not be rendered: {}. Nothing was published.",
                    f.message
                ),
            );
            e.technical = crate::engine::bounded(&f.log, 8192).into();
            Err(Failure::Engine(e))
        }
    }
}

/// The scene's validation, then the export with its overlay. Refusals name the
/// instant and the cap; nothing starts before the scene has passed.
#[cfg(feature = "motion")]
fn run_scene(
    job: Job,
    failed: (&str, &str, &str),
    progress: impl FnMut(&str),
) -> Result<Done, Failure> {
    use crate::motion::{layer, song, thread::ScriptThread, validate::grid};
    let refused = |e: String| {
        Failure::Engine(EngineError::new(
            "error.scene_refused",
            failed.1,
            job.scene.as_ref().map(|s| s.scene.as_path()),
            e,
        ))
    };
    let scene = job.scene.as_ref().unwrap();
    let picture = job.source_size.unwrap_or(job.size);
    let inputs = song::gather(
        &job.ffmpeg,
        &job.source,
        scene.sound,
        job.size,
        picture,
        scene.duration,
        &job.track,
        scene.words,
        job.style.safe_area,
    )
    .map_err(|e| {
        Failure::Engine(EngineError::new(
            "error.scene_input",
            failed.1,
            Some(&job.source),
            e,
        ))
    })?;
    let src = layer::read_scene(&scene.scene).map_err(refused)?;
    let shaders = crate::fx::layer_shader::read_beside(&scene.scene).map_err(refused)?;
    let mut song_inputs = inputs.inputs;
    song_inputs.shaders = shaders.iter().map(|(n, _)| n.clone()).collect();
    let thread = ScriptThread::start(src, song_inputs, inputs.audio, false).map_err(refused)?;
    let mut notes = inputs.notes;
    let fps = match crate::motion::validate::known_fps(scene.fps) {
        Some(fps) => fps,
        None => {
            notes.push(crate::motion::validate::UNKNOWN_FPS.into());
            crate::motion::validate::FALLBACK_FPS
        }
    };
    let checked = thread
        .validate(grid(scene.start, scene.start + scene.span, fps))
        .map_err(refused)?;
    let overlay = layer::MotionLayer::new(thread, job.size, scene.start, shaders)
        .map_err(refused)?
        .with_source(picture);
    let (w, h) = job.size;
    let first = job.first();
    let workspace = job.workspace.clone();
    let matrix = if job.matrix == crate::color::Matrix::Bt601 {
        fx::gl::Matrix::Bt601
    } else {
        fx::gl::Matrix::Bt709
    };
    let (deadline, end) = (job.deadline, job.end);
    let export = fx::export::Job {
        first,
        second: Box::new(move |time_base, first| job.second(time_base, first)),
        workspace,
        width: w,
        height: h,
        source: picture,
        matrix,
        scene: Box::new(overlay),
        deadline,
        end,
    };
    match fx::export::run(export, progress) {
        Ok((status, _, stats)) if status.success() => Ok(Done {
            overflow_at: None,
            too_wide_at: None,
            font_missing: None,
            diagnostic: format!(
                "Motion scene: {} frames checked before, {} drawn, engine set up in {:.0} ms on {}, time base {}, layer and mix p95 {:.2} ms per frame.{}{}",
                checked.frames,
                stats.frames,
                stats.setup_ms,
                stats.adapter,
                stats.time_base,
                stats.render_ms_p95,
                if checked.layout_warnings.is_empty() {
                    String::new()
                } else {
                    format!(" Layout: {}.", checked.layout_warnings.join("; "))
                },
                if notes.is_empty() {
                    String::new()
                } else {
                    format!(" Notes: {}.", notes.join("; "))
                },
            ),
        }),
        Ok((_, log, _)) => {
            let mut e = EngineError::new(failed.0, failed.1, None, failed.2);
            e.technical = crate::engine::bounded(&log, 8192).into();
            Err(Failure::Engine(e))
        }
        Err(f) if f.gl_unavailable => Err(Failure::Engine(EngineError::new(
            "error.motion_gl_unavailable",
            failed.1,
            None,
            format!("This computer cannot draw scenes ({}).", f.message),
        ))),
        Err(f) => {
            let mut e = EngineError::new(
                "error.scene_render",
                failed.1,
                None,
                format!(
                    "The scene could not be drawn: {}. Nothing was published.",
                    f.message
                ),
            );
            e.technical = crate::engine::bounded(&f.log, 8192).into();
            Err(Failure::Engine(e))
        }
    }
}

/// The warning for letters drawn outside the safe area, or outside the picture
/// when there is none (`safe`: a safe area applies), first at `at` seconds.
pub(crate) fn overflow_warning(at: f64, safe: bool) -> String {
    let at = at.max(0.0).round() as u64;
    let code = if safe {
        "warning.effect_overflow"
    } else {
        "warning.effect_off_frame"
    };
    format!("{code}|{}:{:02}", at / 60, at % 60)
}

/// The warning for an installed font the effect names and this machine lacks:
/// its text was drawn in the plugin's own face.
pub(crate) fn font_missing_warning(family: &str) -> String {
    format!("warning.effect_font_missing|{}", family.replace('|', " "))
}

/// A command's arguments as text (tests).
#[cfg(test)]
pub(crate) fn args(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
pub(crate) fn job_for_test(input: &[&str], finish: Option<&str>) -> Job {
    Job {
        scene: None,
        source_size: None,
        ffmpeg: "ffmpeg".into(),
        source: "in.mp4".into(),
        workspace: ".".into(),
        input: input.iter().map(|s| s.to_string()).collect(),
        limit: vec!["-t".into(), "1.640".into()],
        filter:
            "format=yuv420p,scale=1080:608,setsar=1,pad=1080:1920:(ow-iw)/2:(oh-ih)/2:color=black"
                .into(),
        finish: finish.map(str::to_string),
        size: (1080, 1920),
        matrix: crate::color::Matrix::Bt709,
        track: SubtitleTrack {
            voices: Vec::new(),
            language: None,
            duration_ms: 1_000,
            cues: Vec::new(),
        },
        style: CaptionStyle::default(),
        effect: None,
        encode: vec!["-c:v".into(), "libx264".into()],
        deadline: Duration::from_secs(60),
        end: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without a safe area, letters leaving the picture are still found (exports
    /// warn with warning.effect_off_frame), and Classique's layout is unchanged.
    #[test]
    fn letters_leaving_the_picture_are_found_without_a_safe_area() {
        use crate::fx::shaders::Upload;
        struct NoGpu;
        impl Upload for NoGpu {
            fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
            fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
        }
        let mut fonts = crate::fx::text::Fonts::load().unwrap();
        let words: Vec<String> = std::iter::repeat_n("Text".to_string(), 600).collect();
        let scene = |check: bool, words: &[String]| {
            let s = style::plugin::PluginScene::new(
                style::catalog::builtin_classic(),
                320,
                180,
                vec![style::line::split_words(0.0, 2.0, words, " ")],
                None,
            );
            if check { s.with_frame_check() } else { s }
        };
        let mut tall = scene(true, &words);
        let (_, _, main, _) = tall.frame(&mut fonts, &NoGpu, 1.0);
        let ys: Vec<f32> = main.iter().map(|i| i.center[1]).collect();
        let (lo, hi) = ys
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
        assert!(
            tall.overflowed(),
            "600 words at 320 x 180 leave the picture: {} letters, y {lo}..{hi}, colours {:?}",
            main.len(),
            main.iter().take(3).map(|i| i.color).collect::<Vec<_>>()
        );
        let mut unchecked = scene(false, &words);
        unchecked.frame(&mut fonts, &NoGpu, 1.0);
        assert!(
            !unchecked.overflowed(),
            "without the check nothing is said, as before"
        );
        let mut short = scene(true, &words[..3]);
        short.frame(&mut fonts, &NoGpu, 1.0);
        assert!(!short.overflowed(), "a short line stays inside");
    }

    /// Both FFmpeg read the source with the same input options; FFmpeg #1 keeps
    /// passthrough timing; FFmpeg #2 re-tags then scales in one chain; the pipe
    /// input gets -itsoffset only when the first frame is not at zero.
    #[test]
    fn commands_keep_the_clock_the_tags_and_the_sound() {
        let job = job_for_test(
            &["-ss", "0.135"],
            Some(
                "setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv,scale=360:640",
            ),
        );
        let one = args(&job.first()).join(" ");
        assert!(
            one.contains(
                "-ss 0.135 -i in.mp4 -t 1.640 -map 0:v:0 -vf format=yuv420p,scale=1080:608"
            ),
            "{one}"
        );
        assert!(
            one.ends_with("-fps_mode passthrough -enc_time_base:v filter"),
            "{one}"
        );
        assert!(!one.contains("subtitles"), "{one}");
        let two = args(&job.second("1/12800", 0.0)).join(" ");
        assert!(!two.contains("-itsoffset"), "{two}");
        assert!(
            two.contains("-xerror -f matroska -i pipe:0 -ss 0.135 -i in.mp4 -map 0:v -map 1:a:0?"),
            "{two}"
        );
        assert_eq!(two.matches("-filter:v").count(), 1, "one chain: {two}");
        assert!(two.contains("-filter:v setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv,scale=360:640 -t 1.640"), "{two}");
        assert!(two.contains("-fps_mode passthrough -enc_time_base:v 1/12800 -c:v libx264 -progress pipe:1 export.mp4"), "{two}");
        let late = args(&job.second("1/12800", 0.067)).join(" ");
        assert!(
            late.contains("-itsoffset 0.067000 -f matroska -i pipe:0"),
            "{late}"
        );
        // The burn: no seek, no length.
        let mut burn = job_for_test(&[], None);
        burn.limit.clear();
        let two = args(&burn.second("1/90000", 0.0)).join(" ");
        assert!(
            two.contains("-i pipe:0 -i in.mp4 -map 0:v -map 1:a:0? -fps_mode passthrough"),
            "{two}"
        );
    }
}
