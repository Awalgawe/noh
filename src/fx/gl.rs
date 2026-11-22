//! OpenGL caption compositor: instanced glyph quads, an optional half-resolution
//! glow layer, and a resolve pass. The WGSL sources of shaders.rs are validated by
//! naga and written as GLSL 330 with ADJUST_COORDINATE_SPACE, so every render target
//! is stored top row first and readbacks need no flip. Also the YUV mixer used by
//! the export (captions drawn into I420 frames on the GPU).
use super::shaders::{COV_ATLAS, FrameTimes, Inst, MSDF_ATLAS, MSDF_RANGE, Output};
use glow::{self, HasContext};
use std::sync::Arc;
use std::time::Instant;

const BLEND_PREMUL: (u32, u32) = (glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
const BLEND_ADD: (u32, u32) = (glow::ONE, glow::ONE);

/// Draws the resolved (premultiplied gamma) output into the current viewport, for
/// the egui preview. Plain GLSL: the default framebuffer is bottom row first.
const PRESENT_VS: &str = "#version 330 core
out vec2 uv;
void main() {
  // One triangle twice the viewport: p runs 0..2, so the viewport is p in 0..1.
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  uv = vec2(p.x, 1.0 - p.y);
  gl_Position = vec4(p.x * 2.0 - 1.0, p.y * 2.0 - 1.0, 0.0, 1.0);
}";
const PRESENT_FS: &str = "#version 330 core
uniform sampler2D tex;
in vec2 uv;
out vec4 color;
void main() { color = texture(tex, uv); }";

struct Target {
    tex: glow::Texture,
    fbo: glow::Framebuffer,
    w: i32,
    h: i32,
}

/// The largest blur sigma drawn, in canvas px: a stray value cannot ask for more.
pub const BLUR_MAX: f32 = 96.0;

/// How main instances are drawn: a run straight onto the layer, or a group of
/// blurred coverage instances (same sigma, physical px x 4) through a blur layer.
enum Draw {
    Direct(usize, usize),
    Blur(u32, Vec<usize>),
}

pub struct GlRenderer {
    gl: Arc<glow::Context>,
    pub width: u32,
    pub height: u32,
    pub adapter: String,
    pub setup_ms: f64,
    cov: glow::Texture,
    msdf: glow::Texture,
    layer: Target,
    glow_a: Target,
    glow_b: Target,
    output: Target,
    out_mode: Output,
    inst_prog: glow::Program,
    blur_prog: glow::Program,
    down_prog: glow::Program,
    comp_prog: glow::Program,
    resolve_prog: glow::Program,
    present_prog: glow::Program,
    sampler: glow::Sampler,
    globals: glow::Buffer,
    blur_h: glow::Buffer,
    blur_v: glow::Buffer,
    comp_ubo: glow::Buffer,
    /// Blur layers by downscale factor (a pair each), made when first needed.
    blurs: Vec<(u32, Target, Target)>,
    blur_dyn_h: glow::Buffer,
    blur_dyn_v: glow::Buffer,
    comp_layer: glow::Buffer,
    /// Physical px per canvas px.
    px_scale: f32,
    inst_vao: glow::VertexArray,
    empty_vao: glow::VertexArray,
    inst_buf: glow::Buffer,
    inst_cap: usize,
    /// TIME_ELAPSED queries, used round robin; `pending` holds those not yet read.
    queries: Vec<glow::Query>,
    next_query: usize,
    pending: std::collections::VecDeque<glow::Query>,
    pub last_gpu_ms: Option<f64>,
}

fn err<E: std::fmt::Debug>(what: &str) -> impl Fn(E) -> String + '_ {
    move |e| format!("{what}: {e:?}")
}

/// Translate one WGSL module to a linked GL program. Bindings follow the WGSL
/// `@binding` numbers: uniform block binding N, texture unit N.
unsafe fn program(
    gl: &glow::Context,
    wgsl: &str,
    vs: &str,
    fs: &str,
) -> Result<glow::Program, String> {
    use naga::back::glsl;
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.emit_to_string(wgsl))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| format!("naga validation: {e:?}"))?;
    let options = glsl::Options {
        version: glsl::Version::Desktop(330),
        writer_flags: glsl::WriterFlags::ADJUST_COORDINATE_SPACE,
        binding_map: Default::default(),
        zero_initialize_workgroup_memory: false,
    };
    let mut blocks = Vec::new();
    let mut textures = Vec::new();
    let mut attribs = Vec::new();
    let mut outputs = Vec::new();
    let prog = unsafe { gl.create_program() }.map_err(err("create program"))?;
    let mut shaders = Vec::new();
    for (stage, entry, kind) in [
        (naga::ShaderStage::Vertex, vs, glow::VERTEX_SHADER),
        (naga::ShaderStage::Fragment, fs, glow::FRAGMENT_SHADER),
    ] {
        let po = glsl::PipelineOptions {
            shader_stage: stage,
            entry_point: entry.into(),
            multiview: None,
        };
        let mut src = String::new();
        let mut w = glsl::Writer::new(
            &mut src,
            &module,
            &info,
            &options,
            &po,
            naga::proc::BoundsCheckPolicies::default(),
        )
        .map_err(err("glsl writer"))?;
        let r = w.write().map_err(err("glsl write"))?;
        for (h, name) in r.uniforms {
            let b = module.global_variables[h]
                .binding
                .as_ref()
                .ok_or("uniform without binding")?
                .binding;
            blocks.push((name, b));
        }
        for (name, m) in r.texture_mapping {
            let b = module.global_variables[m.texture]
                .binding
                .as_ref()
                .ok_or("texture without binding")?
                .binding;
            textures.push((name, b));
        }
        for (name, loc) in r.varying {
            match stage {
                naga::ShaderStage::Vertex => attribs.push((name, loc.location)),
                _ => outputs.push((name, loc.location)),
            }
        }
        unsafe {
            let sh = gl.create_shader(kind).map_err(err("create shader"))?;
            gl.shader_source(sh, &src);
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                return Err(format!(
                    "GLSL compile ({entry}): {}\n{src}",
                    gl.get_shader_info_log(sh)
                ));
            }
            gl.attach_shader(prog, sh);
            shaders.push(sh);
        }
    }
    unsafe {
        for (name, loc) in &attribs {
            gl.bind_attrib_location(prog, *loc, name);
        }
        for (name, loc) in &outputs {
            gl.bind_frag_data_location(prog, *loc, name);
        }
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) {
            return Err(format!("GL link: {}", gl.get_program_info_log(prog)));
        }
        for sh in shaders {
            gl.detach_shader(prog, sh);
            gl.delete_shader(sh);
        }
        gl.use_program(Some(prog));
        for (name, b) in &blocks {
            let idx = gl
                .get_uniform_block_index(prog, name)
                .ok_or_else(|| format!("no uniform block {name}"))?;
            gl.uniform_block_binding(prog, idx, *b);
        }
        for (name, b) in &textures {
            if let Some(loc) = gl.get_uniform_location(prog, name) {
                gl.uniform_1_i32(Some(&loc), *b as i32);
            }
        }
        gl.use_program(None);
    }
    Ok(prog)
}

unsafe fn plain_program(gl: &glow::Context, vs: &str, fs: &str) -> Result<glow::Program, String> {
    unsafe {
        let prog = gl.create_program().map_err(err("create program"))?;
        for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
            let sh = gl.create_shader(kind).map_err(err("create shader"))?;
            gl.shader_source(sh, src);
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                return Err(format!("GLSL compile: {}", gl.get_shader_info_log(sh)));
            }
            gl.attach_shader(prog, sh);
        }
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) {
            return Err(format!("GL link: {}", gl.get_program_info_log(prog)));
        }
        gl.use_program(Some(prog));
        if let Some(loc) = gl.get_uniform_location(prog, "tex") {
            gl.uniform_1_i32(Some(&loc), 0);
        }
        gl.use_program(None);
        Ok(prog)
    }
}

