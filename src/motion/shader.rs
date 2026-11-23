//! Shaders written by the scene's author for a layer: WGSL that defines
//! `fn layer(i: LayerIn) -> vec4<f32>`, run once per pixel of the layer, reading
//! only the layer's own content and the inputs the header gives (never the
//! video). Validated by naga with every check and no optional capability, then
//! bounded by counts on its syntax tree (no loops, helpers without recursion,
//! reads and cost counted through calls), then compiled to GLSL 330 with every
//! index and texel fetch clamped into range, and wrapped so that whatever it
//! returns, the output is a finite premultiplied colour.
use glow::HasContext;
use std::collections::HashMap;
use std::sync::Arc;

/// What the author's shader sees. Nothing else is bound.
pub const HEADER: &str = r#"
struct LayerIn {
  uv: vec2<f32>,
  px: vec2<f32>,
  size: vec2<f32>,
  time: f32,
  energy: f32,
  onset_age: f32,
  k0: vec4<f32>,
  k1: vec4<f32>,
};
@group(0) @binding(0) var content: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
"#;

/// NOH's own part: the uniforms, the full-frame triangle, and the output clamp (a value that is not finite becomes transparent; colour from 0
/// to 1 and never above its opacity).
const WRAPPER: &str = r#"
struct LayerU { a: vec4<f32>, b: vec4<f32>, k0: vec4<f32>, k1: vec4<f32> };
@group(0) @binding(2) var<uniform> u: LayerU;
@vertex fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
  let x = f32((id << 1u) & 2u);
  let y = f32(id & 2u);
  return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
  var i: LayerIn;
  i.px = p.xy;
  i.size = u.a.xy;
  i.uv = p.xy / u.a.xy;
  i.time = u.a.z;
  i.energy = u.a.w;
  i.onset_age = u.b.x;
  i.k0 = u.k0;
  i.k1 = u.k1;
  let c = layer(i);
  if (!all(abs(c) < vec4<f32>(1e30))) {
    return vec4<f32>(0.0);
  }
  let a = clamp(c.a, 0.0, 1.0);
  return vec4<f32>(min(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(a)), a);
}
"#;

/// Source of one shader.
pub const MAX_SOURCE: usize = 8 * 1024;
/// Helper functions besides `layer`.
pub const MAX_HELPERS: usize = 16;
/// Texture reads a pixel may make at most, every call expanded.
pub const MAX_READS: usize = 12;
/// Cost a pixel may spend at most, every call expanded: each expression node
/// costs the components of its result (a vec4 four), a product involving a
/// matrix four times that; both sides of a branch are counted.
pub const MAX_COST: usize = 2500;
/// Shader layers in one frame.
pub const MAX_SHADER_LAYERS: usize = 4;
/// Pixels the shader layers of a frame compute, in frames (a half-resolution
/// layer counts a quarter).
pub const MAX_SHADER_AREA: f32 = 1.5;
/// The largest array a shader may declare.
const MAX_ARRAY: u32 = 64;

/// What validation found: the counts it bounds, and what it only warns about.
#[derive(Clone, Debug)]
pub struct Checked {
    pub reads: usize,
    pub cost: usize,
    pub helpers: usize,
    pub warnings: Vec<String>,
}

/// Each function's own reads, own cost and the functions it calls.
type Own = HashMap<naga::Handle<naga::Function>, (usize, usize, Vec<naga::Handle<naga::Function>>)>;

/// What one walk of a function's statements collects.
#[derive(Default)]
struct Walk {
    calls: Vec<naga::Handle<naga::Function>>,
    /// Calls made under a branch.
    branch_calls: Vec<naga::Handle<naga::Function>>,
    /// Expressions evaluated under a branch.
    branch_exprs: Vec<naga::Handle<naga::Expression>>,
}

