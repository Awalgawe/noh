//! Engine selection for command-line runs: wgpu (reference) or OpenGL (plan-moteur-gl).
//! `NOH_PROTO_ENGINE=gl` picks OpenGL; anything else keeps wgpu.
use crate::gl::GlRenderer;
use crate::gpu::{FrameTimes, Inst, Output, Renderer, Upload};

pub enum Engine {
    Wgpu(Renderer),
    Gl(GlRenderer),
}

pub fn gl_selected() -> bool {
    std::env::var("NOH_PROTO_ENGINE").as_deref() == Ok("gl")
}

impl Engine {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        Self::with_canvas(width, height, width as f32, height as f32, Output::StraightSrgb)
    }

    pub fn with_canvas(width: u32, height: u32, logical_w: f32, logical_h: f32, out: Output) -> Result<Self, String> {
        if gl_selected() {
            GlRenderer::headless(width, height, logical_w, logical_h, out).map(Engine::Gl)
        } else {
            Renderer::with_canvas(width, height, logical_w, logical_h, out).map(Engine::Wgpu)
        }
    }

    pub fn frame(&mut self, glow: &[Inst], gain: f32, main: &[Inst], read: bool) -> Result<(Vec<u8>, FrameTimes), String> {
        match self {
            Engine::Wgpu(r) => r.frame(glow, gain, main, read),
            Engine::Gl(r) => r.frame(glow, gain, main, read),
        }
    }

    pub fn set_hooks(&mut self, src: Option<&str>, user: [f32; 16]) -> Result<(), String> {
        match self {
            Engine::Wgpu(r) => r.set_hooks(src, user),
            Engine::Gl(r) => r.set_hooks(src, user),
        }
    }

    pub fn adapter(&self) -> String {
        match self {
            Engine::Wgpu(r) => r.adapter.clone(),
            Engine::Gl(r) => r.adapter.clone(),
        }
    }

    pub fn setup_ms(&self) -> f64 {
        match self {
            Engine::Wgpu(r) => r.setup_ms,
            Engine::Gl(r) => r.setup_ms,
        }
    }
}

impl Upload for Engine {
    fn upload_cov(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        match self {
            Engine::Wgpu(r) => r.upload_cov(x, y, w, h, data),
            Engine::Gl(r) => r.upload_cov(x, y, w, h, data),
        }
    }
    fn upload_msdf(&self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        match self {
            Engine::Wgpu(r) => r.upload_msdf(x, y, w, h, rgba),
            Engine::Gl(r) => r.upload_msdf(x, y, w, h, rgba),
        }
    }
}
