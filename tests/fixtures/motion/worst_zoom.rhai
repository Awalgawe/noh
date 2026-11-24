// noh-motion 0
// The covered surface at its cap with letters far larger than the frame: the
// largest size at the largest scale, every pass, five different blurs.
fn frame(t, ctx) {
    for i in 0..5 {
        draw(text("M").size(0.5).scale(4.0).at(0.5, 0.5)
            .shadow(0.5, 0.01 + i.to_float() * 0.004, 0.04)
            .outline("#C8102E", 0.25).dissolve(0.3).glow(1.5));
    }
}
