"""Controls for release acceptance; no network, no container and no real release."""
import hashlib
from pathlib import Path
import tempfile
import unittest

import release_acceptance as acceptance


def sha(data):
    return hashlib.sha256(data).hexdigest()


class Acceptance(unittest.TestCase):
    def test_checksums_must_be_plain_lines_with_safe_unique_names(self):
        good = "a" * 64 + "  NOH-1.2.3-linux-x64.zip\n" + "b" * 64 + "  linux-x64-DELIVERY.json\n"
        self.assertEqual(set(acceptance.parse_checksums(good)), {"NOH-1.2.3-linux-x64.zip", "linux-x64-DELIVERY.json"})
        for bad in ("", "a" * 64 + " one-space.zip\n", "a" * 63 + "  short.zip\n", "a" * 64 + "  ../escape.zip\n",
                    "a" * 64 + "  x.zip\n" + "b" * 64 + "  x.zip\n", "a" * 64 + "  SHA256SUMS\n"):
            with self.assertRaises(ValueError, msg=repr(bad)):
                acceptance.parse_checksums(bad)

    def served(self, files):
        def fetch(url, target):
            self.assertTrue(url.startswith(acceptance.DOWNLOADS + "v1.2.3/"))
            Path(target).write_bytes(files[url.rsplit("/", 1)[1]])
        return fetch

    def test_downloads_are_checked_against_the_published_checksums(self):
        package = b"package"
        files = {"NOH-1.2.3-linux-x64.zip": package,
                 "SHA256SUMS": (sha(package) + "  NOH-1.2.3-linux-x64.zip\n").encode()}
        with tempfile.TemporaryDirectory() as temporary:
            result = acceptance.download_release_files("v1.2.3", ["NOH-1.2.3-linux-x64.zip"],
                                                       Path(temporary) / "d", self.served(files))
            self.assertEqual(result, {"SHA256SUMS": sha(files["SHA256SUMS"]), "NOH-1.2.3-linux-x64.zip": sha(package)})

    def test_a_changed_unlisted_or_prerelease_tagged_download_is_refused(self):
        package = b"package"
        sums = (sha(package) + "  NOH-1.2.3-linux-x64.zip\n").encode()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, "differs from SHA256SUMS"):
                acceptance.download_release_files("v1.2.3", ["NOH-1.2.3-linux-x64.zip"], root / "a", self.served(
                    {"NOH-1.2.3-linux-x64.zip": b"replaced", "SHA256SUMS": sums}))
            with self.assertRaisesRegex(ValueError, "do not list"):
                acceptance.download_release_files("v1.2.3", ["other.zip"], root / "b", self.served(
                    {"other.zip": b"x", "SHA256SUMS": sums}))
            with self.assertRaisesRegex(ValueError, "final version tag"):
                acceptance.download_release_files("v1.2.3-pre.1", [], root / "c", self.served({}))
            with self.assertRaisesRegex(ValueError, "final version tag"):
                acceptance.download_release_files("v1.2.3;rm", [], root / "d", self.served({}), rehearsal=True)
        self.assertEqual(acceptance.REHEARSAL_TAG.fullmatch("v1.2.3-pre.1").group(1), "1.2.3")

    def test_installers_are_laid_out_only_when_they_are_the_tested_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            downloads = root / "downloads"
            downloads.mkdir()
            artifacts = {}
            for profile in ("web", "minimal", "standard", "complete"):
                name = f"NOH-1.2.3-windows-x64-{profile.title()}-Setup.exe"
                (downloads / name).write_bytes(profile.encode())
                artifacts[profile] = {"installer": {"name": name, "sha256": sha(profile.encode()), "size": len(profile)}}
            records = acceptance.installer_layout({"artifacts": artifacts}, downloads, root / "tests")
            self.assertIs(records, artifacts)
            web = root / "tests/output/web"
            self.assertEqual([p.name for p in web.iterdir()], ["NOH-1.2.3-windows-x64-Web-Setup.exe"])
            artifacts["standard"]["installer"]["sha256"] = "0" * 64
            with self.assertRaisesRegex(ValueError, "not the one the installer tests recorded"):
                acceptance.installer_layout({"artifacts": artifacts}, downloads, root / "other")

    def test_only_release_downloads_of_this_repository_are_fetched(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(ValueError, "Unexpected download address"):
                acceptance.fetch("https://example.com/NOH.zip", Path(temporary) / "x")


if __name__ == "__main__":
    unittest.main()
