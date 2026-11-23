// noh-motion 0
// The heaviest legal measuring: as many measures as a frame may make, each of a
// long text at a size not seen before, so that nothing is remembered.
fn frame(t, ctx) {
    let s = "The quick brown fox jumps over a lazy dog and ";
    let line = s + s + s + s + s;
    let f = (t * 24.0).to_int();
    let w = 0.0;
    for i in 0..64 {
        w += measure(text(line).size(0.02 + (f * 64 + i).to_float() * 0.00001)).w;
    }
    draw(rect(0.1, 0.01).at(0.5, 0.5).opacity(clampf(w, 0.0, 1.0)));
}