fn block_ok(block: &naga::Block, depth: usize, branch: bool, w: &mut Walk) -> Result<(), String> {
    if depth > 12 {
        return Err("control nesting exceeds twelve".into());
    }
    for s in block.iter() {
        match s {
            naga::Statement::Loop { .. } => {
                return Err("loops are not allowed (write the steps out)".into());
            }
            naga::Statement::Call { function, .. } => {
                w.calls.push(*function);
                if branch {
                    w.branch_calls.push(*function);
                }
            }
            naga::Statement::Block(b) => block_ok(b, depth + 1, branch, w)?,
            naga::Statement::If { accept, reject, .. } => {
                block_ok(accept, depth + 1, true, w)?;
                block_ok(reject, depth + 1, true, w)?;
            }
            naga::Statement::Switch { cases, .. } => {
                for c in cases {
                    block_ok(&c.body, depth + 1, true, w)?;
                }
            }
            naga::Statement::Emit(range) => {
                if branch {
                    w.branch_exprs.extend(range.clone());
                }
            }
            naga::Statement::Store { .. }
            | naga::Statement::Return { .. }
            | naga::Statement::Kill => {}
            other => return Err(format!("statement not allowed: {other:?}")),
        }
    }
    Ok(())
}

/// Components a value of this type holds: what one node of it costs.
fn components(t: &naga::TypeInner) -> usize {
    match t {
        naga::TypeInner::Vector { size, .. } => *size as usize,
        naga::TypeInner::Matrix { columns, rows, .. } => *columns as usize * *rows as usize,
        _ => 1,
    }
}

fn is_matrix(
    f: &naga::Function,
    info: &naga::valid::FunctionInfo,
    types: &naga::UniqueArena<naga::Type>,
    e: naga::Handle<naga::Expression>,
) -> bool {
    let _ = f;
    matches!(info[e].ty.inner_with(types), naga::TypeInner::Matrix { .. })
}

