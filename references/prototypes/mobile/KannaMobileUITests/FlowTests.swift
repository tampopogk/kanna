import XCTest

final class FlowTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
    }

    private func shot(_ name: String) {
        let a = XCTAttachment(screenshot: app.screenshot())
        a.name = name
        a.lifetime = .keepAlways
        add(a)
    }

    private func see(_ text: String, _ wait: TimeInterval = 15) {
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", text)).firstMatch.waitForExistence(timeout: wait), "missing: \(text)")
    }

    private func tap(_ text: String) {
        let b = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", text)).firstMatch
        XCTAssertTrue(b.waitForExistence(timeout: 15), "missing button: \(text)")
        b.tap()
    }

    private func say(_ text: String) {
        let field = app.textFields["composer"].exists ? app.textFields["composer"] : app.textViews["composer"]
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        field.typeText(text)
        shot("typing")
        app.buttons["send"].tap()
    }

    private func tabBar(_ name: String) { app.tabBars.buttons[name].tap() }

    func testPairingThenChat() {
        app.launchArguments = ["-paired", "NO"]
        app.launch()
        app.buttons["scan"].tap()
        see("Point at the code")
        shot("scan")
        see("Confirm on studio")
        shot("confirm")
        see("Paired with studio")
        shot("paired")
        tap("kanna")
        see("Planner · idle")
        say("is anything on the shelf out of date")
        see("Planner · working")
        shot("working")
        see("Schedule history page", 12)
        see("Planner · asks you", 12)
        shot("question")
        tap("Cut it down")
        see("You chose")
        shot("answered")
        see("Schedule export", 12)
        see("Planner · idle", 12)
        tabBar("Shelf")
        see("Schedule export")
        shot("shelf")
    }

    func testPickThroughAFullDesk() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        tabBar("Shelf")
        tap("Native diff viewer")
        app.buttons["pick"].tap()
        tap("mbp")
        see("mbp's Desk is full")
        shot("full")
        tap("Restore task tabs")
        see("On mbp's Desk", 8)
        see("Working")
        shot("picked")
    }

    func testQuizRoundWithAMiss() {
        app.launchArguments = ["-paired", "YES", "-score", "both", "-slowWriter", "YES"]
        app.launch()
        tabBar("Quiz")
        app.buttons["next-round"].tap()
        see("1 of 5")
        tap("B")
        see("Right.")
        app.buttons["next-question"].tap()
        tap("With --no-daemon")
        app.buttons["next-question"].tap()
        tap("In the kanna CLI")
        see("Not quite.")
        shot("miss")
        XCTAssertTrue(app.buttons["code-card"].waitForExistence(timeout: 3))
        app.buttons["code-card"].tap()
        see("on main at ad2317c")
        shot("code-from-quiz")
        app.buttons["close-code"].tap()
        app.buttons["next-question"].tap()
        tap("The app")
        app.buttons["next-question"].tap()
        tap("526")
        app.buttons["next-question"].tap()
        see("4 of 5")
        see("7 days")
        shot("end")
        app.buttons["round-done"].tap()
        see("Writing your next round")
        shot("writing")
    }

    func testStatsScrub() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        tabBar("Stats")
        let start = app.staticTexts["72.4%"].firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        let from = start.coordinate(withNormalizedOffset: CGVector(dx: 1, dy: 4))
        let to = start.coordinate(withNormalizedOffset: CGVector(dx: 4, dy: 4))
        let held = expectation(description: "shot while pressed")
        var image: XCUIScreenshot?
        DispatchQueue.global().asyncAfter(deadline: .now() + 3) {
            image = XCUIScreen.main.screenshot()
            held.fulfill()
        }
        from.press(forDuration: 0.5, thenDragTo: to, withVelocity: .slow, thenHoldForDuration: 4)
        wait(for: [held], timeout: 10)
        let a = XCTAttachment(screenshot: image!)
        a.name = "scrub-held"
        a.lifetime = .keepAlways
        add(a)
    }

    func testOfflineQueueFlushes() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        app.buttons["repo-switcher"].tap()
        app.buttons["prototype-controls"].tap()
        tap("Path")
        tap("Offline: machine asleep")
        app.buttons["Done"].tap()
        see("Can't reach studio")
        say("what landed overnight")
        see("Sends when studio is back")
        shot("offline-queued")
        app.buttons["repo-switcher"].tap()
        app.buttons["prototype-controls"].tap()
        tap("Path")
        tap("Direct · LAN")
        app.buttons["Done"].tap()
        see("Three merges on main", 12)
        shot("flushed")
    }
}
