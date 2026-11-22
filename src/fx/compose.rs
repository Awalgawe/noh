//! Stacks several frames of the engine into one overlay (the passes of a motion
//! scene: each layer, and each run of loose draws between two layers). Each
//! pass is read from the engine's own half-float layer (`GlRenderer::layer_texture`:
//! premultiplied colour in linear light), so cutting a scene into layers keeps
//! every pass's precision; the stack is encoded for display once, at the end,
//! with the engine's own formula. The result is the same kind of texture as the
//! engine's premultiplied output: what the preview presents and what `YuvMix`
//! mixes into the video.
use super::gl::GlRenderer;
use glow::HasContext;
use std::sync::Arc;

const VS: &str = "#version 330 core
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}";

/// One texel of a pass (premultiplied linear light) onto the stack, times the
/// layer's opacity; the blend puts it over what is already there.
const FS_STACK: &str = "#version 330 core
uniform sampler2D src;
uniform float opacity;
out vec4 o;
void main() {
  o = texelFetch(src, ivec2(gl_FragCoord.xy), 0) * opacity;
}";

/// The stack encoded for display, premultiplied, as the engine encodes its own
/// layer for the preview (`shaders::RESOLVE_PREMUL`).
const FS_OUT: &str = "#version 330 core
uniform sampler2D src;
out vec4 o;
float enc(float c) {
  return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}
void main() {
  vec4 c = texelFetch(src, ivec2(gl_FragCoord.xy), 0);
  if (c.a <= 0.00001) { o = vec4(0.0); return; }
  float a = clamp(c.a, 0.0, 1.0);
  vec3 s = clamp(c.rgb / c.a, 0.0, 1.0);
  o = vec4(vec3(enc(s.r), enc(s.g), enc(s.b)) * a, a);
}";

struct Target {
    tex: glow::Texture,
    fbo: glow::Framebuffer,
}

pub struct Compositor {
    gl: Arc<glow::Context>,
    w: i32,
    h: i32,
    /// Premultiplied linear light, half float.
    stack: Target,
    /// Display-encoded, premultiplied, 8 bits per channel.
    out: Target,
    stack_prog: glow::Program,
    stack_opacity: Option<glow::UniformLocation>,
    out_prog: glow::Program,
    vao: glow::VertexArray,
}

fn err<E: std::fmt::Debug>(what: &'static str) -> impl Fn(E) -> String {
    move |e| format!("compositor {what}: {e:?}")
}

unsafe fn target(
    gl: &glow::Context,
    w: i32,
    h: i32,
    internal: u32,
    ty: u32,
) -> Result<Target, String> {
    unsafe {
        let tex = gl.create_texture().map_err(err("texture"))?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            internal as i32,
            w,
            h,
            0,
            glow::RGBA,
            ty,
            glow::PixelUnpackData::Slice(None),
        );
        for (k, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        let fbo = gl.create_framebuffer().map_err(err("framebuffer"))?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(tex),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            gl.delete_texture(tex);
            return Err(format!("compositor framebuffer incomplete: 0x{status:x}"));
        }
        Ok(Target { tex, fbo })
    }
}

unsafe fn program(gl: &glow::Context, fs: &str) -> Result<glow::Program, String> {
    unsafe {
        let prog = gl.create_program().map_err(err("program"))?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, fs)] {
            let s = gl.create_shader(kind).map_err(err("shader"))?;
            gl.shader_source(s, src);
            gl.compile_shader(s);
            if !gl.get_shader_compile_status(s) {
                let log = gl.get_shader_info_log(s);
                gl.delete_shader(s);
                return Err(format!("compositor shader: {log}"));
            }
            gl.attach_shader(prog, s);
            shaders.push(s);
        }
        gl.link_program(prog);
        for s in shaders {
            gl.detach_shader(prog, s);
            gl.delete_shader(s);
        }
        if !gl.get_program_link_status(prog) {
            return Err(format!(
                "compositor link: {}",
                gl.get_program_info_log(prog)
            ));
        }
        Ok(prog)
    }
}

impl Compositor {
    /// Targets the size of `r`'s frame, on its context (which must be current).
    /// GPU memory: one half-float and one 8-bit target, 12 bytes per pixel
    /// (about 24 MiB at 1920 x 1080), beside the engine's own.
    pub fn new(r: &GlRenderer) -> Result<Self, String> {
        let gl = r.context().clone();
        let (w, h) = (r.width as i32, r.height as i32);
        unsafe {
            let stack = target(&gl, w, h, glow::RGBA16F, glow::HALF_FLOAT)?;
            let out = target(&gl, w, h, glow::RGBA8, glow::UNSIGNED_BYTE)?;
            let stack_prog = program(&gl, FS_STACK)?;
            let out_prog = program(&gl, FS_OUT)?;
            let stack_opacity = gl.get_uniform_location(stack_prog, "opacity");
            let vao = gl.create_vertex_array().map_err(err("vertex array"))?;
            Ok(Self {
                gl,
                w,
                h,
                stack,
                out,
                stack_prog,
                stack_opacity,
                out_prog,
                vao,
            })
        }
    }

    /// GPU memory this compositor holds, in bytes.
    pub fn bytes(&self) -> usize {
        self.w as usize * self.h as usize * (8 + 4)
    }

    fn draw(&self, fbo: glow::Framebuffer, prog: glow::Program, src: glow::Texture) {
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.viewport(0, 0, self.w, self.h);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.use_program(Some(prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(src));
            gl.bind_sampler(0, None);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    /// Starts a frame: the stack is cleared.
    pub fn begin(&mut self) {
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(self.stack.fbo));
            self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    /// Puts one pass over the stack, faded by `opacity` as one picture.
    /// `layer` is premultiplied linear light, the size of the stack.
    pub fn stack(&mut self, layer: glow::Texture, opacity: f32) {
        let gl = &self.gl;
        unsafe {
            gl.use_program(Some(self.stack_prog));
            gl.uniform_1_f32(self.stack_opacity.as_ref(), opacity.clamp(0.0, 1.0));
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }
        self.draw(self.stack.fbo, self.stack_prog, layer);
        unsafe { gl.disable(glow::BLEND) };
    }

    /// Ends a frame: the stack is encoded into the output texture.
    pub fn finish(&mut self) {
        self.draw(self.out.fbo, self.out_prog, self.stack.tex);
        unsafe { self.gl.flush() };
    }

    /// The finished overlay: premultiplied, display-encoded RGBA8, top row first.
    pub fn texture(&self) -> glow::Texture {
        self.out.tex
    }

    /// The finished overlay's bytes (tests and stills).
    pub fn read(&self) -> Vec<u8> {
        let mut px = vec![0u8; self.w as usize * self.h as usize * 4];
        unsafe {
            self.gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.out.fbo));
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            self.gl.read_pixels(
                0,
                0,
                self.w,
                self.h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut px)),
            );
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            self.gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        }
        px
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        unsafe {
            for t in [&self.stack, &self.out] {
                self.gl.delete_framebuffer(t.fbo);
                self.gl.delete_texture(t.tex);
            }
            self.gl.delete_program(self.stack_prog);
            self.gl.delete_program(self.out_prog);
            self.gl.delete_vertex_array(self.vao);
        }
    }
}
