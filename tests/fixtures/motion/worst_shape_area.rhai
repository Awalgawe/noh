// noh-motion 0
// The surface cap with the dearest paint: four fills the size of the frame,
// each with an eight-stop radial gradient and a wide stroke, see-through.
fn frame(t, ctx) {
    for k in 0..4 {
        let p = rounded_rect(0.5, 0.5, 0.97, 0.95, 0.05)
            .radial(0.5 + 0.1 * sin(t + k.to_float()), 0.5, 0.6)
            .stop(0.0, "#FFFFFF", 0.3).stop(0.14, "#FFE7A0", 0.3).stop(0.28, "#F0A030", 0.3)
            .stop(0.42, "#C8102E", 0.3).stop(0.56, "#7A0A1E", 0.3).stop(0.7, "#301018", 0.3)
            .stop(0.85, "#101828", 0.3).stop(1.0, "#000000", 0.0)
            .stroke("#F2E8D8", 0.02);
        draw(p.opacity(0.5));
    }
}
