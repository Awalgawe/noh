// A generative background: slow bands of light that swell with the sound.
// Works in linear light; returns a premultiplied colour.
fn band(x: f32, t: f32, k: f32) -> f32 {
  let d = sin(x * 6.0 + t * k) * 0.5 + 0.5;
  return d * d;
}

fn layer(i: LayerIn) -> vec4<f32> {
  let p = i.uv - vec2<f32>(0.5, 0.5);
  let r = length(p * vec2<f32>(i.size.x / i.size.y, 1.0));
  let t = i.time * 0.3;
  let b = band(p.x + 0.3 * sin(p.y * 3.0 + t), t, 1.0) * 0.6 + band(p.y + 0.2 * cos(p.x * 4.0 - t), t, 0.7) * 0.4;
  let glow = (0.35 + 0.9 * i.energy) * b * (1.0 - smoothstep(0.1, 0.9, r));
  let tint = mix(vec3<f32>(0.6, 0.05, 0.05), vec3<f32>(1.0, 0.55, 0.2), b);
  let a = clamp(glow * i.k0.x, 0.0, 1.0);
  return vec4<f32>(tint * a, a);
}
