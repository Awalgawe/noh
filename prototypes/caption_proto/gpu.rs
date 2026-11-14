//! Offscreen wgpu compositor for the caption prototype: instanced glyph quads,
//! an optional half-resolution glow layer, and a straight-alpha sRGB readback.
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

pub const COV_ATLAS: u32 = 8192;
pub const MSDF_ATLAS: u32 = 4096;
pub const MSDF_RANGE: f32 = 8.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Inst {
    /// Quad centre in canvas pixels (y down).
    pub center: [f32; 2],
    /// Quad size in canvas pixels before the instance scale.
    pub size: [f32; 2],
    /// Rotation (radians), scale x, scale y, unused.
    pub xform: [f32; 4],
    /// Atlas rectangle u0, v0, u1, v1 (normalised).
    pub uv: [f32; 4],
    /// Premultiplied linear colour.
    pub color: [f32; 4],
    /// Second colour (karaoke wipe), premultiplied linear.
    pub color2: [f32; 4],
    /// Mode (0 coverage, 1 MSDF, 2 solid), blur px, outline px (outline only), canvas px per atlas texel.
    pub params: [f32; 4],
    /// Wipe split in local x (0..1; negative = no wipe), unused x3.
    pub extra: [f32; 4],
}

/// Engine glyph shader; `HOOKS` is replaced by the plugin hooks (or the defaults).
pub const SHADER: &str = r#"
struct Globals { canvas: vec2<f32>, cov_size: f32, msdf_size: f32, msdf_range: f32, p0: f32, p1: f32, p2: f32, user: array<vec4<f32>, 4> };
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var cov_tex: texture_2d<f32>;
@group(0) @binding(2) var msdf_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
struct GlyphIn { coverage: f32, local: vec2<f32>, uv: vec2<f32>, color: vec4<f32>, color2: vec4<f32>, time: f32, progress: f32 };
struct VertexIn { corner: vec2<f32>, center: vec2<f32>, time: f32, index: f32 };
HOOKS

struct Inst {
  @location(0) center: vec2<f32>,
  @location(1) size: vec2<f32>,
  @location(2) xform: vec4<f32>,
  @location(3) uv: vec4<f32>,
  @location(4) color: vec4<f32>,
  @location(5) color2: vec4<f32>,
  @location(6) params: vec4<f32>,
  @location(7) extra: vec4<f32>,
};
struct VOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
  @location(1) local: vec2<f32>,
  @location(2) color: vec4<f32>,
  @location(3) color2: vec4<f32>,
  @location(4) params: vec4<f32>,
  @location(5) extra: vec4<f32>,
  @location(6) uvspan: vec4<f32>,
};

@vertex fn vs(@builtin(vertex_index) vi: u32, i: Inst) -> VOut {
  let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
  let p = (corner - vec2<f32>(0.5)) * i.size * vec2<f32>(i.xform.y, i.xform.z);
  let c = cos(i.xform.x);
  let s = sin(i.xform.x);
  var r = vec2<f32>(p.x * c - p.y * s, p.x * s + p.y * c) + i.center;
  r = r + displace(VertexIn(corner, i.center, i.extra.y, i.extra.w));
  var o: VOut;
  o.pos = vec4<f32>(r.x / g.canvas.x * 2.0 - 1.0, 1.0 - r.y / g.canvas.y * 2.0, 0.0, 1.0);
  o.uv = mix(i.uv.xy, i.uv.zw, corner);
  o.local = corner;
  o.color = i.color;
  o.color2 = i.color2;
  let sc = 0.5 * (abs(i.xform.y) + abs(i.xform.z));
  o.params = vec4<f32>(i.params.x, i.params.y, i.params.z, i.params.w * sc);
  o.extra = i.extra;
  o.uvspan = i.uv;
  return o;
}

fn median3(v: vec3<f32>) -> f32 { return max(min(v.r, v.g), min(max(v.r, v.g), v.b)); }

