# Motion scene format

**Experimental: this format can change without notice; a scene written today may
no longer be read by a later version of NOH.** No compatibility is promised.

A motion scene is one text file, a script, that says for any instant what NOH draws
over the video, the lyrics included: texts, shapes, particles, layers. NOH checks it
before it draws anything. This page is the reference of the format: what exists,
its limits, and the tools that check and show a scene.

## The file

- Put it beside the song and its lyrics: `<song>.noh-motion.rhai`.
- Its **first line** names the format it is written for, exactly:

  ```
  // noh-motion 0
  ```

  A scene without that line, or written for another format, is refused with the
  reason.
- It defines `fn frame(t, ctx)`, which NOH calls for every frame: `t` is the time in
  seconds on the song's clock. The function builds draws and hands each over with
  `draw(…)`. It returns nothing.

```rhai
// noh-motion 0
fn frame(t, ctx) {
    if t >= 1.0 && t < 4.0 {
        draw(text("hello").at(0.5, 0.8).size(0.06).opacity(smooth(1.0, 1.5, t)));
    }
}
```

`frame` must be a pure function of `t`: nothing survives from one frame to the next,
and frames may be asked in any order (the check asks them twice, in two orders).

## Working with NOH

With NOH's MCP tools (or the same options on the command line, `docs/CLI.md`):

1. `noh_scene_check`: the scene against the video and its reviewed subtitles. Every
   frame is evaluated and checked; `report.json` gives the first refused frame
   (instant and reason) or the counted maxima, the warnings, what was not checked,
   and two times measured on this machine, for information.
2. `noh_scene_look`: frames drawn by the export's engine over the video, as PNGs:
   `stills` at chosen instants, `strip` (eight frames over a second, to see a
   movement), `sheet` (frames spread over the song), `measure` (where each text
   falls, as JSON). `grid` draws the frame's edges, a line every tenth of the frame
   and the safe area; `bare` shows the video alone, without the scene, to compose
   on it. Open the PNGs and look.
3. `noh_scene_clip`: a short MP4 (at most 30 s) to judge a rhythm.
4. `noh_burn_subtitles` with `motion: {"scene": …}`: the video with the scene,
   instead of the captions (the scene draws the lyrics itself).
   `noh_export_project` with `motion` does the same for a montage and its short.

For a **vertical** video, the safe area of the platform is required: ask the user
which platform the video is for (propose YouTube Shorts). If you cannot ask yet,
draft with `universal` (the strictest: it fits all three platforms) and ask before
the export. The scene reads it as `ctx.safe_x0` … `ctx.safe_y1`; `report.json`,
`look.json` and `measure.json` give its values for the video (fractions of the
frame).

NOH runs one job at a time: a look started while an export runs is refused
(busy); start it again when the export is done.

Over one video (a caption burn), the video's sound gives the energy and onsets, its
length is the song's, its size is the canvas. Over a montage (`noh_export_project`),
the song's WAV gives them, the project's size is the canvas, and the scene sees the
song's clock: a short's first frame is at its start. The scene tools take one video:
check and look at a montage's scene over one of its clips with the song's sound.

In NOH's window, a scene beside the song is listed under Motion; the user checks it
and uses it in one click, and NOH draws it in the preview and the export.

## The language

Scenes are written in a small subset of Rhai: numbers, booleans, strings, `let`,
`if` / `else`, `for i in 0..n`, `while`, your own functions (`fn name(a, b) { … }`,
the last expression is the result), and the functions on this page. Nothing else
exists: no arrays, no object maps, no closures, no imports, no clock, no randomness,
no files. A call to anything not listed here is refused when the scene is checked.

- Decimal numbers and whole numbers are different types. Write `1.0`, not `1`,
  wherever a decimal is expected (every builder below except word indices and
  `seed`). `i.to_float()` turns a counter into a decimal, `x.to_int()` the reverse.
- Functions cannot see variables outside themselves: pass what they need.
- A small table is written with `pick(i, a, b, c)`: the i-th value (2 to 5 decimal
  values, or 2 to 4 strings; the last one when `i` is beyond them).
