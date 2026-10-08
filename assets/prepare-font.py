"""Build NOHCJK.otf from NotoSansCJKsc-Regular.otf (fontTools required).

Keep all encoded characters for UI text and arbitrary filenames, while removing
unused glyph variants and OpenType layout tables (egui does not use these).
Usage: python assets/prepare-font.py path/to/NotoSansCJKsc-Regular.otf
"""
import sys
from pathlib import Path
from fontTools import subset
from fontTools.ttLib import TTFont

font = TTFont(sys.argv[1])
copyright_notice = font['name'].getDebugName(0)
options = subset.Options()
options.layout_features = []
options.name_IDs = ['*']
options.name_languages = ['*']
options.recalc_timestamp = False
subsetter = subset.Subsetter(options=options)
subsetter.populate(unicodes=list(font.getBestCmap()))
subsetter.subset(font)
names = {1: 'NOH CJK', 2: 'Regular', 3: 'NOHCJK-Regular-1', 4: 'NOH CJK Regular', 6: 'NOHCJK-Regular', 16: 'NOH CJK', 17: 'Regular'}
for record in font['name'].names:
    if record.nameID in names:
        record.string = names[record.nameID].encode(record.getEncoding())
if 'CFF ' in font:
    cff = font['CFF '].cff
    cff.fontNames = ['NOHCJK-Regular']
    cff.topDictIndex[0].FullName = 'NOH CJK Regular'
    cff.topDictIndex[0].FamilyName = 'NOH CJK'
folder = Path(__file__).resolve().parent
font.save(folder / 'NOHCJK.otf')
print(copyright_notice)
print(f'{len(font.getBestCmap())} characters, {(folder / "NOHCJK.otf").stat().st_size:,} bytes')