/// Validate an author's shader. Returns its counts.
pub fn validate(src: &str) -> Result<Checked, String> {
    if src.len() > MAX_SOURCE {
        return Err(format!(
            "a shader of {} bytes exceeds {MAX_SOURCE}",
            src.len()
        ));
    }
    let full = format!("{HEADER}\n{src}");
    let first = naga::front::wgsl::parse_str(&full).map_err(|e| e.emit_to_string(&full))?;
    let module = &first;
    if !module.entry_points.is_empty() {
        return Err("a layer shader declares no entry point".into());
    }
    if module.global_variables.len() != 2 {
        return Err("a layer shader declares no bindings or globals of its own".into());
    }
    for (_, t) in module.types.iter() {
        if let naga::TypeInner::Array { size, .. } = t.inner {
            match size {
                naga::ArraySize::Constant(n) if n.get() <= MAX_ARRAY => {}
                _ => {
                    return Err(format!(
                        "arrays of more than {MAX_ARRAY} elements are not allowed"
                    ));
                }
            }
        }
    }
    if !module
        .functions
        .iter()
        .any(|(_, f)| f.name.as_deref() == Some("layer"))
    {
        return Err("a layer shader defines fn layer(i: LayerIn) -> vec4<f32>".into());
    }
    // The whole program as it will run: every check, no optional capability.
    // Its analysis gives each expression's type, to weigh the cost.
    let program = format!("{HEADER}\n{src}\n{WRAPPER}");
    let m = naga::front::wgsl::parse_str(&program).map_err(|e| e.emit_to_string(&program))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&m)
    .map_err(|e| e.emit_to_string(&program))?;
    let module = m;
    let mut own: Own = HashMap::new();
    let mut layer = None;
    let mut warnings = Vec::new();
    let mut auto_sample: Vec<naga::Handle<naga::Function>> = Vec::new();
    let mut walks = Vec::new();
    for (h, f) in module.functions.iter() {
        let name = f.name.clone().unwrap_or_default();
        if name == "layer" {
            layer = Some(h);
        }
        let mut w = Walk::default();
        block_ok(&f.body, 0, false, &mut w)?;
        let fi = &info[h];
        let reads = f
            .expressions
            .iter()
            .filter(|(_, e)| {
                matches!(
                    e,
                    naga::Expression::ImageSample { .. } | naga::Expression::ImageLoad { .. }
                )
            })
            .count();
        // The cost of a node is the components of its result; a product that
        // involves a matrix costs four times that (a 4 x 4 product is 64
        // multiply-adds, as many as its result has components times four).
        let mut cost = 0usize;
        for (eh, e) in f.expressions.iter() {
            let c = components(fi[eh].ty.inner_with(&module.types));
            let k = match e {
                naga::Expression::Binary {
                    op: naga::BinaryOperator::Multiply,
                    left,
                    right,
                } if is_matrix(f, fi, &module.types, *left)
                    || is_matrix(f, fi, &module.types, *right) =>
                {
                    4
                }
                _ => 1,
            };
            cost += c * k;
        }
        let implicit = |eh: &naga::Handle<naga::Expression>| {
            matches!(
                f.expressions[*eh],
                naga::Expression::ImageSample {
                    level: naga::SampleLevel::Auto | naga::SampleLevel::Bias(_),
                    ..
                }
            )
        };
        if f.expressions.iter().any(|(eh, _)| implicit(&eh)) {
            auto_sample.push(h);
        }
        if w.branch_exprs.iter().any(implicit) {
            warnings.push(format!(
                "{name}: textureSample (implicit level) under a branch is undefined where neighbouring pixels skip it; use textureSampleLevel"
            ));
        }
        own.insert(h, (reads, cost, w.calls.clone()));
        walks.push((name, w));
    }
    for (name, w) in &walks {
        if w.branch_calls.iter().any(|c| auto_sample.contains(c)) {
            warnings.push(format!(
                "{name}: calls under a branch a function that uses textureSample (implicit level): undefined where neighbouring pixels skip it"
            ));
        }
    }
    let layer = layer.ok_or("a layer shader defines fn layer(i: LayerIn) -> vec4<f32>")?;
    let helpers = own.len() - 1;
    if helpers > MAX_HELPERS {
        return Err(format!("{helpers} helper functions exceed {MAX_HELPERS}"));
    }
    // Reads and cost through calls; a cycle (recursion) is refused.
    fn total(
        h: naga::Handle<naga::Function>,
        own: &Own,
        stack: &mut Vec<naga::Handle<naga::Function>>,
        memo: &mut HashMap<naga::Handle<naga::Function>, (usize, usize)>,
    ) -> Result<(usize, usize), String> {
        if let Some(v) = memo.get(&h) {
            return Ok(*v);
        }
        if stack.contains(&h) {
            return Err("a function calls itself (recursion is not allowed)".into());
        }
        stack.push(h);
        let (mut r, mut c, calls) = own[&h].clone();
        for callee in calls {
            let (cr, cc) = total(callee, own, stack, memo)?;
            r = r.saturating_add(cr);
            c = c.saturating_add(cc);
        }
        stack.pop();
        memo.insert(h, (r, c));
        Ok((r, c))
    }
    let (reads, cost) = total(layer, &own, &mut Vec::new(), &mut HashMap::new())?;
    if reads > MAX_READS {
        return Err(format!(
            "{reads} texture reads per pixel (every call counted) exceed {MAX_READS}"
        ));
    }
    if cost > MAX_COST {
        return Err(format!(
            "a cost of {cost} per pixel (every call counted) exceeds {MAX_COST}"
        ));
    }
    Ok(Checked {
        reads,
        cost,
        helpers,
        warnings,
    })
}

/// Every index into an array, vector or matrix, and every texel fetch, is
/// clamped into range in the generated code: an author's computed index cannot
/// read outside, whatever the driver's own robustness.
fn restricted() -> naga::proc::BoundsCheckPolicies {
    use naga::proc::BoundsCheckPolicy::Restrict;
    naga::proc::BoundsCheckPolicies {
        index: Restrict,
        buffer: Restrict,
        image_load: Restrict,
        binding_array: Restrict,
    }
}