- Maths: `sin`, `cos`, `abs`, `sqrt`, `exp`, `floor`, `round`, `pow(x, y)`,
  `atan2(y, x)` (the angle of a direction, in radians), `min(a, b)`, `max(a, b)`,
  `pi()`, written `x.sin()` or `sin(x)`.
- Shaping: `clampf(x, lo, hi)`, `mixf(a, b, x)`, `smooth(a, b, x)` (0 before `a`, 1
  after `b`, eased between), `ease(name, x)` with `x` in 0..1 and `name` among
  `linear`, `ease_in`, `ease_out`, `ease_in_out`, `ease_in_cubic`, `ease_out_cubic`,
  `back`.
- Strings: `a + b`, `len(s)`, `upper(s)`, `lower(s)`.

## The frame

- Positions and widths are fractions of the frame: x from 0 (left) to 1 (right), y
  from 0 (top) to 1 (bottom). Text sizes and particle sizes are fractions of the
  frame's short side. Colours are `"#RRGGBB"`. Angles are radians, clockwise.
- Draw order is the order of the `draw(…)` calls: later over earlier.
- `ctx.w`, `ctx.h`: the frame in pixels. `ctx.src_w`, `ctx.src_h`: the video's own
  size (the same today). `ctx.duration`: seconds.
- `ctx.safe_x0`, `ctx.safe_y0`, `ctx.safe_x1`, `ctx.safe_y1`: the surface text should
  stay in (the whole frame unless a safe area is given).

## Text

`text("…")` makes a text; each builder below sets one thing and gives the text back,
so they chain. Called on a variable as a statement (`d.size(0.1);`) a builder
changes that variable.

| Builder | Default | Meaning |
|---|---|---|
| `.at(x, y)` | 0.5, 0.5 | Where the block's anchor point sits |
| `.anchor(name)` | `center` | Which point of the block is at (x, y): `center`, `left`, `right`, `top`, `bottom`, `top_left`, `top_right`, `bottom_left`, `bottom_right`; or, on the baseline of its first line, `baseline`, `baseline_left`, `baseline_right` |
| `.size(v)` | 0.08 | Font size |
| `.width(v)` | 0.88 | Width at which the words wrap |
| `.align(name)` | `center` | `left`, `center` or `right`: how the lines sit in the block |
| `.face(name)` | `jost_extrabold` | `jost_extrabold`, `jost_medium`, `inter_semibold`, `condensed_extrabold`, `serif_italic`, `sans`, `sans_semibold` |
| `.line_height(v)` | 1.15 | From 0.5 to 4 |
| `.tracking(v)` | 0 | Extra space between letters, in em (−0.2 to 2) |
| `.rot(v)` | 0 | Rotation about the anchor point |
| `.scale(v)` | 1 | About the anchor point; does not re-wrap; at most 4 |
| `.color(c)` | `#FFFFFF` | |
| `.opacity(v)` | 1 | |
| `.shadow(opacity, blur, dy)` | none | A soft shadow; blur (at most 1) and offset (−2 to 2) in em |
| `.outline(c, width)` | none | A contour; width in em, at most 0.25 |
| `.dissolve(v)` | 0 | 0 = whole, 1 = gone: the letters are eaten away |
| `.edge(c)` | `#FFB060` | Colour of the lit edge of the dissolve |
| `.glow(v)` | 0 | Halo of that lit edge; nothing without a dissolve |

Per word (`k` counts words from 0, at most 32 per text): `.word(k, opacity, dy,
scale)` (`dy` in em, positive down), `.word_dissolve(k, v)`, `.word_color(k, c)`.
A word's own dissolve replaces the text's `.dissolve` for that word (it does not
add to it): to dissolve a whole line whose words have their own dissolve, fold the
line's value into each word's.

The block's box is as wide as its widest line and as high as its lines (the line
height times the size, so it holds the room above and below the letters). Words of
different sizes or faces set side by side with a `center` anchor are centred on each
other, not on one baseline: to share a baseline, give them all a `baseline_left`
anchor (or `baseline`, `baseline_right`) and the same `y`.

A builder called in a chain works on a copy: `draw(a.at(x, 0.5))` leaves `a` as it
was. Only a builder called as a statement on a variable (`a.at(x, 0.5);`) changes it.

