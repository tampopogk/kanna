#!/usr/bin/env python3
"""Generate a tiny macOS app with an actual XCUITest, then dirty its Git tree."""
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
root.mkdir(parents=True, exist_ok=False)
for directory in ["App", "Tests", "UnitTests"]:
    (root / directory).mkdir()
(root / "project.yml").write_text('''name: ShimProbe
options:
  bundleIdPrefix: dev.kanna.model
  deploymentTarget:
    macOS: "14.0"
settings:
  CODE_SIGN_IDENTITY: "-"
  CODE_SIGN_STYLE: Manual
  SWIFT_VERSION: "5.0"
  GENERATE_INFOPLIST_FILE: YES
targets:
  ShimProbe:
    type: application
    platform: macOS
    sources: [App]
    preBuildScripts:
      - name: Check snapshot semantics
        script: |
          test ! -e "$SRCROOT/deleted.txt"
          test -x "$SRCROOT/probe.sh"
          test -L "$SRCROOT/value-link"
          test "$(cat "$SRCROOT/value-link")" = untracked
          git -C "$SRCROOT" rev-parse HEAD
          "$SRCROOT/probe.sh"
        basedOnDependencyAnalysis: false
  ShimProbeTests:
    type: bundle.unit-test
    platform: macOS
    sources: [UnitTests]
  ShimProbeUITests:
    type: bundle.ui-testing
    platform: macOS
    sources: [Tests]
    dependencies:
      - target: ShimProbe
    settings:
      TEST_TARGET_NAME: ShimProbe
schemes:
  ShimProbe:
    build:
      targets:
        ShimProbe: all
    test:
      targets: [ShimProbeTests, ShimProbeUITests]
''')
app = '''import SwiftUI
@main struct ShimProbe: App {
    var body: some Scene {
        WindowGroup {
            VStack {
                Text(MESSAGE).accessibilityIdentifier("snapshot-label")
                Button("Confirm") { }.accessibilityIdentifier("confirm")
            }.padding(40)
        }
    }
}
'''
(root / "App/App.swift").write_text(app.replace("MESSAGE", '"clean baseline"'))
(root / "App/DirtyValue.swift").write_text('let dirtyValue = "dirty + untracked"\n')
(root / "Tests/ProbeTests.swift").write_text('''import XCTest
final class ProbeTests: XCTestCase {
    func testDirtySnapshotInConsole() {
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.staticTexts["dirty + untracked"].waitForExistence(timeout: 15))
        app.buttons["confirm"].click()
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = "dirty-snapshot-console"
        shot.lifetime = .keepAlways
        add(shot)
    }
}
''')
(root / "UnitTests/UnitTests.swift").write_text('''import XCTest
final class UnitTests: XCTestCase {
    func testSwiftRunsOnMac() { XCTAssertEqual(2 + 2, 4) }
}
''')
(root / "deleted.txt").write_text("must disappear from the mirror\n")
(root / "probe.sh").write_text('#!/bin/sh\nprintf "SNAPSHOT_INPUTS_OK\\n"\n')
(root / "probe.sh").chmod(0o755)
(root / ".gitignore").write_text(".kanna/\n.shim-last-job.json\n")
subprocess.run(["xcodegen", "generate"], cwd=root, check=True)
subprocess.run(["git", "init", "-q"], cwd=root, check=True)
subprocess.run(["git", "add", "."], cwd=root, check=True)
# The project refers to this file but it is deliberately absent from the commit.
subprocess.run(["git", "rm", "--cached", "App/DirtyValue.swift"], cwd=root, check=True)
subprocess.run(["git", "-c", "user.name=Shim Model", "-c", "user.email=model@example.invalid",
                "commit", "-qm", "Unpushed fixture baseline"], cwd=root, check=True)
(root / "App/App.swift").write_text('#warning("SHIM_DIRTY_WARNING")\n' + app.replace("MESSAGE", "dirtyValue"))
(root / "deleted.txt").unlink()
(root / "untracked.txt").write_text("untracked\n")
(root / "value-link").symlink_to("untracked.txt")
print(root)
