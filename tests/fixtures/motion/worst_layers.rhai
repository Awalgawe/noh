// noh-motion 0
// Everything at its cap, and cut into the eight passes a frame may have.
fn frame(t, ctx) {
    let s = "The quick brown fox jumps over a lazy dog and ";
    let line = s + s + s + s + s;
    let f = (t * 24.0).to_int();
    let w = 0.0;
    for i in 0..64 {
        w += measure(text(line).size(0.02 + (f * 64 + i).to_float() * 0.00001)).w;
    }
    for i in 0..2 {
        let l = layer();
        l.add(particles_in(0.0, 0.0, 1.0, 1.0).during(0.0, 100.0).rate(499.0).life(4.0)
            .shape("petal").size(0.02).spin(2.0).swirl(0.05).wind(0.02, 0.0).glow(0.5)
            .color("#C8102E").color("#F2E8D8").seed(i));
        draw(l.opacity(0.9));
    }
    let small = layer();
    for i in 0..7 {
        let k = (f + i) % 16;
        if i == 4 { draw(small.opacity(0.9)); small = layer(); }
        small.add(text(line).size(0.016 + k.to_float() * 0.0005).width(0.96)
            .at(0.5, 0.06 + i.to_float() * 0.13)
            .shadow(0.5, 0.08, 0.04).outline("#C8102E", 0.04).dissolve(0.3).glow(1.5));
    }
    draw(small.opacity(0.9));
    // The surface left under its cap: letters far larger than the frame, then
    // large ones, with five more blurs.
    for i in 0..2 {
        let l = layer();
        l.add(text("M").size(0.5).scale(4.0).at(0.5, 0.5)
            .shadow(0.5, 0.01 + i.to_float() * 0.004, 0.04)
            .outline("#C8102E", 0.25).dissolve(0.3).glow(1.5));
        draw(l.opacity(0.9));
    }
    let big = layer();
    for i in 0..2 {
        big.add(text("MWMWMW MWM").size(0.2).width(2.0)
            .at(0.5, 0.3 + i.to_float() * 0.3)
            .shadow(0.5, 0.3 + i.to_float() * 0.02, 0.04)
            .outline("#C8102E", 0.25).dissolve(0.3).glow(1.5));
    }
    draw(big.opacity(0.9));
    let heavy = text(line);
    for k in 0..32 { heavy.word(k, 1.0, 0.0, 1.0); heavy.word_color(k, "#FFFFFF"); }
    for i in 0..1500 { let b = heavy; }
    draw(text(s + s + s).size(0.02).at(0.5, 0.96).shadow(0.5, 0.08, 0.04)
        .outline("#C8102E", 0.04).dissolve(0.3).glow(1.5).opacity(clampf(w, 0.5, 1.0)));
}
