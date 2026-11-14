# Embedded fonts

## Noto Sans SemiBold

`NotoSans-SemiBold.ttf` is the unmodified Noto Sans SemiBold face (weight 600),
used for headings and primary actions. Copyright 2018 The Noto Project Authors.
It is licensed under SIL Open Font License 1.1; `NotoSans-OFL.txt` is distributed
as `FONT-NOTO-SANS-LICENSE.txt` in the portable application.

Retrieved on 2026-09-28 from the official Noto archive at pinned revision
`ffebf8c1ee449e544955a7e813c54f9b73848eac`:
[font source](https://github.com/notofonts/noto-fonts/blob/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSans/NotoSans-SemiBold.ttf),
[license](https://github.com/notofonts/noto-fonts/blob/ffebf8c1ee449e544955a7e813c54f9b73848eac/LICENSE).

SHA-256: `87a8b90ece1e89746b544e4e086f85a3710e41485a8078f9be874837dfad45d5`.
No font installation or runtime download is needed. CJK glyphs use the existing
regular fallback below; their hierarchy also depends on size and colour.

## Noto Sans Regular

`NotoSans-Regular.ttf` is the unmodified Noto Sans Regular face (weight 400) from
the same family, copyright and SIL Open Font License 1.1 (`NotoSans-OFL.txt`).
It is the embedded body text face of the interface, ahead of egui's defaults.

Retrieved on 2026-09-29 from the same pinned revision
`ffebf8c1ee449e544955a7e813c54f9b73848eac`:
[font source](https://github.com/notofonts/noto-fonts/blob/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSans/NotoSans-Regular.ttf).

SHA-256: `b85c38ecea8a7cfb39c24e395a4007474fa5a4fc864f6ee33309eb4948d232d5`.

## Jost ExtraBold

`Jost-ExtraBold.ttf` is Jost at weight 800, used for the NOH wordmark in the
header and by caption effects (`jost_extrabold`). Copyright 2020 The Jost Project Authors
(https://github.com/indestructible-type). It is licensed under SIL Open Font
License 1.1; `Jost-OFL.txt` is distributed as `FONT-JOST-LICENSE.txt` in the
portable application.

Upstream publishes only a variable font; egui cannot select its weight.
Retrieved on 2026-10-01 from Google Fonts `ofl/jost/Jost[wght].ttf`
(SHA-256 `6343b70971000b04c5d401c96ae08ce371086135e999d5e1e1413039c0213076`)
and instantiated at `wght=800` with the fontTools variable-font instancer;
no outline was edited.

SHA-256: `75be210e246fa61353d5a141a2d1becb0df65c10b2c430f1cd1b595b7c7358f9`.

## NOH desktop icon

`noh-icon.svg`, `noh-icon.png` and `noh-icon.ico` reproduce the NOH wordmark
from `src/app_icons.rs`, with Jost ExtraBold outlines on a dark square. The PNG
supplies the native window and Windows executable
icon; the ICO also provides 16, 24, 32, 48, 64, 128 and 256 pixel variants.
Regenerate with `prepare-icon.py` (optional fontTools and Node.js/sharp tools).
These tools are not required to compile or run NOH. The Jost license
above applies to the font used to produce the lettering.

## Caption fonts (lyrics step 7)

Unmodified files from the official Google Fonts repository `google/fonts`,
retrieved on 2026-11-11 at pinned commit
`c30b221beb0c1da59d563feb91ec8656a1fa972a`. Each file was checked against the
git blob hash the repository publishes for it before use. They are embedded for
caption effects only. All are licensed under SIL Open Font License 1.1.

### Inter

`Inter-Variable.ttf` is `ofl/inter/Inter[opsz,wght].ttf` (git blob
`047c92f6e2212473dc436020afed689527076d44`), drawn at weight 600; its optical
size axis stays at the file's default. Copyright 2020 The Inter Project Authors
(https://github.com/rsms/inter). `Inter-OFL.txt` (blob
`21f6aff961064c2e429f570995e446bcdd555422`) is distributed as
`FONT-INTER-LICENSE.txt` in the portable application.
[font source](https://github.com/google/fonts/blob/c30b221beb0c1da59d563feb91ec8656a1fa972a/ofl/inter/Inter%5Bopsz,wght%5D.ttf).

SHA-256: `29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031`.

### Barlow Condensed ExtraBold

`BarlowCondensed-ExtraBold.ttf` is `ofl/barlowcondensed/BarlowCondensed-ExtraBold.ttf`
(git blob `d5a6c7505459f142b20d65d3579e1b840462b75f`). Copyright 2017 The Barlow
Project Authors (https://github.com/jpt/barlow). `BarlowCondensed-OFL.txt` (blob
`2f22ba6ca4dbc12145ae40b14c3368f387cba0e2`) is distributed as
`FONT-BARLOW-CONDENSED-LICENSE.txt` in the portable application.
[font source](https://github.com/google/fonts/blob/c30b221beb0c1da59d563feb91ec8656a1fa972a/ofl/barlowcondensed/BarlowCondensed-ExtraBold.ttf).

SHA-256: `724c9c25952d5f4a2d87185d9767aa006144c5f0d944dc05bf7d5d603551c260`.

### Jost (variable)

`Jost-Variable.ttf` is `ofl/jost/Jost[wght].ttf` (git blob
`cdf8630280e37a7e8040dd5a32e3c13a9794e6d4`), the same file the Jost ExtraBold
instance above was made from (same SHA-256), drawn at weight 500. Its license is
`Jost-OFL.txt` (same text as the repository's `ofl/jost/OFL.txt`).

SHA-256: `6343b70971000b04c5d401c96ae08ce371086135e999d5e1e1413039c0213076`.

## Noto Serif Italic

`NotoSerif-Italic.ttf` is the unmodified variable file
`ofl/notoserif/NotoSerif-Italic[wdth,wght].ttf` of `google/fonts`, drawn at its
default instance. Copyright 2022 The Noto Project Authors. It is licensed under
SIL Open Font License 1.1; `NotoSerif-OFL.txt` is distributed as
`FONT-NOTO-SERIF-LICENSE.txt` in the portable application. It was embedded on
2026-11-08 without this record; on 2026-11-11 its git blob
`0e60f3e9248b524e6c9a28d38fbfc147463b1f5a` and its license's blob
`6843f31878c9945e1e71e1baa275546b4eefead8` were found identical to the files of
that name at commit `c30b221beb0c1da59d563feb91ec8656a1fa972a`.

SHA-256: `e87acbc6c0efd0d9a20d6a8cbbda2b266c14be3a3a6f5af8ec9d7b2460570ad1`.

## NOH CJK Regular

NOH CJK Regular is derived from Noto Sans CJK SC Regular.

SHA-256: `4490560cbf2fd2856f282a48c570ad0683d843eb82a9ff5d0543eec60e1f32f4`.

Original copyright: © 2014–2021 Adobe (http://www.adobe.com/).

Source: https://github.com/notofonts/noto-cjk/tree/main/Sans/OTF/SimplifiedChinese

License: SIL Open Font License 1.1, reproduced in `OFL.txt` (distributed as `FONT-LICENSE.txt` in the portable application).

The derived font retains all 44,810 Unicode characters, removes unused glyph variants and layout features, and uses the new family name “NOH CJK”. This covers Asian UI text and filenames without loading system fonts or accessing the network. It uses the Simplified Chinese forms of shared Han characters.

To reproduce it, install fontTools and run `python assets/prepare-font.py path/to/NotoSansCJKsc-Regular.otf`. The source font was retrieved from the official repository on 2026-09-26. Font tools are needed only to regenerate this asset, not to build, test or run NOH. See `docs/ARCHITECTURE.md` for the build and runtime boundaries.
