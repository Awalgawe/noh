//! A texture drawn through four corners: how a preview shows its own picture
//! reframed by a motion scene's `video()` (`crate::fx::reframe::Framing::quad`).
//! libmpv renders the whole picture, unframed, into a texture; this draws it
//! as a textured quad, transformed, into the preview's target, black around it.
//! Positions are pixels of the target from its top left; texture coordinates
//! run from the top left of the image, rows stored top first.
use glow::HasContext;
use std::sync::Arc;

const VS: &str = "#version 330 core
layout(location = 0) in vec2 pos;
layout(location = 1) in vec2 uv;
uniform vec2 size;
out vec2 v_uv;
void main() {
  v_uv = uv;
  gl_Position = vec4(pos / size * 2.0 - 1.0, 0.0, 1.0);
}";

const FS: &str = "#version 330 core
uniform sampler2D src;
in vec2 v_uv;
out vec4 o;
void main() {
  o = texture(src, v_uv);
}";

pub struct QuadPainter {
    gl: Arc<glow::Context>,
    prog: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    linear: glow::Sampler,
}

fn err<E: std::fmt::Debug>(what: &'static str) -> impl Fn(E) -> String {
    move |e| format!("quad {what}: {e:?}")
}

impl QuadPainter {
    pub fn new(gl: &Arc<glow::Context>) -> Result<Self, String> {
        unsafe {
            let prog = gl.create_program().map_err(err("program"))?;
            for (kind, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, FS)] {
                let sh = gl.create_shader(kind).map_err(err("shader"))?;
                gl.shader_source(sh, src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    return Err(format!("quad shader: {}", gl.get_shader_info_log(sh)));
                }
                gl.attach_shader(prog, sh);
                gl.delete_shader(sh);
            }
            gl.link_program(prog);
            if !gl.get_program_link_status(prog) {
                return Err(format!("quad link: {}", gl.get_program_info_log(prog)));
            }
            let vao = gl.create_vertex_array().map_err(err("vertex array"))?;
            let vbo = gl.create_buffer().map_err(err("buffer"))?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            let linear = gl.create_sampler().map_err(err("sampler"))?;
            for (k, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.sampler_parameter_i32(linear, k, v as i32);
            }
            Ok(Self {
                gl: gl.clone(),
                prog,
                vao,
                vbo,
                linear,
            })
        }
    }

    /// Clear `target` (of `size` pixels) to black and draw `texture` through
    /// `quad`: corners in order around the quad, each its position and its
    /// texture coordinate.
    pub fn draw(
        &self,
        target: Option<glow::Framebuffer>,
        size: (i32, i32),
        texture: glow::Texture,
        quad: [([f32; 2], [f32; 2]); 4],
    ) -> Result<(), String> {
        let gl = &self.gl;
        let mut v = Vec::with_capacity(24);
        for k in [0, 1, 2, 0, 2, 3] {
            let (p, uv) = quad[k];
            v.extend_from_slice(&[p[0], p[1], uv[0], uv[1]]);
        }
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, target);
            gl.viewport(0, 0, size.0, size.1);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.prog));
            gl.uniform_2_f32(
                gl.get_uniform_location(self.prog, "size").as_ref(),
                size.0 as f32,
                size.1 as f32,
            );
            gl.uniform_1_i32(gl.get_uniform_location(self.prog, "src").as_ref(), 0);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.bind_sampler(0, Some(self.linear));
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&v),
                glow::STREAM_DRAW,
            );
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_sampler(0, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            let e = gl.get_error();
            if e != glow::NO_ERROR {
                return Err(format!("GL error 0x{e:x} drawing the reframed picture"));
            }
        }
        Ok(())
    }
}

impl Drop for QuadPainter {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.prog);
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_sampler(self.linear);
        }
    }
}
