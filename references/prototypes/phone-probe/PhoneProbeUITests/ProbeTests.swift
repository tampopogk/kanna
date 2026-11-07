import XCTest

final class ProbeTests: XCTestCase {
    let app = XCUIApplication()
    let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")

    func allow(for seconds: TimeInterval, until done: () -> Bool) -> Bool {
        let end = Date().addingTimeInterval(seconds)
        while Date() < end {
            if done() { return true }
            for name in ["Allow", "Allow While Using App", "OK"] {
                let button = springboard.alerts.buttons[name]
                if button.exists {
                    print("[probe-ui] tapped \(name)")
                    button.tap()
                }
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        return done()
    }

    func dump(_ title: String) {
        print("[probe-ui] \(title): " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
    }

    func away(_ seconds: TimeInterval, backs: Int) {
        print("[probe-ui] away \(seconds)s")
        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: seconds)
        app.activate()
        let came = allow(for: 25) { self.app.staticTexts["backs \(backs)"].exists }
        dump("back \(backs) \(came)")
    }

    func testChecks() {
        continueAfterFailure = true
        app.launchEnvironment["PROBE_TARGET"] = ProcessInfo.processInfo.environment["PROBE_TARGET"]
        app.launch()
        let finished = allow(for: 60) { self.app.staticTexts["finished 1"].exists }
        dump("first run \(finished)")
        if !finished { return }
        away(10, backs: 1)
        away(10, backs: 2)
        away(90, backs: 3)
        dump("end")
    }

    func testScan() {
        continueAfterFailure = true
        app.launch()
        app.buttons["Scan the code on the Mac"].tap()
        let finished = allow(for: 150) { self.app.staticTexts["finished 1"].exists }
        dump("scan \(finished)")
    }

    func testBrowse() {
        continueAfterFailure = true
        app.launchEnvironment["PROBE_TARGET"] = ProcessInfo.processInfo.environment["PROBE_TARGET"]
        app.launch()
        let finished = allow(for: 120) { self.app.staticTexts["finished 1"].exists }
        dump("browse \(finished)")
    }
}