fn cov_at(uv: vec2<f32>, span: vec4<f32>) -> f32 {
  // Stay half a texel inside the glyph's own rectangle: bilinear filtering at the exact
  // edge would blend in the neighbouring texel, which may hold stale atlas data.
  let half = vec2<f32>(0.5 / g.cov_size);
  let q = clamp(uv, min(span.xy, span.zw) + half, max(span.xy, span.zw) - half);
  return textureSampleLevel(cov_tex, samp, q, 0.0).r;
}

@fragment fn fs(v: VOut) -> @location(0) vec4<f32> {
  let mode = v.params.x;
  let blur = v.params.y;
  let outline = v.params.z;
  let ppt = max(v.params.w, 0.0001); // canvas px per atlas texel (scaled)
  var a = 1.0;
  if (mode < 0.5) {
    // Coverage atlas. Radius in atlas texels = canvas px / ppt.
    let texel = 1.0 / g.cov_size;
    if (blur > 0.5) {
      let rad = blur / ppt * texel;
      var acc = 0.0;
      var wsum = 0.0;
      for (var y = -2; y <= 2; y = y + 1) {
        for (var x = -2; x <= 2; x = x + 1) {
          let w = exp(-0.5 * f32(x * x + y * y) / 1.6);
          acc = acc + w * cov_at(v.uv + vec2<f32>(f32(x), f32(y)) * rad * 0.5, v.uvspan);
          wsum = wsum + w;
        }
      }
      a = acc / wsum;
    } else {
      a = cov_at(v.uv, v.uvspan);
    }
    if (outline > 0.0) {
      let rad = outline / ppt * texel;
      var d = 0.0;
      for (var k = 0; k < 16; k = k + 1) {
        let ang = f32(k) * 0.3926991;
        d = max(d, cov_at(v.uv + vec2<f32>(cos(ang), sin(ang)) * rad, v.uvspan));
      }
      a = clamp(d - a, 0.0, 1.0);
    }
  } else if (mode < 1.5) {
    let m = textureSampleLevel(msdf_tex, samp, v.uv, 0.0).rgb;
    let sd = median3(m) - 0.5;
    let dist = sd * g.msdf_range * ppt; // signed distance in canvas px (fdsm: value = d/range + 0.5)
    if (outline > 0.0) {
      let band = abs(dist) - outline * 0.5;
      a = clamp(0.5 - band, 0.0, 1.0);
    } else if (blur > 0.5) {
      a = smoothstep(-blur, blur, dist);
    } else {
      a = clamp(dist + 0.5, 0.0, 1.0);
    }
  }
  var col = v.color;
  if (v.extra.x >= 0.0) {
    let t = smoothstep(v.extra.x - 0.02, v.extra.x + 0.02, v.local.x);
    col = mix(v.color2, v.color, t);
  }
  return shade(GlyphIn(a, v.local, v.uv, col, v.color2, v.extra.y, v.extra.z));
}

@vertex fn fs_tri(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
  let x = f32((id << 1u) & 2u);
  let y = f32(id & 2u);
  return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
"#;

pub const BLUR: &str = r#"
struct B { dir: vec2<f32>, size: vec2<f32> };
@group(0) @binding(0) var<uniform> b: B;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@vertex fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
  let x = f32((id << 1u) & 2u);
  let y = f32(id & 2u);
  return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
  let uv = p.xy / b.size;
  var acc = vec4<f32>(0.0);
  var wsum = 0.0;
  for (var k = -12; k <= 12; k = k + 1) {
    let w = exp(-0.5 * f32(k * k) / 25.0);
    acc = acc + w * textureSampleLevel(src, samp, uv + b.dir * f32(k) / b.size, 0.0);
    wsum = wsum + w;
  }
  return acc / wsum;
}
"#;

