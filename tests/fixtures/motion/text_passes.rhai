// noh-motion 0
// Text through every pass but the dissolve: shadows, outlines, per-word
// opacity, offset, scale and colour, a turned block and a scaled one.
fn frame(t, ctx) {
    let d = text("every word of this line moves").size(0.07).at(0.5, 0.3)
        .shadow(0.6, 0.1, 0.05).outline("#203040", 0.05);
    for k in 0..6 {
        let p = smooth(k.to_float() * 0.3, k.to_float() * 0.3 + 0.6, t);
        d.word(k, p, (1.0 - p) * 0.4, 0.8 + 0.2 * p);
    }
    d.word_color(2, "#F2C14E");
    draw(d);
    draw(text("TURNED").size(0.1).at(0.3, 0.65).rot(-0.2).color("#9AD1F5").shadow(0.5, 0.05, 0.03));
    draw(text("SCALED").size(0.05).at(0.72, 0.68).scale(1.0 + 0.8 * smooth(0.0, 2.0, t)).outline("#000000", 0.08));
}