unsafe fn texture(
    gl: &glow::Context,
    w: i32,
    h: i32,
    internal: u32,
    format: u32,
    ty: u32,
) -> Result<glow::Texture, String> {
    unsafe {
        let t = gl.create_texture().map_err(err("create texture"))?;
        gl.bind_texture(glow::TEXTURE_2D, Some(t));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            internal as i32,
            w,
            h,
            0,
            format,
            ty,
            glow::PixelUnpackData::Slice(None),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(t)
    }
}

unsafe fn target(
    gl: &glow::Context,
    w: i32,
    h: i32,
    internal: u32,
    format: u32,
    ty: u32,
) -> Result<Target, String> {
    unsafe {
        let tex = texture(gl, w, h, internal, format, ty)?;
        let fbo = gl.create_framebuffer().map_err(err("create fbo"))?;
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
            return Err(format!("framebuffer incomplete: 0x{status:x}"));
        }
        Ok(Target { tex, fbo, w, h })
    }
}

unsafe fn ubo(gl: &glow::Context, data: &[f32]) -> Result<glow::Buffer, String> {
    unsafe {
        let b = gl.create_buffer().map_err(err("create buffer"))?;
        gl.bind_buffer(glow::UNIFORM_BUFFER, Some(b));
        gl.buffer_data_u8_slice(
            glow::UNIFORM_BUFFER,
            bytemuck::cast_slice(data),
            glow::DYNAMIC_DRAW,
        );
        gl.bind_buffer(glow::UNIFORM_BUFFER, None);
        Ok(b)
    }
}

/// Plan A2 bis: can this context detect a GPU reset? Reads GL_CONTEXT_FLAGS
/// (robust access bit) and the reset notification strategy.
pub fn robustness(gl: &glow::Context) -> String {
    const CONTEXT_FLAG_ROBUST_ACCESS_BIT: i32 = 0x0000_0004;
    const RESET_NOTIFICATION_STRATEGY: u32 = 0x8256;
    const LOSE_CONTEXT_ON_RESET: i32 = 0x8252;
    unsafe {
        let flags = gl.get_parameter_i32(glow::CONTEXT_FLAGS);
        let strategy = gl.get_parameter_i32(RESET_NOTIFICATION_STRATEGY);
        let _ = gl.get_error();
        format!(
            "robust access {}, reset notification {}",
            if flags & CONTEXT_FLAG_ROBUST_ACCESS_BIT != 0 {
                "yes"
            } else {
                "no"
            },
            if strategy == LOSE_CONTEXT_ON_RESET {
                "lose-context-on-reset"
            } else {
                "none"
            }
        )
    }
}

/// A hidden window whose GL context stays current on the thread that created it,
/// for the whole process (the export worker). Created once; every headless renderer
/// shares it, and must only be used from that thread.
mod headless {
    use std::sync::{Arc, OnceLock};

    struct Keep {
        _window: winit::window::Window,
        _surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
        _context: glutin::context::PossiblyCurrentContext,
        _event_loop: winit::event_loop::EventLoop<()>,
    }

    struct Shared(Arc<glow::Context>, String, std::thread::ThreadId);
    // Only handed out on the thread that created it, where it stays current.
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}
    static GL: OnceLock<Result<Shared, String>> = OnceLock::new();

    pub fn gl() -> Result<(Arc<glow::Context>, String), String> {
        let shared = GL.get_or_init(|| {
            create().map(|(gl, name, keep)| {
                Box::leak(Box::new(keep));
                Shared(gl, name, std::thread::current().id())
            })
        });
        let shared = shared.as_ref().map_err(Clone::clone)?;
        if shared.2 != std::thread::current().id() {
            return Err("the hidden OpenGL context belongs to another thread".into());
        }
        Ok((shared.0.clone(), shared.1.clone()))
    }

    fn create() -> Result<(Arc<glow::Context>, String, Keep), String> {
        use glow::{self, HasContext};
        use glutin::config::{ConfigTemplateBuilder, GlConfig};
        use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext};
        use glutin::display::{GetGlDisplay, GlDisplay};
        use glutin_winit::GlWindow;
        use winit::raw_window_handle::HasWindowHandle;
        let mut builder = winit::event_loop::EventLoop::builder();
        // The export runs on a worker thread, not necessarily the process's first one.
        #[cfg(windows)]
        winit::platform::windows::EventLoopBuilderExtWindows::with_any_thread(&mut builder, true);
        #[cfg(all(unix, not(target_os = "macos")))]
        winit::platform::x11::EventLoopBuilderExtX11::with_any_thread(&mut builder, true);
        let event_loop = builder.build().map_err(|e| format!("event loop: {e}"))?;
        let attrs = winit::window::Window::default_attributes()
            .with_visible(false)
            .with_inner_size(winit::dpi::PhysicalSize::new(64u32, 64u32))
            .with_title("NOH effects");
        let (window, config) = glutin_winit::DisplayBuilder::new()
            .with_window_attributes(Some(attrs))
            .build(&event_loop, ConfigTemplateBuilder::new(), |configs| {
                configs
                    .reduce(|a, b| {
                        if b.alpha_size() > a.alpha_size() {
                            b
                        } else {
                            a
                        }
                    })
                    .expect("glutin offers at least one config")
            })
            .map_err(|e| format!("GL display: {e}"))?;
        let window = window.ok_or("no window")?;
        let display = config.display();
        let raw = window
            .window_handle()
            .map_err(|e| format!("window handle: {e}"))?
            .as_raw();
        // No version requested, like eframe: the driver's newest context. A
        // robust one first, losing the context on a GPU reset, so that an export
        // the reset interrupts fails cleanly instead of drawing garbage: every
        // call then answers GL_CONTEXT_LOST, which `fx::export` checks after
        // each frame's mix (and a motion scene after each frame). The driver's
        // ordinary context when it offers no robust one.
        let robust = ContextAttributesBuilder::new()
            .with_robustness(glutin::context::Robustness::RobustLoseContextOnReset)
            .build(Some(raw));
        let plain = ContextAttributesBuilder::new().build(Some(raw));
        let not_current = unsafe { display.create_context(&config, &robust) }
            .or_else(|_| unsafe { display.create_context(&config, &plain) })
            .map_err(|e| format!("GL context: {e}"))?;
        let surf_attrs = window
            .build_surface_attributes(Default::default())
            .map_err(|e| format!("surface attrs: {e}"))?;
        let surface = unsafe { display.create_window_surface(&config, &surf_attrs) }
            .map_err(|e| format!("GL surface: {e}"))?;
        let context = not_current
            .make_current(&surface)
            .map_err(|e| format!("make current: {e}"))?;
        let gl =
            unsafe { glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s)) };
        let name = unsafe {
            format!(
                "{} / {} (OpenGL {}, alpha bits {}, {})",
                gl.get_parameter_string(glow::VENDOR),
                gl.get_parameter_string(glow::RENDERER),
                gl.get_parameter_string(glow::VERSION),
                config.alpha_size(),
                super::robustness(&gl)
            )
        };
        Ok((
            Arc::new(gl),
            name,
            Keep {
                _window: window,
                _surface: surface,
                _context: context,
                _event_loop: event_loop,
            },
        ))
    }
}