**Measuring.** `measure(d)` gives the box a text will occupy, as drawn (its rotation
aside): `.x0 .y0 .x1 .y1 .w .h`. `word_box(d, k)` gives the box of its k-th word. A
text need not be drawn to be measured; its width and height do not depend on where
it is placed, only the box's position does. At most 64 measures per frame; a text of
at most 256 characters.

```rhai
let a = text("left part").anchor("left").size(0.05);
let b = text("right part").anchor("left").size(0.09);
let x = 0.1;
draw(a.at(x, 0.5));
draw(b.at(x + measure(a).w + 0.02, 0.5));
```

## Rectangle

`rect(w, h)`: `w` a fraction of the frame's width, `h` of its height. Then `.at(x,
y)` (its centre), `.rot(v)`, `.color(c)`, `.opacity(v)`, `.glow(v)`.

## Shapes

`path()` starts a shape; `.move_to(x, y)`, `.line_to(x, y)`, `.quad_to(cx, cy, x,
y)`, `.cubic_to(c1x, c1y, c2x, c2y, x, y)` and `.close()` add to it. Points are
fractions of the frame, like every position (so a path drawn with equal steps in x
and y is not square in a wide frame; the shortcuts below are). Shortcuts, which start
a path: `circle(x, y, r)`, `ellipse(x, y, rx, ry)` (radii in fractions of the short
side), `rounded_rect(x, y, w, h, r)` (centre; `w` of the width, `h` of the height,
`r` of the short side), `line(x0, y0, x1, y1)`.

| Builder | Default | Meaning |
|---|---|---|
| `.fill(c)` | none | Fill with a colour |
| `.linear(x0, y0, x1, y1)` | | Fill with a gradient from one point to the other (fractions of the frame) |
| `.radial(x, y, r)` | | Fill with a gradient from the centre out to `r` (fraction of the short side) |
| `.stop(offset, c, opacity)` | | A colour of the gradient, `offset` from 0 to 1, in order; 2 to 8 stops |
| `.stroke(c, width)` | none | A line along the path; width in fractions of the short side (0.0005 to 0.25) |
| `.caps(name)` | `butt` | `round` or `butt`: the ends of an open stroke. Joins are always round |
| `.trim(a, b)` | 0, 1 | The part of the stroke drawn, from `a` to `b` of its length: a line that draws itself |
| `.opacity(v)` | 1 | |
| `.pivot(x, y)` | 0.5, 0.5 | The point `rot` and `scale` turn about |
| `.rot(v)`, `.scale(v)` | 0, 1 | Radians clockwise; scale 0.01 to 10 (the stroke grows with it) |
| `.offset(x, y)` | 0, 0 | A move, in fractions of the frame |

```rhai
// A ring that draws itself; a band with a gradient behind a line of text.
draw(circle(0.3, 0.5, 0.15).stroke("#CCCCCC", 0.006).trim(0.0, smooth(0.0, 2.0, t)));
draw(rounded_rect(0.5, 0.82, 0.6, 0.12, 0.02).linear(0.2, 0.82, 0.8, 0.82)
    .stop(0.0, "#202020", 0.0).stop(0.3, "#202020", 0.8).stop(1.0, "#404040", 0.8));
