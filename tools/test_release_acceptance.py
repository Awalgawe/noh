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


    def test_the_linux_runners_are_the_matrix_distributions_and_the_effect_burn(self):
        from linux_matrix import cases
        self.assertEqual(acceptance.linux_runners(), [case["id"] for case in cases()] + ["effect-burn"])
        workflow = (Path(acceptance.ROOT) / ".github/workflows/release.yml").read_text(encoding="utf-8")
        job = workflow.split("\n  acceptance-linux:\n", 1)[1].split("\n  acceptance-windows:\n", 1)[0]
        import re
        listed = re.search(r"\n        runner: \[([^\]]+)\]\n", job).group(1)
        self.assertEqual([name.strip() for name in listed.split(",")], acceptance.linux_runners())
        self.assertIn('--runner "$ACCEPTANCE_RUNNER"', job)
        self.assertIn("ACCEPTANCE_RUNNER: ${{ matrix.runner }}", job)
        self.assertIn("name: acceptance-part-linux-x64-${{ matrix.runner }}-${{ github.run_attempt }}", job)
        self.assertIn("fail-fast: false", job)
        # The collecting job still takes every part, Linux runners included.
        self.assertIn("pattern: acceptance-part-*", workflow)

    def test_linux_runner_parts_merge_into_one_platform_part(self):
        merged = acceptance.merge_linux_runners(runner_parts())
        self.assertEqual(merged["platform"], "linux-x64")
        self.assertEqual(merged["status"], "passed")
        self.assertEqual(sorted(merged["checks"]["distribution_matrix"]),
                         sorted(acceptance.linux_runners()[:-1]))
        self.assertEqual(merged["checks"]["caption_effect_burn"], {"effect": "noh.breath", "exit": 0})
        self.assertEqual(merged["runners"], sorted(acceptance.linux_runners()))
        self.assertNotIn("runner", merged)

    def test_a_missing_disagreeing_or_mislabelled_linux_runner_is_refused(self):
        import copy
        parts = runner_parts()
        missing = {name: part for name, part in parts.items() if name != "arch"}
        changed = copy.deepcopy(parts)
        changed["debian-13"]["downloads"] = {"NOH-1.2.3-linux-x64.zip": "f" * 64}
        mislabelled = copy.deepcopy(parts)
        mislabelled["fedora-44"]["checks"] = {"distribution_matrix": {"arch": "passed"}}
        extra_check = copy.deepcopy(parts)
        extra_check["effect-burn"]["checks"]["distribution_matrix"] = {"arch": "passed"}
        unknown = copy.deepcopy(parts) | {"not-a-case": parts["arch"]}
        for name, value in (("missing", missing), ("changed", changed), ("mislabelled", mislabelled),
                            ("extra check", extra_check), ("unknown", unknown)):
            with self.subTest(name), self.assertRaises(ValueError):
                acceptance.merge_linux_runners(value)

    def test_after_a_rerun_the_latest_attempt_of_each_part_counts(self):
        import json
        def write(folder, name, part):
            (folder / name).mkdir()
            (folder / name / "part.json").write_text(json.dumps(part), encoding="utf-8")
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            arch = runner_parts()["arch"]
            write(folder, "acceptance-part-linux-x64-arch-1", arch | {"run_attempt": 1, "source_commit": "1" * 40})
            write(folder, "acceptance-part-linux-x64-arch-2", arch | {"run_attempt": 2})
            write(folder, "acceptance-part-linux-x64-debian-13-1", runner_parts()["debian-13"] | {"run_attempt": 1})
            windows = {"platform": "windows-x64", "status": "passed", "tag": "v1.2.3"}
            write(folder, "acceptance-part-windows-x64-1", windows | {"run_attempt": 1})
            chosen = {(part["platform"], part.get("runner")): part for _, part in acceptance.latest_parts(folder)}
            self.assertEqual(set(chosen), {("linux-x64", "arch"), ("linux-x64", "debian-13"), ("windows-x64", None)})
            self.assertEqual(chosen[("linux-x64", "arch")]["run_attempt"], 2)
            self.assertEqual(chosen[("linux-x64", "arch")]["source_commit"], "c" * 40)
            write(folder, "acceptance-part-linux-x64-arch-copy", arch | {"run_attempt": 2})
            with self.assertRaisesRegex(ValueError, "twice in one attempt"):
                acceptance.latest_parts(folder)

    def test_parts_record_the_attempt_that_wrote_them(self):
        from unittest import mock
        with mock.patch.dict(acceptance.os.environ, {"GITHUB_RUN_ATTEMPT": "3"}):
            self.assertEqual(acceptance.run_attempt(), 3)
        with mock.patch.dict(acceptance.os.environ, {"GITHUB_RUN_ATTEMPT": "0"}), self.assertRaises(ValueError):
            acceptance.run_attempt()
        workflow = (Path(acceptance.ROOT) / ".github/workflows/release.yml").read_text(encoding="utf-8")
        self.assertIn("name: acceptance-part-windows-x64-${{ github.run_attempt }}", workflow)


def runner_parts():
    import copy
    common = {"platform": "linux-x64", "status": "passed", "tag": "v1.2.3", "rehearsal": False,
              "downloads": {"NOH-1.2.3-linux-x64.zip": "a" * 64}, "exercised": ["NOH-1.2.3-linux-x64.zip"],
              "delivery_sha256": "b" * 64, "source_commit": "c" * 40, "limitations": acceptance.LINUX_LIMITATIONS}
    parts = {}
    for runner in acceptance.linux_runners():
        if runner == acceptance.EFFECT_BURN_RUNNER:
            checks = {"caption_effect_burn": {"effect": "noh.breath", "exit": 0}}
        else:
            checks = {"distribution_matrix": {runner: "passed; results under linux-matrix/"}}
        parts[runner] = copy.deepcopy(common) | {"runner": runner, "checks": checks}
    return parts


if __name__ == "__main__":
    unittest.main()