/// The fragment stage's GLSL as it will be compiled (for the tests).
#[cfg(test)]
pub fn fragment_glsl(src: &str) -> Result<String, String> {
    use naga::back::glsl;
    let program = format!("{HEADER}\n{src}\n{WRAPPER}");
    let module = naga::front::wgsl::parse_str(&program).map_err(|e| e.emit_to_string(&program))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| format!("{e:?}"))?;
    let options = glsl::Options {
        version: glsl::Version::Desktop(330),
        writer_flags: glsl::WriterFlags::empty(),
        binding_map: Default::default(),
        zero_initialize_workgroup_memory: false,
    };
    let po = glsl::PipelineOptions {
        shader_stage: naga::ShaderStage::Fragment,
        entry_point: "fs".into(),
        multiview: None,
    };
    let mut out = String::new();
    let mut w = glsl::Writer::new(&mut out, &module, &info, &options, &po, restricted())
        .map_err(|e| format!("{e:?}"))?;
    w.write().map_err(|e| format!("{e:?}"))?;
    Ok(out)
}

/// A compiled layer shader.
pub struct Compiled {
    pub prog: glow::Program,
    pub ubo_binding: u32,
    pub compile_ms: f64,
    pub checked: Checked,
}

/// Compile (WGSL to GLSL 330 through naga, then the driver), and draw once on a
/// small target so that the driver's own compilation is paid and measured here.
pub fn compile(gl: &Arc<glow::Context>, src: &str) -> Result<Compiled, String> {
    use naga::back::glsl;
    let checked = validate(src)?;
    let started = std::time::Instant::now();
    let program = format!("{HEADER}\n{src}\n{WRAPPER}");
    let module = naga::front::wgsl::parse_str(&program).map_err(|e| e.emit_to_string(&program))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| format!("{e:?}"))?;
    let options = glsl::Options {
        version: glsl::Version::Desktop(330),
        writer_flags: glsl::WriterFlags::empty(),
        binding_map: Default::default(),
        zero_initialize_workgroup_memory: false,
    };
    let (mut blocks, mut textures, mut outputs) = (Vec::new(), Vec::new(), Vec::new());
    unsafe {
        let prog = gl.create_program().map_err(|e| format!("program: {e}"))?;
        let mut shaders = Vec::new();
        for (stage, entry, kind) in [
            (naga::ShaderStage::Vertex, "vs", glow::VERTEX_SHADER),
            (naga::ShaderStage::Fragment, "fs", glow::FRAGMENT_SHADER),
        ] {
            let po = glsl::PipelineOptions {
                shader_stage: stage,
                entry_point: entry.into(),
                multiview: None,
            };
            let mut out = String::new();
            let mut w = glsl::Writer::new(&mut out, &module, &info, &options, &po, restricted())
                .map_err(|e| format!("glsl writer: {e:?}"))?;
            let r = w.write().map_err(|e| format!("glsl write: {e:?}"))?;
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
            if stage == naga::ShaderStage::Fragment {
                for (name, loc) in r.varying {
                    outputs.push((name, loc.location));
                }
            }
            let sh = gl.create_shader(kind).map_err(|e| format!("shader: {e}"))?;
            gl.shader_source(sh, &out);
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                return Err(format!(
                    "the driver refused the shader: {}",
                    gl.get_shader_info_log(sh)
                ));
            }
            gl.attach_shader(prog, sh);
            shaders.push(sh);
        }
        for (name, loc) in &outputs {
            gl.bind_frag_data_location(prog, *loc, name);
        }
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) {
            return Err(format!(
                "the driver could not link the shader: {}",
                gl.get_program_info_log(prog)
            ));
        }
        for sh in shaders {
            gl.detach_shader(prog, sh);
            gl.delete_shader(sh);
        }
        gl.use_program(Some(prog));
        let mut ubo_binding = 2;
        for (name, b) in &blocks {
            let idx = gl
                .get_uniform_block_index(prog, name)
                .ok_or_else(|| format!("no uniform block {name}"))?;
            // Binding points 8 and up, out of the engine's way.
            ubo_binding = 8 + b;
            gl.uniform_block_binding(prog, idx, ubo_binding);
        }
        for (name, b) in &textures {
            if let Some(loc) = gl.get_uniform_location(prog, name) {
                gl.uniform_1_i32(Some(&loc), *b as i32);
            }
        }
        gl.use_program(None);
        Ok(Compiled {
            prog,
            ubo_binding,
            compile_ms: started.elapsed().as_secs_f64() * 1000.0,
            checked,
        })
    }
}