pub const COMPOSITE: &str = r#"
@group(0) @binding(0) var glow: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct S { canvas: vec2<f32>, gain: f32, p: f32 };
@group(0) @binding(2) var<uniform> s: S;
@vertex fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
  let x = f32((id << 1u) & 2u);
  let y = f32(id & 2u);
  return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
  let c = textureSampleLevel(glow, samp, p.xy / s.canvas, 0.0) * s.gain;
  let a = clamp(max(c.r, max(c.g, c.b)), 0.0, 1.0);
  return vec4<f32>(c.rgb, a);
}
"#;

/// Premultiplied linear -> premultiplied gamma (sRGB encoded), for egui textures.
pub const RESOLVE_PREMUL: &str = r#"
@group(0) @binding(0) var premultiplied: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
  let x = f32((id << 1u) & 2u);
  let y = f32(id & 2u);
  return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
fn enc(c: f32) -> f32 {
  if (c <= 0.0031308) { return c * 12.92; }
  return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
  let c = textureLoad(premultiplied, vec2<i32>(p.xy), 0);
  if (c.a <= 0.00001) { return vec4<f32>(0.0); }
  let a = clamp(c.a, 0.0, 1.0);
  let s = clamp(c.rgb / c.a, vec3<f32>(0.0), vec3<f32>(1.0));
  return vec4<f32>(vec3<f32>(enc(s.r), enc(s.g), enc(s.b)) * a, a);
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Straight alpha, sRGB (FFmpeg overlay input).
    StraightSrgb,
    /// Premultiplied, sRGB-encoded colour (egui / OpenGL texture).
    PremulGamma,
}

pub const RESOLVE: &str = r#"
@group(0) @binding(0) var premultiplied: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
  let x = f32((id << 1u) & 2u);
  let y = f32(id & 2u);
  return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
  let c = textureLoad(premultiplied, vec2<i32>(p.xy), 0);
  if (c.a <= 0.00001) { return vec4<f32>(0.0); }
  return vec4<f32>(clamp(c.rgb / c.a, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(c.a, 0.0, 1.0));
}
"#;

/// Atlas uploads: what text layout and scenes need from either engine.
pub trait Upload {
    fn upload_cov(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]);
    fn upload_msdf(&self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]);
}

impl Upload for Renderer {
    fn upload_cov(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        self.upload_cov_impl(x, y, w, h, data)
    }
    fn upload_msdf(&self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        self.upload_msdf_impl(x, y, w, h, rgba)
    }
}

pub struct FrameTimes {
    /// Instance/uniform upload, encoding and command recording, before submit.
    pub pre_submit_ms: f64,
    pub gpu_ms: Option<f64>,
    pub submit_to_mapped_ms: f64,
    pub copy_out_ms: f64,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub width: u32,
    pub height: u32,
    pub adapter: String,
    pub cov: wgpu::Texture,
    pub msdf: wgpu::Texture,
    layer: wgpu::Texture,
    glow_a: wgpu::Texture,
    glow_b: wgpu::Texture,
    output: wgpu::Texture,
    readback: wgpu::Buffer,
    row_bytes: u32,
    inst_pipe: wgpu::RenderPipeline,
    glow_pipe: wgpu::RenderPipeline,
    blur_pipe: wgpu::RenderPipeline,
    comp_pipe: wgpu::RenderPipeline,
    resolve_pipe: wgpu::RenderPipeline,
    inst_bind: wgpu::BindGroup,
    inst_pl: wgpu::PipelineLayout,
    globals_buf: wgpu::Buffer,
    blur_h: wgpu::BindGroup,
    blur_v: wgpu::BindGroup,
    comp_bind: wgpu::BindGroup,
    comp_uniform: wgpu::Buffer,
    resolve_bind: wgpu::BindGroup,
    inst_buf: wgpu::Buffer,
    inst_cap: usize,
    queries: Option<(wgpu::QuerySet, wgpu::Buffer, wgpu::Buffer)>,
    nowait: bool,
    period_ns: f64,
    pub setup_ms: f64,
}

