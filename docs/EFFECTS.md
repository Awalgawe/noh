# Caption effect format

A caption effect is a folder holding an `effect.json` (at most 16 KiB) and, when it
needs one, a shader file. NOH reads the effects from two places, on demand: the
`effects` folder beside `noh.exe` (the bundled ones; the command line and the MCP
read the same folder one level up from `bin/`), then the user's own `noh/effects`
folder. Each effect is validated when it is read; a folder that fails is listed
with its reason (`noh --list-effects`) and never drawn. Validation is strict: an
unknown field, property, value or unit is refused, never ignored.

The engine knows layouts, tracks and passes; it knows no effect by name. The six
bundled effects in [`assets/effects`](../assets/effects) are complete examples.

Some features below are marked **(after 0.3.1)**: NOH 0.3.1 and earlier refuse an
effect that uses them (as an unknown field or property). `engine` stays `1`.

## Top level

```json
{
  "engine": 1,
  "id": "author.style",
  "name": {"fr": "Mon style", "en": "My style"},
  "version": "0.1.0",
  "license": "GPL-3.0-only",
  "target": "text",
  "params": [],
  "presets": [{"id": "default", "name": {"en": "Default"}, "values": {}}],
  "layout": {},
  "animate": [],
  "render": {"passes": []},
  "shader": "shaders/look.wgsl"
}
```

- `engine` must be `1`; `target` must be `"text"`; `id` is required and unique
  among the folders read (two folders with one id are both refused).
- `name`, and every `name` or `label` below, maps language codes to text. NOH
  shows the user's language, else English, else French, else any.
- `layout` and `render` are required. `shader` is optional (see Shader hooks).

## Parameters and presets

```json
"params": [
  {"id": "sung", "type": "color", "default": "#FFD34D", "level": 1, "label": {"en": "Sung colour"}},
  {"id": "size", "type": "choice", "default": "medium", "choices": ["small", "medium", "large"],
   "choice_labels": {"small": {"en": "Small"}}},
  {"id": "pal", "type": "palette", "default": ["#FF5D73", "#2E86FF"]},
  {"id": "font", "type": "font", "default": "jost_extrabold", "level": 2}
]
```

- Types: `color` (`#RRGGBB`), `number` (with optional `min`, `max`, `unit`),
  `choice` (one of `choices`), `bool`, `palette` (a list of colours), `font` (a
  bundled face, or `system:<Family>` for an installed one; its default must be a
  bundled face).
- `level` 1 settings are shown first (at most 5); level 2 (the default) under them.
  At most 32 parameters.
- A default may differ by orientation: `{"landscape": …, "portrait": …}`.
- A preset sets some parameters; the first preset is the effect's default look.
- Only a `font` parameter may name an installed family (`system:…`), never a
  layout, a role or another parameter.

Bundled faces: `sans` (Noto Sans), `sans_semibold`, `serif_italic` (Noto Serif
Italic), `jost_extrabold`, `jost_medium`, `inter_semibold`, `condensed_extrabold`
(Barlow Condensed), `cjk`.

## Values

Wherever a number, colour or text is expected, a value may be:

| Form | Meaning |
|---|---|
| `12`, `"#FFFFFF"`, `"bottom"` | A constant |
| `"$id"` | The parameter `id` |
| `"@w"`, `"@h"`, `"@short"` | Canvas width, height and the smaller of the two, in px |
| `"@inset_l"`, `"@inset_t"`, `"@inset_r"`, `"@inset_b"`, `"@safe"` | The safe area's insets in px (0 without one); `@safe` is 1 when a safe area applies |
| `"@word_duration"`, `"@word_longest"` | The drawn word's sung length in seconds, and 1 for the longest-sung word of its line (else 0) |
| `"@line_index"` **(after 0.3.1)** | The drawn line's rank among the lyrics' lines (a chunk's among the chunks), from 0 |
| `{"landscape": v, "portrait": v}` | By orientation |
| `{"add": [a, b, …]}`, `sub`, `mul`, `div`, `min`, `max` | Arithmetic, 2 to 8 arguments; a zero divisor gives 0 |
| `{"round": [a]}`, `ceil`, `floor` | Rounding |
| `{"hash": [a]}` **(after 0.3.1)** | A number from -1 to 1 that depends only on `a` rounded to a whole number: with `@line_index`, a value steady for a line and different for the next |
| `{"choose": [selector, {"case": v, "default": v}]}` | By the text of a choice parameter |
| `{"mix": [colour, colour, t]}` | sRGB blend, `t` from 0 to 1 |
| `{"scale": [a, k]}` | `a * k` |
| `{"pick": [palette, scope]}` | A palette colour by `word_index`, `index` (letter in the line), `index_in_word`, or `line_index` **(after 0.3.1)** |
| `{"by_state": {"past": v, "active": v, "next": v}}` | By the state of the drawn line in a window |
| `{"by_offset": [v0, v1, v2, v3]}` | By the drawn line's distance from the active slot (the last repeats) |

## Layout

```json
"layout": {
  "mode": "line",
  "font": "$font", "size": {"mul": ["@short", 0.1]}, "case": "none", "line_height": 1.18,
  "box": {"x": 0.5, "y": 0.88, "width": 0.86, "anchor": "bottom", "space": "safe"},
  "align": "center",
  "visible": "line", "anchor": "unit", "line_breaks": "rewrap",
  "word_lead": 0.08, "preroll": 0.3, "hold": 0,
  "chunk": {"max": 3, "rows": "flow"},
  "word_size": 1,
  "window": {"before": 1, "after": 2, "lead": 10, "gap": 0.3, "glide": {"duration": 0.42, "curve": "ease_out"}}
}
```

- `mode`: `line` (the cue laid out as text in a box) or `compose` (each word
  placed alone by roles and slots, `layout.compose`).
- `size` is in canvas px. `case`: `none`, `upper`, `lower`.
- `box`: `x`, `y` and `width` are fractions of the safe area (`space: "safe"`, the
  default) or of the canvas (`"canvas"`); `anchor` is the block's `top`, `center`
  or `bottom` at `y`. Lines wrap between words inside `width`; a word wider than
  the box is kept whole and reported.
- `align` **(after 0.3.1)**: `center` (the default), `left` or `right`: each row
  set against that edge of the box. Letter spacing (`tracking`) then grows from
  that edge. Line mode only.
- `visible`: `line` shows the whole line; `from_start` shows each word from its
  start on.
- `anchor`: what letters scale and turn about: `unit` (each letter about itself),
  `word`, `word_x_glyph_y`, `baseline` (the word's centre across, the letter's
  height), `pivot` (compose roles).
- `line_breaks`: `rewrap` joins the cue's typed lines; `keep` keeps them.
- `word_lead` (0 to 0.3 s): words and syllables read as starting and ending this
  much earlier, so a highlight meets the voice. Line times keep.
- `preroll` (0 to 1 s): a line shows this much before it starts, never cutting the
  previous one short; its line `enter` runs from there.
- `hold` (0 to 10 s): after its end a line stays until the next one shows, at most
  this long; its line `exit` runs at the end of that span.
- `chunk`: each cue cut in order into groups of `max` words (2 to 4), each drawn
  as a line; `rows: "word"` puts each word on its own row. Line mode only.
- `word_size`: a size factor per word (0.25 to 4), read with the word values; the
  row reflows. Line mode only.
- `window`: lines around the active one, line mode only. `before` past lines above
  and `after` upcoming lines below (0 to 3 each); an upcoming line shows `lead`
  seconds before its start; lines stack `gap` font sizes apart. Between lines, the
  first upcoming line within its lead takes the active slot.
  - `glide` **(after 0.3.1)**: when the active slot passes to another line, the
    column moves to its new place over `duration` seconds (above 0, at most 2)
    along `curve`, instead of jumping. A line leaving the window keeps its look,
    moves with the column and fades out over the same time. Values read with
    `by_offset` and `by_state` take their new slot at once.

## Animation tracks

```json
"animate": [
  {"unit": "word", "slot": "enter", "property": "scale", "duration": 0.22, "curve": "back",
   "keys": [[0, 0.6], [0.6, 1.12], [1, 1]]},
  {"unit": "word", "slot": "active", "property": "wipe", "keys": [[0, 0], [1, 1]]},
  {"unit": "letter", "slot": "loop", "property": "offset_y", "stagger": 0.05,
   "wave": {"amplitude": 0.1, "frequency": 1.5, "phase_per_unit": 0.6}}
]
```

At most 64 tracks. Each track animates one `property` of one `unit` during one
`slot`.

**Units**: `line`, `word`, `syllable` (a syllable of a word cut into syllables, the
whole word otherwise), `letter`. Values fold from the line down: a letter's
opacity is the line's times the word's times the syllable's times its own.

**Slots** (times are the unit's own, `word_lead` applied to words, syllables and
letters):

| Slot | Runs | Key time 0 to 1 |
|---|---|---|
| `enter` | from the unit's start for `duration` (a line's from its pre-roll), plus `stagger` × its index; then holds its last key | over `duration` |
| `exit` | the last `duration` seconds of the line's visible span | over `duration` |
| `loop` | always | — (keys read their first key; use `wave`, `noise` or `value`) |
| `active` | while the unit is sung | over the unit's sung span |
| `past` | once the unit has ended | — ; **(after 0.3.1)** with its own `duration`, the keys run over that time from the unit's end: a timed return, held at the last key after |
| `future` | before the unit starts | — |