/// The layer shaders beside a scene, compiled on one context; their programs
/// are deleted with it.
pub struct ShaderSet {
    gl: Arc<glow::Context>,
    compiled: HashMap<String, Compiled>,
}

impl ShaderSet {
    /// No shader (a scene without a shaders folder).
    pub fn empty(gl: &Arc<glow::Context>) -> Self {
        Self {
            gl: gl.clone(),
            compiled: HashMap::new(),
        }
    }

    /// Compile every `(name, source)`: validated again, translated, compiled
    /// and linked by the driver. A refusal names the shader.
    pub fn compile(gl: &Arc<glow::Context>, sources: &[(String, String)]) -> Result<Self, String> {
        // Tests stand for a driver that dies compiling a shader.
        if !sources.is_empty()
            && std::env::var_os("NOH_TEST_SHADER_ABORT").is_some_and(|v| !v.is_empty())
        {
            std::process::abort();
        }
        let mut set = Self::empty(gl);
        for (name, src) in sources {
            let c = compile(gl, src).map_err(|e| format!("shader {name:?}: {e}"))?;
            set.compiled.insert(name.clone(), c);
        }
        Ok(set)
    }

    pub fn get(&self, name: &str) -> Option<&Compiled> {
        self.compiled.get(name)
    }

    /// Names in order, with what each one's validation counted.
    pub fn list(&self) -> Vec<(&str, &Compiled)> {
        let mut v: Vec<(&str, &Compiled)> =
            self.compiled.iter().map(|(k, c)| (k.as_str(), c)).collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }
}

impl Drop for ShaderSet {
    fn drop(&mut self) {
        for c in self.compiled.values() {
            unsafe { self.gl.delete_program(c.prog) };
        }
    }
}

/// Files of a scene's shaders folder.
pub const MAX_FILES: usize = 8;

