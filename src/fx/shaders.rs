//! Shared engine data: the glyph instance layout, atlas sizes and the WGSL sources
//! that naga translates to GLSL (see gl.rs).

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
    /// Rotation (radians), scale x, scale y, and the reveal clip: pixels whose local
    /// y (0 top, 1 bottom) is above it are not drawn (0 = no clip).
    pub xform: [f32; 4],
    /// Atlas rectangle u0, v0, u1, v1 (normalised).
    pub uv: [f32; 4],
    /// Premultiplied linear colour.
    pub color: [f32; 4],
    /// Second colour (karaoke wipe), premultiplied linear.
    pub color2: [f32; 4],
    /// Mode (0 coverage, 1 MSDF, 2 solid), blur, outline px (outline only), canvas
    /// px per atlas texel. Blur is a gaussian sigma in canvas px: on coverage glyphs
    /// of the main pass the renderer blurs them on a layer (gl.rs, capped at
    /// BLUR_MAX). Exceptions: the MSDF branch below keeps its own smoothstep, and a
    /// glow-pass instance keeps the old in-shader 5 x 5 kernel (the `blur > 0.5`
    /// coverage branch below) before the glow buffer's own blur.
    pub params: [f32; 4],
    /// Wipe split in local x (0..1; negative = no wipe), time, word progress and
    /// glyph index (the last three for plugin shader hooks).
    pub extra: [f32; 4],
    /// Soft wipe edge width in local x (0 = the engine's hard edge), the pass
    /// being drawn (PASS_*), and the line's seconds since it showed and until it
    /// goes (the last three for plugin shader hooks).
    pub aux: [f32; 4],
    /// For plugin shader hooks: the left and right edges of the instance's row in
    /// the frame (0..1 across), the line's seconds since its sung end (negative
    /// before), and a spare zero.
    pub lane: [f32; 4],
}

/// `GlyphIn.pass_kind` values: what an instance draws.
pub const PASS_FILL: f32 = 0.0;
pub const PASS_OUTLINE: f32 = 1.0;
pub const PASS_SHADOW: f32 = 2.0;
pub const PASS_GLOW: f32 = 3.0;
/// A plate, or a compose-mode shape.
pub const PASS_PLATE: f32 = 4.0;

/// Engine glyph shader; `HOOKS` is replaced by the plugin hooks (or the defaults).
pub const SHADER: &str = r#"
struct Globals { canvas: vec2<f32>, cov_size: f32, msdf_size: f32, msdf_range: f32, p0: f32, p1: f32, p2: f32, user: array<vec4<f32>, 4> };
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var cov_tex: texture_2d<f32>;
@group(0) @binding(2) var msdf_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
struct GlyphIn { coverage: f32, local: vec2<f32>, uv: vec2<f32>, color: vec4<f32>, color2: vec4<f32>, time: f32, progress: f32, frame: vec2<f32>, index: f32, line: f32, line_time: vec2<f32>, pass_kind: f32, span: vec2<f32>, line_end: f32 };
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
  @location(8) aux: vec4<f32>,
  @location(9) lane: vec4<f32>,
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
  @location(7) clip: f32,
  @location(8) frame: vec2<f32>,
  @location(9) @interpolate(flat) aux: vec4<f32>,
  @location(10) @interpolate(flat) lane: vec4<f32>,
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
  o.clip = i.xform.w;
  // Where the pixel is in the frame, 0..1 across and down: the same at every
  // pixel scale (the canvas is the export's size in the preview too).
  o.frame = r / g.canvas;
  o.aux = i.aux;
  o.lane = i.lane;
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
      // Reached only by glow-pass instances: main-pass blur runs on a layer (gl.rs).
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
  } else if (v.uvspan.w > 0.0) {
    // A rounded plate: uvspan holds its width, height and corner radius and the
    // anti-aliasing width, in canvas px; the quad is the plate grown by that width.
    let quad = v.uvspan.xy + vec2<f32>(2.0 * v.uvspan.w);
    let p = (v.local - vec2<f32>(0.5)) * quad;
    let half = v.uvspan.xy * 0.5;
    let r = min(v.uvspan.z, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    a = clamp(0.5 - d / v.uvspan.w, 0.0, 1.0);
  }
  // Reveal clip (line reveal_y): nothing above the clip line.
  if (v.local.y < v.clip) {
    a = 0.0;
  }
  var col = v.color;
  if (v.aux.x > 0.0) {
    // A soft edge (aux.x, its width in local x); its split may lie left of the glyph.
    let h = 0.5 * v.aux.x;
    let t = smoothstep(v.extra.x - h, v.extra.x + h, v.local.x);
    col = mix(v.color2, v.color, t);
  } else if (v.extra.x >= 0.0) {
    let t = smoothstep(v.extra.x - 0.02, v.extra.x + 0.02, v.local.x);
    col = mix(v.color2, v.color, t);
  }
  let shown = v.aux.z + v.aux.w;
  let line = select(0.0, clamp(v.aux.z / shown, 0.0, 1.0), shown > 0.0);
  return shade(GlyphIn(a, v.local, v.uv, col, v.color2, v.extra.y, v.extra.z, v.frame, v.extra.w, line, v.aux.zw, v.aux.y, v.lane.xy, v.lane.z));
}

@vertex fn fs_tri(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
  let x = f32((id << 1u) & 2u);
  let y = f32(id & 2u);
  return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
"#;

pub const BLUR: &str = r#"
struct B { dir: vec2<f32>, size: vec2<f32>, sigma: f32, p0: f32, p1: f32, p2: f32 };
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
    let w = exp(-0.5 * f32(k * k) / (b.sigma * b.sigma));
    acc = acc + w * textureSampleLevel(src, samp, uv + b.dir * f32(k) / b.size, 0.0);
    wsum = wsum + w;
  }
  return acc / wsum;
}
"#;

/// Halves a layer: each texel the 2 x 2 average of its source (one bilinear tap
/// at their shared corner). `b.size` is the source size.
pub const DOWNSAMPLE: &str = r#"
struct B { dir: vec2<f32>, size: vec2<f32>, sigma: f32, p0: f32, p1: f32, p2: f32 };
@group(0) @binding(0) var<uniform> b: B;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@vertex fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
  let x = f32((id << 1u) & 2u);
  let y = f32(id & 2u);
  return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
  return textureSampleLevel(src, samp, p.xy * 2.0 / b.size, 0.0);
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
  // p = 1: a premultiplied layer (blurred glyphs), drawn over as it is.
  if (s.p > 0.5) {
    return c;
  }
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

pub struct FrameTimes {
    /// Instance/uniform upload, encoding and command recording, before submit.
    pub pre_submit_ms: f64,
    pub gpu_ms: Option<f64>,
    pub submit_to_mapped_ms: f64,
    pub copy_out_ms: f64,
}
