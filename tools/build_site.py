#!/usr/bin/env python3
"""Build the download site (site/ -> _site/) for the latest published release.

The page is regenerated at each release: the browser never asks GitHub for the
version. Release data comes from the GitHub API (or --release-json, for tests and
local previews). The drawn NOH window speaks the words of the app it draws: they are
frozen in site/app-strings.json, taken from that version's tag (--snapshot-app-strings),
so work on main can neither break the build nor show unreleased words. The heading
font is the OFL Jost already in assets/.
"""
import argparse
import html
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
LANGS = ("en", "fr", "de", "es", "ja", "ko", "zh")
# BCP 47 tags: the Chinese page is Simplified only.
LANG_TAGS = {"en": "en", "fr": "fr", "de": "de", "es": "es", "ja": "ja", "ko": "ko", "zh": "zh-Hans"}
# Browser languages sent from the root page to a translated page. Traditional Chinese (zh-TW, zh-HK, zh-Hant…)
# stays on English: the Chinese page is Simplified only. The same pattern runs in Python tests and in the page.
REDIRECT_ZH = r"^zh(-(cn|sg|hans)(-|$)|$)"
LANG_NAMES = {"en": "English", "fr": "Français", "de": "Deutsch", "es": "Español",
              "ja": "日本語", "ko": "한국어", "zh": "简体中文"}
REPO = "Awalgawe/noh"
SITE_URL = "https://awalgawe.github.io/noh/"
# The NOH version the drawings were made from (they follow its interface, not the latest release).
# Redrawing for another version: change DRAWN, then run --snapshot-app-strings v<DRAWN>.
DRAWN = "0.4.0"
# Release files offered by the page, by their published names.
ASSETS = {
    "web": r"NOH-(?P<v>[0-9][^-]*)-windows-x64-Web-Setup\.exe",
    "standard": r"NOH-(?P<v>[0-9][^-]*)-windows-x64-Standard-Setup\.exe",
    "complete": r"NOH-(?P<v>[0-9][^-]*)-windows-x64-Complete-Setup\.exe",
    "minimal": r"NOH-(?P<v>[0-9][^-]*)-windows-x64-Minimal-Setup\.exe",
    "portable": r"NOH-(?P<v>[0-9][^-]*)-windows-x64\.zip",
    "linux": r"NOH-(?P<v>[0-9][^-]*)-linux-x64\.zip",
    "sums": r"SHA256SUMS",
}
# Jost for the headings only (135 KB); text uses the system fonts, CJK included.
FONTS = ("Jost-Variable.ttf", "Jost-OFL.txt")
# The bundled styles in the app's order (src/app_effects.rs: Classique first, then the folders by name);
# site/template.html draws one tile per entry, in this order (SIM_STYLES).
EFFECTS = ("classique", "band", "block", "breath", "drop", "dust", "highlighter", "karaoke-simple",
           "lit", "punch", "scroll", "sticker", "sweep")
# The app strings the drawings use (A("key") in the template), frozen in site/app-strings.json.
APP_KEYS = (
    "strip.add", "song.add", "lyrics.title", "lyrics.add", "lyrics.generating", "lyrics.lines.other",
    "lyrics.generate", "lyrics.generate_help", "lyrics.import",
    "style.tab_styles", "style.tab_settings", "style.show", "style.used",
    "style.no_lyrics", "style.off", "style.one_style", "fades.label",
    "ui.missing_inputs", "ui.export_running", "bar.ready", "bar.change", "bar.export_video", "status.done",
    "home.kind_pictures", "home.kind_videos", "home.kind_song", "home.kind_lyrics",
    "home.drop_title", "home.drop_body", "home.add_files", "home.local",
    "lyrics_screen.title", "lyrics_screen.lines.other", "lyrics_screen.edit", "lyrics_screen.done",
    "lyrics_screen.not_on_video", "lyrics_screen.text", "lyrics_screen.text_hint", "lyrics_screen.cut",
    "lyrics_screen.join", "edit.delete", "lyrics_screen.level_lines", "lyrics_screen.level_words",
    "lyrics_screen.level_syllables", "lyrics_screen.start_at", "lyrics_screen.sync", "words.tap_count",
    "lyrics_screen.game_stop", "lyrics_screen.game_again", "lyrics_screen.text_waits",
    "lyrics_screen.foot_idle", "lyrics_screen.game_foot",
)
UI_KEYS = ("pause", "play", "aria_step", "aria_hero", "copy", "copied", "selected",
           "copied_link", "copy_this", "ss_caption", "ss_opens", "ss_click")
