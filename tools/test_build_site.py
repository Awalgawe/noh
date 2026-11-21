"""Check the download site build offline, from a recorded release."""
import json
from pathlib import Path
import re
import tempfile
import unittest

import build_site

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "tools" / "testdata" / "site-release.json"


def fixture():
    return json.loads(FIXTURE.read_text(encoding="utf-8"))


class FormatTests(unittest.TestCase):
    def test_sizes_follow_the_release_page_units(self):
        self.assertEqual(build_site.format_size(51365664, "en"), "51 MB")
        self.assertEqual(build_site.format_size(1233128464, "en"), "1.23 GB")
        self.assertEqual(build_site.format_size(1233128464, "fr"), "1,23 Go")
        self.assertEqual(build_site.format_size(500048640, "de"), "500 MB")

    def test_dates_are_written_in_each_language(self):
        when = "2026-11-20T13:16:57Z"
        self.assertEqual(build_site.format_date(when, "en"), "20 November 2026")
        self.assertEqual(build_site.format_date(when, "fr"), "20 novembre 2026")
        self.assertEqual(build_site.format_date(when, "de"), "20. November 2026")
        self.assertEqual(build_site.format_date(when, "es"), "20 de noviembre de 2026")
        self.assertEqual(build_site.format_date(when, "ja"), "2026年11月20日")
        self.assertEqual(build_site.format_date(when, "ko"), "2026년 11월 20일")


class ReleaseTests(unittest.TestCase):
    def test_every_offered_file_is_found(self):
        version, files = build_site.pick_assets(fixture())
        self.assertEqual(version, "0.3.1")
        self.assertEqual(files["web"]["name"], "NOH-0.3.1-windows-x64-Web-Setup.exe")
        self.assertEqual(files["linux"]["name"], "NOH-0.3.1-linux-x64.zip")
        self.assertEqual(sorted(files), sorted(build_site.ASSETS))

    def test_a_missing_file_fails_the_build(self):
        raw = fixture()
        raw["assets"] = [a for a in raw["assets"] if not a["name"].endswith("Web-Setup.exe")]
        with self.assertRaisesRegex(build_site.BuildError, "web"):
            build_site.pick_assets(raw)

    def test_a_file_of_another_version_fails_the_build(self):
        raw = fixture()
        raw["tag_name"] = "v9.9.9"
        with self.assertRaisesRegex(build_site.BuildError, "not to v9.9.9"):
            build_site.pick_assets(raw)


class StringTests(unittest.TestCase):
    def test_every_language_has_every_key_and_placeholder(self):
        english = json.loads((ROOT / "site" / "i18n" / "en.json").read_text(encoding="utf-8"))
        for lang in build_site.LANGS:
            strings = json.loads((ROOT / "site" / "i18n" / ("%s.json" % lang)).read_text(encoding="utf-8"))
            with self.subTest(lang=lang):
                self.assertEqual(sorted(strings), sorted(english))
                for key, text in english.items():
                    self.assertEqual(sorted(set(re.findall(r"\{\w+\}", strings[key]))),
                                     sorted(set(re.findall(r"\{\w+\}", text))), key)

    def test_the_drawings_use_only_frozen_app_strings(self):
        template = (ROOT / "site" / "template.html").read_text(encoding="utf-8")
        used = set(re.findall(r'\bA\("([\w.]+)"', template))
        self.assertTrue(used)
        self.assertEqual(sorted(used - set(build_site.APP_KEYS)), [])
        frozen = build_site.load_app_strings()
        self.assertIn("v%s" % build_site.DRAWN, frozen["_source"])
        for lang in build_site.LANGS:
            with self.subTest(lang=lang):
                self.assertEqual(sorted(frozen["app"][lang]), sorted(build_site.APP_KEYS))
                self.assertEqual(len(frozen["styles"][lang]), len(build_site.EFFECTS))

    def test_only_simplified_chinese_goes_to_the_chinese_page(self):
        for want, sent in [("zh", True), ("zh-cn", True), ("zh-sg", True), ("zh-hans", True), ("zh-hans-cn", True),
                           ("zh-tw", False), ("zh-hk", False), ("zh-hant-tw", False), ("zh-mo", False)]:
            with self.subTest(want=want):
                self.assertEqual(bool(re.search(build_site.REDIRECT_ZH, want)), sent)

    def test_windows_wording_exists_for_every_language(self):
        data = json.loads((ROOT / "site" / "windows-strings.json").read_text(encoding="utf-8"))
        for lang in build_site.LANGS:
            with self.subTest(lang=lang):
                self.assertTrue({"title", "body", "more", "app", "pub", "unknown", "run", "dont", "sep"}
                                <= set(data["smartscreen"][lang]))
                self.assertEqual(set(data["edge"][lang]), {"keep", "showmore", "keepanyway", "del"})


class BuildTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.out = Path(cls.tmp.name) / "_site"
        cls.release = build_site.release_data(fixture())
        build_site.build(cls.out, cls.release)
        cls.pages = {lang: (cls.out / build_site.page_path(lang) / "index.html").read_text(encoding="utf-8")
                     for lang in build_site.LANGS}

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_every_language_has_a_complete_page(self):
        for lang, page in self.pages.items():
            with self.subTest(lang=lang):
                self.assertIn('<html lang="%s">' % build_site.LANG_TAGS[lang], page)
                self.assertNotIn("{{", page)
                self.assertIn("window.NOH = {", page)

    def test_links_and_sizes_come_from_the_release(self):
        for lang, page in self.pages.items():
            for key, item in self.release["files"].items():
                with self.subTest(lang=lang, file=key):
                    self.assertIn('href="%s"' % item["url"], page)
                    if key != "sums":
                        self.assertIn(build_site.format_size(item["size"], lang), page)
        self.assertIn("Version 0.3.1, released on 20 November 2026.", self.pages["en"])

    def test_only_github_is_ever_requested_or_linked(self):
        allowed = ("https://github.com/Awalgawe/noh", build_site.SITE_URL)
        for lang, page in self.pages.items():
            for url in re.findall(r'(?:href|src)="(https?://[^"]+)"', page) + re.findall(r'url\("?(https?://[^")]+)', page):
                with self.subTest(lang=lang, url=url):
                    self.assertTrue(url.startswith(allowed), url)
            self.assertNotRegex(page, r"googleapis|gstatic|<script[^>]+src=|<link[^>]+stylesheet")

    def test_fonts_licences_and_icon_are_published(self):
        for name in build_site.FONTS:
            self.assertTrue((self.out / "fonts" / name).is_file(), name)
        self.assertTrue((self.out / "noh-icon.svg").is_file())

    def test_no_local_path_is_published(self):
        for path in self.out.rglob("*"):
            if path.suffix in (".html", ".svg", ".txt"):
                text = path.read_text(encoding="utf-8")
                # The only path shown is the MCP example's placeholder, C:\\Users\\<you>.
                self.assertNotRegex(text, r"[A-Za-z]:\\+Users\\+(?!\\*&lt;you&gt;)", str(path))

    def test_languages_link_to_each_other(self):
        self.assertIn('href="fr/" hreflang="fr"', self.pages["en"])
        self.assertIn('href="zh/" hreflang="zh-Hans"', self.pages["en"])
        self.assertIn('if (others.indexOf(saved) >= 0)', self.pages["en"])
        self.assertNotIn("location.replace", self.pages["fr"])
        self.assertIn('href="../" hreflang="en"', self.pages["fr"])
        self.assertIn('href="../de/" hreflang="de"', self.pages["ja"])
        self.assertIn('hreflang="x-default" href="%s"' % build_site.SITE_URL, self.pages["zh"])


if __name__ == "__main__":
    unittest.main()