impl GlRenderer {
    /// Command-line use: a hidden window's context, current on this thread.
    pub fn headless(
        width: u32,
        height: u32,
        logical_w: f32,
        logical_h: f32,
        out_mode: Output,
    ) -> Result<Self, String> {
        // Test only: fail here, where a machine without OpenGL 3.3 fails.
        if std::env::var_os("NOH_TEST_GL_FAIL").is_some_and(|v| !v.is_empty()) {
            return Err("no OpenGL context (NOH_TEST_GL_FAIL)".into());
        }
        let started = Instant::now();
        let (gl, name) = headless::gl()?;
        Self::on_context(
            gl, name, started, width, height, logical_w, logical_h, out_mode,
        )
    }

    /// On a caller's context (egui's), which must be current whenever this renderer is used.
    pub fn on_egui(
        gl: Arc<glow::Context>,
        width: u32,
        height: u32,
        logical_w: f32,
        logical_h: f32,
    ) -> Result<Self, String> {
        let started = Instant::now();
        let name = unsafe {
            format!(
                "{} (OpenGL {}, egui context, {})",
                gl.get_parameter_string(glow::RENDERER),
                gl.get_parameter_string(glow::VERSION),
                robustness(&gl)
            )
        };
        Self::on_context(
            gl,
            name,
            started,
            width,
            height,
            logical_w,
            logical_h,
            Output::PremulGamma,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn on_context(
        gl: Arc<glow::Context>,
        adapter: String,
        started: Instant,
        width: u32,
        height: u32,
        logical_w: f32,
        logical_h: f32,
        out_mode: Output,
    ) -> Result<Self, String> {
        unsafe {
            let max = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE);
            if max < COV_ATLAS as i32 {
                return Err(format!("GL max texture size {max} < {COV_ATLAS}"));
            }
            let (w, h) = (width as i32, height as i32);
            let (gw, gh) = (width.div_ceil(2) as i32, height.div_ceil(2) as i32);
            let layer = target(&gl, w, h, glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT)?;
            let glow_a = target(&gl, gw, gh, glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT)?;
            let glow_b = target(&gl, gw, gh, glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT)?;
            let out_internal = match out_mode {
                Output::StraightSrgb => glow::SRGB8_ALPHA8,
                Output::PremulGamma => glow::RGBA8,
            };
            let output = target(&gl, w, h, out_internal, glow::RGBA, glow::UNSIGNED_BYTE)?;
            let cov = texture(
                &gl,
                COV_ATLAS as i32,
                COV_ATLAS as i32,
                glow::R8,
                glow::RED,
                glow::UNSIGNED_BYTE,
            )?;
            let msdf = texture(
                &gl,
                MSDF_ATLAS as i32,
                MSDF_ATLAS as i32,
                glow::RGBA8,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
            )?;
            let sampler = gl.create_sampler().map_err(err("sampler"))?;
            gl.sampler_parameter_i32(sampler, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.sampler_parameter_i32(sampler, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.sampler_parameter_i32(sampler, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.sampler_parameter_i32(sampler, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            let mut g = [0f32; 24];
            g[..5].copy_from_slice(&[
                logical_w,
                logical_h,
                COV_ATLAS as f32,
                MSDF_ATLAS as f32,
                MSDF_RANGE,
            ]);
            let globals = ubo(&gl, &g)?;
            // The glow's kernel: sigma 5 texels of its half-resolution buffer.
            let blur_h = ubo(&gl, &[1.0, 0.0, gw as f32, gh as f32, 5.0, 0.0, 0.0, 0.0])?;
            let blur_v = ubo(&gl, &[0.0, 1.0, gw as f32, gh as f32, 5.0, 0.0, 0.0, 0.0])?;
            let comp_ubo = ubo(&gl, &[width as f32, height as f32, 1.0, 0.0])?;
            let blur_dyn_h = ubo(&gl, &[0.0; 8])?;
            let blur_dyn_v = ubo(&gl, &[0.0; 8])?;
            let comp_layer = ubo(&gl, &[width as f32, height as f32, 1.0, 1.0])?;
            let inst_src =
                super::shaders::SHADER.replace("HOOKS", &super::plugin_shader::hooks(None));
            let inst_prog = program(&gl, &inst_src, "vs", "fs")?;
            let blur_prog = program(&gl, super::shaders::BLUR, "vs", "fs")?;
            let down_prog = program(&gl, super::shaders::DOWNSAMPLE, "vs", "fs")?;
            let comp_prog = program(&gl, super::shaders::COMPOSITE, "vs", "fs")?;
            let resolve_src = match out_mode {
                Output::StraightSrgb => super::shaders::RESOLVE,
                Output::PremulGamma => super::shaders::RESOLVE_PREMUL,
            };
            let resolve_prog = program(&gl, resolve_src, "vs", "fs")?;
            let present_prog = plain_program(&gl, PRESENT_VS, PRESENT_FS)?;
            let inst_vao = gl.create_vertex_array().map_err(err("vao"))?;
            let empty_vao = gl.create_vertex_array().map_err(err("vao"))?;
            let inst_cap = 4096;
            let inst_buf = gl.create_buffer().map_err(err("instances"))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(inst_buf));
            gl.buffer_data_size(
                glow::ARRAY_BUFFER,
                (inst_cap * std::mem::size_of::<Inst>()) as i32,
                glow::STREAM_DRAW,
            );
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            let mut queries = Vec::new();
            for _ in 0..6 {
                queries.push(gl.create_query().map_err(err("query"))?);
            }
            let e = gl.get_error();
            if e != glow::NO_ERROR {
                return Err(format!("GL error 0x{e:x} during setup"));
            }
            let mut r = Self {
                gl,
                width,
                height,
                adapter,
                setup_ms: started.elapsed().as_secs_f64() * 1000.0,
                cov,
                msdf,
                layer,
                glow_a,
                glow_b,
                output,
                out_mode,
                inst_prog,
                blur_prog,
                down_prog,
                comp_prog,
                resolve_prog,
                present_prog,
                sampler,
                globals,
                blur_h,
                blur_v,
                comp_ubo,
                blurs: Vec::new(),
                blur_dyn_h,
                blur_dyn_v,
                comp_layer,
                px_scale: width as f32 / logical_w.max(1.0),
                inst_vao,
                empty_vao,
                inst_buf,
                inst_cap,
                queries,
                next_query: 0,
                pending: Default::default(),
                last_gpu_ms: None,
            };
            r.warm_up()?;
            Ok(r)
        }
    }

    /// Rebuild the glyph program with plugin hooks (validated beforehand by plugin_shader).
    pub fn set_hooks(&mut self, plugin_src: Option<&str>, user: [f32; 16]) -> Result<(), String> {
        let src = super::shaders::SHADER.replace("HOOKS", &super::plugin_shader::hooks(plugin_src));
        let prog = unsafe { program(&self.gl, &src, "vs", "fs") }
            .map_err(|e| format!("plugin shader rejected by OpenGL: {e}"))?;
        unsafe { self.gl.delete_program(self.inst_prog) };
        self.set_user(user);
        self.inst_prog = prog;
        self.warm_up()
    }

    /// The values shader hooks read in `g.user` (`PluginScene::hook_user`).
    pub fn set_user(&mut self, user: [f32; 16]) {
        unsafe {
            self.gl
                .bind_buffer(glow::UNIFORM_BUFFER, Some(self.globals));
            self.gl
                .buffer_sub_data_u8_slice(glow::UNIFORM_BUFFER, 32, bytemuck::cast_slice(&user));
            self.gl.bind_buffer(glow::UNIFORM_BUFFER, None);
        }
    }

    /// Drivers compile GL programs lazily, at their first draw. Draw every pass once
    /// (one transparent glyph through the glow and main paths) and wait, so that cost
    /// lands in setup or in the plugin switch, not in the first caption frame.
    fn warm_up(&mut self) -> Result<(), String> {
        // First texture uploads are slow on some drivers (lazy allocation): touch both atlases.
        self.upload(
            self.cov,
            COV_ATLAS - 1,
            COV_ATLAS - 1,
            1,
            1,
            glow::RED,
            &[0],
        );
        self.upload(
            self.msdf,
            MSDF_ATLAS - 1,
            MSDF_ATLAS - 1,
            1,
            1,
            glow::RGBA,
            &[0, 0, 0, 0],
        );
        let ghost = Inst {
            size: [1.0, 1.0],
            xform: [0.0, 1.0, 1.0, 0.0],
            extra: [-1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        self.frame(&[ghost], 0.0, &[ghost], false)?;
        unsafe { self.gl.finish() };
        self.pending.clear();
        self.last_gpu_ms = None;
        Ok(())
    }

    fn upload(&self, tex: glow::Texture, x: u32, y: u32, w: u32, h: u32, format: u32, data: &[u8]) {
        unsafe {
            let gl = &self.gl;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                x as i32,
                y as i32,
                w as i32,
                h as i32,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(data)),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    unsafe fn bind_instances(&self, first: usize) {
        let gl = &self.gl;
        let stride = std::mem::size_of::<Inst>() as i32;
        let sizes = [2, 2, 4, 4, 4, 4, 4, 4, 4, 4];
        unsafe {
            gl.bind_vertex_array(Some(self.inst_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.inst_buf));
            let mut off = (first * stride as usize) as i32;
            for (loc, n) in sizes.iter().enumerate() {
                gl.enable_vertex_attrib_array(loc as u32);
                gl.vertex_attrib_pointer_f32(loc as u32, *n, glow::FLOAT, false, stride, off);
                gl.vertex_attrib_divisor(loc as u32, 1);
                off += n * 4;
            }
        }
    }

    unsafe fn pass(&self, t: &Target, blend: Option<(u32, u32)>) {
        unsafe {
            self.bind(t, blend);
            self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// Draw into `t` without clearing it.
    unsafe fn bind(&self, t: &Target, blend: Option<(u32, u32)>) {
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.fbo));
            gl.viewport(0, 0, t.w, t.h);
            match blend {
                Some((s, d)) => {
                    gl.enable(glow::BLEND);
                    gl.blend_equation(glow::FUNC_ADD);
                    gl.blend_func(s, d);
                }
                None => gl.disable(glow::BLEND),
            }
        }
    }

    /// Blur key of an instance: its sigma in physical px x 4 (0 = drawn as is).
    /// From a quarter of a physical pixel, so the first blurred step is almost
    /// sharp and a blur fading out has no visible last jump.
    fn blur_key(&self, i: &Inst) -> u32 {
        if i.params[0] < 0.5 && i.params[1] > 0.0 {
            (i.params[1].min(BLUR_MAX) * self.px_scale * 4.0).round() as u32
        } else {
            0
        }
    }

    /// The draws of `main` in order, and the sharp copies of its blurred instances
    /// (one group per sigma, in group order). A group sits where its first member
    /// falls; later members of the same sigma join it, so a line's blurred shadows
    /// all lie beneath its text, as CSS draws text-shadow.
    fn plan(&self, main: &[Inst]) -> (Vec<Draw>, Vec<Inst>) {
        let mut draws: Vec<Draw> = Vec::new();
        for (i, inst) in main.iter().enumerate() {
            let key = self.blur_key(inst);
            if key == 0 {
                match draws.last_mut() {
                    Some(Draw::Direct(first, n)) if *first + *n == i => *n += 1,
                    _ => draws.push(Draw::Direct(i, 1)),
                }
            } else if let Some(Draw::Blur(_, members)) = draws
                .iter_mut()
                .find(|d| matches!(d, Draw::Blur(k, _) if *k == key))
            {
                members.push(i);
            } else {
                draws.push(Draw::Blur(key, vec![i]));
            }
        }
        let mut sharp = Vec::new();
        for d in &draws {
            if let Draw::Blur(_, members) = d {
                sharp.extend(members.iter().map(|&i| {
                    let mut s = main[i];
                    s.params[1] = 0.0;
                    s
                }));
            }
        }
        (draws, sharp)
    }

    /// Downscale of a blur layer: the smallest power of two that brings sigma to
    /// 4 texels or less (the kernel spans +-12 texels).
    fn downscale(key: u32) -> u32 {
        let sigma = key as f32 / 4.0;
        let mut d = 1;
        while sigma / d as f32 > 4.0 && d < 64 {
            d *= 2;
        }
        d
    }

    /// The pairs of blur layers for every downscale from 1 to `d` (the sharp draw
    /// is full size, then halved down to `d`), made the first time.
    fn blur_layers(&mut self, d: u32) -> Result<(), String> {
        let mut level = 1;
        while level <= d {
            if !self.blurs.iter().any(|(k, _, _)| *k == level) {
                let (w, h) = (
                    self.width.div_ceil(level) as i32,
                    self.height.div_ceil(level) as i32,
                );
                let gl = &self.gl;
                let (a, b) = unsafe {
                    (
                        target(gl, w, h, glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT)?,
                        target(gl, w, h, glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT)?,
                    )
                };
                self.blurs.push((level, a, b));
            }
            level *= 2;
        }
        Ok(())
    }

    fn blur_pair(&self, level: u32) -> Option<(&Target, &Target)> {
        self.blurs
            .iter()
            .find(|(k, _, _)| *k == level)
            .map(|(_, a, b)| (a, b))
    }

    /// One group of blurred instances: drawn sharp at full size, halved down to
    /// 1/d by 2 x 2 averages (so thin strokes keep their weight at any d), blurred
    /// horizontally then vertically by its sigma, drawn over the main layer.
    /// Work is scissored to the group's box grown by 3 sigma.
    unsafe fn draw_blurred(&self, key: u32, group: &[&Inst], sharp_at: usize) {
        let gl = &self.gl;
        let d = Self::downscale(key);
        let (Some((full, _)), Some((a, b))) = (self.blur_pair(1), self.blur_pair(d)) else {
            return;
        };
        let sigma_px = key as f32 / 4.0;
        // The group's box in physical px, grown by 3 sigma and a texel.
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for i in group {
            let r = 0.5 * i.size[0].hypot(i.size[1]) * i.xform[1].abs().max(i.xform[2].abs());
            x0 = x0.min(i.center[0] - r);
            x1 = x1.max(i.center[0] + r);
            y0 = y0.min(i.center[1] - r);
            y1 = y1.max(i.center[1] + r);
        }
        let grow = 3.0 * sigma_px + 2.0 * d as f32;
        let s = self.px_scale;
        let scissor = |t: &Target, k: f32| {
            let fx0 = ((x0 * s - grow) / k).floor().max(0.0) as i32;
            let fx1 = ((x1 * s + grow) / k).ceil().min(t.w as f32) as i32;
            let fy0 = ((y0 * s - grow) / k).floor().max(0.0) as i32;
            let fy1 = ((y1 * s + grow) / k).ceil().min(t.h as f32) as i32;
            // The WGSL to GLSL translation flips y: canvas row y is GL row y.
            (fx0, fy0, (fx1 - fx0).max(0), (fy1 - fy0).max(0))
        };
        unsafe {
            // Layers cleared whole: the kernel and the halving read past the box.
            let mut level = 1;
            while level <= d {
                if let Some((la, lb)) = self.blur_pair(level) {
                    self.pass(la, None);
                    if level == d {
                        self.pass(lb, None);
                    }
                }
                level *= 2;
            }
            self.bind(full, Some(BLEND_PREMUL));
            let (sx, sy, sw, sh) = scissor(full, 1.0);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(sx, sy, sw, sh);
            gl.use_program(Some(self.inst_prog));
            gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.globals));
            self.bind_tex(1, self.cov);
            self.bind_tex(2, self.msdf);
            self.bind_instances(sharp_at);
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, group.len() as i32);
            gl.bind_vertex_array(Some(self.empty_vao));
            gl.use_program(Some(self.down_prog));
            let mut level = 2;
            let mut src = full;
            while level <= d {
                let Some((dst, _)) = self.blur_pair(level) else {
                    break;
                };
                self.bind(dst, None);
                let (sx, sy, sw, sh) = scissor(dst, level as f32);
                gl.scissor(sx, sy, sw, sh);
                gl.bind_buffer(glow::UNIFORM_BUFFER, Some(self.blur_dyn_h));
                gl.buffer_sub_data_u8_slice(
                    glow::UNIFORM_BUFFER,
                    0,
                    bytemuck::cast_slice(&[
                        0.0f32,
                        0.0,
                        src.w as f32,
                        src.h as f32,
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                    ]),
                );
                gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.blur_dyn_h));
                self.bind_tex(1, src.tex);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                src = dst;
                level *= 2;
            }
            let (sx, sy, sw, sh) = scissor(a, d as f32);
            gl.scissor(sx, sy, sw, sh);
            let sigma = sigma_px / d as f32;
            gl.bind_vertex_array(Some(self.empty_vao));
            gl.use_program(Some(self.blur_prog));
            for (from, to, ubo, dir) in [
                (a, b, self.blur_dyn_h, [1.0f32, 0.0]),
                (b, a, self.blur_dyn_v, [0.0, 1.0]),
            ] {
                self.bind(to, None);
                gl.bind_buffer(glow::UNIFORM_BUFFER, Some(ubo));
                gl.buffer_sub_data_u8_slice(
                    glow::UNIFORM_BUFFER,
                    0,
                    bytemuck::cast_slice(&[
                        dir[0],
                        dir[1],
                        from.w as f32,
                        from.h as f32,
                        sigma,
                        0.0,
                        0.0,
                        0.0,
                    ]),
                );
                gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(ubo));
                self.bind_tex(1, from.tex);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            self.bind(&self.layer, Some(BLEND_PREMUL));
            let (lx, ly, lw, lh) = scissor(&self.layer, 1.0);
            gl.scissor(lx, ly, lw, lh);
            gl.use_program(Some(self.comp_prog));
            self.bind_tex(0, a.tex);
            gl.bind_buffer_base(glow::UNIFORM_BUFFER, 2, Some(self.comp_layer));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.disable(glow::SCISSOR_TEST);
        }
    }

    unsafe fn bind_tex(&self, unit: u32, tex: glow::Texture) {
        unsafe {
            self.gl.active_texture(glow::TEXTURE0 + unit);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            self.gl.bind_sampler(unit, Some(self.sampler));
        }
    }

    /// Read finished GPU timers without waiting.
    fn poll_queries(&mut self, wait: bool) {
        while let Some(q) = self.pending.front().copied() {
            let ready = wait
                || unsafe {
                    self.gl
                        .get_query_parameter_u32(q, glow::QUERY_RESULT_AVAILABLE)
                } != 0;
            if !ready {
                break;
            }
            let ns = unsafe { self.gl.get_query_parameter_u64(q, glow::QUERY_RESULT) };
            self.last_gpu_ms = Some(ns as f64 / 1e6);
            self.pending.pop_front();
        }
    }

    /// Render one frame into the output texture; returns straight (export) or
    /// premultiplied-gamma (preview) RGBA8 rows, top row first, when `read`.
    pub fn frame(
        &mut self,
        glow_i: &[Inst],
        glow_gain: f32,
        main: &[Inst],
        read: bool,
    ) -> Result<(Vec<u8>, FrameTimes), String> {
        let entered = Instant::now();
        let gl = self.gl.clone();
        let (draws, sharp) = self.plan(main);
        for d in &draws {
            if let Draw::Blur(key, _) = d {
                self.blur_layers(Self::downscale(*key))?;
            }
        }
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.color_mask(true, true, true, true);
            let total = glow_i.len() + main.len() + sharp.len();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.inst_buf));
            if total > self.inst_cap {
                self.inst_cap = total.next_power_of_two();
                gl.buffer_data_size(
                    glow::ARRAY_BUFFER,
                    (self.inst_cap * std::mem::size_of::<Inst>()) as i32,
                    glow::STREAM_DRAW,
                );
            }
            if !glow_i.is_empty() {
                gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytemuck::cast_slice(glow_i));
                gl.bind_buffer(glow::UNIFORM_BUFFER, Some(self.comp_ubo));
                gl.buffer_sub_data_u8_slice(
                    glow::UNIFORM_BUFFER,
                    8,
                    bytemuck::cast_slice(&[glow_gain]),
                );
            }
            if !main.is_empty() {
                gl.buffer_sub_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    (glow_i.len() * std::mem::size_of::<Inst>()) as i32,
                    bytemuck::cast_slice(main),
                );
            }
            if !sharp.is_empty() {
                gl.buffer_sub_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    ((glow_i.len() + main.len()) * std::mem::size_of::<Inst>()) as i32,
                    bytemuck::cast_slice(&sharp),
                );
            }
            let query = self.queries[self.next_query];
            let timing = self.pending.len() < self.queries.len();
            if timing {
                self.next_query = (self.next_query + 1) % self.queries.len();
                gl.begin_query(glow::TIME_ELAPSED, query);
            }
            gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.globals));
            self.bind_tex(1, self.cov);
            self.bind_tex(2, self.msdf);
            if !glow_i.is_empty() {
                self.pass(&self.glow_a, Some(BLEND_ADD));
                gl.use_program(Some(self.inst_prog));
                self.bind_instances(0);
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, glow_i.len() as i32);
                gl.bind_vertex_array(Some(self.empty_vao));
                gl.use_program(Some(self.blur_prog));
                self.pass(&self.glow_b, None);
                gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.blur_h));
                self.bind_tex(1, self.glow_a.tex);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                self.pass(&self.glow_a, None);
                gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.blur_v));
                self.bind_tex(1, self.glow_b.tex);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            self.pass(&self.layer, Some(BLEND_PREMUL));
            if !glow_i.is_empty() {
                gl.bind_vertex_array(Some(self.empty_vao));
                gl.use_program(Some(self.comp_prog));
                self.bind_tex(0, self.glow_a.tex);
                gl.bind_buffer_base(glow::UNIFORM_BUFFER, 2, Some(self.comp_ubo));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            let mut sharp_at = glow_i.len() + main.len();
            for d in &draws {
                match d {
                    Draw::Direct(first, n) => {
                        self.bind(&self.layer, Some(BLEND_PREMUL));
                        gl.use_program(Some(self.inst_prog));
                        gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.globals));
                        self.bind_tex(1, self.cov);
                        self.bind_tex(2, self.msdf);
                        self.bind_instances(glow_i.len() + first);
                        gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, *n as i32);
                    }
                    Draw::Blur(key, members) => {
                        let group: Vec<&Inst> = members.iter().map(|&i| &main[i]).collect();
                        self.draw_blurred(*key, &group, sharp_at);
                        sharp_at += members.len();
                    }
                }
            }
            self.pass(&self.output, None);
            if self.out_mode == Output::StraightSrgb {
                gl.enable(glow::FRAMEBUFFER_SRGB);
            }
            gl.bind_vertex_array(Some(self.empty_vao));
            gl.use_program(Some(self.resolve_prog));
            self.bind_tex(0, self.layer.tex);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            if timing {
                gl.end_query(glow::TIME_ELAPSED);
                self.pending.push_back(query);
            }
            let pre_submit_ms = entered.elapsed().as_secs_f64() * 1000.0;
            let submitted = Instant::now();
            let finish_ms = 0.0;
            let mut pixels = Vec::new();
            if read {
                pixels = vec![0u8; self.width as usize * self.height as usize * 4];
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.output.fbo));
                gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
                gl.read_pixels(
                    0,
                    0,
                    self.width as i32,
                    self.height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixels)),
                );
                gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            } else {
                gl.flush();
            }
            // Leave a neutral state for whoever shares the context (egui restores its own).
            for unit in 0..3 {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.bind_sampler(unit, None);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::UNIFORM_BUFFER, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.disable(glow::BLEND);
            let submit_to_mapped_ms = submitted.elapsed().as_secs_f64() * 1000.0;
            if read {
                self.poll_queries(true);
            }
            let e = gl.get_error();
            if e != glow::NO_ERROR {
                return Err(format!("GL error 0x{e:x}"));
            }
            let gpu_ms = if read { self.last_gpu_ms } else { None };
            Ok((
                pixels,
                FrameTimes {
                    pre_submit_ms,
                    gpu_ms,
                    submit_to_mapped_ms,
                    copy_out_ms: finish_ms,
                },
            ))
        }
    }

    /// Preview: draw the last output over the current viewport (premultiplied).
    pub fn present(&self) {
        unsafe {
            let gl = &self.gl;
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.bind_vertex_array(Some(self.empty_vao));
            gl.use_program(Some(self.present_prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.output.tex));
            gl.bind_sampler(0, Some(self.sampler));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_sampler(0, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// The GL context this renderer draws with.
    pub fn context(&self) -> &Arc<glow::Context> {
        &self.gl
    }

    /// The engine's own layer after the last `frame`: premultiplied colour in
    /// linear light, half float, the glow included, before the resolve to the
    /// 8-bit output. A caller that stacks several frames (the motion scene
    /// compositor) reads it, so that each pass keeps its precision.
    pub fn layer_texture(&self) -> glow::Texture {
        self.layer.tex
    }

    /// The resolved output texture (preview: egui draws it as a native texture).
    pub fn output_texture(&self) -> glow::Texture {
        self.output.tex
    }

    /// Preview timers: one finished GPU time per call, oldest first, never waiting.
    pub fn take_gpu_ms(&mut self) -> Option<f64> {
        let q = *self.pending.front()?;
        if unsafe {
            self.gl
                .get_query_parameter_u32(q, glow::QUERY_RESULT_AVAILABLE)
        } == 0
        {
            return None;
        }
        let ns = unsafe { self.gl.get_query_parameter_u64(q, glow::QUERY_RESULT) };
        self.pending.pop_front();
        self.last_gpu_ms = Some(ns as f64 / 1e6);
        self.last_gpu_ms
    }

    /// Preview timers: read whatever finished, never wait.
    pub fn poll_gpu_ms(&mut self) -> Option<f64> {
        self.poll_queries(false);
        self.last_gpu_ms
    }
}

impl super::shaders::Upload for GlRenderer {
    fn upload_cov(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        self.upload(self.cov, x, y, w, h, glow::RED, data);
    }
    fn upload_msdf(&self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        self.upload(self.msdf, x, y, w, h, glow::RGBA, rgba);
    }
}

/// YUV matrix of the video the captions are mixed into (limited range in both cases).
/// NOH's montage pieces carry no colour tags after FFV1/NUT, and FFmpeg (the preview's
/// RGB conversion, libass) reads untagged frames as BT.601: the export uses `Bt601`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
}
impl Matrix {
    /// (Kr, Kb).
    pub fn coefficients(self) -> (f32, f32) {
        match self {
            Matrix::Bt601 => (0.299, 0.114),
            Matrix::Bt709 => (0.2126, 0.0722),
        }
    }
}

/// Mix the caption layer into an I420 video frame on the GPU, the way FFmpeg's
/// `overlay` filter does: Y at full size, U and V at half size with the overlay alpha
/// (and premultiplied colour) averaged over 2x2, limited range, in the given matrix.
/// Each output code is rounded to nearest explicitly (the R8 store does not guarantee
/// it). Where the overlay alpha is 0 the video bytes pass unchanged.
const MIX_VS: &str = "#version 330 core
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}";
const MIX_Y_FS: &str = "#version 330 core
uniform sampler2D ytex;
uniform sampler2D ov;
uniform vec2 k;
out vec4 o;
float quant(float x) { return floor(x * 255.0 + 0.5) / 255.0; }
void main() {
  ivec2 p = ivec2(gl_FragCoord.xy);
  float ym = texelFetch(ytex, p, 0).r;
  vec4 c = texelFetch(ov, p, 0);
  float y = (16.0 / 255.0) * c.a + (219.0 / 255.0) * dot(vec3(k.x, 1.0 - k.x - k.y, k.y), c.rgb) + ym * (1.0 - c.a);
  o = vec4(quant(y), 0.0, 0.0, 1.0);
}";
const MIX_UV_FS: &str = "#version 330 core
uniform sampler2D utex;
uniform sampler2D vtex;
uniform sampler2D ov;
uniform ivec2 size;
uniform vec2 k;
float quant(float x) { return floor(x * 255.0 + 0.5) / 255.0; }
layout(location = 0) out vec4 ou;
layout(location = 1) out vec4 ovv;
void main() {
  ivec2 q = ivec2(gl_FragCoord.xy);
  vec4 c = vec4(0.0);
  for (int dy = 0; dy < 2; dy++) {
    for (int dx = 0; dx < 2; dx++) {
      ivec2 p = min(q * 2 + ivec2(dx, dy), size - 1);
      c += texelFetch(ov, p, 0);
    }
  }
  c *= 0.25;
  float yp = dot(vec3(k.x, 1.0 - k.x - k.y, k.y), c.rgb);
  float u = (128.0 / 255.0) * c.a + (224.0 / 255.0) * (c.b - yp) / (2.0 * (1.0 - k.y)) + texelFetch(utex, q, 0).r * (1.0 - c.a);
  float v = (128.0 / 255.0) * c.a + (224.0 / 255.0) * (c.r - yp) / (2.0 * (1.0 - k.x)) + texelFetch(vtex, q, 0).r * (1.0 - c.a);
  ou = vec4(quant(u), 0.0, 0.0, 1.0);
  ovv = vec4(quant(v), 0.0, 0.0, 1.0);
}";

