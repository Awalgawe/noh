//! A validated scene as the overlay of an export or of a look: its frames come
//! checked from the script thread (a frame over a cap is an error naming the
//! instant and the cap: the export fails, nothing is published), and are painted
//! by the engine and stacked by the compositor (`paint::draw_frame`), the same
//! drawing as every other path.
use super::thread::ScriptThread;
use crate::fx::compose::Compositor;
use crate::fx::draw::Block;
use crate::fx::export::Overlay;
use crate::fx::gl::GlRenderer;
use crate::fx::layer_shader::ShaderSet;
use crate::fx::paint::{MOTION_HOOKS, Painter, Shaders, draw_frame_with};
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
    /// The layer shaders beside the scene, validated, and once set up their
    /// programs, compiled on the engine's context (in the worker: the trial
    /// compilation the window relies on).
    sources: Vec<(String, String)>,
    shaders: Option<ShaderSet>,
    /// Whether shader layers are drawn (stills and the export) or left out
    /// and counted (the window's preview by default).
    show_shaders: bool,
    /// What the last frame drew.
    pub stats: crate::fx::paint::PaintStats,
    /// How the last frame framed the picture, the frame's size and the source
    /// picture's.
    video: Option<crate::fx::reframe::VideoDraw>,
    size: (u32, u32),
    source: (u32, u32),
}

/// GL_CONTEXT_LOST: what every call answers once the driver has reset a
/// robust context.
const CONTEXT_LOST: u32 = 0x0507;

impl MotionLayer {
    /// The overlay of a scene validated on `thread`, for frames of `w` x `h`,
    /// with the shaders read beside it (`shader::read_beside`).
    pub fn new(
        thread: ScriptThread,
        (w, h): (u32, u32),
        offset: f64,
        sources: Vec<(String, String)>,
    ) -> Result<Self, String> {
        let fonts = Rc::new(RefCell::new(Fonts::load()?));
        let blocks = thread.blocks()?;
        Ok(Self {
            thread,
            painter: Painter::new(w, h, fonts),
            comp: None,
            blocks,
            offset,
            sources,
            shaders: None,
            show_shaders: true,
            stats: Default::default(),
            video: None,
            size: (w, h),
            source: (w, h),
        })
    }

    /// The source picture is `source` pixels, not the frame's size: a frame of
    /// another shape (a vertical frame from a wide video) is made from it by
    /// the scene's framing, or by covering it, centred, without one.
    pub fn with_source(mut self, source: (u32, u32)) -> Self {
        self.source = source;
        self
    }

    /// The framing of the last frame drawn: the scene's `video()`, or covering
    /// and centred when the frame has another shape than the picture.
    pub fn framing(&self) -> Option<crate::fx::reframe::Framing> {
        let size = (self.size.0 as f32, self.size.1 as f32);
        let src = (self.source.0 as f32, self.source.1 as f32);
        match &self.video {
            Some(v) => Some(v.framing(size, src)),
            None if self.source != self.size => {
                Some(crate::fx::reframe::VideoDraw::default().framing(size, src))
            }
            None => None,
        }
    }

    /// Leave shader layers out (counted in `stats`), or draw them.
    pub fn show_shaders(&mut self, show: bool) {
        self.show_shaders = show;
    }

    /// How the last frame drawn framed the picture.
    pub fn video(&self) -> Option<&crate::fx::reframe::VideoDraw> {
        self.video.as_ref()
    }

    /// The last overlay drawn, until the next `draw` (a window shows it again
    /// while its clock has not moved).
    pub fn texture(&self) -> Option<glow::Texture> {
        self.comp.as_ref().map(Compositor::texture)
    }

    /// The compiled shaders, once set up.
    pub fn shader_set(&self) -> Option<&ShaderSet> {
        self.shaders.as_ref()
    }

    /// Draw one frame made of an empty layer running shader `name` at full
    /// resolution (the check's trial draw and its timing).
    pub fn draw_shader_alone(&mut self, gpu: &mut GlRenderer, name: &str) -> Result<(), String> {
        let span = crate::fx::draw::Span {
            first: 0,
            len: 0,
            opacity: 1.0,
            shader: Some(crate::fx::draw::ShaderSpec {
                name: name.into(),
                full: true,
                k: [0.0; 8],
            }),
        };
        let set = self
            .shaders
            .as_ref()
            .ok_or("the shaders are not compiled")?;
        let comp = self
            .comp
            .as_mut()
            .ok_or("the scene's overlay is not set up")?;
        let shaders = Shaders::Draw {
            set,
            energy: 0.5,
            onset_age: 0.25,
        };
        draw_frame_with(gpu, &mut self.painter, comp, &[], &[span], 0.0, shaders)?;
        lost(gpu)
    }
}

/// An error if the driver reset the context (a robust context reports it).
fn lost(gpu: &GlRenderer) -> Result<(), String> {
    use glow::HasContext;
    let e = unsafe { gpu.context().get_error() };
    if e == CONTEXT_LOST {
        return Err(
            "the graphics driver was reset while drawing the scene (a layer shader may be too heavy for this computer)"
                .into(),
        );
    }
    Ok(())
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
        // The shaders are compiled here, on the drawing context.
        self.shaders = Some(ShaderSet::compile(gpu.context(), &self.sources)?);
        lost(gpu)
    }

    fn draw(&mut self, gpu: &mut GlRenderer, t: f64) -> Result<glow::Texture, String> {
        let t = t + self.offset;
        let (f, _) = self.thread.frame(t)?;
        let comp = self
            .comp
            .as_mut()
            .ok_or("the scene's overlay is not set up")?;
        let set = self
            .shaders
            .as_ref()
            .ok_or("the shaders are not compiled")?;
        let shaders = if self.show_shaders {
            Shaders::Draw {
                set,
                energy: f.energy,
                onset_age: f.onset_age,
            }
        } else {
            Shaders::Hide
        };
        self.stats = draw_frame_with(
            gpu,
            &mut self.painter,
            comp,
            &f.draws,
            &f.layers,
            t,
            shaders,
        )?;
        lost(gpu)?;
        self.video = f.frame.video;
        Ok(comp.texture())
    }

    fn reframe(&self) -> Option<crate::fx::gl::Reframe> {
        self.framing().map(|f| crate::fx::gl::Reframe {
            inv: f.inverse(),
            center: f.center,
            focus: f.focus,
            crop: f.crop,
        })
    }
}
