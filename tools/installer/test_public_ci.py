"""Exercise rejection paths without installing software or using the network."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("public_ci", Path(__file__).with_name("public-ci.py"))
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


class InstallerGuards(unittest.TestCase):
    def test_local_host_is_rejected(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "disposable hosted Windows"):
                ci.runner()

    def test_installed_payload_changes_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "noh.exe").write_bytes(b"qualified application")
            expected = ci.delivery.inventory(root)
            (root / "unins000.exe").write_bytes(b"uninstaller")
            ci.verify_installed(root, expected)
            (root / "noh.exe").write_bytes(b"changed application")
            with self.assertRaisesRegex(ValueError, "payload differs"):
                ci.verify_installed(root, expected)

    def test_extra_payload_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "noh.exe").write_bytes(b"qualified application")
            expected = ci.delivery.inventory(root)
            (root / "unins000.exe").write_bytes(b"uninstaller")
            (root / "extra.dll").write_bytes(b"unexpected native dependency")
            with self.assertRaisesRegex(ValueError, "Unexpected installed files"):
                ci.verify_installed(root, expected)


if __name__ == "__main__":
    unittest.main()
