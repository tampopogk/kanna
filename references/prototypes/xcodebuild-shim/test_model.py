import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import model


class ContractTests(unittest.TestCase):
    def test_snapshot_captures_dirty_untracked_deleted_and_unpushed_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            root.mkdir()
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            (root / "modified").write_text("before")
            (root / "deleted").touch()
            subprocess.run(["git", "-C", str(root), "add", "."], check=True)
            subprocess.run(["git", "-C", str(root), "-c", "user.name=Model", "-c",
                            "user.email=model@example.invalid", "commit", "-qm", "base"], check=True)
            (root / "modified").write_text("after")
            (root / "deleted").unlink()
            (root / "untracked").write_text("new")
            (root / "untracked").chmod(0o755)
            (root / "link").symlink_to("untracked")
            payload, meta = model.snapshot(root)
            (root / "modified").write_text("too late")
            dest = Path(tmp) / "unpacked"
            model.unpack(payload, dest)
            self.assertEqual((dest / "src/modified").read_text(), "after")
            self.assertFalse((dest / "src/deleted").exists())
            self.assertEqual((dest / "src/link").readlink(), Path("untracked"))
            self.assertEqual((dest / "src/untracked").stat().st_mode & 0o777, 0o755)
            clone = Path(tmp) / "clone"
            subprocess.run(["git", "clone", "-q", str(dest / "source.bundle"), str(clone)], check=True)
            self.assertEqual(model.run(["git", "-C", str(clone), "rev-parse", "HEAD"]), meta["commit"])

    def test_paths_spaces_aliases_and_raw_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            argv = ["xcodebuild", "-project", "/work/a b/Project.xcodeproj", "build"]
            mapped, paths = model.translate(argv, "/work/a b", Path("/private/tmp/src"),
                                             Path(tmp) / "results", Path(tmp) / "cache", ".")
            self.assertEqual(mapped[2], "/private/tmp/src/Project.xcodeproj")
            raw = b'/tmp/src/App.swift:4:2: error: example\n{"path":"/tmp/src/App.swift"}\n'
            self.assertEqual(model.diagnostics(raw, paths), "/work/a b/App.swift:4:2: error: example\n")
            self.assertIn(b'{"path":"/tmp/src/App.swift"}', raw)
            with self.assertRaises(ValueError):
                model.translate(["xcodebuild", "-project", "/elsewhere/P.xcodeproj"],
                                "/work/a b", Path(tmp), Path(tmp), Path(tmp), ".")


if __name__ == "__main__":
    unittest.main()
