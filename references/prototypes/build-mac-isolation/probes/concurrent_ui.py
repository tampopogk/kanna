#!/usr/bin/env python3
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import threading
import time

HERE = Path(__file__).resolve().parent
MODEL = HERE.parent
sys.path.insert(0, str(MODEL))
sys.path.insert(0, str(MODEL.parent / "xcodebuild-shim"))
import client

work = Path(sys.argv[1]).resolve()
tokens = Path(sys.argv[2]).resolve()
evidence = Path(sys.argv[3]).resolve()
tenants = sys.argv[4:] or ["tenant-a", "tenant-b"]
evidence.mkdir(parents=True, exist_ok=True)


def fixture(tenant):
    root = work / tenant
    if root.exists():
        shutil.rmtree(root)
    subprocess.run([sys.executable, str(MODEL.parent / "xcodebuild-shim/make_fixture.py"), str(root)],
                   check=True, capture_output=True)
    marker = "%s %s" % (tenant, secrets.token_hex(4))
    (root / "App/DirtyValue.swift").write_text('let dirtyValue = "dirty %s"\n' % marker)
    tests = root / "Tests/ProbeTests.swift"
    text = tests.read_text().replace('"dirty + untracked"', '"dirty %s"' % marker)
    if os.environ.get("PROBE_SCREEN"):
        text = text.replace('        app.buttons["confirm"].click()\n',
                            '        let screen = XCTAttachment(screenshot: XCUIScreen.main.screenshot())\n'
                            '        screen.name = "screen-before-click"\n'
                            '        screen.lifetime = .keepAlways\n'
                            '        add(screen)\n'
                            '        app.buttons["confirm"].click()\n')
    if os.environ.get("PROBE_ACTIVATE"):
        text = text.replace('        app.buttons["confirm"].click()\n', '        app.activate()\n        app.buttons["confirm"].click()\n')
    tests.write_text(text)
    return root, marker


results = {}


def run(tenant):
    root, marker = fixture(tenant)
    token = (tokens / (tenant + ".token")).read_text().strip()
    argv = ["xcodebuild", "-project", str(root / "ShimProbe.xcodeproj"), "-scheme", "ShimProbe",
            "-destination", "platform=macOS", "-parallel-testing-enabled", "NO", "test"]
    started = time.time()
    job = client.submit(root, argv, ui=True, task="concurrent-" + tenant, token=token)
    dest = work / (tenant + "-result")
    status = client.attach(job, dest, token=token, echo=False)
    ended = time.time()
    execution = json.loads((dest / "execution.json").read_text()) if (dest / "execution.json").exists() else {}
    shots = sorted(str(p.relative_to(dest)) for p in dest.rglob("*") if p.suffix.lower() in (".png", ".jpeg", ".jpg", ".heic"))
    out = evidence / tenant
    if out.exists():
        shutil.rmtree(out)
    out.mkdir()
    for name in shots:
        shutil.copy(dest / name, out / Path(name).name)
    for name in ("status.json", "execution.json", "tests.json", "diagnostics.txt"):
        if (dest / name).exists():
            shutil.copy(dest / name, out / name)
    results[tenant] = {"job": job, "marker": marker, "started": started, "ended": ended,
                       "state": status.get("state"), "build_exit": status.get("build_exit"),
                       "artifacts": status.get("artifacts"), "rejected_total": status.get("rejected_total"),
                       "vm": status.get("vm"), "guest_serial": execution.get("identity", {}).get("serial"),
                       "guest_user": execution.get("identity", {}).get("user"),
                       "console_user": execution.get("identity", {}).get("console_user"),
                       "screenshots": shots,
                       "marker_in_stdout": marker in (dest / "stdout.raw").read_text(errors="replace")
                       if (dest / "stdout.raw").exists() else False}


threads = [threading.Thread(target=run, args=(t,)) for t in tenants]
for t in threads:
    t.start()
for t in threads:
    t.join()
spans = [(r["started"], r["ended"]) for r in results.values()]
overlap = max(0.0, min(e for _, e in spans) - max(s for s, _ in spans)) if len(spans) > 1 else 0.0
report = {"tenants": results, "overlap_seconds": round(overlap, 1),
          "distinct_vms": len({r["vm"] for r in results.values()}),
          "distinct_serials": len({r["guest_serial"] for r in results.values()})}
(evidence / "concurrent-ui.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
print(json.dumps(report, indent=2, sort_keys=True))
