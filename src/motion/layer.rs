//! A validated scene as the overlay of an export or of a look: its frames come
//! checked from the script thread (a frame over a cap is an error naming the
//! instant and the cap: the export fails, nothing is published), and are painted
//! by the engine and stacked by the compositor (`paint::draw_frame`), the same
//! drawing as every other path.
use super::draw::Block;
use super::paint::{MOTION_HOOKS, Painter, draw_frame};
use super::thread::ScriptThread;
use crate::fx::compose::Compositor;
use crate::fx::export::Overlay;
use crate::fx::gl::GlRenderer;
use crate::fx::text::Fonts;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// Largest scene file read.
pub const MAX_SCENE_BYTES: u64 = 512 * 1024;

/// The scene's text, read once (a scene is a small text file).
pub fn read_scene(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_SCENE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("cannot read the scene: {e}"))?;
    if bytes.len() as u64 > MAX_SCENE_BYTES {
        return Err(format!(
            "the scene is larger than {} KiB",
            MAX_SCENE_BYTES / 1024
        ));
    }
    String::from_utf8(bytes).map_err(|_| "the scene is not UTF-8 text".to_string())
}

pub struct MotionLayer {
    thread: ScriptThread,
    painter: Painter,
    comp: Option<Compositor>,
    blocks: Vec<(Block, f32)>,
    /// Added to each frame's time: the scene's clock (a clip of the source
    /// starts at zero on FFmpeg's clock, later on the song's).
    offset: f64,
}

impl MotionLayer {
    /// The overlay of a scene validated on `thread`, for frames of `w` x `h`.
    pub fn new(thread: ScriptThread, (w, h): (u32, u32), offset: f64) -> Result<Self, String> {
        let fonts = Rc::new(RefCell::new(Fonts::load()?));
        let blocks = thread.blocks()?;
        Ok(Self {
            thread,
            painter: Painter::new(w, h, fonts),
            comp: None,
            blocks,
            offset,
        })
    }
}

impl MotionLayer {
    /// The last overlay drawn, as premultiplied, display-encoded RGBA, top row
    /// first (looks and tests).
    pub fn read(&self) -> Vec<u8> {
        self.comp.as_ref().map(Compositor::read).unwrap_or_default()
    }
}

impl Overlay for MotionLayer {
    fn setup(&mut self, gpu: &mut GlRenderer) -> Result<(), String> {
        gpu.set_hooks(Some(MOTION_HOOKS), [0.0; 16])?;
        self.comp = Some(Compositor::new(gpu)?);
        // Every letter the validation saw, at the largest scale it reached.
        self.painter.prepare(&*gpu, &self.blocks)?;
        Ok(())
    }

    fn draw(&mut self, gpu: &mut GlRenderer, t: f64) -> Result<glow::Texture, String> {
        let t = t + self.offset;
        let (f, _) = self.thread.frame(t)?;
        let comp = self
            .comp
            .as_mut()
            .ok_or("the scene's overlay is not set up")?;
        draw_frame(gpu, &mut self.painter, comp, &f.draws, &f.layers, t)?;
        Ok(comp.texture())
    }
}