```

A path is a handle, like a layer: each builder changes the path itself, and `let b =
a;` names the same path. `draw(p)` (or `layer.add(p)`) hands over what it holds at
that moment. Gradients move, turn and scale with the path; their colours are mixed
as a design tool mixes them.

A filled path is one contour that does not cross or touch itself: a figure-eight, a
corner lying on another edge, or a second `move_to` (a hole), is refused. A chain of
more than about a hundred builders in one expression is too deep for the language:
write `p.line_to(…);` as statements. Shapes are smoothed on their edges (4 samples
per pixel). A run of shapes drawn one after the other is one pass; text and shapes
interleaved make a pass each (see the passes limit).

## The picture

`video()` frames the video picture; `draw(v)` hands it over. Without it, the picture
is the video as it is. The frame is the video's own size, or a vertical Short
(1080 x 1920) made from it when asked (`vertical` in the scene tools, framing `scene`
in noh_make_short): there `video()` frames the Short from the whole picture, and
without it the picture covers the Short, centred. `ctx.w` and `ctx.h` are the frame's
size, `ctx.src_w` and `ctx.src_h` the video picture's. A vertical frame needs a safe
area.

| Builder | Default | Meaning |
|---|---|---|
| `.focus(x, y)` | 0.5, 0.5 | The point of the video (fractions of the video picture) that is put at `at` |
| `.at(x, y)` | 0.5, 0.5 | Where that point goes in the frame |
| `.scale(v)` | 1 | 1 = the video just covers the frame; from 0.25 to 1.5 |
| `.rot(v)` | 0 | Radians, clockwise, about that point; at most 0.35 |
| `.crop(x0, y0, x1, y1)` | 0, 0, 1, 1 | The part of the video drawn (fractions of the video picture) |

```rhai
// Framed a little tighter; the framing slides from left to right between 10 and 12 s.
let x = mixf(0.4, 0.6, smooth(10.0, 12.0, t));
draw(video().scale(1.2).focus(x, 0.5));
```

One per frame; it is always drawn beneath everything else, wherever the call is. It
cannot go in a layer. Where the picture does not reach (scale under 1, a crop, a turn
without enough zoom) the frame is black, and the check says so. A turn needs some zoom
to keep the corners covered: about 1 + the angle times the frame's width over its
height. The export reframes the video at its full size; the preview reframes its own
smaller picture the same way, at the same instants.

## Layers

A layer groups draws so that they behave as one picture. `layer()` makes one,
`.add(d)` puts a text, a rectangle, a shape or particles in it (in order, later over
earlier), `.opacity(v)` fades the whole layer, and `draw(l)` hands it over.

```rhai
let pair = layer();
pair.add(text("one").size(0.1).at(0.46, 0.5).color("#DDDDDD"));
pair.add(text("two").size(0.1).at(0.54, 0.52).color("#888888"));
draw(pair.opacity(smooth(0.0, 2.0, t)));
```

Faded as a layer, the two words fade as one picture: where they overlap, the one
beneath does not show through the one above, as it would if each had its own
opacity. Draws outside any layer are drawn in the order of the `draw(…)` calls,
between the layers.

A layer is a handle: `let b = a;` gives a second name for the same layer, not a copy.
A layer does not go in a layer. Make it inside `frame`, every frame.

## Layer shaders

A layer can run a shader you write: a small program, run once for every pixel of the
frame, that computes the layer's colour there. It can read what the layer holds (its
texts, shapes, particles), or hold nothing and draw everything itself (a generative
background). It never sees the video.

**Where.** Beside the scene file `<song>.noh-motion.rhai`, a folder
`<song>.noh-motion.shaders/` holds the shaders, one per file, `name.wgsl` (at most 8
files, 8 KB each). In the scene:

```rhai
fn frame(t, ctx) {
    // A generative background: an empty layer, the shader draws everything.
    draw(layer().shader("field").param(0, smooth(0.0, 2.0, t)));

    // A treatment of a text: the shader reads the layer's content.
    let words = layer().shader("soft");
    words.add(text("sample").size(0.08).at(0.5, 0.5).color("#CCCCCC"));
    draw(words.opacity(0.9));
}
```

| Builder | Meaning |
|---|---|
| `.shader(name)` | The file `name.wgsl` in the shaders folder |
| `.resolution(r)` | `half` (the default: a quarter of the pixels, read back smoothly) or `full` |
| `.param(k, v)` | One of eight free numbers, `k` from 0 to 7, that the shader reads as `i.k0.x … i.k1.w` |

An empty layer with a shader is drawn. Every shader is checked with the scene, before
anything is drawn: one outside the rules below is refused with its name and the
reason. NOH then compiles each one with the graphics driver and draws it once **in
its worker process**, never in the window first: a driver that fails on a shader
stops the worker, and the job says so (`error.scene_worker_stopped`). The check's
`report.json` gives each shader's reads, cost and what one full frame of it costs on
this machine.

**What you write.** WGSL. A file defines one function, and may define helpers:

```wgsl
fn layer(i: LayerIn) -> vec4<f32> {
  let c = textureSampleLevel(content, samp, i.uv, 0.0);
  return c * (0.5 + 0.5 * sin(i.time + i.uv.x * 6.0));
}
```

What it receives, and nothing else:

| Name | Type | What |
|---|---|---|
| `i.uv` | `vec2<f32>` | The pixel's place, 0 to 1 across and down the frame (0, 0 top left) |
| `i.px` | `vec2<f32>` | The same in pixels of the layer (half resolution: half the frame's) |
| `i.size` | `vec2<f32>` | The layer's size in pixels. One pixel in `uv` is `1.0 / i.size` |
| `i.time` | `f32` | The time of the scene, seconds |
| `i.energy` | `f32` | Loudness, 0 to 1, smoothed (as `energy(t)`) |
| `i.onset_age` | `f32` | Seconds since the last sudden rise of the sound (as `onset_age(t)`) |
| `i.k0`, `i.k1` | `vec4<f32>` | The eight numbers set with `.param(k, v)`: `k0.x` is 0, `k0.y` 1 … `k1.w` 7 |
| `content`, `samp` | texture, sampler | The layer's own content, to read with `textureSampleLevel(content, samp, uv, 0.0)` |

Everything else of WGSL that runs per pixel is there: `let`, `var`, `if`, `switch`,
your own functions, and the built-in functions (`sin`, `mix`, `smoothstep`, `length`,
`dot`, `pow`, `exp`, `clamp`, `fract`…).

Colours are in linear light and premultiplied: an opaque red is `vec4(1, 0, 0, 1)`, a
half-transparent red is `vec4(0.5, 0, 0, 0.5)`. The content reads that way too. A
colour picked as `#RRGGBB` is in display encoding: turn each channel `c` (0 to 1) into
linear light with `pow(c, 2.2)` (close enough). Whatever you return is made safe: a
value that is not a number becomes transparent, colours are kept between 0 and 1 and
never above their opacity. An index out of range, or `textureLoad` outside the
texture, is brought back into range: it reads something, never garbage.

