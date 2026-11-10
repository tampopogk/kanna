import hashlib
import os
from pathlib import Path
import tempfile
import unittest

import intake


def h(data):
    return hashlib.sha256(data).hexdigest()


class IntakeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.outside = self.tmp / "outside.txt"
        self.outside.write_text("canary")
        self.store = intake.Store(self.tmp / "job")

    def tearDown(self):
        self.store.close()

    def test_accepts_nested_file(self):
        self.store.put("attachments/shot.png", b"png", h(b"png"))
        self.assertEqual((self.tmp / "job/attachments/shot.png").read_bytes(), b"png")

    def test_rejects_hostile_paths(self):
        for rel in ["../outside.txt", "/etc/passwd", "a/../../outside.txt", "a//b", "./a", "a\\..\\b",
                    "a\x00b", "", "x" * 300, "é"]:
            with self.subTest(rel=rel), self.assertRaises(intake.Rejected):
                self.store.put(rel, b"x", h(b"x"))
        self.assertEqual(self.outside.read_text(), "canary")

    def test_rejects_case_collision_and_overwrite(self):
        self.store.put("Report.json", b"1", h(b"1"))
        for rel in ["report.json", "Report.json", "Report.json/child"]:
            with self.subTest(rel=rel), self.assertRaises(intake.Rejected):
                self.store.put(rel, b"2", h(b"2"))
        self.assertEqual((self.tmp / "job/Report.json").read_bytes(), b"1")

    def test_never_follows_symlinks(self):
        os.symlink(self.tmp, self.tmp / "job/link")
        with self.assertRaises(intake.Rejected):
            self.store.put("link/outside.txt", b"x", h(b"x"))
        os.symlink(self.outside, self.tmp / "job/file-link")
        with self.assertRaises(intake.Rejected):
            self.store.put("file-link", b"x", h(b"x"))
        self.assertEqual(self.outside.read_text(), "canary")

    def test_checksum_and_size(self):
        with self.assertRaises(intake.Rejected):
            self.store.put("a", b"x", h(b"y"))
        old = intake.MAX_FILE
        intake.MAX_FILE = 4
        try:
            with self.assertRaises(intake.Rejected):
                self.store.put("b", b"12345", h(b"12345"))
        finally:
            intake.MAX_FILE = old


if __name__ == "__main__":
    unittest.main()
