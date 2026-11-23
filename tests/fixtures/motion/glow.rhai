// noh-motion 0
// Two layer shaders: a generative background (an empty layer: the shader draws
// everything) and a treatment of a line of text (the shader reads the layer).
// The shaders are the .wgsl files in glow.shaders/.
fn frame(t, ctx) {
    draw(layer().shader("field").param(0, smooth(0.0, 2.0, t)));
    let words = layer().shader("aura").param(0, 1.0);
    words.add(text("FIRST LIGHT").size(0.11).at(0.5, 0.5).color("#F2E8D8").opacity(smooth(0.5, 1.5, t)));
    draw(words);
}
