// noh-motion 0
// The heaviest legal text frame: as many letters as a frame may draw, each with
// a shadow, an outline and a glowing dissolve edge (four passes per letter).
fn frame(t, ctx) {
    let s = "The quick brown fox jumps over a lazy dog and ";
    let line = s + s + s + s + s;
    let f = (t * 24.0).to_int();
    for i in 0..7 {
        let k = (f + i) % 16;
        draw(text(line).size(0.016 + k.to_float() * 0.0005).width(0.96)
            .at(0.5, 0.06 + i.to_float() * 0.13)
            .shadow(0.5, 0.08, 0.04).outline("#C8102E", 0.04).dissolve(0.3).glow(1.5));
    }
    draw(text(s + s + s).size(0.02).at(0.5, 0.96).shadow(0.5, 0.08, 0.04)
        .outline("#C8102E", 0.04).dissolve(0.3).glow(1.5));
}
