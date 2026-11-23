// Fade to dust: once its line has been sung, each letter is eaten by a noise
// behind a warm edge, left to right or from the centre out.
//
// The hooks cannot read a setting or the time since the line ended, so
// effect.json hands them over in the instance opacity (i.color.a):
// - from 0 to 1: the line is being read; the opacity is the line's and the
//   word's as drawn (fade-in, unsung words at 0.7);
// - from the line's end on, the line `past` opacity track sets it to
//   8 - tau (left to right) or -(8 - tau) (from the centre), where tau is the
//   dissolve's time since its start (60 ms after the line's end) over its
//   duration: 0 when it starts, 1 when the last grain has gone.
// Every pass keeps alpha 1 in effect.json so the opacity reaches the hooks as
// set; the looks below (shadow 0.5, edge 0.9) are applied here.
//
// Passes (effect.json): a blurred black shadow, the white fill, and a sharp
// shadow pass in the edge colour that draws the warm edge only. The two shadow
// passes are told apart by their colour: the edge pass is the one not black.

fn shade(i: GlyphIn) -> vec4<f32> {
  let a = i.color.a;
  // Unpremultiplied pass colour (a may be negative: the sign cancels out).
  let rgb = i.color.rgb / select(a, 1.0, abs(a) < 0.000001);
  let is_shadow_kind = i.pass_kind > 1.5 && i.pass_kind < 2.5;
  let is_edge = is_shadow_kind && max(rgb.r, max(rgb.g, rgb.b)) > 0.002;
  let is_shadow = is_shadow_kind && !is_edge;

  // Being read: drawn as set, the shadow at half opacity, no edge.
  if (a >= 0.0 && a <= 1.5) {
    if (is_edge) {
      return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    if (is_shadow) {
      return i.color * (0.5 * i.coverage);
    }
    return i.color * i.coverage;
  }

  // Dissolving.
  let from_centre = a < 0.0;
  let tau = 8.0 - abs(a);
  let q = -0.05 + 1.35 * tau;

  // Grains of 2 px at a short side of 1080 px, the same at every export size.
  let short = min(g.canvas.x, g.canvas.y);
  let cell = floor(i.frame * g.canvas / (2.0 * short / 1080.0));
  var p3 = fract(vec3<f32>(cell.x, cell.y, cell.x) * 0.1031);
  p3 = p3 + vec3<f32>(dot(p3, p3.yzx + vec3<f32>(33.33)));
  let r = fract((p3.x + p3.y) * p3.z);

  // Where the grain sits across the caption box (the line's own width is not
  // known to the hooks): the box of effect.json, by orientation.
  let portrait = g.canvas.y > g.canvas.x;
  let mid = select(0.5, 0.4333, portrait);
  let half = select(0.44, 0.3889, portrait);
  let xn = clamp((i.frame.x - (mid - half)) / (2.0 * half), 0.0, 1.0);
  let xc = clamp(abs(i.frame.x - mid) / half, 0.0, 1.0);
  let thr = select(0.78 * xn + 0.22 * r, 0.8 * xc + 0.2 * r, from_centre);
  let past = q - thr;

  if (is_edge) {
    let lit = select(0.0, 0.9, past >= 0.0 && past < 0.035);
    return vec4<f32>(rgb, 1.0) * (lit * i.coverage);
  }
  let keep = select(0.0, 1.0, past < 0.0);
  if (is_shadow) {
    return vec4<f32>(0.0, 0.0, 0.0, 0.5) * (keep * i.coverage);
  }
  return vec4<f32>(rgb, 1.0) * (keep * i.coverage);
}
