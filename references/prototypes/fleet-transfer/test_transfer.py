#!/usr/bin/env python3
"""Offline tests cover Git/restarts/refusals, never pretend to prove agent memory."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

import transfer as t


class Transfer(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.source, self.dest = self.root / "sender", self.root / "different home/receiver"
        t.init(self.source, "claude", synthetic=True)

    def tearDown(self):
        self.temp.cleanup()

    def step(self, action, root, phase=None):
        cmd = ["python3", str(Path(t.__file__)), action, str(root)]
        if phase:
            cmd += ["--stop-after", phase]
        result = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(result.returncode, 75 if phase else 0, result.stderr)

    def package(self):
        t.seed(self.source, None)
        self.step("pack", self.source, "prepared")
        self.step("pack", self.source)
        self.dest.mkdir(parents=True)
        for name in ("base.bundle", "task.bundle", "transcript.jsonl", "manifest.json"):
            shutil.copyfile(self.source / name, self.dest / name)

    def test_git_and_every_import_checkpoint_survive_new_processes(self):
        self.package()
        state = t.read(self.source / "state.json")
        self.step("pack", self.source)
        self.assertEqual(t.read(self.source / "state.json")["wip"], state["wip"])
        for phase in ("received", "fetched", "restored", "installed"):
            self.step("receive", self.dest, phase)
        self.step("receive", self.dest)
        work = self.dest / "work"
        self.assertEqual(t.git(work, "rev-parse", "HEAD"), state["before"])
        self.assertEqual(t.git(work, "write-tree"), state["tree"])
        self.assertEqual(t.git(work, "diff", "--name-only"), "")
        self.assertEqual((work / "binary.bin").read_bytes(), bytes(range(256)))
        self.assertTrue((work / "executable").stat().st_mode & 0o111)
        self.assertTrue((work / "link").is_symlink())
        self.assertFalse((work / "deleted.txt").exists())
        self.assertFalse((work / "ignored.txt").exists())
        self.assertEqual((work / "tracked.txt").read_text(), "staged plus unstaged\n")
        self.assertTrue(Path(state["transcript"]).exists())
        # Bundle has a prerequisite: receiver really needs its base objects.
        empty = self.root / "empty"
        empty.mkdir()
        t.git(empty, "init")
        with self.assertRaises(subprocess.CalledProcessError):
            t.git(empty, "bundle", "verify", str(self.source / "task.bundle"))
        self.assertEqual(t.read(self.source / "state.json")["phase"], "packed")
        with self.assertRaises(AssertionError):
            t.resume(self.dest, None)

    def test_running_or_changed_session_cannot_be_packed(self):
        with self.assertRaises(AssertionError):
            t.pack(self.source, None)
        t.seed(self.source, None)
        state = t.read(self.source / "state.json")
        Path(state["transcript"]).write_text("changed after stop")
        with self.assertRaises(AssertionError):
            t.pack(self.source, None)

    def test_corrupt_payload_refused_before_import(self):
        self.package()
        with (self.dest / "transcript.jsonl").open("a") as f:
            f.write("corrupt")
        with self.assertRaises(AssertionError):
            t.receive(self.dest, None)
        self.assertFalse((self.dest / "clone").exists())

    def test_wrong_memory_or_receipt_does_not_acknowledge(self):
        self.package()
        state = t.read(self.source / "state.json")
        receipt = dict(transfer_id=state["transfer_id"], session=state["session"], answer_sha256="wrong")
        t.write(self.source / "receipt.json", receipt)
        with self.assertRaises(AssertionError):
            t.acknowledge(self.source, None)
        self.assertEqual(t.read(self.source / "state.json")["phase"], "packed")
        receipt["answer_sha256"] = hashlib.sha256(state["nonce"].encode()).hexdigest()
        t.write(self.source / "receipt.json", receipt)
        self.step("ack", self.source, "done")
        self.step("ack", self.source)
        self.assertTrue((self.source / "work").exists())
        self.assertTrue(Path(state["transcript"]).exists())

    def test_complete_resume_output_recovers_without_another_turn(self):
        self.package()
        t.receive(self.dest, None)
        state = t.read(self.dest / "state.json")
        state.update(synthetic=False, phase="resuming")
        t.write(self.dest / "state.json", state)
        # Fake CLI output tests recovery logic only. The live matrix proves actual memory.
        (self.dest / "resume.jsonl").write_text(json.dumps(dict(type="result", session_id=state["session"],
                                                              result="remembered", is_error=False)) + "\n")
        with Path(state["installed_transcript"]).open("a") as f:
            f.write('{"type":"assistant","message":"remembered"}\n')
        self.step("resume", self.dest, "answered")
        self.step("resume", self.dest)
        self.step("resume", self.dest)
        self.assertEqual(t.read(self.dest / "state.json")["phase"], "done")

    def test_uncertain_resume_waits_instead_of_replaying(self):
        self.package()
        t.receive(self.dest, None)
        state = t.read(self.dest / "state.json")
        state.update(synthetic=False, phase="resuming")
        t.write(self.dest / "state.json", state)
        with self.assertRaises(FileNotFoundError):
            t.resume(self.dest, None)
        self.assertEqual(t.read(self.dest / "state.json")["phase"], "resuming")
        self.assertFalse((self.dest / "receipt.json").exists())


if __name__ == "__main__":
    unittest.main()