MONTHS = {
    "en": "January February March April May June July August September October November December",
    "fr": "janvier février mars avril mai juin juillet août septembre octobre novembre décembre",
    "de": "Januar Februar März April Mai Juni Juli August September Oktober November Dezember",
    "es": "enero febrero marzo abril mayo junio julio agosto septiembre octubre noviembre diciembre",
}
NBSP = " "


class BuildError(Exception):
    pass


def fetch_release(repo, token=None):
    request = urllib.request.Request(
        "https://api.github.com/repos/%s/releases/latest" % repo,
        headers={"Accept": "application/vnd.github+json", "User-Agent": "noh-site-build",
                 "X-GitHub-Api-Version": "2022-11-28"})
    if token:
        request.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def pick_assets(release):
    """The offered files of a release, by key; every one is required."""
    tag = release["tag_name"]
    version = tag[1:] if tag.startswith("v") else tag
    found = {}
    for asset in release.get("assets", []):
        for key, pattern in ASSETS.items():
            match = re.fullmatch(pattern, asset["name"])
            if not match:
                continue
            if "v" in match.groupdict() and match.group("v") != version:
                raise BuildError("%s belongs to %s, not to %s" % (asset["name"], match.group("v"), tag))
            found[key] = {"name": asset["name"], "size": int(asset["size"]), "url": asset["browser_download_url"]}
    missing = sorted(set(ASSETS) - set(found))
    if missing:
        raise BuildError("release %s lacks the files the page offers: %s" % (tag, ", ".join(missing)))
    return version, found


def format_size(size, lang):
    """1 233 128 464 bytes: 1.23 GB; 51 365 664 bytes: 51 MB (decimal units, as the release page)."""
    comma = lang in ("fr", "de", "es")
    if size >= 1e9:
        number, unit = "%.2f" % (size / 1e9), "GB"
    else:
        number, unit = "%d" % round(size / 1e6), "MB"
    if comma:
        number = number.replace(".", ",")
    if lang == "fr":
        unit = {"GB": "Go", "MB": "Mo"}[unit]
    return number + NBSP + unit


def format_date(iso, lang):
    year, month, day = (int(part) for part in iso[:10].split("-"))
    if lang in ("ja", "zh"):
        return "%d年%d月%d日" % (year, month, day)
    if lang == "ko":
        return "%d년 %d월 %d일" % (year, month, day)
    name = MONTHS[lang].split()[month - 1]
    return {"en": "%d %s %d", "fr": "%d %s %d", "de": "%d. %s %d", "es": "%d de %s de %d"}[lang] % (day, name, year)


def interpolate(text, tokens, where):
    def one(match):
        key = match.group(1)
        if key not in tokens:
            raise BuildError("%s: unknown placeholder {%s}" % (where, key))
        return tokens[key]
    return re.sub(r"\{(\w+)\}", one, text)


def load_strings(lang):
    english = json.loads((SITE / "i18n" / "en.json").read_text(encoding="utf-8"))
    strings = json.loads((SITE / "i18n" / ("%s.json" % lang)).read_text(encoding="utf-8"))
    missing = sorted(set(english) - set(strings))
    extra = sorted(set(strings) - set(english))
    if missing or extra:
        raise BuildError("site/i18n/%s.json: missing %s, unknown %s" % (lang, missing, extra))
    return strings


def steps_html(strings, tokens):
    out = []
    for n in range(1, 7):
        caption = interpolate(strings["step.caption"], dict(tokens, n=str(n)), "step.caption")
        out.append('''        <li class="flow-step">
          <div class="flow-text">
            <h3>%s</h3>
            <p>%s</p>
          </div>
          <figure class="flow-video">
            <div class="flow-frame" data-sim="%d"></div>
            <div class="flow-bar"><span>%s</span><button class="flow-pause" type="button" data-step="%d">%s</button></div>
          </figure>
        </li>
''' % (interpolate(strings["step%d.title" % n], tokens, "step%d.title" % n),
       interpolate(strings["step%d.text" % n], tokens, "step%d.text" % n),
       n, caption, n, strings["ui.pause"]))
    return "".join(out)


