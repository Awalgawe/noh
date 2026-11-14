"""Build NOH's syllable dictionaries from public datasets (Task 41 S6b).

The datasets are never committed. This copies the language manifests from
assets/languages and writes each pack's dictionary.txt beside them, from the
D:\\data copies or from the archives a release build downloaded (Task 44:
tools/delivery.py acquire-datasets, pinned in assets/syllable-sources.lock.json):

    py -I tools/build_syllable_dicts.py --data D:\\data --out target\\release\\languages
    py -I tools/build_syllable_dicts.py --inputs <acquired datasets> --out <languages>
    py -I tools/build_syllable_dicts.py --check

The archive members and the built dictionaries are checked against the lock's
sha256 and word counts: a different input or a different build fails.

Dictionary format: UTF-8, one "word<TAB>split" line per word, lower case,
sorted, syllables split with "-" (a hyphen of the word itself is written
before the cut, as NOH's parse_syllables reads it).
"""
import argparse
import csv
import hashlib
import io
import json
import shutil
import sys
import tempfile
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LOCK = REPO / "assets" / "syllable-sources.lock.json"
MOBY = Path("datasets/text/syllabification/moby-hyphenation/mhyph.txt")
LEXIQUE = Path("datasets/text/syllabification/lexique-3.83/Lexique383.zip")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def pinned(data, expected, what):
    """`data` must hash to the lock's value, else the build stops."""
    got = sha256(data)
    if got != expected:
        raise SystemExit(f"{what}: sha256 {got} differs from the lock's {expected}")
    return data


def member(archive, spec):
    """The locked member of a downloaded archive, checked."""
    with zipfile.ZipFile(archive) as z:
        return pinned(z.read(spec["member"]), spec["member_sha256"], f"{archive.name}:{spec['member']}")


VOWELS = set("aeiouyàâäáãåæéèêëíìîïóòôöõœúùûüýÿ")


def sung(split):
    """A part without a vowel is not sung on its own: it joins the one before
    (Lexique writes some plurals "in-vo-lu-tif-s")."""
    parts = [p for p in split.split("-") if p]
    out = []
    for p in parts:
        if out and not VOWELS.intersection(p):
            out[-1] += p
        elif out and not VOWELS.intersection(out[-1]):
            out[-1] += p
        else:
            out.append(p)
    return "-".join(out)


def every_part_sung(split):
    """Each part has a vowel (else the word is left to the rule)."""
    return all(VOWELS.intersection(p) for p in split.split("-") if p)


def moby(lines):
    """English: Moby is Mac Roman text; byte 0xA5 (•) marks the syllable breaks."""
    out = {}
    for line in lines:
        line = line.strip()
        if not line or " " in line:
            continue
        word = line.replace("•", "").lower()
        # A hyphen of the word itself is written doubled: it stays in the word, then cuts.
        split = line.replace("-", "--").replace("•", "-").lower()
        # Some Moby entries cut a vowel-less part ("a-c-res"): the rule does better.
        if word and word not in out and every_part_sung(split):
            out[word] = split
    return out


def lexique(rows):
    """French: Lexique's orthosyll column; words with their own hyphen or a space are left to the rule."""
    out = {}
    for row in rows:
        word, split = row["ortho"].lower(), row["orthosyll"].lower()
        if not word or " " in word or "-" in word or split.replace("-", "") != word:
            continue
        split = sung(split)
        if every_part_sung(split):
            out.setdefault(word, split)
    return out


