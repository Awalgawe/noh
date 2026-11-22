//! Stacks several frames of the engine into one overlay (the passes of a motion
//! scene: each layer, and each run of loose draws between two layers). Each
//! pass is read from the engine's own half-float layer (`GlRenderer::layer_texture`:
//! premultiplied colour in linear light), so cutting a scene into layers keeps
//! every pass's precision; the stack is encoded for display once, at the end,
//! with the engine's own formula. The result is the same kind of texture as the
//! engine's premultiplied output: what the preview presents and what `YuvMix`
//! mixes into the video.
//!
//! Two more kinds of pass: a run of vector shapes (already cut into triangles),
//! drawn with 4 samples per pixel and resolved; and a group, the passes of one
//! layer stacked on their own first, so that the layer still fades as one
//! picture when it holds both text and shapes. Both use targets made at their
//! first use only.
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

/// Samples per pixel of the shapes' pass.
pub const SHAPE_SAMPLES: i32 = 4;

const VS_SHAPE: &str = "#version 330 core
layout(location = 0) in vec2 pos;
uniform vec2 canvas;
void main() {
  gl_Position = vec4(pos / canvas * 2.0 - 1.0, 0.0, 1.0);
}";

/// A shape's paint: a solid colour, or a linear or radial gradient whose stops
/// are mixed in display encoding (as a designer's tools mix them), then decoded
/// to linear light and premultiplied. Gradient places are in canvas pixels.
const FS_SHAPE: &str = "#version 330 core
uniform int mode;
uniform vec4 color;
uniform vec2 g0;
uniform vec2 g1;
uniform float gr;
uniform int nstops;
uniform float soff[8];
uniform vec4 scol[8];
uniform float opacity;
uniform vec2 px_per_canvas;
out vec4 o;
float dec(float c) {
  return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}
void main() {
  vec4 c = color;
  if (mode != 0) {
    vec2 p = gl_FragCoord.xy / px_per_canvas;
    float t;
    if (mode == 1) {
      vec2 d = g1 - g0;
      t = dot(p - g0, d) / max(dot(d, d), 1e-6);
    } else {
      t = length(p - g0) / max(gr, 1e-6);
    }
    t = clamp(t, 0.0, 1.0);
    c = scol[0];
    for (int i = 1; i < 8; i++) {
      if (i >= nstops) break;
      if (t >= soff[i]) { c = scol[i]; }
      else if (t > soff[i - 1]) {
        c = mix(scol[i - 1], scol[i], (t - soff[i - 1]) / max(soff[i] - soff[i - 1], 1e-6));
        break;
      }
    }
  }
  float a = clamp(c.a * opacity, 0.0, 1.0);
  o = vec4(dec(c.r) * a, dec(c.g) * a, dec(c.b) * a, a);
}";

/// How a shape's fill is painted. Colours are display-encoded red, green, blue
/// (0..1) and an opacity; gradient places are canvas pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Solid([f32; 4]),
    Linear {
        from: [f32; 2],
        to: [f32; 2],
        stops: Vec<(f32, [f32; 4])>,
    },
    Radial {
        center: [f32; 2],
        radius: f32,
        stops: Vec<(f32, [f32; 4])>,
    },
}

/// One shape of a shapes' pass: its fill and its stroke, cut into triangles
/// (canvas pixels, three corners each).
pub struct Shape<'a> {
    pub fill: &'a [[f32; 2]],
    pub paint: Fill,
    pub stroke: &'a [[f32; 2]],
    pub stroke_color: [f32; 4],
    pub opacity: f32,
}

struct ShapeLocs {
    canvas: Option<glow::UniformLocation>,
    px_per_canvas: Option<glow::UniformLocation>,
    mode: Option<glow::UniformLocation>,
    color: Option<glow::UniformLocation>,
    g0: Option<glow::UniformLocation>,
    g1: Option<glow::UniformLocation>,
    gr: Option<glow::UniformLocation>,
    nstops: Option<glow::UniformLocation>,
    soff: Option<glow::UniformLocation>,
    scol: Option<glow::UniformLocation>,
    opacity: Option<glow::UniformLocation>,
}

/// The shapes' pass, made at its first use: a multisampled target, its resolve
/// (premultiplied linear light, half float), a program and a vertex buffer.
struct ShapePass {
    ms_fbo: glow::Framebuffer,
    ms_color: glow::Renderbuffer,
    ms_ds: glow::Renderbuffer,
    resolved: Target,
    prog: glow::Program,
    locs: ShapeLocs,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
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
    /// A group's own stack (a layer made of several passes), at first use.
    group: Option<Target>,
    in_group: bool,
    shapes: Option<ShapePass>,
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
    unsafe { program_with(gl, VS, fs) }
}

