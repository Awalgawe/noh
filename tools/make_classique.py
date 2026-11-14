"""The Classique effect (assets/effects/classique/effect.json), written out from
libass's geometry (captions.rs Canvas::new) as expressions, so the JSON stays
exact and readable. Run from anywhere: py -I tools/make_classique.py"""
import json
import os
import struct

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# libass sets Fontsize as FreeType REAL_DIM: ascender - descender (hhea), not the em.
# The engine's size is an em: em = F x upem / (ascender - descender), read from the font.
_font = open(os.path.join(REPO, 'assets', 'NOHCJK.otf'), 'rb').read()
_tables = {}
for _i in range(struct.unpack('>H', _font[4:6])[0]):
    _tag, _cs, _off, _len = struct.unpack('>4sIII', _font[12 + 16 * _i:28 + 16 * _i])
    _tables[_tag.decode()] = _off
UPEM = struct.unpack('>H', _font[_tables['head'] + 18:_tables['head'] + 20])[0]
ASC, DESC = struct.unpack('>hh', _font[_tables['hhea'] + 4:_tables['hhea'] + 8])
_fs = struct.unpack('>H', _font[_tables['OS/2'] + 62:_tables['OS/2'] + 64])[0]
assert not _fs & 0x80, 'USE_TYPO_METRICS set: FreeType would use OS/2 typo metrics'
assert (UPEM, ASC, DESC) == (1000, 1160, -288), (UPEM, ASC, DESC)
REAL = (ASC - DESC) / UPEM
EM = UPEM / (ASC - DESC)

F = {"max": [{"round": [{"mul": ["@short", {"choose": ["$size", {"small": 0.04, "medium": 0.055, "large": 0.07}]}]}]}, 8]}
SIZE = {"mul": [F, EM]}
O = {"max": [{"ceil": [{"div": [F, 16]}]}, 1]}
RES = {"add": [O, {"ceil": [{"mul": [F, 0.1]}]}, 2]}
M = {"max": [{"ceil": [{"mul": ["@short", 0.05]}]}, 4]}


def margin(inset):
    # libass: the safe area's inset plus the reserve, else 5 % of the short side.
    return {"choose": ["@safe", {"1": {"add": [inset, {"ceil": [RES]}]}, "0": M}]}


L, T, R, B = margin("@inset_l"), margin("@inset_t"), margin("@inset_r"), margin("@inset_b")
NAMES = {
    "fr": "Classique", "en": "Classic", "de": "Klassisch", "es": "Clásico",
    "ja": "クラシック", "ko": "클래식", "zh": "经典",
}
SIZE_LABEL = {"fr": "Taille", "en": "Size", "de": "Größe", "es": "Tamaño", "ja": "サイズ", "ko": "크기", "zh": "大小"}
PLACEMENT_LABEL = {"fr": "Position", "en": "Position", "de": "Position", "es": "Posición", "ja": "位置", "ko": "위치", "zh": "位置"}
WIDTH_LABEL = {
    "fr": "Largeur des lignes (% de la zone utile)", "en": "Line width (% of the usable area)",
    "de": "Zeilenbreite (% des nutzbaren Bereichs)", "es": "Ancho de las líneas (% de la zona útil)",
    "ja": "行の幅(使用可能領域の %)", "ko": "줄 너비(사용 가능 영역의 %)", "zh": "行宽（可用区域的 %）",
}
# The line width, in % of the usable width between the margins. Its default is
# the width NOH always had: its libass pre-wrap measured letters at the em F
# while libass draws them at F x EM, so lines wrapped at EM (69.06 %) of the
# usable width. It is now a setting (maintainer, 9 Nov.), and the libass fallback
# follows it (caption_layout).
WIDTH_DEFAULT = EM * 100

effect = {
    "engine": 1,
    "id": "noh.classic",
    "name": NAMES,
    "version": "0.1.0",
    "license": "GPL-3.0-only",
    "target": "text",
    "params": [
        {"id": "size", "type": "choice", "default": "medium", "choices": ["small", "medium", "large"], "level": 1, "label": SIZE_LABEL},
        {"id": "placement", "type": "choice", "default": "bottom", "choices": ["bottom", "top"], "level": 1, "label": PLACEMENT_LABEL},
        {"id": "width", "type": "number", "default": WIDTH_DEFAULT, "min": 40, "max": 100, "unit": "%", "level": 2, "label": WIDTH_LABEL},
    ],
    "presets": [
        {"id": "classique", "name": NAMES, "values": {}},
    ],
    "layout": {
        "mode": "line",
        "font": "cjk",
        "size": SIZE,
        "line_height": REAL,  # libass advances lines by its REAL_DIM height (F px)
        "box": {
            "space": "canvas",
            # Centred between the left and right margins, as libass alignment 2/8.
            "x": {"div": [{"div": [{"add": [L, {"sub": ["@w", R]}]}, 2]}, "@w"]},
            # Width: canvas minus both margins minus twice the reserve (libass
            # max_width), times the line-width setting.
            "width": {"mul": [{"div": [{"sub": ["@w", L, R, {"mul": [RES, 2]}]}, "@w"]}, {"div": ["$width", 100]}]},
            "y": {"choose": ["$placement", {"bottom": {"div": [{"sub": ["@h", B]}, "@h"]}, "top": {"div": [T, "@h"]}}]},
            "anchor": {"choose": ["$placement", {"bottom": "bottom", "top": "top"}]},
        },
        "visible": "line",
        "anchor": "unit",
        # The lines typed in a cue stay lines, as libass draws them (maintainer, 9 Nov.:
        # « laisser le choix à l'effet »).
        "line_breaks": "keep",
    },
    "animate": [],
    "render": {
        "passes": [
            {"kind": "outline_only", "color": "#000000", "outline": {"div": [O, SIZE]}},
            {"kind": "fill", "color": "#FFFFFF"},
        ]
    },
}


# Readable form: structure indented, each formula on one line.
def dump(v, ind=0):
    pad = ' ' * ind
    if isinstance(v, dict) and any(isinstance(x, (dict, list)) for x in v.values()) and not set(v) & {"max", "min", "add", "sub", "mul", "div", "round", "ceil", "floor", "choose"}:
        items = [f'{pad}  {json.dumps(k, ensure_ascii=False)}: {dump(x, ind + 2).lstrip()}' for k, x in v.items()]
        return pad + '{\n' + ',\n'.join(items) + '\n' + pad + '}'
    if isinstance(v, list) and v and all(isinstance(x, dict) for x in v):
        items = [pad + '  ' + json.dumps(x, ensure_ascii=False, separators=(', ', ': ')) for x in v]
        return pad + '[\n' + ',\n'.join(items) + '\n' + pad + ']'
    return pad + json.dumps(v, ensure_ascii=False, separators=(', ', ': '))


target = os.path.join(REPO, 'assets', 'effects', 'classique')
os.makedirs(target, exist_ok=True)
text = dump(effect) + '\n'
assert json.loads(text) == effect
open(os.path.join(target, 'effect.json'), 'w', encoding='utf-8', newline='\n').write(text)
print('written', len(text.encode('utf-8')), 'bytes')
