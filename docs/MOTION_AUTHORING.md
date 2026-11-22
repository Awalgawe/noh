# Writing a motion scene

**Experimental: this format can change without notice; a scene written today may
no longer be read by a later version of NOH.** No compatibility is promised.

A motion scene draws over a video what a style could not: the lyrics set as you
want them, shapes, particles, layers, all moving with the song. It is one text
file, a script, which NOH checks before it draws anything.

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

For a **vertical** video, the safe area of the platform is required: ask the user
which platform the video is for (propose YouTube Shorts). If you cannot ask yet,
draft with `universal` (the strictest: it fits all three platforms) and ask before
the export. The scene reads it as `ctx.safe_x0` … `ctx.safe_y1`; `report.json`,
`look.json` and `measure.json` give its values for the video (fractions of the
frame).

NOH runs one job at a time: a look started while an export runs is refused
(busy); start it again when the export is done.

In this version a scene is drawn over one video (a caption burn): the video's sound
gives the energy and onsets, its length is the song's, its size is the canvas.
Scenes over a montage (a song and several clips) come later.

## What NOH does not draw yet

`video()` (reframing the picture) and layer shaders come in later versions: a scene
that calls them is refused with that reason. The picture under the scene is the
video as it is.

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
| `.shadow(opacity, blur, dy)` | none | A soft shadow; blur (at most 1) and offset (−2 to 2) in em (try 0.5, 0.08, 0.04) |
| `.outline(c, width)` | none | A contour; width in em, at most 0.25 (try 0.04) |
| `.dissolve(v)` | 0 | 0 = whole, 1 = gone: the letters are eaten away |
| `.edge(c)` | `#FFB060` | Colour of the lit edge of the dissolve |
| `.glow(v)` | 0 | Halo of that lit edge (try 1 to 3); nothing without a dissolve |

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
| Passes per frame | 8: each layer, each run of loose draws between two layers, and within either each run of shapes and each run of other draws |
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

Said but not refused: a text that leaves the frame or the safe area, two texts that
overlap while both are well visible (more than half opaque, less than half
dissolved: a cross-fade is not reported), a fill that could not be cut cleanly.
Rotated text is not checked against the frame, the safe area or other texts, and the
check says so.