def page_path(lang):
    return "" if lang == "en" else lang + "/"


def relative(from_lang, to_lang):
    up = "" if from_lang == "en" else "../"
    return (up + page_path(to_lang)) or "./"


def lang_menu(lang, strings):
    items = "".join(
        '<li><a href="%s" hreflang="%s" lang="%s" data-lang="%s"%s>%s</a></li>' % (
            relative(lang, other), LANG_TAGS[other], LANG_TAGS[other], other,
            ' aria-current="true"' if other == lang else "", LANG_NAMES[other])
        for other in LANGS)
    return ('<details class="lang"><summary aria-label="%s">%s</summary><ul>%s</ul></details>'
            % (html.escape(strings["nav.language"]), lang.upper(), items))


def head_links(site_url):
    links = ['<link rel="alternate" hreflang="%s" href="%s%s">' % (LANG_TAGS[lang], site_url, page_path(lang)) for lang in LANGS]
    links.append('<link rel="alternate" hreflang="x-default" href="%s">' % site_url)
    return "\n".join(links)


def language_script(lang):
    """The menu remembers a choice; the English page honours it, or sends a first visit to the browser's language."""
    redirect = ""
    if lang == "en":
        redirect = '''
  var others = ["fr", "de", "es", "ja", "ko", "zh"], code = "";
  if (others.indexOf(saved) >= 0) {
    code = saved;
  } else if (saved !== "en") {
    var want = ((navigator.languages && navigator.languages[0]) || navigator.language || "").toLowerCase();
    code = want.slice(0, 2);
    if (code === "zh" && !/%s/.test(want)) code = "";
  }
  if (others.indexOf(code) >= 0) location.replace(code + "/" + location.hash);''' % REDIRECT_ZH
    return '''<script>
(function () {
  var saved = null;
  try { saved = localStorage.getItem("noh-lang"); } catch (e) {}%s
  document.addEventListener("click", function (e) {
    var a = e.target.closest && e.target.closest(".lang a[data-lang]");
    if (a) { try { localStorage.setItem("noh-lang", a.getAttribute("data-lang")); } catch (err) {} }
  });
})();
</script>''' % redirect


def snapshot_app_strings(tag):
    """Freeze the app words the drawings use, from a released tag (git show), into site/app-strings.json."""
    def show(path):
        return subprocess.run(["git", "show", "%s:%s" % (tag, path)], cwd=ROOT, check=True,
                              capture_output=True, encoding="utf-8").stdout
    app, styles = {}, {}
    for lang in LANGS:
        catalog = {}
        for line in show("locales/%s.lang" % lang).splitlines():
            if line.strip() and not line.lstrip().startswith("#") and "=" in line:
                key, value = line.split("=", 1)
                catalog[key.strip()] = value
        missing = [key for key in APP_KEYS if key not in catalog]
        if missing:
            raise BuildError("%s: locales/%s.lang lacks %s" % (tag, lang, missing))
        app[lang] = {key: catalog[key] for key in APP_KEYS}
    for effect in EFFECTS:
        names = json.loads(show("assets/effects/%s/effect.json" % effect))["name"]
        for lang in LANGS:
            styles.setdefault(lang, []).append(names.get(lang, names["en"]))
    data = {"_source": "Taken from %s with tools/build_site.py --snapshot-app-strings %s; regenerate when the drawings are redrawn." % (tag, tag),
            "app": app, "styles": styles}
    (SITE / "app-strings.json").write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")


def load_app_strings():
    return json.loads((SITE / "app-strings.json").read_text(encoding="utf-8"))