`duration` and `stagger` are constant seconds (default 0.3 and 0). A `past` track
given a `duration` must give one above 0.

**Shapes**: `keys` (`[[t, value], …]`, `t` from 0 to 1, eased between keys by
`curve`), `wave` (`amplitude` × sin(2π × `frequency` × time − `phase_per_unit` ×
unit index + `phase`)), `noise` (`amplitude`, `frequency`, `seed`), or `value` (a
constant or any value form). Curves: `linear`, `ease_in`, `ease_out`,
`ease_in_out`, `back`, `ease_in_cubic`, `ease_out_cubic`.

**Properties**:

| Property | Units | Effect |
|---|---|---|
| `opacity` | all | Multiplies |
| `scale`, `scale_x` | all; line **(after 0.3.1)** | Multiply (scale_x on top of scale) |
| `rotation` | all; line **(after 0.3.1)** | Adds degrees |
| `offset_x`, `offset_y` | all | Add, in em (the line's em for the line unit) |
| `blur` | all | Gaussian sigma in px (line and letter blurs add) |
| `color`, `color2` | word, syllable, letter | The fill colour, and the wipe's sung colour |
| `wipe` | word, syllable, letter | Share of the unit drawn in its sung colour, left to right; a syllable wipe runs over the syllable's own letters |
| `wipe_edge` **(after 0.3.1)** | word, syllable, letter | Width of the wipe's edge fade in em (0, the default, is the engine's hard edge). A soft edge enters wholly left of the unit and leaves wholly right of it, so it never pops in or out |
| `glow` | word, syllable, letter | Multiplies the glow pass's gain |
| `tracking` | line | Letter spacing in em, from the row's centre (or the edge it is set against) |
| `reveal_y` | line | Share of the block shown from its bottom up |
| `plate_opacity`, `plate_margin`, `plate_scale_x`, `plate_wipe` **(after 0.3.1)** | line, word, letter | The plates measured on that unit (see Plates) |

The line unit's `scale`, `scale_x` and `rotation` **(after 0.3.1)** reach every
letter and plate of the line, about the centre of its laid-out text; they apply
after letter spacing and before the line's offsets. Not in compose mode, where
the line unit draws `opacity` and `blur` only.

## Passes

```json
"render": {"passes": [
  {"kind": "plate", "on": "word", "color": "#FFE600", "alpha": 1, "margin_x": 0.12, "margin_y": 0, "radius": 0.12},
  {"kind": "glow", "color": "$sung", "alpha": 0.3, "alpha2": 0.65, "gain": 1.1},
  {"kind": "shadow", "color": "#000000", "alpha": 0.5, "offset_y": 0.06, "blur": 4},
  {"kind": "outline_only", "color": "#000000", "outline": 0.08},
  {"kind": "fill", "color": "#FFFFFF", "color2": "$sung", "alpha": 0.6, "alpha2": 1}
]}
```

Each letter is drawn once per pass, in pass order. The glow passes are drawn on
their own layer, blurred, and laid under everything else.