fn tex(device: &wgpu::Device, w: u32, h: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: Option<&wgpu::PipelineLayout>,
    vs: &str,
    fs: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
    strip: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout,
        vertex: wgpu::VertexState { module, entry_point: Some(vs), compilation_options: Default::default(), buffers },
        primitive: wgpu::PrimitiveState {
            topology: if strip { wgpu::PrimitiveTopology::TriangleStrip } else { wgpu::PrimitiveTopology::TriangleList },
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn attachment(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
    }
}

const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
    alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
};

impl Renderer {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        Self::with_canvas(width, height, width as f32, height as f32, Output::StraightSrgb)
    }

    /// `width`×`height` physical pixels showing a `logical_w`×`logical_h` canvas.
    pub fn with_canvas(width: u32, height: u32, logical_w: f32, logical_h: f32, out_mode: Output) -> Result<Self, String> {
        let started = Instant::now();
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = match std::env::var("NOH_PROTO_BACKEND").as_deref() {
            Ok("vulkan") => wgpu::Backends::VULKAN,
            _ => wgpu::Backends::DX12,
        };
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("adapter: {e}"))?;
        Self::from_adapter(adapter, started, width, height, logical_w, logical_h, out_mode)
    }

    /// wgpu on the caller's current OpenGL context (WGL). Safety: that context must be
    /// current on this thread for every call into this renderer, and when it is dropped.
    /// `restore` must make the caller's context current again: creating a wgpu GL
    /// instance on Windows creates (and then releases) a WGL context of its own.
    pub unsafe fn on_current_gl(loader: impl FnMut(&str) -> *const std::ffi::c_void, restore: impl Fn(), width: u32, height: u32, logical_w: f32, logical_h: f32) -> Result<Self, String> {
        let started = Instant::now();
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::GL;
        let instance = wgpu::Instance::new(descriptor);
        restore();
        let exposed = unsafe { wgpu::hal::gles::Adapter::new_external(loader, wgpu::GlBackendOptions::default()) }.ok_or("no external GL adapter")?;
        let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
        Self::from_adapter(adapter, started, width, height, logical_w, logical_h, Output::PremulGamma)
    }

    /// The GL texture holding the resolved output (GL backend only).
    pub fn output_gl_texture(&self) -> Option<eframe::glow::Texture> {
        let hal = unsafe { self.output.as_hal::<wgpu::hal::api::Gles>() }?;
        match &hal.inner {
            wgpu::hal::gles::TextureInner::Texture { raw, .. } => Some(*raw),
            _ => None,
        }
    }

    /// Record and submit one frame without readback and without waiting (GL variant):
    /// the result stays in the output texture for the caller's own drawing.
    pub fn frame_nowait(&mut self, glow: &[Inst], glow_gain: f32, main: &[Inst]) -> Result<f64, String> {
        self.nowait = true;
        let r = self.frame(glow, glow_gain, main, false);
        self.nowait = false;
        r.map(|(_, t)| t.pre_submit_ms)
    }

    fn from_adapter(adapter: wgpu::Adapter, started: Instant, width: u32, height: u32, logical_w: f32, logical_h: f32, out_mode: Output) -> Result<Self, String> {
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?}, driver {})", info.name, info.backend, info.driver_info);
        let want = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let ts = adapter.features().contains(want);
        let features = if ts { want } else { wgpu::Features::empty() };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: features,
            // GL: request exactly what the external adapter reports; DX12/Vulkan: defaults + 8K atlas.
            required_limits: if info.backend == wgpu::Backend::Gl { adapter.limits() } else { wgpu::Limits { max_texture_dimension_2d: COV_ATLAS, ..Default::default() } },
            ..Default::default()
        }))
        .map_err(|e| format!("device: {e}"))?;
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let layer = tex(&device, width, height, wgpu::TextureFormat::Rgba16Float, rt, "layer");
        let (gw, gh) = (width.div_ceil(2), height.div_ceil(2));
        let glow_a = tex(&device, gw, gh, wgpu::TextureFormat::Rgba16Float, rt, "glow a");
        let glow_b = tex(&device, gw, gh, wgpu::TextureFormat::Rgba16Float, rt, "glow b");
        let out_format = match out_mode {
            Output::StraightSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            Output::PremulGamma => wgpu::TextureFormat::Rgba8Unorm,
        };
        let output = tex(&device, width, height, out_format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, "output");
        let up = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let cov = tex(&device, COV_ATLAS, COV_ATLAS, wgpu::TextureFormat::R8Unorm, up, "coverage atlas");
        let msdf = tex(&device, MSDF_ATLAS, MSDF_ATLAS, wgpu::TextureFormat::Rgba8Unorm, up, "msdf atlas");
        let row_bytes = (width * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: row_bytes as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let mut globals = [0f32; 24];
        globals[..5].copy_from_slice(&[logical_w, logical_h, COV_ATLAS as f32, MSDF_ATLAS as f32, MSDF_RANGE]);
        let globals_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globals"),
            contents: bytemuck::cast_slice(&globals),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instances"),
            source: wgpu::ShaderSource::Wgsl(SHADER.replace("HOOKS", &crate::plugin_shader::hooks(None)).into()),
        });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let vbl = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Inst>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &attrs };
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let inst_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("instances"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let inst_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&inst_layout)], immediate_size: 0 });
        let inst_pipe = pipeline(&device, &module, Some(&inst_pl), "vs", "fs", wgpu::TextureFormat::Rgba16Float, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), &[Some(vbl.clone())], true);
        let glow_pipe = pipeline(&device, &module, Some(&inst_pl), "vs", "fs", wgpu::TextureFormat::Rgba16Float, Some(ADDITIVE), &[Some(vbl.clone())], true);
        let cov_view = cov.create_view(&Default::default());
        let msdf_view = msdf.create_view(&Default::default());
        let inst_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("instances"),
            layout: &inst_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&cov_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&msdf_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        // The glow pipeline has its own (identical) layout object; build its bind group too.
        let blur_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("blur"), source: wgpu::ShaderSource::Wgsl(BLUR.into()) });
        let blur_pipe = pipeline(&device, &blur_module, None, "vs", "fs", wgpu::TextureFormat::Rgba16Float, None, &[], false);
        let mk_blur = |dir: [f32; 2], src: &wgpu::Texture| {
            let u: [f32; 4] = [dir[0], dir[1], gw as f32, gh as f32];
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&u), usage: wgpu::BufferUsages::UNIFORM });
            let view = src.create_view(&Default::default());
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &blur_pipe.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            })
        };
        let blur_h = mk_blur([1.0, 0.0], &glow_a);
        let blur_v = mk_blur([0.0, 1.0], &glow_b);
        let comp_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("composite"), source: wgpu::ShaderSource::Wgsl(COMPOSITE.into()) });
        let comp_pipe = pipeline(&device, &comp_module, None, "vs", "fs", wgpu::TextureFormat::Rgba16Float, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), &[], false);
        let comp_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("composite"),
            contents: bytemuck::cast_slice(&[width as f32, height as f32, 1.0f32, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let glow_a_view = glow_a.create_view(&Default::default());
        let comp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &comp_pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&glow_a_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: comp_uniform.as_entire_binding() },
            ],
        });
        let resolve_src = match out_mode {
            Output::StraightSrgb => RESOLVE,
            Output::PremulGamma => RESOLVE_PREMUL,
        };
        let resolve_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("resolve"), source: wgpu::ShaderSource::Wgsl(resolve_src.into()) });
        let resolve_pipe = pipeline(&device, &resolve_module, None, "vs", "fs", out_format, None, &[], false);
        let layer_view = layer.create_view(&Default::default());
        let resolve_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &resolve_pipe.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&layer_view) }],
        });
        let inst_cap = 4096;
        let inst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (inst_cap * std::mem::size_of::<Inst>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let queries = ts.then(|| {
            let set = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("frame"), ty: wgpu::QueryType::Timestamp, count: 2 });
            let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
            let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            (set, resolve, read)
        });
        let period_ns = queue.get_timestamp_period() as f64;
        Ok(Self {
            device,
            queue,
            width,
            height,
            adapter: adapter_name,
            cov,
            msdf,
            layer,
            glow_a,
            glow_b,
            output,
            readback,
            row_bytes,
            inst_pipe,
            glow_pipe,
            blur_pipe,
            comp_pipe,
            resolve_pipe,
            inst_bind,
            inst_pl,
            globals_buf,
            blur_h,
            blur_v,
            comp_bind,
            comp_uniform,
            resolve_bind,
            inst_buf,
            inst_cap,
            queries,
            nowait: false,
            period_ns,
            setup_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// Rebuild the glyph pipelines with plugin hooks (validated beforehand). Errors
    /// are caught in an error scope and pinned on this plugin.
    pub fn set_hooks(&mut self, plugin_src: Option<&str>, user: [f32; 16]) -> Result<(), String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instances+plugin"),
            source: wgpu::ShaderSource::Wgsl(SHADER.replace("HOOKS", &crate::plugin_shader::hooks(plugin_src)).into()),
        });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let vbl = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Inst>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &attrs };
        let inst = pipeline(&self.device, &module, Some(&self.inst_pl), "vs", "fs", wgpu::TextureFormat::Rgba16Float, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), &[Some(vbl.clone())], true);
        let glow = pipeline(&self.device, &module, Some(&self.inst_pl), "vs", "fs", wgpu::TextureFormat::Rgba16Float, Some(ADDITIVE), &[Some(vbl)], true);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("plugin shader rejected by wgpu: {e}"));
        }
        self.inst_pipe = inst;
        self.glow_pipe = glow;
        self.queue.write_buffer(&self.globals_buf, 32, bytemuck::cast_slice(&user));
        Ok(())
    }

    fn upload_cov_impl(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.cov, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    fn upload_msdf_impl(&self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.msdf, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Render one frame. `glow` instances go through the blurred glow layer (gain
    /// applied at composite); `main` instances are drawn on top. Returns straight
    /// RGBA8 sRGB pixels when `read` is true.
    pub fn frame(&mut self, glow: &[Inst], glow_gain: f32, main: &[Inst], read: bool) -> Result<(Vec<u8>, FrameTimes), String> {
        let entered = Instant::now();
        let total = glow.len() + main.len();
        if total > self.inst_cap {
            self.inst_cap = total.next_power_of_two();
            self.inst_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instances"),
                size: (self.inst_cap * std::mem::size_of::<Inst>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !glow.is_empty() {
            self.queue.write_buffer(&self.inst_buf, 0, bytemuck::cast_slice(glow));
            self.queue.write_buffer(&self.comp_uniform, 8, bytemuck::cast_slice(&[glow_gain]));
        }
        if !main.is_empty() {
            self.queue.write_buffer(&self.inst_buf, (glow.len() * std::mem::size_of::<Inst>()) as u64, bytemuck::cast_slice(main));
        }
        let layer_view = self.layer.create_view(&Default::default());
        let ga = self.glow_a.create_view(&Default::default());
        let gb = self.glow_b.create_view(&Default::default());
        let out_view = self.output.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        if let Some((set, _, _)) = self.queries.as_ref().filter(|_| !self.nowait) {
            enc.write_timestamp(set, 0);
        }
        if !glow.is_empty() {
            {
                let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("glow"), color_attachments: &[Some(attachment(&ga))], ..Default::default() });
                p.set_pipeline(&self.glow_pipe);
                p.set_bind_group(0, &self.inst_bind, &[]);
                p.set_vertex_buffer(0, self.inst_buf.slice(..));
                p.draw(0..4, 0..glow.len() as u32);
            }
            {
                let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("blur h"), color_attachments: &[Some(attachment(&gb))], ..Default::default() });
                p.set_pipeline(&self.blur_pipe);
                p.set_bind_group(0, &self.blur_h, &[]);
                p.draw(0..3, 0..1);
            }
            {
                let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("blur v"), color_attachments: &[Some(attachment(&ga))], ..Default::default() });
                p.set_pipeline(&self.blur_pipe);
                p.set_bind_group(0, &self.blur_v, &[]);
                p.draw(0..3, 0..1);
            }
        }
        {
            let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("main"), color_attachments: &[Some(attachment(&layer_view))], ..Default::default() });
            if !glow.is_empty() {
                p.set_pipeline(&self.comp_pipe);
                p.set_bind_group(0, &self.comp_bind, &[]);
                p.draw(0..3, 0..1);
            }
            if !main.is_empty() {
                p.set_pipeline(&self.inst_pipe);
                p.set_bind_group(0, &self.inst_bind, &[]);
                p.set_vertex_buffer(0, self.inst_buf.slice(..));
                let first = glow.len() as u32;
                p.draw(0..4, first..first + main.len() as u32);
            }
        }
        {
            let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("resolve"), color_attachments: &[Some(attachment(&out_view))], ..Default::default() });
            p.set_pipeline(&self.resolve_pipe);
            p.set_bind_group(0, &self.resolve_bind, &[]);
            p.draw(0..3, 0..1);
        }
        if read {
            enc.copy_texture_to_buffer(
                self.output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.readback,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.row_bytes), rows_per_image: Some(self.height) },
                },
                wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
            );
        }
        if let Some((set, resolve, readq)) = self.queries.as_ref().filter(|_| !self.nowait) {
            enc.write_timestamp(set, 1);
            enc.resolve_query_set(set, 0..2, resolve, 0);
            enc.copy_buffer_to_buffer(resolve, 0, readq, 0, 16);
        }
        let cmd = enc.finish();
        let pre_submit_ms = entered.elapsed().as_secs_f64() * 1000.0;
        let submitted = Instant::now();
        let index = self.queue.submit([cmd]);
        if self.nowait {
            return Ok((Vec::new(), FrameTimes { pre_submit_ms, gpu_ms: None, submit_to_mapped_ms: 0.0, copy_out_ms: 0.0 }));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        if read {
            let tx = tx.clone();
            self.readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        }
        if let Some((_, _, readq)) = &self.queries {
            readq.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        }
        self.device
            .poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: Some(Duration::from_secs(5)) })
            .map_err(|e| format!("poll: {e}"))?;
        let expected = usize::from(read) + usize::from(self.queries.is_some());
        for _ in 0..expected {
            rx.recv_timeout(Duration::from_secs(5)).map_err(|e| format!("map: {e}"))?.map_err(|e| format!("map: {e}"))?;
        }
        let submit_to_mapped_ms = submitted.elapsed().as_secs_f64() * 1000.0;
        let copy_started = Instant::now();
        let mut pixels = Vec::new();
        if read {
            let view = self.readback.slice(..).get_mapped_range().map_err(|e| format!("read: {e}"))?;
            pixels.reserve_exact(self.width as usize * self.height as usize * 4);
            for row in view.chunks_exact(self.row_bytes as usize) {
                pixels.extend_from_slice(&row[..self.width as usize * 4]);
            }
            drop(view);
            self.readback.unmap();
        }
        let copy_out_ms = copy_started.elapsed().as_secs_f64() * 1000.0;
        let gpu_ms = if let Some((_, _, readq)) = &self.queries {
            let view = readq.slice(..).get_mapped_range().map_err(|e| format!("query: {e}"))?;
            let t: &[u64] = bytemuck::cast_slice(&view);
            let ms = t[1].saturating_sub(t[0]) as f64 * self.period_ns / 1e6;
            drop(view);
            readq.unmap();
            Some(ms)
        } else {
            None
        };
        Ok((pixels, FrameTimes { pre_submit_ms, gpu_ms, submit_to_mapped_ms, copy_out_ms }))
    }
}