/// A reframing of the picture before the mix (a motion scene's `video()`), in
/// luma pixels measured from the top left: the output pixel `p` shows the
/// source position `inv * (p - center) + focus` (column-major 2 x 2), black
/// outside `crop` (x0, y0, x1, y1). The chroma planes take the same at half size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reframe {
    pub inv: [f32; 4],
    pub center: [f32; 2],
    pub focus: [f32; 2],
    pub crop: [f32; 4],
}

/// One plane of the picture resampled through a reframing (bilinear), black
/// outside the crop, each code rounded to nearest.
const FRAME_FS: &str = "#version 330 core
uniform sampler2D src;
uniform vec2 src_size;
uniform mat2 inv;
uniform vec2 center;
uniform vec2 focus;
uniform vec4 crop;
uniform float black;
out vec4 o;
float quant(float x) { return floor(x * 255.0 + 0.5) / 255.0; }
void main() {
  vec2 s = inv * (gl_FragCoord.xy - center) + focus;
  if (s.x < crop.x || s.y < crop.y || s.x > crop.z || s.y > crop.w) {
    o = vec4(black, 0.0, 0.0, 1.0);
    return;
  }
  o = vec4(quant(texture(src, s / src_size).r), 0.0, 0.0, 1.0);
}";

/// The reframing pass, made at its first use: the reframed planes, their
/// framebuffers, the program and a bilinear sampler.
struct Framed {
    y: Target,
    u: Target,
    v: Target,
    prog: glow::Program,
    linear: glow::Sampler,
}