def write(words, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        for word in sorted(words):
            f.write(f"{word}\t{words[word]}\n")


def check_outputs(out, outputs):
    """The built dictionaries must be the reviewed ones (lock: sha256, words)."""
    for lang, want in outputs.items():
        data = (out / lang / "dictionary.txt").read_bytes()
        words = data.count(b"\n")
        if words != want["words"]:
            raise SystemExit(f"{lang}: {words} words, the lock says {want['words']}")
        pinned(data, want["sha256"], f"{lang}/dictionary.txt")


def check():
    """The converters on a few lines written here (no dataset read), and the lock
    checks on small stand-in files."""
    en = moby(["re•mem•ber", "cam•er•a", "a cappella", "Re•mem•ber", "ca•fé", "a•c•res", "ab•sent-mind•ed"])
    assert en == {"remember": "re-mem-ber", "camera": "cam-er-a", "café": "ca-fé", "absent-minded": "ab-sent--mind-ed"}, en
    rows = [{"ortho": "chante", "orthosyll": "chan-te"}, {"ortho": "peut-être", "orthosyll": "peut-ê-tre"},
            {"ortho": "a priori", "orthosyll": "a pri-o-ri"}, {"ortho": "été", "orthosyll": "é-té"},
            {"ortho": "involutifs", "orthosyll": "in-vo-lu-tif-s"}, {"ortho": "cm", "orthosyll": "cm"}]
    fr = lexique(rows)
    assert fr == {"chante": "chan-te", "été": "é-té", "involutifs": "in-vo-lu-tifs"}, fr
    with tempfile.TemporaryDirectory() as tmp:
        archive = Path(tmp) / "a.zip"
        with zipfile.ZipFile(archive, "w") as z:
            z.writestr("m.txt", b"re\xa5mem\xa5ber\r\n")
        spec = {"member": "m.txt", "member_sha256": sha256(b"re\xa5mem\xa5ber\r\n")}
        assert member(archive, spec) == b"re\xa5mem\xa5ber\r\n"
        try:
            member(archive, spec | {"member_sha256": "0" * 64})
            raise AssertionError("a changed member must be refused")
        except SystemExit as e:
            assert "differs from the lock" in str(e), e
        out = Path(tmp) / "out"
        write({"remember": "re-mem-ber"}, out / "en" / "dictionary.txt")
        good = {"en": {"words": 1, "sha256": sha256(b"remember\tre-mem-ber\n")}}
        check_outputs(out, good)
        for bad in ({"words": 2}, {"sha256": "0" * 64}):
            try:
                check_outputs(out, {"en": good["en"] | bad})
                raise AssertionError(f"a different output must be refused: {bad}")
            except SystemExit:
                pass
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    assert set(lock["outputs"]) == {"en", "fr"} and all(d["url"].startswith("https://") for d in lock["datasets"].values())
    print("check ok")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--data", type=Path, help="the D:\\data folder")
    p.add_argument("--inputs", type=Path, help="the archives acquired by tools/delivery.py acquire-datasets")
    p.add_argument("--out", type=Path, help="the languages folder of a build")
    p.add_argument("--check", action="store_true")
    a = p.parse_args()
    if a.check:
        check()
        return
    if not a.out or bool(a.data) == bool(a.inputs):
        p.error("--out and one of --data or --inputs are required")
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    moby_spec, lexique_spec = lock["datasets"]["moby"], lock["datasets"]["lexique"]
    if a.inputs:
        moby_bytes = member(a.inputs / moby_spec["file"], moby_spec)
        lexique_bytes = member(a.inputs / lexique_spec["file"], lexique_spec)
    else:
        moby_bytes = pinned((a.data / MOBY).read_bytes(), moby_spec["member_sha256"], "mhyph.txt")
        lexique_bytes = member(a.data / LEXIQUE, lexique_spec)
    for manifest in sorted((REPO / "assets" / "languages").glob("*/language.json")):
        target = a.out / manifest.parent.name
        target.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(manifest, target / "language.json")
    en = moby(io.TextIOWrapper(io.BytesIO(moby_bytes), encoding="mac_roman"))
    write(en, a.out / "en" / "dictionary.txt")
    fr = lexique(csv.DictReader(io.TextIOWrapper(io.BytesIO(lexique_bytes), encoding="utf-8"), delimiter="\t"))
    write(fr, a.out / "fr" / "dictionary.txt")
    check_outputs(a.out, lock["outputs"])
    print(f"en {len(en)} words, fr {len(fr)} words, as locked -> {a.out}")


if __name__ == "__main__":
    sys.exit(main())
