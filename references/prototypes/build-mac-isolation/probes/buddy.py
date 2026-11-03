#!/usr/bin/env python3
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
MODEL = HERE.parent
sys.path.insert(0, str(MODEL))
sys.path.insert(0, str(MODEL.parent / "xcodebuild-shim"))
import client

TEST = '''import XCTest
final class BuddyTests: XCTestCase {
    func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
    func testDismissSetupAssistant() {
        let buddy = XCUIApplication(bundleIdentifier: "com.apple.SetupAssistant")
        for step in 0..<12 {
            if buddy.state != .runningForeground && buddy.state != .runningBackground { break }
            snap("buddy-\\(step)")
            let offline = buddy.radioButtons.matching(NSPredicate(format: "title == %@", "My computer does not connect to the internet")).firstMatch
            if offline.exists && offline.isHittable { offline.click() }
            var clicked = false
            for name in ["Continue", "Skip", "Not Now", "Don't Use", "Set Up Later", "Get Started", "Done"] {
                for query in [buddy.sheets.buttons, buddy.dialogs.buttons, buddy.buttons] {
                    for button in query.matching(NSPredicate(format: "title == %@ OR label == %@", name, name)).allElementsBoundByIndex
                        where button.exists && button.isEnabled {
                        button.click()
                        clicked = true
                        break
                    }
                    if clicked { break }
                }
                if clicked { break }
            }
            if !clicked { break }
            sleep(3)
        }
        snap("buddy-final")
        print("BUDDY_STATE=\\(buddy.state.rawValue)")
    }
}
'''

work = Path(sys.argv[1]).resolve()
token = Path(sys.argv[2]).read_text().strip()
out = Path(sys.argv[3]).resolve()
root = work / "buddy"
if root.exists():
    shutil.rmtree(root)
subprocess.run([sys.executable, str(MODEL.parent / "xcodebuild-shim/make_fixture.py"), str(root)], check=True, capture_output=True)
(root / "Tests/BuddyTests.swift").write_text(TEST)
subprocess.run(["xcodegen", "generate"], cwd=root, check=True, capture_output=True)
argv = ["xcodebuild", "-project", str(root / "ShimProbe.xcodeproj"), "-scheme", "ShimProbe", "-destination", "platform=macOS",
        "-only-testing:ShimProbeUITests/BuddyTests", "test"]
check = ["/bin/sh", "-c", 'pgrep -f "Setup Assistant.app/Contents/MacOS" >/dev/null && echo BUDDY_RUNNING || echo BUDDY_GONE']
if out.exists():
    shutil.rmtree(out)
out.mkdir(parents=True)
attempts = []
for attempt in range(6):
    job = client.submit(root, check, task="buddy-check", token=token)
    status = client.attach(job, work / "buddy-check", token=token, echo=False)
    state = (work / "buddy-check/stdout.raw").read_text().strip()
    if state == "BUDDY_GONE":
        attempts.append({"check": state})
        break
    job = client.submit(root, argv, ui=True, task="buddy", token=token)
    dest = work / "buddy-result"
    status = client.attach(job, dest, token=token, echo=False)
    raw = (dest / "stdout.raw").read_text(errors="replace")
    clicks = [l.split("Click", 1)[1].strip() for l in raw.splitlines() if " Click " in l]
    for p in dest.rglob("*.png"):
        shutil.copy(p, out / ("%d-%s" % (attempt, p.name)))
    attempts.append({"check": state, "job": job, "vm": status.get("vm"), "build_exit": status.get("build_exit"), "clicks": clicks})
    time.sleep(20)
report = {"ready": attempts[-1].get("check") == "BUDDY_GONE", "attempts": attempts}
(out / "readiness.json").write_text(json.dumps(report, indent=1) + "\n")
print(json.dumps(report, indent=1))
sys.exit(0 if report["ready"] else 1)
