//! A scene's light check, for a window that follows the agent's work: the file
//! reads, its format line and its language load, and its first frame draws.
//! Nothing is rendered and no frame after the first is checked; that is
//! `noh_scene_check`'s, in the worker.
use super::{Inputs, audio::Audio, thread::ScriptThread};
use crate::subtitle_track::SubtitleTrack;
use std::path::Path;

/// Whether the scene at `scene` loads and draws its first frame, on a frame of
/// `size` with `track`'s lines, `duration` seconds long, without sound.
pub fn loads(
    scene: &Path,
    track: Option<&SubtitleTrack>,
    size: (u32, u32),
    duration: f64,
) -> Result<(), String> {
    // Read as the export and the scene tools read it: capped, UTF-8.
    let src = super::layer::read_scene(scene)?;
    let shaders = crate::fx::layer_shader::read_beside(scene)?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let inputs = Inputs {
        w: size.0,
        h: size.1,
        src_w: size.0,
        src_h: size.1,
        duration,
        cues: track.map(super::song::cues).unwrap_or_default(),
        words_estimated: false,
        safe: [0.0, 0.0, 1.0, 1.0],
        shaders,
    };
    let thread = ScriptThread::start(src, inputs, Audio::silent(), true)?;
    thread.frame(0.0).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::loads;

    #[test]
    fn a_scene_loads_or_says_why_not() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, text).unwrap();
            p
        };
        let good = write(
            "good.noh-motion.rhai",
            "// noh-motion 0\nfn frame(t, ctx) { draw(rect(0.1, 0.1).at(0.5, 0.5)); }\n",
        );
        assert_eq!(loads(&good, None, (640, 360), 10.0), Ok(()));
        let no_line = write("no-line.rhai", "fn frame(t, ctx) { }\n");
        assert!(
            loads(&no_line, None, (640, 360), 10.0)
                .unwrap_err()
                .contains("first line")
        );
        let broken = write(
            "broken.rhai",
            "// noh-motion 0\nfn frame(t, ctx) { draw(; }\n",
        );
        assert!(loads(&broken, None, (640, 360), 10.0).is_err());
        let missing = dir.path().join("missing.rhai");
        assert!(loads(&missing, None, (640, 360), 10.0).is_err());
        // Over the cap the export reads scenes with: refused here too, unread.
        let big = format!(
            "// noh-motion 0
fn frame(t, ctx) {{ }}
{}",
            " ".repeat(super::super::layer::MAX_SCENE_BYTES as usize)
        );
        let too_big = write("big.rhai", &big);
        assert_eq!(
            loads(&too_big, None, (640, 360), 10.0),
            super::super::layer::read_scene(&too_big).map(|_| ())
        );
        assert!(loads(&too_big, None, (640, 360), 10.0).is_err());
    }
}
