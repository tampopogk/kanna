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

    private func tabBar(_ name: String) {
        app.tabBars.buttons[name].tap()
    }

    private func shelf() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna"]
        app.launch()
        tabBar("Shelf")
    }

    func testShelfListsTheRepoSTasks() {
        shelf()
        see("studio's Desk")
        see("2 of 3")
        see("Review build output")
        see("needs you")
        app.swipeUp()
        see("blocked by Restore task tabs")
        see("not started")
        see("5 done lately")
        shot("shelf")
    }

    func testPickThroughAFullDesk() {
        shelf()
        app.swipeUp()
        tap("Native diff viewer")
        app.buttons["pick"].tap()
        tap("mbp")
        see("mbp's Desk is full")
        shot("full")
        tap("Restore task tabs")
        see("On mbp's Desk", 8)
        see("shows when it next opens")
        see("Working")
        shot("picked")
    }

    func testPickShowsOnlyForShelfTasks() {
        shelf()
        tap("Keep terminal output")
        see("On studio's Desk")
        XCTAssertFalse(app.buttons["pick"].exists)
        see("Merged, and the test passes")
    }

    func testAnsweringATaskStartsIt() {
        shelf()
        tap("Review build output")
        see("It said,")
        say("Drop them and restyle the crash page")
        see("Got it. Carrying on", 12)
        see("Working")
        XCTAssertFalse(app.descendants(matching: .any)["last-words"].exists)
        shot("answered-task")
    }

    func testAMessageToATaskThatHasnTStartedGoesWithItsFirstMessage() {
        shelf()
        app.swipeUp()
        tap("Schedule history page")
        say("Add an export too")
        see("Waits until it starts")
        see("First message")
        shot("waits")
        app.buttons["pick"].tap()
        tap("studio")
        see("Read", 8)
        see("Working")
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
