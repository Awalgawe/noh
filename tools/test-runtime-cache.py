"""Synthetic policy controls; no downloads, cache mutation or application build."""
import tempfile
import sys
import copy
import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import runtime_cache as cache

spec = importlib.util.spec_from_file_location("cache_measure", Path(__file__).with_name("measure-runtime-cache.py"))
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class CacheKeys(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for name in cache.LOCKS + cache.PRODUCERS:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name + "\n", encoding="utf-8")
        self.fingerprint = cache.fingerprint(self.root, cache.LOCKS)

    def test_producer_edit_preserves_validated_raw_inputs(self):
        with patch.object(cache, "LEGACY_INPUTS", self.fingerprint):
            before = cache.resolve(self.root)
            (self.root / cache.PRODUCERS[1]).write_text("different extraction code")
            after = cache.resolve(self.root)
        self.assertEqual(before["primary"], after["primary"])
        self.assertEqual(after["legacy"], cache.LEGACY_KEY)
        self.assertNotEqual(before["implementation_key"], after["implementation_key"])

    def test_every_locked_input_change_invalidates_both_identity_and_fallback(self):
        with patch.object(cache, "LEGACY_INPUTS", self.fingerprint):
            before = cache.resolve(self.root)
            for name in cache.LOCKS:
                with self.subTest(name=name):
                    path = self.root / name
                    original = path.read_bytes()
                    path.write_bytes(original + b"changed locked input\n")
                    try:
                        after = cache.resolve(self.root)
                        self.assertNotEqual(before["primary"], after["primary"])
                        self.assertEqual(after["legacy"], "")
                    finally:
                        path.write_bytes(original)

    def test_missing_lock_stops_before_restoration(self):
        (self.root / cache.LOCKS[0]).unlink()
        with self.assertRaises(FileNotFoundError):
            cache.resolve(self.root)


class PromotionPrescreen(unittest.TestCase):
    def setUp(self):
        self.baseline = {key: "same" for key in ("python", "platform", "keys", "commit", "source", "harness_sha256", "scope")}
        self.baseline.update(arm="baseline", success=True, seconds=400,
                             test_inventory={"ffmpeg.exe": "checked"}, full_inventory={"runtime": "checked"},
                             requests=[{"phase": "test-runtime", "name": "archive", "sha256": "checked", "bytes": 100, "cached": False}])
        self.candidate = copy.deepcopy(self.baseline)
        self.candidate.update(arm="candidate", seconds=50)
        self.candidate["requests"][0]["cached"] = True

    def test_success_is_only_provisional_before_actual_promotion_cost(self):
        value = measure.compare(self.baseline, self.candidate)
        self.assertTrue(value["promotion_eligible"])
        self.assertTrue(value["final_decision_pending"])

    def test_unequal_output_or_producer_is_refused(self):
        for key in ("test_inventory", "full_inventory", "source"):
            with self.subTest(key=key):
                changed = copy.deepcopy(self.candidate)
                changed[key] = {"different": "bytes"}
                with self.assertRaises(ValueError):
                    measure.compare(self.baseline, changed)

    def test_missing_cache_contrast_is_refused(self):
        self.candidate["requests"][0]["cached"] = False
        with self.assertRaises(ValueError):
            measure.compare(self.baseline, self.candidate)
        self.candidate["requests"][0]["cached"] = True
        self.baseline["requests"][0]["cached"] = True
        with self.assertRaises(ValueError):
            measure.compare(self.baseline, self.candidate)

    def test_small_gain_does_not_save_a_new_cache(self):
        self.candidate["seconds"] = 300
        self.assertFalse(measure.compare(self.baseline, self.candidate)["promotion_eligible"])


if __name__ == "__main__":
    unittest.main()