Use `textureSampleLevel(…, 0.0)`, not `textureSample`: the latter picks its detail
from neighbouring pixels, which is undefined inside a branch that some pixels skip.
The check says so when it sees one there.

**The rules.**

| Rule | Value |
|---|---|
| Loops (`for`, `while`, `loop`) | none: write the steps out, or call a helper several times |
| Helper functions | 16 at most (besides `layer`), never calling themselves |
| Reads of `content` per pixel | 12 at most, counted through calls (a helper that reads twice, called three times, is six) |
| Cost per pixel | 2 500 at most, counted through calls: each operation costs the components of its result (a number 1, a `vec4` 4, a `mat4x4` 16, a product with a matrix four times that); both sides of a branch count |
| Arrays | 64 elements at most |
| Bindings, globals, entry points of your own | none |
| Shader layers per frame | 4; their pixels at most 1.5 frames (a full-resolution layer is 1, a half-resolution one a quarter) |

A shader layer is one more pass (see the passes limit).

**In the window.** NOH's preview does not draw shader layers by default: it shows the
scene without them and says so (a driver reset in the window could not be seen
there). Stills, clips and the export draw them.

## Particles

`particles(d)` are born from the ink of the text `d` as it is laid out and placed
(its opacity, dissolve and scale do not matter); `particles_in(x0, y0, x1, y1)` are
born in a rectangle. Each particle's place is computed from its age, so any instant
can be drawn directly.

| Builder | Default | Meaning |
|---|---|---|
| `.during(a, b)` | required | Births happen from `a` to `b` |
| `.rate(v)` | 20 | Births per second |
| `.life(v)` | 4 | Seconds; `rate × life` at most 2 000, and 4 000 particles on screen |
| `.gone_by(t)` | none | Every particle has left by `t`, whatever its birth |
| `.shape(name)` | `dust` | `dust` (a soft dot) or `petal` (a leaf that turns) |
| `.size(v)` | 0.012 | At most 0.5 |
| `.color(c)` | white | Call once per colour (at most 8); used in turn |
| `.wind(x, y)` | 0, 0 | Drift per second |
| `.rise(v)` | 0 | Upward speed |
| `.swirl(v)` | 0 | Wobble reached at the end of life |
| `.spin(v)` | 0 | Radians per second |
| `.opacity(v)` | 1 | Particles grow in over the first 5 % of life, fade over the last 25 % |
| `.glow(v)` | 0 | Halo |
| `.seed(n)` | 0 | A whole number: another seed, another scatter |

