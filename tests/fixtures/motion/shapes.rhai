// noh-motion 0
// Shapes: a ring that draws itself, a star filled with a radial gradient that
// turns, a band with a linear gradient under a line of text.
fn star(cx, cy, r0, r1, n) {
    let p = path();
    for i in 0..(2 * n) {
        let a = i.to_float() * pi() / n.to_float() - pi() / 2.0;
        let r = if i % 2 == 0 { r1 } else { r0 };
        // Radii in fractions of the height, x corrected for a 16:9 frame.
        let x = cx + r * cos(a) * 1080.0 / 1916.0;
        let y = cy + r * sin(a);
        if i == 0 { p.move_to(x, y); } else { p.line_to(x, y); }
    }
    p.close()
}

fn frame(t, ctx) {
    let band = rounded_rect(0.5, 0.82, 0.62, 0.12, 0.03)
        .linear(0.19, 0.82, 0.81, 0.82)
        .stop(0.0, "#C8102E", 0.0).stop(0.3, "#C8102E", 0.85).stop(1.0, "#101828", 0.9);
    draw(band.opacity(smooth(0.5, 1.5, t)));
    draw(text("shapes, drawn by the scene").size(0.04).at(0.5, 0.82).opacity(smooth(1.0, 2.0, t)));

    draw(circle(0.5, 0.42, 0.22).stroke("#F2E8D8", 0.008).caps("round").trim(0.0, smooth(0.5, 3.5, t)));

    let s = star(0.5, 0.42, 0.06, 0.15, 5)
        .radial(0.5, 0.42, 0.15).stop(0.0, "#FFE7A0", 1.0).stop(0.6, "#F0A030", 0.95).stop(1.0, "#C8102E", 0.0)
        .pivot(0.5, 0.42).rot(t * 0.4).scale(0.6 + 0.4 * smooth(1.0, 3.0, t));
    draw(s);

    draw(line(0.12, 0.2, 0.88, 0.2).stroke("#FFFFFF", 0.003).trim(smooth(3.0, 5.0, t), 1.0).opacity(0.7));
}
