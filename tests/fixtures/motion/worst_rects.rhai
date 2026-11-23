// noh-motion 0
// The covered surface at its cap with rectangles: eight glowing ones, each the
// size of the frame.
fn frame(t, ctx) {
    for i in 0..8 {
        draw(rect(1.0, 1.0).at(0.5, 0.5).color("#204060").opacity(0.05).glow(1.0));
    }
}
