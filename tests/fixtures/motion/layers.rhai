// noh-motion 0
// Three layers: a band, a title over it in a layer of its own that fades as a
// whole, and loose draws on top. A layer fades as one picture: where its own
// draws overlap, the overlap does not show through.
fn frame(t, ctx) {
    let back = layer();
    back.add(rect(0.7, 0.22).at(0.5, 0.5).color("#101828").opacity(0.8));
    back.add(rect(0.7, 0.012).at(0.5, 0.39).color("#C8102E"));
    draw(back);

    let title = layer();
    title.add(text("FIRST").size(0.14).at(0.46, 0.5).color("#F2E8D8"));
    title.add(text("LIGHT").size(0.14).at(0.54, 0.52).color("#C8102E"));
    title.opacity(0.35 + 0.65 * smooth(0.0, 2.0, t));
    draw(title);

    draw(text("three layers").size(0.03).at(0.5, 0.66).opacity(0.7));
}
