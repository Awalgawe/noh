// noh-motion 0
// The covered surface at its cap with the dearest passes: large letters, each
// with a shadow (eight different blurs), the widest outline and a lit edge.
fn frame(t, ctx) {
    for i in 0..13 {
        draw(text("MWMWMW MWMWMW").size(0.2).width(2.0)
            .at(0.5, 0.15 + (i % 8).to_float() * 0.1)
            .shadow(0.5, 0.3 + (i % 8).to_float() * 0.02, 0.04)
            .outline("#C8102E", 0.25).dissolve(0.3).glow(1.5));
    }
}
