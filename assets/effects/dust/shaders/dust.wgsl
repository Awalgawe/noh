// Fade to dust: once its line has been sung, each letter is eaten by a noise
// behind a warm edge, left to right or from the centre out.
//
// Settings (g.user, packed in the order of params in effect.json):
// float 0 direction (0 left to right, 1 from the centre), float 1 duration
// (0 short 0.7 s, 1 medium 0.9 s, 2 long 1.2 s), floats 2 to 4 the edge colour
// (linear red, green, blue).
//
// The rise is the window's glide (effect.json): when the next line shows, this
// line moves up into the window's past slot and keeps dissolving there.
//
// Passes (effect.json): a blurred black shadow, the white fill, and a sharp
// shadow pass that draws the edge only. The edge pass is told from the shadow
// by its marker colour (white in effect.json, black for the shadow); the edge
// is painted in the edge setting's colour.

fn shade(i: GlyphIn) -> vec4<f32> {
  let is_shadow_kind = i.pass_kind > 1.5 && i.pass_kind < 2.5;
  let is_edge = is_shadow_kind && i.color.g > 0.5 * i.color.a;

  // The dissolve starts 60 ms after the line's sung end and lasts the duration;
  // tau runs from 0 when it starts to 1 when the last grain has gone, and is
  // negative while the line is read (nothing dissolved, no edge).
  let from_centre = g.user[0].x > 0.5;
  let duration = select(select(0.7, 0.9, g.user[0].y > 0.5), 1.2, g.user[0].y > 1.5);
  let tau = (i.line_end - 0.06) / duration;
  let q = -0.05 + 1.35 * tau;

  // Grains of 2 px at a short side of 1080 px, the same at every export size.
  // The noise is Dave Hoskins' « Hash without Sine » (hash12, MIT licence).
  let short = min(g.canvas.x, g.canvas.y);
  let cell = floor(i.frame * g.canvas / (2.0 * short / 1080.0));
  var p3 = fract(vec3<f32>(cell.x, cell.y, cell.x) * 0.1031);
  p3 = p3 + vec3<f32>(dot(p3, p3.yzx + vec3<f32>(33.33)));
  let r = fract((p3.x + p3.y) * p3.z);

  // Where the grain sits across its row, 0 at the left edge, 1 at the right.
  let width = max(i.span.y - i.span.x, 0.000001);
  let xn = clamp((i.frame.x - i.span.x) / width, 0.0, 1.0);
  let thr = select(0.78 * xn + 0.22 * r, 0.8 * abs(xn - 0.5) * 2.0 + 0.2 * r, from_centre);
  let past = q - thr;

  if (is_edge) {
    let lit = select(0.0, 1.0, past >= 0.0 && past < 0.035);
    let edge = vec3<f32>(g.user[0].z, g.user[0].w, g.user[1].x);
    return vec4<f32>(edge, 1.0) * (i.color.a * lit * i.coverage);
  }
  let keep = select(0.0, 1.0, past < 0.0);
  return i.color * (keep * i.coverage);
}
