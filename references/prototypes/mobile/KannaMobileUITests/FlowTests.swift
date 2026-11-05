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
        if app.keyboards.element.exists {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.02, dy: 0.28)).tap()
            let gone = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: app.keyboards.element)
            wait(for: [gone], timeout: 4)
        }
        app.tabBars.buttons[name].tap()
    }

    private func onShelf(_ name: String) -> XCUIElement {
        let row = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", name)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 8), "not on the Shelf: \(name)")
        return row
    }

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
        _ = onShelf("Schedule export")
        app.swipeUp()
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

    func testNoPlannerStartsOne() {
        app.launchArguments = ["-paired", "YES", "-speed", "2.5"]
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
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        app.buttons["repo-switcher"].tap()
        app.buttons["prototype-controls"].tap()
        tap("Busy with something else")
        see("Working on something else")
        say("explain codex")
        see("Waiting for its turn to end")
        shot("queued")
        see("Kanna adds", 25)
        XCTAssertTrue(app.buttons["code-card"].waitForExistence(timeout: 8))
        app.buttons["code-card"].tap()
        see("on main at 270bf73")
        app.buttons["close-code"].tap()
    }

    func testQuizMeLandsInChat() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        tabBar("Quiz")
        tap("41 commits")
        app.buttons["quiz-me"].tap()
        see("Writing it now")
        shot("quiz-me")
        see("Round ready: Machines", 10)
        tabBar("Chat")
        XCTAssertTrue(app.buttons["round-card"].waitForExistence(timeout: 5))
        shot("card")
        app.buttons["round-card"].tap()
        see("1 of 5")
    }

    func testMovePlanner() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        tabBar("Machines")
        tap("studio")
        tap("Run the planner on another machine")
        tap("dev-1")
        tabBar("Chat")
        see("now runs on dev-1", 8)
        XCTAssertTrue(app.staticTexts["Relayed"].waitForExistence(timeout: 3))
        shot("moved")
    }

    func testWalkOutOfTheHouse() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        app.buttons["repo-switcher"].tap()
        app.buttons["prototype-controls"].tap()
        tap("Walk out of the house")
        XCTAssertTrue(app.staticTexts["Relayed"].waitForExistence(timeout: 6))
        app.buttons["chip"].tap()
        see("can't read them")
        shot("relay-pop")
        app.tap()
        XCTAssertTrue(app.staticTexts["Direct · internet"].waitForExistence(timeout: 12))
    }

    func testADayOldMessageAsksAgain() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        app.buttons["repo-switcher"].tap()
        app.buttons["prototype-controls"].tap()
        tap("Path")
        tap("Offline: no network")
        app.buttons["Done"].tap()
        see("No network")
        say("what landed overnight")
        app.buttons["repo-switcher"].tap()
        app.buttons["prototype-controls"].tap()
        tap("Age queued messages")
        see("A day old, not sent")
        shot("aged")
        tap("Keep waiting")
        see("Sends when studio is back")
    }

    func testNoScoreByDefault() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        tabBar("Quiz")
        XCTAssertTrue(app.buttons["next-round"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["1,240"].exists)
        XCTAssertFalse(app.staticTexts["6"].exists)
        shot("no-score-home")
        app.buttons["next-round"].tap()
        tap("B")
        see("Right.")
        XCTAssertFalse(app.staticTexts["+20"].exists)
        app.buttons["next-question"].tap()
        tap("With --no-daemon")
        see("Right.")
        XCTAssertFalse(app.staticTexts["2 in a row"].exists)
        shot("no-score-right")
        app.buttons["next-question"].tap()
        tap("In the daemon")
        app.buttons["next-question"].tap()
        tap("The app")
        app.buttons["next-question"].tap()
        tap("526")
        app.buttons["next-question"].tap()
        see("5 of 5")
        XCTAssertFalse(app.staticTexts["7 days"].exists)
        XCTAssertFalse(app.staticTexts["+105"].exists)
        shot("no-score-end")
    }

    func testScoreModes() {
        app.launchArguments = ["-paired", "YES", "-score", "streak"]
        app.launch()
        tabBar("Quiz")
        see("6")
        XCTAssertFalse(app.staticTexts["1,240"].exists)
        shot("streak")
        app.terminate()
        app.launchArguments = ["-paired", "YES", "-score", "points"]
        app.launch()
        tabBar("Quiz")
        XCTAssertTrue(app.staticTexts["1,240"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["6"].exists)
        shot("points")
    }

    func testAnsweringATaskStartsIt() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        tabBar("Shelf")
        tap("Review build output")
        see("It said, 9:01")
        say("Drop them and restyle the crash page")
        see("Got it. Carrying on", 12)
        see("Working")
        XCTAssertFalse(app.staticTexts["It said, 9:01"].exists)
        shot("answered-task")
    }

    func testChatStartsATask() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        say("start native diff viewer")
        see("Started Native diff viewer", 12)
        tabBar("Shelf")
        let row = onShelf("Native diff viewer")
        let started = NSPredicate(format: "label CONTAINS %@", "working")
        expectation(for: started, evaluatedWith: row)
        waitForExpectations(timeout: 8)
        app.swipeUp()
        shot("started")
    }

    func testARoundResumesAndAMissComesBack() {
        app.launchArguments = ["-paired", "YES", "-speed", "0.1"]
        app.launch()
        tabBar("Quiz")
        app.buttons["next-round"].tap()
        tap("B")
        app.buttons["next-question"].tap()
        app.buttons["close-round"].tap()
        tap("Back to your round")
        see("2 of 5")
        tap("With --no-daemon")
        app.buttons["next-question"].tap()
        tap("In the kanna CLI")
        app.buttons["next-question"].tap()
        tap("The app")
        app.buttons["next-question"].tap()
        tap("526")
        app.buttons["next-question"].tap()
        see("4 of 5")
        app.buttons["round-done"].tap()
        XCTAssertTrue(app.buttons["next-round"].waitForExistence(timeout: 6))
        app.buttons["next-round"].tap()
        see("A miss, back again")
        shot("miss-back")
    }

    func testStatsStillScrolls() {
        app.launchArguments = ["-paired", "YES"]
        app.launch()
        tabBar("Stats")
        let last = app.staticTexts["Erosion"]
        XCTAssertTrue(app.staticTexts["72.4%"].waitForExistence(timeout: 5))
        let before = last.frame.maxY
        app.swipeUp()
        app.swipeUp()
        XCTAssertLessThan(last.frame.maxY, before - 200)
        shot("scrolled")
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