pub struct YuvMix {
    gl: Arc<glow::Context>,
    w: i32,
    h: i32,
    cw: i32,
    ch: i32,
    y_in: glow::Texture,
    u_in: glow::Texture,
    v_in: glow::Texture,
    y_out: Target,
    uv_fbo: glow::Framebuffer,
    u_out: glow::Texture,
    v_out: glow::Texture,
    prog_y: glow::Program,
    prog_uv: glow::Program,
    vao: glow::VertexArray,
    /// Two pixel-pack buffers: frame N reads back into one while frame N-1's bytes
    /// are copied out of the other, so the CPU does not wait for the GPU each frame.
    pbo: [glow::Buffer; 2],
    fence: [Option<glow::Fence>; 2],
    /// The next frames' reframing (none: the picture as decoded), and its pass.
    reframe: Option<Reframe>,
    framed: Option<Framed>,
}

impl YuvMix {
    pub fn new(r: &GlRenderer, matrix: Matrix) -> Result<Self, String> {
        let gl = r.gl.clone();
        let (kr, kb) = matrix.coefficients();
        let (w, h) = (r.width as i32, r.height as i32);
        let (cw, ch) = ((w + 1) / 2, (h + 1) / 2);
        unsafe {
            let r8 = |w, h| texture(&gl, w, h, glow::R8, glow::RED, glow::UNSIGNED_BYTE);
            let (y_in, u_in, v_in) = (r8(w, h)?, r8(cw, ch)?, r8(cw, ch)?);
            let y_out = target(&gl, w, h, glow::R8, glow::RED, glow::UNSIGNED_BYTE)?;
            let (u_out, v_out) = (r8(cw, ch)?, r8(cw, ch)?);
            let uv_fbo = gl.create_framebuffer().map_err(err("fbo"))?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(uv_fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(u_out),
                0,
            );
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT1,
                glow::TEXTURE_2D,
                Some(v_out),
                0,
            );
            gl.draw_buffers(&[glow::COLOR_ATTACHMENT0, glow::COLOR_ATTACHMENT1]);
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(format!("uv framebuffer incomplete: 0x{status:x}"));
            }
            let link = |fs: &str, units: &[(&str, i32)]| -> Result<glow::Program, String> {
                let p = gl.create_program().map_err(err("program"))?;
                for (kind, src) in [(glow::VERTEX_SHADER, MIX_VS), (glow::FRAGMENT_SHADER, fs)] {
                    let sh = gl.create_shader(kind).map_err(err("shader"))?;
                    gl.shader_source(sh, src);
                    gl.compile_shader(sh);
                    if !gl.get_shader_compile_status(sh) {
                        return Err(format!("mix shader: {}", gl.get_shader_info_log(sh)));
                    }
                    gl.attach_shader(p, sh);
                }
                gl.link_program(p);
                if !gl.get_program_link_status(p) {
                    return Err(format!("mix link: {}", gl.get_program_info_log(p)));
                }
                gl.use_program(Some(p));
                for (name, unit) in units {
                    if let Some(l) = gl.get_uniform_location(p, name) {
                        gl.uniform_1_i32(Some(&l), *unit);
                    }
                }
                if let Some(l) = gl.get_uniform_location(p, "size") {
                    gl.uniform_2_i32(Some(&l), w, h);
                }
                if let Some(l) = gl.get_uniform_location(p, "k") {
                    gl.uniform_2_f32(Some(&l), kr, kb);
                }
                gl.use_program(None);
                Ok(p)
            };
            let prog_y = link(MIX_Y_FS, &[("ytex", 0), ("ov", 1)])?;
            let prog_uv = link(MIX_UV_FS, &[("utex", 0), ("vtex", 2), ("ov", 1)])?;
            let vao = gl.create_vertex_array().map_err(err("vao"))?;
            let bytes = (w * h + 2 * cw * ch) as i32;
            let mut pbo = [
                gl.create_buffer().map_err(err("pbo"))?,
                gl.create_buffer().map_err(err("pbo"))?,
            ];
            for b in &mut pbo {
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(*b));
                gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, bytes, glow::STREAM_READ);
            }
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            Ok(Self {
                gl,
                w,
                h,
                cw,
                ch,
                y_in,
                u_in,
                v_in,
                y_out,
                uv_fbo,
                u_out,
                v_out,
                prog_y,
                prog_uv,
                vao,
                pbo,
                fence: [None, None],
                reframe: None,
                framed: None,
            })
        }
    }

    pub fn frame_bytes(&self) -> usize {
        (self.w * self.h + 2 * self.cw * self.ch) as usize
    }

    /// `i420` in, mixed I420 out (same layout), with `overlay` = the engine's output
    /// texture in premultiplied-gamma mode.
    /// Synchronous: submit then fetch at once.
    pub fn mix(
        &mut self,
        i420: &[u8],
        overlay: glow::Texture,
        out: &mut [u8],
    ) -> Result<(), String> {
        self.submit(i420, overlay, 0)?;
        self.fetch(0, out)
    }

    /// Wait for slot `slot`'s readback and copy it out. A readback that does not
    /// finish within 10 s, or a GL error, fails: the frame is never written partially.
    pub fn fetch(&mut self, slot: usize, out: &mut [u8]) -> Result<(), String> {
        let gl = &self.gl;
        unsafe {
            if let Some(f) = self.fence[slot].take() {
                // glow takes the timeout as i32 nanoseconds: wait in 2 s steps, 10 s in all.
                let mut status = glow::TIMEOUT_EXPIRED;
                for _ in 0..5 {
                    status = gl.client_wait_sync(f, glow::SYNC_FLUSH_COMMANDS_BIT, 2_000_000_000);
                    if status != glow::TIMEOUT_EXPIRED {
                        break;
                    }
                }
                gl.delete_sync(f);
                if status != glow::ALREADY_SIGNALED && status != glow::CONDITION_SATISFIED {
                    return Err(format!("GPU readback did not complete (0x{status:x})"));
                }
            }
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(self.pbo[slot]));
            gl.get_buffer_sub_data(glow::PIXEL_PACK_BUFFER, 0, out);
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            let e = gl.get_error();
            if e != glow::NO_ERROR {
                return Err(format!("GL error 0x{e:x} in readback"));
            }
        }
        Ok(())
    }

    /// Upload `i420`, mix it with `overlay` and start the readback into slot `slot`.
    /// Reframe the next frames' picture before the mix (none: as decoded, the
    /// path the shipped styles take, unchanged).
    pub fn set_reframe(&mut self, reframe: Option<Reframe>) {
        self.reframe = reframe;
    }

    fn make_framed(&self) -> Result<Framed, String> {
        let gl = &self.gl;
        unsafe {
            let y = target(gl, self.w, self.h, glow::R8, glow::RED, glow::UNSIGNED_BYTE)?;
            let u = target(
                gl,
                self.cw,
                self.ch,
                glow::R8,
                glow::RED,
                glow::UNSIGNED_BYTE,
            )?;
            let v = target(
                gl,
                self.cw,
                self.ch,
                glow::R8,
                glow::RED,
                glow::UNSIGNED_BYTE,
            )?;
            let prog = gl.create_program().map_err(err("program"))?;
            for (kind, src) in [
                (glow::VERTEX_SHADER, MIX_VS),
                (glow::FRAGMENT_SHADER, FRAME_FS),
            ] {
                let sh = gl.create_shader(kind).map_err(err("shader"))?;
                gl.shader_source(sh, src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    return Err(format!("reframe shader: {}", gl.get_shader_info_log(sh)));
                }
                gl.attach_shader(prog, sh);
                gl.delete_shader(sh);
            }
            gl.link_program(prog);
            if !gl.get_program_link_status(prog) {
                return Err(format!("reframe link: {}", gl.get_program_info_log(prog)));
            }
            let linear = gl.create_sampler().map_err(err("sampler"))?;
            for (k, val) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.sampler_parameter_i32(linear, k, val as i32);
            }
            Ok(Framed {
                y,
                u,
                v,
                prog,
                linear,
            })
        }
    }

    /// Resample the uploaded planes through `r` into the reframed planes.
    fn run_reframe(&mut self, r: Reframe) -> Result<(), String> {
        if self.framed.is_none() {
            self.framed = Some(self.make_framed()?);
        }
        let f = self.framed.as_ref().expect("made above");
        let gl = &self.gl;
        unsafe {
            gl.use_program(Some(f.prog));
            let loc = |n: &str| gl.get_uniform_location(f.prog, n);
            gl.uniform_1_i32(loc("src").as_ref(), 0);
            gl.uniform_matrix_2_f32_slice(loc("inv").as_ref(), false, &r.inv);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_sampler(0, Some(f.linear));
            gl.bind_vertex_array(Some(self.vao));
            for (src, out, (w, h), k, black) in [
                (self.y_in, &f.y, (self.w, self.h), 1.0, 16.0),
                (self.u_in, &f.u, (self.cw, self.ch), 2.0, 128.0),
                (self.v_in, &f.v, (self.cw, self.ch), 2.0, 128.0),
            ] {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out.fbo));
                gl.viewport(0, 0, w, h);
                gl.uniform_2_f32(loc("src_size").as_ref(), w as f32, h as f32);
                gl.uniform_2_f32(loc("center").as_ref(), r.center[0] / k, r.center[1] / k);
                gl.uniform_2_f32(loc("focus").as_ref(), r.focus[0] / k, r.focus[1] / k);
                gl.uniform_4_f32(
                    loc("crop").as_ref(),
                    r.crop[0] / k,
                    r.crop[1] / k,
                    r.crop[2] / k,
                    r.crop[3] / k,
                );
                gl.uniform_1_f32(loc("black").as_ref(), black / 255.0);
                gl.bind_texture(glow::TEXTURE_2D, Some(src));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            gl.bind_sampler(0, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.use_program(None);
        }
        Ok(())
    }

    pub fn submit(
        &mut self,
        i420: &[u8],
        overlay: glow::Texture,
        slot: usize,
    ) -> Result<(), String> {
        let (ys, cs) = ((self.w * self.h) as usize, (self.cw * self.ch) as usize);
        if i420.len() != self.frame_bytes() {
            return Err(format!(
                "I420 frame of {} bytes, expected {}",
                i420.len(),
                self.frame_bytes()
            ));
        }
        let gl = &self.gl;
        unsafe {
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            for (tex, data, w, h) in [
                (self.y_in, &i420[..ys], self.w, self.h),
                (self.u_in, &i420[ys..ys + cs], self.cw, self.ch),
                (self.v_in, &i420[ys + cs..ys + 2 * cs], self.cw, self.ch),
            ] {
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    w,
                    h,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
            }
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        }
        // The picture the mix reads: as decoded, or reframed.
        let (y_src, u_src, v_src) = match self.reframe {
            None => (self.y_in, self.u_in, self.v_in),
            Some(r) => {
                self.run_reframe(r)?;
                let f = self.framed.as_ref().expect("made by run_reframe");
                (f.y.tex, f.u.tex, f.v.tex)
            }
        };
        let gl = &self.gl;
        unsafe {
            gl.disable(glow::BLEND);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_vertex_array(Some(self.vao));
            for unit in 0..3 {
                gl.bind_sampler(unit, None);
            }
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(overlay));
            // Y
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.y_out.fbo));
            gl.viewport(0, 0, self.w, self.h);
            gl.use_program(Some(self.prog_y));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(y_src));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            // U, V
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.uv_fbo));
            gl.viewport(0, 0, self.cw, self.ch);
            gl.use_program(Some(self.prog_uv));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(u_src));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(v_src));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            // Read back the three planes.
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(self.pbo[slot]));
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.y_out.fbo));
            gl.read_pixels(
                0,
                0,
                self.w,
                self.h,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::BufferOffset(0),
            );
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.uv_fbo));
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.read_pixels(
                0,
                0,
                self.cw,
                self.ch,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::BufferOffset(ys as u32),
            );
            gl.read_buffer(glow::COLOR_ATTACHMENT1);
            gl.read_pixels(
                0,
                0,
                self.cw,
                self.ch,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::BufferOffset((ys + cs) as u32),
            );
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            if let Some(old) = self.fence[slot].take() {
                gl.delete_sync(old);
            }
            self.fence[slot] = Some(
                gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0)
                    .map_err(err("fence"))?,
            );
            gl.flush();
            for unit in 0..3 {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            let e = gl.get_error();
            if e != glow::NO_ERROR {
                return Err(format!("GL error 0x{e:x} in mix"));
            }
        }
        Ok(())
    }
}

