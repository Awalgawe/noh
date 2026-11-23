//! Black-box (level 3) plugin shaders: validation profile and the hook points
//! the engine splices into its glyph shader (archi-greffons-v3 §3.7 and §4).

/// What plugin code can see. Mirrors the bindings and helper types of gpu.rs SHADER.
pub const HEADER: &str = r#"
struct Globals { canvas: vec2<f32>, cov_size: f32, msdf_size: f32, msdf_range: f32, p0: f32, p1: f32, p2: f32, user: array<vec4<f32>, 4> };
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var cov_tex: texture_2d<f32>;
@group(0) @binding(2) var msdf_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
struct GlyphIn { coverage: f32, local: vec2<f32>, uv: vec2<f32>, color: vec4<f32>, color2: vec4<f32>, time: f32, progress: f32, frame: vec2<f32>, index: f32, line: f32, line_time: vec2<f32>, pass_kind: f32 };
struct VertexIn { corner: vec2<f32>, center: vec2<f32>, time: f32, index: f32 };
"#;

/// Default hooks: plain premultiplied fill, no displacement.
pub const DEFAULT_HOOKS: &str = r#"
fn shade(i: GlyphIn) -> vec4<f32> { return i.color * i.coverage; }
fn displace(v: VertexIn) -> vec2<f32> { return vec2<f32>(0.0, 0.0); }
"#;

const MAX_READS: usize = 32;

fn block_ok(block: &naga::Block, depth: usize) -> Result<(), String> {
    if depth > 12 {
        return Err("control nesting exceeds twelve".into());
    }
    for s in block.iter() {
        match s {
            naga::Statement::Loop { .. } => {
                return Err(
                    "loops are outside the plugin shader profile (naga cannot prove termination)"
                        .into(),
                );
            }
            naga::Statement::Call { .. } => {
                return Err("helper function calls are outside the plugin shader profile".into());
            }
            naga::Statement::Block(b) => block_ok(b, depth + 1)?,
            naga::Statement::If { accept, reject, .. } => {
                block_ok(accept, depth + 1)?;
                block_ok(reject, depth + 1)?;
            }
            naga::Statement::Switch { cases, .. } => {
                for c in cases {
                    block_ok(&c.body, depth + 1)?;
                }
            }
            naga::Statement::Emit(_)
            | naga::Statement::Store { .. }
            | naga::Statement::Return { .. }
            | naga::Statement::Kill
            | naga::Statement::Break => {}
            other => return Err(format!("unsupported statement {other:?}")),
        }
    }
    Ok(())
}

/// Validate plugin WGSL: only `shade` and/or `displace`, no entry points, no own
/// bindings, no loops or helper calls, at most 32 texture reads, naga-valid.
pub fn validate(src: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(&format!("{HEADER}\n{src}"))
        .map_err(|e| e.emit_to_string(src))?;
    if !module.entry_points.is_empty() {
        return Err("plugin shaders must not declare entry points".into());
    }
    if module.global_variables.len() != 4 {
        return Err("plugin shaders must not declare their own bindings or globals".into());
    }
    let mut names = Vec::new();
    let mut reads = 0;
    for (_, f) in module.functions.iter() {
        let name = f.name.clone().unwrap_or_default();
        if !matches!(name.as_str(), "shade" | "displace") {
            return Err(format!(
                "function {name:?} is not a plugin hook (shade, displace)"
            ));
        }
        block_ok(&f.body, 0)?;
        reads += f
            .expressions
            .iter()
            .filter(|(_, e)| {
                matches!(
                    e,
                    naga::Expression::ImageSample { .. } | naga::Expression::ImageLoad { .. }
                )
            })
            .count();
        names.push(name);
    }
    if names.is_empty() {
        return Err("a plugin shader must define shade() or displace()".into());
    }
    if reads > MAX_READS {
        return Err(format!("{reads} texture reads exceed {MAX_READS}"));
    }
    // Validate the module together with the default hooks it does not override.
    let mut full = format!("{HEADER}\n{src}\n");
    if !names.iter().any(|n| n == "shade") {
        full += "fn shade(i: GlyphIn) -> vec4<f32> { return i.color * i.coverage; }\n";
    }
    if !names.iter().any(|n| n == "displace") {
        full += "fn displace(v: VertexIn) -> vec2<f32> { return vec2<f32>(0.0, 0.0); }\n";
    }
    let m = naga::front::wgsl::parse_str(&full).map_err(|e| e.emit_to_string(&full))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&m)
    .map_err(|e| format!("{e:?}"))?;
    Ok(())
}

/// Hooks to splice into the engine shader: the plugin's own plus defaults for the rest.
pub fn hooks(src: Option<&str>) -> String {
    let Some(src) = src else {
        return DEFAULT_HOOKS.to_string();
    };
    let mut out = src.to_string();
    if !src.contains("fn shade") {
        out += "\nfn shade(i: GlyphIn) -> vec4<f32> { return i.color * i.coverage; }\n";
    }
    if !src.contains("fn displace") {
        out += "\nfn displace(v: VertexIn) -> vec2<f32> { return vec2<f32>(0.0, 0.0); }\n";
    }
    out
}

#[cfg(test)]
mod tests {
    /// A hook knows where its pixel is in the frame (0..1 across and down), the
    /// same at every pixel scale.
    #[test]
    fn a_hook_can_read_where_its_pixel_is_in_the_frame() {
        super::validate(
            "fn shade(i: GlyphIn) -> vec4<f32> { return vec4<f32>(i.frame, 0.0, 1.0) * i.coverage; }",
        )
        .unwrap();
    }
}
