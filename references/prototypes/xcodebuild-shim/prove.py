#!/usr/bin/env python3
"""Run in the Linux fixture. Two invocations deliberately break the SSH tunnel."""
import base64
import json
from pathlib import Path
import subprocess
import sys
import time
import uuid

import model

root = Path.cwd()
proof = root / ".kanna/proof"
proof.mkdir(parents=True, exist_ok=True)
build = ["xcodebuild", "-project", str(root / "ShimProbe.xcodeproj"), "-scheme", "ShimProbe",
         "-destination", "platform=macOS", "-parallel-testing-enabled", "NO"]
proof_task = "proof-" + uuid.uuid4().hex[:8]


def submit(argv, task=proof_task, ui=False, app=None):
    payload, snap = model.snapshot(root)
    job = str(uuid.uuid4())
    request = {"identity": [model.os.uname().nodename, model.repository_id(root), task], "root": str(root), "cwd": ".",
               "snapshot_hash": snap["hash"], "argv": argv, "ui": ui, "app": app,
               "developer_dir": "/Applications/Xcode.app/Contents/Developer"}
    body = {"job": job, "request": request, "snapshot": base64.b64encode(payload).decode()}
    model.api("submit", body)
    model.api("submit", body)  # Retry must attach to the same job, never execute again.
    return job


def attach(job, expected):
    destination = proof / job
    with (proof / (job + ".log")).open("wb") as log:
        result = subprocess.run([sys.executable, str(Path(model.__file__)), "attach", job, str(destination)],
                                stdout=log, stderr=log)
    assert result.returncode == expected, (job, result.returncode, expected, (proof / (job + ".log")).read_text()[-2000:])
    status = json.loads((destination / "status.json").read_text())
    assert status["artifacts"] == "ready", status
    return destination, status


if sys.argv[1] == "builds":
    report = {}
    for name in ["cold", "warm"]:
        job = submit(build + ["build"], app="ShimProbe.app")
        dest, status = attach(job, 0)
        raw = (dest / "stdout.raw").read_text()
        assert "SNAPSHOT_INPUTS_OK" in raw
        assert (dest / "app.tgz").stat().st_size > 0
        report[name] = {"job": job, "seconds": status["ended"] - status["started"],
                        "swift_compile_lines": raw.count("\nSwiftCompile ")}
    assert report["cold"]["swift_compile_lines"] > 0, report
    assert report["warm"]["swift_compile_lines"] == 0, report
    source = root / "App/App.swift"
    original = source.read_text()
    try:
        source.write_text(original + '\nlet deliberateFailure: Int = "not an Int"\n')
        job = submit(build + ["build"])
        dest, status = attach(job, 65)
        mapped = (dest / "diagnostics.txt").read_text()
        assert str(source) + ":" in mapped and "error:" in mapped, mapped
        assert "/mirrors/" in (dest / "stdout.raw").read_text()
        report["diagnostic_error"] = {"job": job, "build_exit": status["build_exit"], "mapped": mapped}
    finally:
        source.write_text(original)
    job = submit(build + ["-only-testing:ShimProbeTests", "test"])
    dest, status = attach(job, 0)
    summary = json.loads((dest / "tests.json").read_text())
    assert summary["passedTests"] == 1, summary
    report["swift_test"] = {"job": job, "summary": {k: summary[k] for k in ["result", "passedTests", "failedTests", "totalTestCount"]}}
    model.save(proof / "builds.json", report)
    print(json.dumps(report, indent=2))
elif sys.argv[1] == "disconnect":
    (root / "snapshot-marker").write_text("captured")
    job = submit(["/usr/bin/python3", "-c",
        'import pathlib,time,sys; time.sleep(8); print(pathlib.Path("snapshot-marker").read_text()); print(sys.argv[1]); sys.exit(7)',
        'literal spaces; $(never-execute) "quotes"'])
    (root / "snapshot-marker").write_text("edited after submission")
    model.save(proof / "disconnect.json", {"job": job})
    print("Submitted " + job + "; end this SSH connection, then run reconnect.")
elif sys.argv[1] == "reconnect":
    job = json.loads((proof / "disconnect.json").read_text())["job"]
    dest, status = attach(job, 7)
    raw = (dest / "stdout.raw").read_text()
    assert 'captured\nliteral spaces; $(never-execute) "quotes"\n' == raw, raw
    assert (root / "snapshot-marker").read_text() == "edited after submission"
    report = {"disconnected_job": job, "build_exit": status["build_exit"], "raw": raw}
    job = submit(["/usr/bin/python3", "-c", "import time; time.sleep(60)"])
    deadline = time.monotonic() + 30
    while model.api("status", {"job": job})["state"] != "running":
        assert time.monotonic() < deadline
        time.sleep(.2)
    model.api("cancel", {"job": job})
    dest, status = attach(job, 143)
    report["cancel"] = {"job": job, "state": status["state"], "build_exit": status["build_exit"]}
    # Two mirrors admitted together, serialized by the one heavy-build slot.
    jobs = [submit(["/bin/echo", "task " + name], task=name) for name in ["one", "two"]]
    for name, job in zip(["one", "two"], jobs):
        dest, status = attach(job, 0)
        assert (dest / "stdout.raw").read_text() == "task " + name + "\n"
    report["two_tasks"] = jobs
    model.save(proof / "lifecycle.json", report)
    print(json.dumps(report, indent=2))
elif sys.argv[1] == "ui":
    expected_user = sys.argv[2] if len(sys.argv) > 2 else "kanna-build"
    job = submit(build + ["-only-testing:ShimProbeUITests", "test"], ui=True)
    dest, status = attach(job, 0)
    execution = json.loads((dest / "execution.json").read_text())
    assert execution["identity"]["user"] == expected_user, execution["identity"]
    assert execution["identity"]["uid"] == execution["identity"]["console_uid"]
    assert execution["identity"]["launchd_domain"] == "Aqua"
    summary = json.loads((dest / "tests.json").read_text())
    assert summary["passedTests"] == 1, summary
    assert list((dest / "attachments").rglob("*.png")), "screenshot missing"
    model.save(proof / "ui.json", {"job": job, "expected_user": expected_user, "identity": execution["identity"],
               "summary": {k: summary[k] for k in ["result", "passedTests", "failedTests", "totalTestCount"]}})
    print((proof / "ui.json").read_text())
