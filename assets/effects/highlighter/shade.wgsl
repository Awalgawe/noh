// The highlighted word drops its shadow while it is sung: the shadow would darken
// the highlighter's colour around the dark letters.
fn shade(i: GlyphIn) -> vec4<f32> {
  if (i.pass_kind == 2.0 && i.progress > 0.0 && i.progress < 1.0) {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
  }
  return i.color * i.coverage;
}
