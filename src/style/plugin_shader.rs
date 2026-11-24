//! Black-box (level 3) plugin shaders: validation profile and the hook points
//! the engine splices into its glyph shader (archi-greffons-v3 §3.7 and §4).

use crate::fx::shaders::HOOK_HEADER as HEADER;

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
