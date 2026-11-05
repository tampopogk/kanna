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

    func testPairingThenChat() {
        app.launchArguments = ["-fixture", "YES", "-paired", "NO"]
        app.launch()
        app.buttons["scan"].tap()
        see("Point at the code")
        shot("scan")
        see("Confirm on the Mac")
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
    }

    func testNoPlannerStartsOne() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES"]
        app.launch()
        app.buttons["repo-switcher"].tap()
        tap("kmux")
        see("Planner · not running")
        say("what landed overnight")
        see("Waiting for the planner")
        see("so one is starting on studio")
        see("Planner · starting on studio")
        shot("starting")
        see("Three merges on main", 30)
        see("Read")
    }

    func testBusyPlannerHoldsAMessage() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-planner", "busy", "-repo", "kanna"]
        app.launch()
        see("Working on something else")
        say("explain codex")
        see("Waiting for its turn to end")
        shot("queued")
        see("Kanna adds", 25)
        XCTAssertTrue(app.buttons["code-card"].waitForExistence(timeout: 8))
        app.buttons["code-card"].tap()
        see("at 270bf73")
        app.buttons["close-code"].tap()
    }
}