impl Drop for YuvMix {
    fn drop(&mut self) {
        let gl = &self.gl;
        unsafe {
            for f in self.fence.iter_mut().filter_map(Option::take) {
                gl.delete_sync(f);
            }
            for t in [
                self.y_in,
                self.u_in,
                self.v_in,
                self.y_out.tex,
                self.u_out,
                self.v_out,
            ] {
                gl.delete_texture(t);
            }
            gl.delete_framebuffer(self.y_out.fbo);
            gl.delete_framebuffer(self.uv_fbo);
            if let Some(f) = self.framed.take() {
                for t in [&f.y, &f.u, &f.v] {
                    gl.delete_framebuffer(t.fbo);
                    gl.delete_texture(t.tex);
                }
                gl.delete_program(f.prog);
                gl.delete_sampler(f.linear);
            }
            gl.delete_program(self.prog_y);
            gl.delete_program(self.prog_uv);
            gl.delete_vertex_array(self.vao);
            for b in self.pbo {
                gl.delete_buffer(b);
            }
        }
    }
}

impl Drop for GlRenderer {
    fn drop(&mut self) {
        let gl = &self.gl;
        unsafe {
            for t in [&self.layer, &self.glow_a, &self.glow_b, &self.output]
                .into_iter()
                .chain(self.blurs.iter().flat_map(|(_, a, b)| [a, b]))
            {
                gl.delete_texture(t.tex);
                gl.delete_framebuffer(t.fbo);
            }
            gl.delete_texture(self.cov);
            gl.delete_texture(self.msdf);
            for p in [
                self.inst_prog,
                self.blur_prog,
                self.down_prog,
                self.comp_prog,
                self.resolve_prog,
                self.present_prog,
            ] {
                gl.delete_program(p);
            }
            gl.delete_sampler(self.sampler);
            for b in [
                self.globals,
                self.blur_h,
                self.blur_v,
                self.comp_ubo,
                self.blur_dyn_h,
                self.blur_dyn_v,
                self.comp_layer,
                self.inst_buf,
            ] {
                gl.delete_buffer(b);
            }
            gl.delete_vertex_array(self.inst_vao);
            gl.delete_vertex_array(self.empty_vao);
            for q in &self.queries {
                gl.delete_query(*q);
            }
        }
    }
}