| Kind | Fields |
|---|---|
| `fill` | `color`, `color2` (the wipe's sung colour), `alpha`, `alpha2` (the sung side's opacity) |
| `outline_only` | as `fill`, plus `outline` (the stroke width in em) |
| `shadow` | `color`, `alpha`, `offset_y` (em), `blur` (px) |
| `glow` | `color`, `alpha`, `alpha2`, `gain` |
| `plate` **(after 0.3.1)** | `on`, `color`, `alpha`, `margin_x`, `margin_y`, `radius` |

### Plates (after 0.3.1)

A plate is a rounded rectangle drawn behind text, measured on its box:

- `on: "line"` (the default): one plate per row of the line (a chunk is a line),
  from its first letter's pen position to its last's, the row's height (font size
  × `line_height`).
- `on: "word"`: one plate per word, its pen span on its row.
- `on: "letter"`: one plate per letter, its pen span on its row.

`margin_x` and `margin_y` grow it on each side, `radius` rounds its corners, all
in the unit's em (negative margins shrink it). Plates are drawn under every letter
of their line, whatever the pass order; line mode only.

The unit's plate properties animate it: `plate_opacity` multiplies its opacity,
`plate_margin` adds a margin on every side (em), `plate_scale_x` scales it across
about its centre, and `plate_wipe` (0 to 1) draws only that share of it from its
left edge. A plate also takes its unit's opacity (and the line's), its offsets, and
its scale and rotation (a word plate scales with its word unless the layout
`anchor` is `unit`, where each letter scales in place). A plate property needs a
plate pass on its unit, and a syllable has no plate.

A highlighter on the sung word, as an example:

```json
"animate": [
  {"unit": "word", "slot": "future", "property": "plate_opacity", "value": 0},
  {"unit": "word", "slot": "enter", "property": "plate_wipe", "duration": 0.09, "curve": "ease_out", "keys": [[0, 0], [1, 1]]},
  {"unit": "word", "slot": "past", "property": "plate_opacity", "duration": 0.14, "curve": "ease_in", "keys": [[0, 1], [1, 0]]},
  {"unit": "word", "slot": "active", "property": "color", "value": "#121212"}
]
```

## Shader hooks

`shader` names a WGSL file inside the folder (plain relative names, `/` only, at
most 64 KiB). It may define `shade` and `displace`, nothing else:

```wgsl
fn shade(i: GlyphIn) -> vec4<f32> { return i.color * i.coverage; }
fn displace(v: VertexIn) -> vec2<f32> { return vec2<f32>(0.0, 0.0); }
```

`shade` returns the premultiplied linear colour of a pixel of every instance:
letters in every pass, plates and compose shapes. `displace` moves a quad corner
in canvas px.

```wgsl
struct GlyphIn {
  coverage: f32,          // the letter's (or plate's) ink at this pixel, 0 to 1
  local: vec2<f32>,       // the pixel in its quad, 0 to 1 across and down
  uv: vec2<f32>,          // the atlas coordinate (letters only)
  color: vec4<f32>,       // the pass colour, the wipe already applied
  color2: vec4<f32>,      // the wipe's sung colour
  time: f32,              // seconds
  progress: f32,          // the word's sung progress, 0 to 1
  frame: vec2<f32>,       // the pixel in the frame, 0 to 1 across and down (after 0.3.1)
  index: f32,             // the letter's number in its line (after 0.3.1)
  line: f32,              // the line's progress, 0 when it shows, 1 when it goes (after 0.3.1)
  line_time: vec2<f32>,   // seconds since the line showed, and until it goes (after 0.3.1)
  pass_kind: f32,         // 0 fill, 1 outline_only, 2 shadow, 3 glow, 4 plate or shape (after 0.3.1)
};
struct VertexIn { corner: vec2<f32>, center: vec2<f32>, time: f32, index: f32 };
```

`frame` and `line_time` are the same in the preview and the export at any size:
a noise drawn from `frame` dissolves alike everywhere. `line_time.y` gives an exit
its own clock (for example a dissolve over the line's last 0.7 s, with `hold` long
enough for it); `pass_kind` lets a glow light only an edge the fill cuts.

The shader profile keeps every frame bounded: no loops, no helper function calls,
no bindings or globals of its own, control nesting at most 12, at most 32 texture
reads. The hooks can read `g.canvas` (the canvas size), `cov_tex`/`msdf_tex` and
`samp`.

## Limits

16 KiB of JSON, 64 tracks, 32 parameters (5 at level 1), 64 KiB of WGSL.
