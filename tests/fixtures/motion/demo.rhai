// noh-motion 0
// A title that fades in, is eaten away, and sheds a few specks.
fn frame(t, ctx) {
    if t < 10.0 {
        draw(rect(mixf(0.05, 0.5, energy(t)), 0.004).at(0.5, 0.9).color("#F2E8D8").opacity(0.5));
    }
    let title = text("A TITLE").at(0.5, 0.3).size(0.16).face("condensed_extrabold").color("#F2E8D8");
    if t >= 0.5 && t < 6.0 {
        draw(title
            .opacity(ease("ease_out_cubic", (t - 0.5) / 0.8))
            .dissolve(clampf((t - 3.5) / 2.5, 0.0, 1.0))
            .glow(2.0));
    }
    if t >= 3.5 && t < 9.0 {
        draw(particles(title).during(3.5, 6.0).shape("dust").rate(30.0).life(3.0).size(0.008)
            .wind(-0.05, 0.0).rise(0.06).swirl(0.03).color("#B0B0B0").opacity(0.7));
    }
}
