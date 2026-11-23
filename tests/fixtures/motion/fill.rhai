// noh-motion 0
// An empty layer whose shader paints the whole frame red at half opacity
// (its number 0).
fn frame(t, ctx) { draw(layer().shader("fill").resolution("full").param(0, 0.5)); }
