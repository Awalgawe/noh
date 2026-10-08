"""Regenerate the NOH desktop icon from the header wordmark and bundled font.

Optional asset tooling only: fontTools, Node.js and sharp are not build/runtime
dependencies. Run: python assets/prepare-icon.py [path/to/sharp]
"""
from pathlib import Path
import subprocess
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont

assets = Path(__file__).resolve().parent
font = TTFont(assets / "Jost-ExtraBold.ttf")
glyphs = font.getGlyphSet()
cmap = font.getBestCmap()
letters = []
for letter, x in [("N", -71.93), ("O", 1029.92), ("H", 2150.19)]:
    pen = SVGPathPen(glyphs)
    glyphs[cmap[ord(letter)]].draw(pen)
    letters.append(f'<path transform="translate({x} 735) scale(1 -1)" d="{pen.getCommands()}"/>')

# Geometry matches src/app_icons.rs::wordmark (2886.1 x 770 font units), 190
# pixels wide and centred. The square background keeps the light mark legible
# on both light and dark desktop/taskbar surfaces.
scale = 190 / 2886.1
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <rect x="4" y="4" width="248" height="248" rx="44" fill="#19191c"/>
  <g transform="translate(33 {128 - 385 * scale:.3f}) scale({scale:.6f})" fill="#f5f4f1">
    ''' + "\n    ".join(letters) + "\n  </g>\n</svg>\n"
(assets / "noh-icon.svg").write_text(svg, encoding="utf-8")

# PNG payloads in a multi-resolution ICO preserve alpha on modern Windows.
render = r"""
const fs = require('node:fs');
const path = require('node:path');
const sharp = require(process.argv[2]);
const assets = process.argv[1];
(async () => {
  const sizes = [16, 24, 32, 48, 64, 128, 256];
  const frames = await Promise.all(sizes.map(size =>
    sharp(path.join(assets, 'noh-icon.svg'), {density: 384})
      .resize(size, size).ensureAlpha().png({palette: false}).toBuffer()));
  fs.writeFileSync(path.join(assets, 'noh-icon.png'), frames.at(-1));
  const directory = Buffer.alloc(6 + 16 * sizes.length);
  directory.writeUInt16LE(1, 2);
  directory.writeUInt16LE(sizes.length, 4);
  let offset = directory.length;
  frames.forEach((frame, index) => {
    const entry = 6 + index * 16;
    directory[entry] = directory[entry + 1] = sizes[index] % 256;
    directory.writeUInt16LE(1, entry + 4);
    directory.writeUInt16LE(32, entry + 6);
    directory.writeUInt32LE(frame.length, entry + 8);
    directory.writeUInt32LE(offset, entry + 12);
    offset += frame.length;
  });
  fs.writeFileSync(path.join(assets, 'noh-icon.ico'), Buffer.concat([directory, ...frames]));
  console.log(JSON.stringify({sizes, pngBytes: frames.at(-1).length, icoBytes: offset}));
})().catch(error => { console.error(error); process.exitCode = 1; });
"""
subprocess.run(
    ["node", "-e", render, str(assets), sys.argv[1] if len(sys.argv) > 1 else "sharp"],
    check=True,
)