def render(lang, template, release, site_url):
    version, files = release["version"], release["files"]
    strings = load_strings(lang)
    frozen = load_app_strings()
    app, styles = frozen["app"][lang], frozen["styles"][lang]
    windows = json.loads((SITE / "windows-strings.json").read_text(encoding="utf-8"))
    ss, edge = windows["smartscreen"][lang], windows["edge"][lang]

    tokens = {"version": version, "date": format_date(release["published_at"], lang), "drawn": DRAWN,
              "styles": str(len(EFFECTS))}
    for key, item in files.items():
        tokens["size_" + key] = format_size(item["size"], lang)
        tokens["file_" + key] = item["name"]
    for key in ("more", "run"):
        tokens[key] = '<b data-t="%s">%s</b>' % (key, html.escape(ss[key]))
    for key in ("keep", "keepanyway", "showmore", "del"):
        tokens[key] = '<b data-e="%s">%s</b>' % (key, html.escape(edge[key]))
    tokens["title"] = html.escape(ss["title"])

    data = {
        "lang": lang,
        "ui": {key: strings["ui." + key] for key in UI_KEYS},
        "app": {key: app[key] for key in APP_KEYS},
        "styles": styles,
        "ss": windows["smartscreen"],
        "edge": windows["edge"],
        "webFile": files["web"]["name"],
    }
    values = {
        "lang": LANG_TAGS[lang],
        "base": "" if lang == "en" else "../",
        "home": "./",
        "hreflang": head_links(site_url),
        "redirect": language_script(lang),
        "lang_menu": lang_menu(lang, strings),
        "steps": steps_html(strings, tokens),
        "data": json.dumps(data, ensure_ascii=False).replace("</", "<\\/"),
    }
    for key, item in files.items():
        values["url." + key] = html.escape(item["url"], quote=True)
        values["file." + key] = html.escape(item["name"])
        values["size." + key] = tokens["size_" + key]
    # step.caption is filled per step above; the ui.* strings go to the script as they are ({action}, {n} at run time).
    for key, text in strings.items():
        if not key.startswith(("_", "ui.")) and key != "step.caption":
            values[key] = interpolate(text, tokens, "%s/%s" % (lang, key))
    values["ui.pause"], values["ui.copy"] = strings["ui.pause"], strings["ui.copy"]
    for key in ("meta.title", "meta.description"):
        values[key] = html.escape(re.sub(r"<[^>]+>", "", values[key]), quote=True)

    def one(match):
        key = match.group(1)
        if key not in values:
            raise BuildError("template: no value for {{%s}}" % key)
        return values[key]
    page = re.sub(r"\{\{([\w.]+)\}\}", one, template)
    if "{{" in page:
        raise BuildError("template: unresolved placeholder left in the %s page" % lang)
    return page


def build(out, release, site_url=SITE_URL):
    template = (SITE / "template.html").read_text(encoding="utf-8")
    out = Path(out)
    if out.exists():
        shutil.rmtree(out)
    (out / "fonts").mkdir(parents=True)
    for lang in LANGS:
        target = out / page_path(lang) / "index.html"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(render(lang, template, release, site_url), encoding="utf-8", newline="\n")
    for name in FONTS:
        shutil.copyfile(ROOT / "assets" / name, out / "fonts" / name)
    shutil.copyfile(ROOT / "assets" / "noh-icon.svg", out / "noh-icon.svg")
    (out / ".nojekyll").write_text("", encoding="utf-8")


def release_data(raw):
    version, files = pick_assets(raw)
    return {"version": version, "tag": raw["tag_name"], "published_at": raw["published_at"], "files": files}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", default="_site")
    parser.add_argument("--repo", default=REPO)
    parser.add_argument("--site-url", default=SITE_URL)
    parser.add_argument("--release-json", help="use this release JSON (GitHub API shape) instead of the API")
    parser.add_argument("--snapshot-app-strings", metavar="TAG",
                        help="write site/app-strings.json from this released tag (when the drawings are redrawn), then stop")
    args = parser.parse_args(argv)
    if args.snapshot_app_strings:
        try:
            snapshot_app_strings(args.snapshot_app_strings)
        except (BuildError, subprocess.CalledProcessError) as error:
            print("snapshot failed: %s" % error, file=sys.stderr)
            return 1
        print("site/app-strings.json written from %s" % args.snapshot_app_strings)
        return 0
    if args.release_json:
        raw = json.loads(Path(args.release_json).read_text(encoding="utf-8"))
    else:
        raw = fetch_release(args.repo, os.environ.get("GITHUB_TOKEN"))
    site_url = args.site_url if args.site_url.endswith("/") else args.site_url + "/"
    try:
        release = release_data(raw)
        build(args.out, release, site_url)
    except BuildError as error:
        print("site build failed: %s" % error, file=sys.stderr)
        return 1
    print("site built for %s (%s) into %s" % (release["tag"], release["published_at"][:10], args.out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