/// The shaders folder beside a scene (`<scene without extension>.shaders`), and
/// its `.wgsl` files as `(name, source)`, each validated. No folder: none. A
/// file over the size cap, more than eight files, or a shader the validator
/// refuses: the scene is refused with the shader's name and the reason.
pub fn read_beside(scene: &std::path::Path) -> Result<Vec<(String, String)>, String> {
    let dir = scene.with_extension("shaders");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in entries {
        let path = e.map_err(|e| format!("shaders folder: {e}"))?.path();
        if path.extension().and_then(|x| x.to_str()) != Some("wgsl") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("a shader's file name is not text")?
            .to_string();
        let len = std::fs::metadata(&path)
            .map_err(|e| format!("shader {name:?}: {e}"))?
            .len();
        if len > MAX_SOURCE as u64 {
            return Err(format!(
                "shader {name:?}: a shader of {len} bytes exceeds {MAX_SOURCE}"
            ));
        }
        let src = std::fs::read_to_string(&path).map_err(|e| format!("shader {name:?}: {e}"))?;
        validate(&src).map_err(|e| format!("shader {name:?}: {e}"))?;
        out.push((name, src));
        if out.len() > MAX_FILES {
            return Err(format!("more than {MAX_FILES} shaders beside the scene"));
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = "fn layer(i: LayerIn) -> vec4<f32> { \
        let c = textureSampleLevel(content, samp, i.uv, 0.0); \
        return c * (0.5 + 0.5 * sin(i.time + i.uv.x * 6.0)); }";

    fn refused(src: &str, why: &str) {
        match validate(src) {
            Ok(_) => panic!("accepted: {src}"),
            Err(e) => assert!(e.contains(why), "{src}\n  refused for another reason: {e}"),
        }
    }

    #[test]
    fn a_plain_shader_passes() {
        let c = validate(OK).unwrap();
        assert_eq!(c.reads, 1);
        assert!(c.cost < 100, "{}", c.cost);
        // Helpers, branches and a switch are allowed.
        validate(
            "fn h(x: f32) -> f32 { return x * x; } \
             fn layer(i: LayerIn) -> vec4<f32> { var v = h(i.uv.x); if (v > 0.5) { v = 1.0 - v; } \
             switch (i32(i.k0.x)) { case 1: { v = v * 2.0; } default: {} } return vec4<f32>(v); }",
        )
        .unwrap();
    }

    /// An index or a texel fetch out of range is legal WGSL; the generated code
    /// clamps it, so it reads something defined.
    #[test]
    fn out_of_range_accesses_are_clamped() {
        let arr = "fn layer(i: LayerIn) -> vec4<f32> { var a: array<f32, 8>; a[0] = 0.5; \
            return vec4<f32>(a[i32(i.k0.x * 1e9)]); }";
        let load = "fn layer(i: LayerIn) -> vec4<f32> { \
            return textureLoad(content, vec2<i32>(-1000000000, 1000000000), 0); }";
        for src in [arr, load] {
            validate(src).unwrap();
            let g = fragment_glsl(src).unwrap();
            assert!(
                g.contains("clamp(") || g.contains("min("),
                "no clamp in:\n{g}"
            );
        }
    }

    /// Vector and matrix nodes cost their components.
    #[test]
    fn cost_counts_components() {
        let f = validate("fn layer(i: LayerIn) -> vec4<f32> { let a = i.uv.x; let b = a * a * a * a; return vec4<f32>(b); }").unwrap();
        let v = validate(
            "fn layer(i: LayerIn) -> vec4<f32> { let a = i.k0; let b = a * a * a * a; return b; }",
        )
        .unwrap();
        let m = validate(
            "fn layer(i: LayerIn) -> vec4<f32> { let a = mat4x4<f32>(i.k0, i.k1, i.k0, i.k1); \
            let b = a * a; return b[0]; }",
        )
        .unwrap();
        assert!(v.cost > f.cost, "{} {}", v.cost, f.cost);
        assert!(m.cost > v.cost + 64, "{} {}", m.cost, v.cost);
    }

    /// An implicit-level read under a branch is said.
    #[test]
    fn implicit_reads_under_a_branch_are_said() {
        let w = validate("fn layer(i: LayerIn) -> vec4<f32> { if (i.uv.x > 0.5) { return textureSample(content, samp, i.uv); } \
            return vec4<f32>(0.0); }").unwrap();
        assert!(
            w.warnings.iter().any(|x| x.contains("under a branch")),
            "{:?}",
            w.warnings
        );
        let ok = validate("fn layer(i: LayerIn) -> vec4<f32> { if (i.uv.x > 0.5) { return textureSampleLevel(content, samp, i.uv, 0.0); } \
            return vec4<f32>(0.0); }").unwrap();
        assert!(ok.warnings.is_empty(), "{:?}", ok.warnings);
        let via = validate("fn r(uv: vec2<f32>) -> vec4<f32> { return textureSample(content, samp, uv); } \
            fn layer(i: LayerIn) -> vec4<f32> { if (i.uv.x > 0.5) { return r(i.uv); } return vec4<f32>(0.0); }").unwrap();
        assert!(
            via.warnings
                .iter()
                .any(|x| x.contains("calls under a branch")),
            "{:?}",
            via.warnings
        );
    }

    /// The shaders kept with the examples pass; the heaviest sits at the caps.
    #[test]
    fn the_example_shaders_pass() {
        validate(include_str!(
            "../../tests/fixtures/motion/glow.shaders/field.wgsl"
        ))
        .unwrap();
        validate(include_str!(
            "../../tests/fixtures/motion/glow.shaders/aura.wgsl"
        ))
        .unwrap();
        for src in [
            include_str!("../../tests/fixtures/motion/worst_shader_scalar.shaders/heavy.wgsl"),
            include_str!("../../tests/fixtures/motion/worst_shader_vector.shaders/heavy.wgsl"),
            include_str!("../../tests/fixtures/motion/worst_shader_random.shaders/heavy.wgsl"),
            include_str!("../../tests/fixtures/motion/worst_shader_both.shaders/heavy.wgsl"),
        ] {
            let h = validate(src).unwrap();
            assert_eq!(h.reads, MAX_READS);
            assert!(h.cost * 100 > MAX_COST * 90, "{}", h.cost);
        }
    }

    /// The corpus of hostile shaders: every one is refused, for its reason.
    #[test]
    fn hostile_shaders_are_refused() {
        refused(
            "fn nope(i: LayerIn) -> vec4<f32> { return vec4<f32>(0.0); }",
            "defines fn layer",
        );
        refused(
            "fn layer(i: LayerIn) -> vec4<f32> { var v = 0.0; for (var k = 0; k < 4; k++) { v += 0.1; } return vec4<f32>(v); }",
            "loops are not allowed",
        );
        refused(
            "fn layer(i: LayerIn) -> vec4<f32> { var v = 0.0; loop { v += 0.1; if (v > 1.0) { break; } } return vec4<f32>(v); }",
            "loops are not allowed",
        );
        refused(
            "fn layer(i: LayerIn) -> vec4<f32> { var v = 0.0; while (v < 1.0) { v += 0.1; } return vec4<f32>(v); }",
            "loops are not allowed",
        );
        // A binding of its own (no way to read the video or anything else).
        refused(
            "@group(0) @binding(5) var video: texture_2d<f32>; \
             fn layer(i: LayerIn) -> vec4<f32> { return textureSampleLevel(video, samp, i.uv, 0.0); }",
            "no bindings",
        );
        refused(
            "var<private> state: f32; fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(state); }",
            "no bindings",
        );
        refused(
            "@fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); } \
             fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(0.0); }",
            "no entry point",
        );
        // Reads counted through calls: 4 reads in a helper called 5 times.
        refused(
            "fn r(uv: vec2<f32>) -> vec4<f32> { return textureSampleLevel(content, samp, uv, 0.0) + \
             textureSampleLevel(content, samp, uv + 0.01, 0.0) + textureSampleLevel(content, samp, uv + 0.02, 0.0) + \
             textureSampleLevel(content, samp, uv + 0.03, 0.0); } \
             fn layer(i: LayerIn) -> vec4<f32> { return r(i.uv) + r(i.uv * 0.5) + r(i.uv * 0.25) + r(i.uv * 0.1) + r(i.uv * 0.2); }",
            "texture reads",
        );
        // Cost counted through calls: a helper called many times, nested.
        let mut h = String::from(
            "fn h0(x: f32) -> f32 { return sin(x) * cos(x) + sqrt(abs(x)) * exp(-x * x) + pow(abs(x), 1.3); } ",
        );
        for k in 1..6 {
            h += &format!(
                "fn h{k}(x: f32) -> f32 {{ return h{p}(x) + h{p}(x + 1.0) + h{p}(x * 2.0) + h{p}(x - 0.5); }} ",
                p = k - 1
            );
        }
        h += "fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(h5(i.uv.x)); }";
        refused(&h, "a cost of");
        // Seventeen helpers.
        let mut many = String::new();
        for k in 0..17 {
            many += &format!("fn f{k}(x: f32) -> f32 {{ return x + {k}.0; }} ");
        }
        many += "fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(f0(i.uv.x)); }";
        refused(&many, "helper functions exceed");
        // Recursion (WGSL forbids it; refused either way).
        let rec = "fn f(x: f32) -> f32 { return f(x - 1.0); } fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(f(1.0)); }";
        assert!(validate(rec).is_err());
        refused(
            "fn layer(i: LayerIn) -> vec4<f32> { var a: array<f32, 100000>; a[3] = 1.0; return vec4<f32>(a[3]); }",
            "arrays of more than",
        );
        refused(
            &format!(
                "fn layer(i: LayerIn) -> vec4<f32> {{ return vec4<f32>(0.0); }} // {}",
                "x".repeat(9000)
            ),
            "exceeds 8192",
        );
        // An invalid program is refused by naga.
        refused("fn layer(i: LayerIn) -> vec4<f32> { return 1.0; }", "");
    }
}
