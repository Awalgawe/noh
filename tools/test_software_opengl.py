"""Controls for the software OpenGL test material; no download, no 7z."""
import json
from pathlib import Path
import tempfile
import unittest

import software_opengl


class SoftwareOpenGL(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.archive_bytes = b"synthetic archive"
        self.lock = self.root / "lock.json"
        self.write_lock(software_opengl.hashlib.sha256(self.archive_bytes).hexdigest())

    def tearDown(self):
        self.temporary.cleanup()

    def write_lock(self, digest, **changes):
        mesa = {"name": "mesa.7z",
                "url": "https://github.com/pal1000/mesa-dist-win/releases/download/1/mesa.7z",
                "algorithm": "SHA256", "hash": digest, "folder": "x64",
                "files": ["opengl32.dll", "libgallium_wgl.dll"], **changes}
        self.lock.write_text(json.dumps({"schema": 1, "mesa": mesa}), encoding="utf-8")

    def download(self, url, target):
        Path(target).write_bytes(self.archive_bytes)

    def extract_both(self, archive, destination):
        (Path(destination) / "x64").mkdir(parents=True)
        for name in ("opengl32.dll", "libgallium_wgl.dll"):
            (Path(destination) / "x64" / name).write_bytes(name.encode())

    def test_the_repository_pin_is_valid(self):
        mesa = software_opengl.pin()
        self.assertTrue(mesa["name"].endswith(".7z"))

    def test_another_source_or_file_list_is_refused(self):
        for changes in ({"url": "https://example.com/mesa.7z"}, {"files": ["opengl32.dll"]},
                        {"algorithm": "MD5"}):
            self.write_lock("0" * 64, **changes)
            with self.assertRaisesRegex(ValueError, "Invalid software OpenGL pin"):
                software_opengl.pin(self.lock)

    def test_a_changed_archive_is_refused_before_extraction(self):
        self.write_lock("0" * 64)
        def never(*_):
            self.fail("extracted an unverified archive")
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            software_opengl.acquire(self.root / "cache", self.lock, self.download, never)

    def test_an_archive_without_the_driver_deploys_nothing(self):
        def loader_only(archive, destination):
            (Path(destination) / "x64").mkdir(parents=True)
            (Path(destination) / "x64/opengl32.dll").write_bytes(b"loader")
        with self.assertRaisesRegex(ValueError, "nothing was deployed"):
            software_opengl.acquire(self.root / "cache", self.lock, self.download, loader_only)

    def test_deploy_then_remove_leaves_the_folder_as_it_was(self):
        files = software_opengl.acquire(self.root / "cache", self.lock, self.download, self.extract_both)
        again = software_opengl.acquire(self.root / "cache", self.lock, self.fail, self.fail)
        self.assertEqual(files, again)
        folder = self.root / "application"
        folder.mkdir()
        (folder / "noh.exe").write_bytes(b"application")
        software_opengl.deploy(files, [folder])
        self.assertEqual(sorted(p.name for p in folder.iterdir()),
                         ["libgallium_wgl.dll", "noh.exe", "opengl32.dll"])
        with self.assertRaisesRegex(ValueError, "Refusing to replace"):
            software_opengl.deploy(files, [folder])
        software_opengl.remove(files, [folder])
        self.assertEqual([p.name for p in folder.iterdir()], ["noh.exe"])
        with self.assertRaisesRegex(ValueError, "missing or changed"):
            software_opengl.remove(files, [folder])


if __name__ == "__main__":
    unittest.main()
