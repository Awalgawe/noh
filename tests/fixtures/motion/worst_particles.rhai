// noh-motion 0
// The heaviest legal particle frame: two emitters at their cap, 4 000 on screen,
// glowing petals.
fn frame(t, ctx) {
    for i in 0..2 {
        draw(particles_in(0.0, 0.0, 1.0, 1.0).during(0.0, 100.0).rate(499.0).life(4.0)
            .shape("petal").size(0.02).spin(2.0).swirl(0.05).wind(0.02, 0.0).glow(0.5)
            .color("#C8102E").color("#F2E8D8").seed(i));
    }
}