unsafe fn program_with(gl: &glow::Context, vs: &str, fs: &str) -> Result<glow::Program, String> {
    unsafe {
        let prog = gl.create_program().map_err(err("program"))?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
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
                group: None,
                in_group: false,
                shapes: None,
            })
        }
    }

    /// GPU memory this compositor holds now, in bytes: the stack and the output
    /// (12 bytes per pixel); a group's stack once a layer held several passes
    /// (8 more); the shapes' pass once shapes were drawn (4 samples of colour and
    /// depth-stencil, 48, and their resolve, 8). About 24 MiB at 1920 x 1080,
    /// 40 with a group, 150 with a group and shapes.
    pub fn bytes(&self) -> usize {
        let px = self.w as usize * self.h as usize;
        let group = if self.group.is_some() { px * 8 } else { 0 };
        let shapes = if self.shapes.is_some() {
            px * (SHAPE_SAMPLES as usize * (8 + 4) + 8)
        } else {
            0
        };
        px * (8 + 4) + group + shapes
    }

    /// Where passes are stacked: the open group's stack, or the frame's.
    fn target(&self) -> glow::Framebuffer {
        match (&self.group, self.in_group) {
            (Some(g), true) => g.fbo,
            _ => self.stack.fbo,
        }
    }

    /// Starts a group: the next passes stack on their own, and go on the
    /// frame's stack as one picture at `end_group`.
    pub fn begin_group(&mut self) -> Result<(), String> {
        if self.group.is_none() {
            self.group =
                Some(unsafe { target(&self.gl, self.w, self.h, glow::RGBA16F, glow::HALF_FLOAT)? });
        }
        let fbo = self.group.as_ref().map(|g| g.fbo);
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, fbo);
            self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.in_group = true;
        Ok(())
    }

    /// Ends a group: its stack goes on the frame's, faded by `opacity`.
    pub fn end_group(&mut self, opacity: f32) {
        self.in_group = false;
        if let Some(tex) = self.group.as_ref().map(|g| g.tex) {
            self.stack(tex, opacity);
        }
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

    /// Starts a frame: the stack is cleared, and a group a failed frame left
    /// open is closed.
    pub fn begin(&mut self) {
        self.in_group = false;
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
        self.draw(self.target(), self.stack_prog, layer);
        unsafe { gl.disable(glow::BLEND) };
    }

    fn make_shape_pass(&self) -> Result<ShapePass, String> {
        let gl = &self.gl;
        unsafe {
            let ms_color = gl.create_renderbuffer().map_err(err("renderbuffer"))?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(ms_color));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                SHAPE_SAMPLES,
                glow::RGBA16F,
                self.w,
                self.h,
            );
            let ms_ds = gl.create_renderbuffer().map_err(err("renderbuffer"))?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(ms_ds));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                SHAPE_SAMPLES,
                glow::DEPTH24_STENCIL8,
                self.w,
                self.h,
            );
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            let ms_fbo = gl.create_framebuffer().map_err(err("framebuffer"))?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(ms_fbo));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(ms_color),
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_STENCIL_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(ms_ds),
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(ms_fbo);
                gl.delete_renderbuffer(ms_color);
                gl.delete_renderbuffer(ms_ds);
                return Err(format!("shapes framebuffer incomplete: 0x{status:x}"));
            }
            let resolved = target(gl, self.w, self.h, glow::RGBA16F, glow::HALF_FLOAT)?;
            let prog = program_with(gl, VS_SHAPE, FS_SHAPE)?;
            let loc = |n: &str| gl.get_uniform_location(prog, n);
            let locs = ShapeLocs {
                canvas: loc("canvas"),
                px_per_canvas: loc("px_per_canvas"),
                mode: loc("mode"),
                color: loc("color"),
                g0: loc("g0"),
                g1: loc("g1"),
                gr: loc("gr"),
                nstops: loc("nstops"),
                soff: loc("soff"),
                scol: loc("scol"),
                opacity: loc("opacity"),
            };
            let vao = gl.create_vertex_array().map_err(err("vertex array"))?;
            let vbo = gl.create_buffer().map_err(err("buffer"))?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(ShapePass {
                ms_fbo,
                ms_color,
                ms_ds,
                resolved,
                prog,
                locs,
                vao,
                vbo,
            })
        }
    }

    /// Draws a run of shapes as one pass (4 samples per pixel, each primitive
    /// covering a sample once, so the overlapping joins of a translucent stroke
    /// do not darken), and puts it on the current stack faded by `opacity`.
    /// `canvas` is the canvas the shapes' pixels are given in.
    pub fn draw_shapes(
        &mut self,
        shapes: &[Shape],
        canvas: (f32, f32),
        opacity: f32,
    ) -> Result<(), String> {
        if self.shapes.is_none() {
            self.shapes = Some(self.make_shape_pass()?);
        }
        let (w, h) = (self.w, self.h);
        let mut verts: Vec<f32> = Vec::new();
        let mut ranges = Vec::with_capacity(shapes.len());
        for s in shapes {
            let f0 = verts.len() / 2;
            verts.extend(s.fill.iter().flatten());
            let s0 = verts.len() / 2;
            verts.extend(s.stroke.iter().flatten());
            ranges.push((f0, s0 - f0, s0, verts.len() / 2 - s0));
        }
        let gl = &self.gl;
        let pass = self.shapes.as_ref().expect("made above");
        let lc = &pass.locs;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(pass.ms_fbo));
            gl.viewport(0, 0, w, h);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.color_mask(true, true, true, true);
            gl.stencil_mask(0xFF);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear_stencil(0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.enable(glow::STENCIL_TEST);
            gl.stencil_op(glow::KEEP, glow::KEEP, glow::REPLACE);
            gl.use_program(Some(pass.prog));
            gl.uniform_2_f32(lc.canvas.as_ref(), canvas.0, canvas.1);
            gl.uniform_2_f32(
                lc.px_per_canvas.as_ref(),
                w as f32 / canvas.0,
                h as f32 / canvas.1,
            );
            gl.bind_vertex_array(Some(pass.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(pass.vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&verts),
                glow::STREAM_DRAW,
            );
            // Each primitive stamps the stencil with its own value: a sample it
            // already covered is not painted twice. The stencil holds 8 bits, so
            // past 255 primitives it starts over (GL would clamp a larger stamp
            // to 255, and a primitive past it would not cover another).
            let mut stamp = 0i32;
            let mut draw = |first: usize, count: usize| {
                if stamp == 255 {
                    gl.clear(glow::STENCIL_BUFFER_BIT);
                    stamp = 0;
                }
                stamp += 1;
                gl.stencil_func(glow::NOTEQUAL, stamp, 0xFF);
                gl.draw_arrays(glow::TRIANGLES, first as i32, count as i32);
            };
            for (s, &(f0, fnum, s0, snum)) in shapes.iter().zip(&ranges) {
                gl.uniform_1_f32(lc.opacity.as_ref(), s.opacity.clamp(0.0, 1.0));
                if fnum > 0 {
                    match &s.paint {
                        Fill::Solid(c) => {
                            gl.uniform_1_i32(lc.mode.as_ref(), 0);
                            gl.uniform_4_f32(lc.color.as_ref(), c[0], c[1], c[2], c[3]);
                        }
                        Fill::Linear { from, to, stops } => {
                            gl.uniform_1_i32(lc.mode.as_ref(), 1);
                            gl.uniform_2_f32(lc.g0.as_ref(), from[0], from[1]);
                            gl.uniform_2_f32(lc.g1.as_ref(), to[0], to[1]);
                            set_stops(gl, lc, stops);
                        }
                        Fill::Radial {
                            center,
                            radius,
                            stops,
                        } => {
                            gl.uniform_1_i32(lc.mode.as_ref(), 2);
                            gl.uniform_2_f32(lc.g0.as_ref(), center[0], center[1]);
                            gl.uniform_1_f32(lc.gr.as_ref(), *radius);
                            set_stops(gl, lc, stops);
                        }
                    }
                    draw(f0, fnum);
                }
                if snum > 0 {
                    let c = s.stroke_color;
                    gl.uniform_1_i32(lc.mode.as_ref(), 0);
                    gl.uniform_4_f32(lc.color.as_ref(), c[0], c[1], c[2], c[3]);
                    draw(s0, snum);
                }
            }
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.use_program(None);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(pass.ms_fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(pass.resolved.fbo));
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                0,
                0,
                w,
                h,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        let resolved = pass.resolved.tex;
        self.stack(resolved, opacity);
        let e = unsafe { self.gl.get_error() };
        if e != glow::NO_ERROR {
            return Err(format!("GL error 0x{e:x} drawing shapes"));
        }
        Ok(())
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

unsafe fn set_stops(gl: &glow::Context, lc: &ShapeLocs, stops: &[(f32, [f32; 4])]) {
    let mut off = [0f32; 8];
    let mut col = [0f32; 32];
    for (i, (o, c)) in stops.iter().take(8).enumerate() {
        off[i] = *o;
        col[4 * i..4 * i + 4].copy_from_slice(c);
    }
    unsafe {
        gl.uniform_1_i32(lc.nstops.as_ref(), stops.len().min(8) as i32);
        gl.uniform_1_f32_slice(lc.soff.as_ref(), &off);
        gl.uniform_4_f32_slice(lc.scol.as_ref(), &col);
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        unsafe {
            if let Some(g) = self.group.take() {
                self.gl.delete_framebuffer(g.fbo);
                self.gl.delete_texture(g.tex);
            }
            if let Some(s) = self.shapes.take() {
                self.gl.delete_framebuffer(s.ms_fbo);
                self.gl.delete_renderbuffer(s.ms_color);
                self.gl.delete_renderbuffer(s.ms_ds);
                self.gl.delete_framebuffer(s.resolved.fbo);
                self.gl.delete_texture(s.resolved.tex);
                self.gl.delete_program(s.prog);
                self.gl.delete_vertex_array(s.vao);
                self.gl.delete_buffer(s.vbo);
            }
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
