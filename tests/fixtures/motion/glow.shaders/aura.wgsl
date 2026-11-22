// A treatment of the layer's own content (its text): a halo from eight taps
// around each pixel, and the colours split apart on each sudden rise of sound.
fn tap(uv: vec2<f32>, d: vec2<f32>) -> vec4<f32> {
  return textureSampleLevel(content, samp, uv + d, 0.0);
}

fn layer(i: LayerIn) -> vec4<f32> {
  let px = 1.0 / i.size;
  let r = 6.0 * px * (1.0 + 2.0 * i.energy);
  var halo = tap(i.uv, vec2<f32>(r.x, 0.0)) + tap(i.uv, vec2<f32>(-r.x, 0.0));
  halo += tap(i.uv, vec2<f32>(0.0, r.y)) + tap(i.uv, vec2<f32>(0.0, -r.y));
  halo += tap(i.uv, r * 0.7) + tap(i.uv, -r * 0.7);
  halo += tap(i.uv, vec2<f32>(r.x, -r.y) * 0.7) + tap(i.uv, vec2<f32>(-r.x, r.y) * 0.7);
  halo = halo / 8.0;
  let split = 4.0 * px.x * exp(-i.onset_age * 6.0) * i.k0.x;
  let cr = tap(i.uv, vec2<f32>(split, 0.0));
  let c = tap(i.uv, vec2<f32>(0.0, 0.0));
  let cb = tap(i.uv, vec2<f32>(-split, 0.0));
  let core = vec4<f32>(cr.r, c.g, cb.b, max(max(cr.a, c.a), cb.a));
  let warm = vec4<f32>(1.0, 0.45, 0.15, 1.0) * halo.a * 0.8;
  return core + warm * (1.0 - core.a);
}
