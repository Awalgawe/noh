# Language packs

One folder per language (`<code>/language.json`): the language's syllable
rule, and for some languages a dictionary. NOH reads `languages/` next to its
executable, like `effects/`. A pack is data only: no code runs from it.

Chinese, Korean and Japanese need no pack. Their lines are split by
character: one Han character, one Hangul block, or one kana (small kana and
ー join the one before). A kanji stays one unit.

## Dictionaries

The dictionaries are public datasets. They are never committed. Build them
next to the manifests of a build, from local copies or from the archives a
release build downloads (`tools/delivery.py acquire-datasets`, pinned in
`assets/syllable-sources.lock.json`):

    py -I tools/build_syllable_dicts.py --data D:\data --out target\release\languages
    py -I tools/build_syllable_dicts.py --inputs <acquired datasets> --out <languages>

Without its `dictionary.txt`, a pack proposes splits with its rule alone.

## Notice

The full notice, with each dataset's licence and the changes made, is
`licenses/syllable-dictionaries.txt` in a release (`assets/syllable-dictionaries-NOTICE.txt`).

- `en/dictionary.txt`: built from the Moby Hyphenation List by Grady Ward
  (Moby Project). Public domain.
- `fr/dictionary.txt`: built from Lexique 3.83 (http://www.lexique.org),
  column `orthosyll`, by Boris New, Christophe Pallier and contributors.
  Licence CC BY-SA 4.0. The built file is shared under the same licence.
  Cite: New, B., Pallier, C., Brysbaert, M., Ferrand, L. (2004). Lexique 2: A
  New French Lexical Database. Behavior Research Methods, Instruments, &
  Computers, 36(3), 516-524.