Keep handing the particles over until the last one has left (`b + life`, or the
`gone_by` time): a draw that is not handed over is not drawn.

## The song

- `cue_count()`: how many lines. `cue(i)`: line `i`, with `.start`, `.end`, `.text`,
  `.words` (how many words), `.i`.
- `word(i, k)`: word `k` of line `i`, with `.text`, `.start`, `.end`.
- `cue_at(t)`: the index of the last line that has started at `t` (−1 before the
  first).
- `energy(t)`: loudness from 0 to 1 at `t`, smoothed, from the video's sound.
  `onset_age(t)`: seconds since the last sudden rise of the sound. `onset_count()`,
  `onset(i)`: their times. There is no tempo and no beat grid. A video without sound
  has energy 0 and no onset, and the check says so.

Word times come from the lyrics file NOH keeps beside the subtitles
(`<name>.noh-lyrics.json`, created beside an `.srt` given alone), where they were
measured: by NOH's own lyrics screen or a transcription. Elsewhere the check says they
are **estimated** (spread over the line by letter count): good enough for soft
word-by-word effects, not for tight ones.

```rhai
fn line(t, i, y) {
    let c = cue(i);
    if t < c.start - 0.3 || t > c.end + 0.5 { return; }
    let d = text(c.text).at(0.5, y).size(0.05);
    for k in 0..c.words {
        let w = word(i, k);
        d.word(k, smooth(w.start - 0.1, w.start + 0.2, t), 0.0, 1.0);
    }
    draw(d);
}
```

## Limits

Checked before anything is drawn, on every frame of the output, and again on every
frame drawn; a scene beyond them is refused with the instant and the reason.

| Limit | Value |
|---|---|
| Operations per frame (roughly one per expression) | 6 000 |
| Draws per frame | 256, those inside layers included |
| Passes per frame | 8: each layer, each run of loose draws between two layers, and within either each run of shapes and each run of other draws; a layer's shader is one more |
| Shapes | 64 paths per frame; 256 points written per path; a filled path at most 128 points once its curves are flattened, and 512 such points per frame; 1 024 segments per frame once flattened; their boxes cover at most 4 frames |
| Particles | 2 000 per emitter, 4 000 on screen |
| Letters drawn in a frame | 6 000, every pass counted: a shadow, an outline and a glowing edge each draw the letter once more |
| Surface covered in a frame | 16 frames, every pass counted. A letter counts as a square of 1.25 times its size (scale included), less what falls outside the frame; a rectangle by its surface inside the frame; a particle by its largest size. A glow, a shadow, an outline each count once more |
| Different shadow blurs in a frame | 8 (the blur in em times the text's size and scale) |
| Different texts in the scene | 512 (a text with another size, face, width, alignment or tracking is another text) |
| Measures per frame | 64 |
| Calls to the costly functions per frame | 64 |
| Reads of the song per frame (`cue`, `word`, `onset`) | 1 024 |
| A string | 1 024 bytes |
| A text | 256 characters, 32 words with their own settings, scale up to 4 |
| Nesting | 24 levels of calls; expressions 64 deep (32 inside functions) |
| The scene file | 32 KB |
| The scene's letters | must fit the glyph atlas: few very large sizes |
| Layer shaders | 8 files of 8 KB beside the scene; 4 shader layers per frame, 1.5 frames of their pixels; per shader 12 reads and a cost of 2 500 per pixel, 16 helpers, no loop |

Said but not refused: a text that leaves the frame or the safe area, two texts that
overlap while both are well visible (more than half opaque, less than half
dissolved: a cross-fade is not reported), a fill that could not be cut cleanly.
Rotated text is not checked against the frame, the safe area or other texts, and the
check says so.
