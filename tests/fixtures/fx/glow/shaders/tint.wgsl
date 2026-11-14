// Level-3 black box: an animated gradient across each glyph, loaded without recompiling NOH.
fn shade(i: GlyphIn) -> vec4<f32> {
  let k = 0.5 + 0.5 * sin(i.time * 3.0 + i.local.x * 6.2831853);
  let tint = vec3<f32>(1.0, 0.25, 0.85) * i.color.a;
  let c = mix(i.color.rgb, tint, 0.6 * k);
  return vec4<f32>(c, i.color.a) * i.coverage;
}
