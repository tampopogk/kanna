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
        XCTAssertTrue(app.descendants(matching: .any)["last-words"].exists, "sending hides nothing")
        shot("answered-task")
    }

    func testATaskAnswersThePhoneWithCodeAndChoices() {
        shelf()
        tap("Review build output")
        see("It said,")
        say("Where is it, show me")
        see("Which do I keep?", 12)
        see("build_view.swift")
        tap("The new one")
        see("You chose")
        XCTAssertFalse(app.staticTexts["Both"].exists)
        shot("task-answers")
    }

    func testEveryReplyStaysInTheThread() {
        shelf()
        tap("Review build output")
        let said = app.descendants(matching: .any).matching(identifier: "last-words")
        func replies(_ count: Int) -> Bool {
            XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate(format: "count == %d", count), object: said)], timeout: 12) == .completed
        }
        see("It said,")
        XCTAssertTrue(replies(1))
        say("First")
        XCTAssertTrue(replies(2))
        say("Second")
        XCTAssertTrue(replies(3), "a reply replaces none before it")
    }

    func testAMessageToATaskThatHasnTStartedGoesWithItsFirstMessage() {
        shelf()
        app.swipeUp()
        tap("Schedule history page")
        say("Add an export too")
        see("Waits until it starts")
        see("First message")
        see("A page with every schedule's firings")
        shot("waits")
        app.buttons["pick"].tap()
        tap("studio")
        see("Read", 8)
        see("Working")
    }

    private func quiz(_ more: [String] = []) {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna"] + more
        app.launch()
        tabBar("Quiz")
        see("What you know")
    }

    private func play(rightOnce first: Bool = true) {
        XCTAssertTrue(app.buttons["next-round"].waitForExistence(timeout: 8))
        app.buttons["next-round"].tap()
        see("1 of 5")
        shot("drawing")
        app.buttons["drawing-1"].tap()
        see("Right.")
        app.buttons["next-question"].tap()
        tap("With `--no-daemon`".replacingOccurrences(of: "`", with: ""))
        app.buttons["next-question"].tap()
    }

    func testQuizRoundWithAMiss() {
        quiz(["-slowWriter", "YES"])
        shot("map")
        play()
        tap("In the kanna CLI")
        see("Not quite.")
        shot("miss")
        XCTAssertTrue(app.buttons["code-card"].waitForExistence(timeout: 3))
        app.buttons["code-card"].tap()
        see("at ad2317c")
        shot("code-from-quiz")
        app.buttons["close-code"].tap()
        app.buttons["next-question"].tap()
        tap("The app")
        app.buttons["next-question"].tap()
        tap("526")
        app.buttons["next-question"].tap()
        see("4 of 5")
        XCTAssertFalse(app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "days")).firstMatch.exists, "no streak")
        see("The map moved")
        see("72% →")
        shot("end")
        app.buttons["round-done"].tap()
        see("Writing your next round")
        shot("writing")
    }

    func testQuizMeLandsInChat() {
        quiz()
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

    func testARoundResumesAndAMissComesBack() {
        quiz()
        XCTAssertTrue(app.buttons["next-round"].waitForExistence(timeout: 8))
        app.buttons["next-round"].tap()
        app.buttons["drawing-1"].tap()
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
        XCTAssertTrue(app.buttons["next-round"].waitForExistence(timeout: 8))
        see("1 miss back")
        app.buttons["next-round"].tap()
        see("A miss, back again")
        shot("miss-back")
    }

    func testARoundPlaysOfflineAndItsAnswersSyncLater() {
        quiz(["-slowWriter", "YES"])
        XCTAssertTrue(app.buttons["next-round"].waitForExistence(timeout: 8))
        app.buttons["chip"].tap()
        app.buttons["fixture-link"].tap()
        see("A round already on the phone still plays")
        shot("offline")
        play()
        tap("In the daemon")
        app.buttons["next-question"].tap()
        tap("The app")
        app.buttons["next-question"].tap()
        tap("526")
        app.buttons["next-question"].tap()
        see("5 of 5")
        see("These answers sync when studio is back")
        XCTAssertFalse(app.staticTexts["The map moved"].exists)
        shot("offline-end")
        app.buttons["round-done"].tap()
        see("Writing your next round")
        see("72%")
        app.buttons["chip"].tap()
        app.buttons["fixture-link"].tap()
        see("80%", 10)
    }

    func testAnEmptyStoreStartsTheWriterAndAFailureSaysSo() {
        quiz(["-emptyQuiz", "YES", "-writerFails", "YES"])
        see("Writing your next round")
        see("Couldn't write your next round", 12)
        shot("writer-failed")
        app.buttons["try-writing"].tap()
        see("Writing your next round")
    }

    private func stats() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna"]
        app.launch()
        tabBar("Stats")
    }

    private func machines() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna"]
        app.launch()
        tabBar("Machines")
    }

    func testStatsDrawsTheReply() {
        stats()
        see("72.4%")
        see("of 85 answers")
        see("3h 33m")
        see("Tasks landed")
        app.swipeUp()
        see("Worst functions")
        see("CC 52")
        see("a chart this phone can't draw")
        see("Nothing here yet.")
        shot("stats")
    }

    func testStatsScrub() {
        stats()
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

    func testStatsStillScrolls() {
        stats()
        let last = app.staticTexts["Erosion"]
        XCTAssertTrue(app.staticTexts["72.4%"].waitForExistence(timeout: 5))
        let before = last.frame.maxY
        app.swipeUp()
        app.swipeUp()
        XCTAssertLessThan(last.frame.maxY, before - 200)
        shot("scrolled")
    }

    func testMachinesListTheRepoSMachines() {
        machines()
        see("Have kanna")
        see("studio")
        see("Planner")
        see("Desk 2 of 3")
        see("dev-1")
        see("Relayed")
        see("tests-1")
        see("last seen")
        see("1 machine without kanna")
        shot("machines")
        tap("1 machine without kanna")
        see("pi")
    }

    func testAMachineSPageShowsWhatSOnIt() {
        machines()
        tap("mbp")
        see("Mac · aarch64 · 12 cores · Kanna 4.2.0")
        see("Path from studio")
        see("Round trip")
        see("Restore task tabs")
        shot("machine")
    }

    func testRunThePlannerHere() {
        machines()
        tap("dev-1")
        tap("Run the planner here")
        tabBar("Chat")
        see("so one is starting on dev-1", 8)
        XCTAssertTrue(app.staticTexts["Relayed"].waitForExistence(timeout: 5))
        shot("moved")
    }

    func testMovePlannerFromItsOwnPage() {
        machines()
        tap("studio")
        tap("Run the planner on another machine")
        see("It won't know this conversation")
        tap("mbp")
        tabBar("Chat")
        see("so one is starting on mbp", 8)
    }

    func testAnEarlierChatStaysAfterAMove() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna"]
        app.launch()
        say("What's on the Shelf?")
        see("Schedule history page is out of date", 8)
        app.staticTexts["What's on the Shelf?"].tap()
        tabBar("Machines")
        tap("mbp")
        tap("Run the planner here")
        tabBar("Chat")
        see("The planner is on mbp now", 8)
        see("Schedule history page is out of date")
        XCTAssertFalse(app.buttons["Cut it down"].exists, "a question for the old planner can't be answered")
        say("And now?")
        see("Schedule history page is out of date", 8)
        shot("two-planners")
    }

    func testTheRepoSwitcherMovesTheLink() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna"]
        app.launch()
        app.buttons["repo-switcher"].tap()
        tap("kmux")
        XCTAssertTrue(app.staticTexts["Relayed"].waitForExistence(timeout: 8))
        tabBar("Machines")
        see("Have kmux")
        see("dev-1")
        app.buttons["repo-switcher"].tap()
        tap("kanna")
        XCTAssertTrue(app.staticTexts["Direct · LAN"].waitForExistence(timeout: 8))
    }

    func testAnAsleepPlannerMachineStillLetsYouMoveIt() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna", "-planner", "away"]
        app.launch()
        XCTAssertTrue(app.staticTexts["Offline"].waitForExistence(timeout: 10))
        tabBar("Machines")
        see("Asking")
        tap("mbp")
        XCTAssertTrue(app.buttons["run-here"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.staticTexts["Offline"].exists)
        tap("Run the planner here")
        XCTAssertTrue(app.staticTexts["Direct · LAN"].waitForExistence(timeout: 10))
        tabBar("Chat")
        see("Planner")
    }

    func testLeavingTheAppWhileItRedialsDoesNotStrandIt() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna", "-slowDial", "YES"]
        app.launch()
        see("Planner · idle", 10)
        app.buttons["chip"].tap()
        app.buttons["fixture-link"].tap()
        XCTAssertTrue(app.staticTexts["Planner · idle"].waitForNonExistence(timeout: 10))
        Thread.sleep(forTimeInterval: 4)
        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: 1)
        app.activate()
        Thread.sleep(forTimeInterval: 2)
        if !app.buttons["fixture-link"].exists { app.buttons["chip"].tap() }
        XCTAssertTrue(app.buttons["fixture-link"].waitForExistence(timeout: 5))
        app.buttons["fixture-link"].tap()
        see("Planner · idle", 15)
    }

    func testThePhoneTriesItsPlannerItselfWhenAnotherMachineSaysItIsDown() {
        app.launchArguments = ["-fixture", "YES", "-paired", "YES", "-repo", "kanna", "-planner", "away", "-plannerWakes", "YES"]
        app.launch()
        XCTAssertTrue(app.staticTexts["Offline"].waitForExistence(timeout: 10))
        see("Planner · idle", 20)
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
        see("so one is starting on dev-1")
        see("Planner · starting on dev-1")
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
