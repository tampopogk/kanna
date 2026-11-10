import XCTest

final class DeviceTests: XCTestCase {
    private let app = XCUIApplication()
    private let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")

    private func shot(_ name: String) {
        let a = XCTAttachment(screenshot: app.screenshot())
        a.name = name
        a.lifetime = .keepAlways
        add(a)
    }

    private func allow(for seconds: TimeInterval, until done: () -> Bool) -> Bool {
        let end = Date().addingTimeInterval(seconds)
        while Date() < end {
            if done() { return true }
            for name in ["Allow", "Allow While Using App", "OK"] {
                let button = springboard.alerts.buttons[name]
                if button.exists { button.tap() }
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        return done()
    }

    private func seen(_ text: String) -> XCUIElement {
        app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", text)).firstMatch
    }

    private func repo(_ name: String) {
        XCTAssertTrue(app.buttons["repo-switcher"].waitForExistence(timeout: 15))
        XCTAssertTrue(allow(for: 20) { !seen("Can't reach").exists }, "no link")
        app.buttons["repo-switcher"].tap()
        let row = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", name)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 10), "no repo \(name)")
        row.tap()
    }

    func testMachinesAndStats() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let name = environment["KANNA_REPO"], let machine = environment["KANNA_MACHINE"] else {
            throw XCTSkip("a real phone paired with a real daemon, and the machine that has the repo")
        }
        app.launch()
        repo(name)
        app.tabBars.buttons["Stats"].tap()
        XCTAssertTrue(seen("Commits").waitForExistence(timeout: 60), app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("stats")
        app.tabBars.buttons["Machines"].tap()
        XCTAssertTrue(seen("Have \(name)").waitForExistence(timeout: 15))
        XCTAssertTrue(seen(machine).waitForExistence(timeout: 15), app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("machines")
        guard let target = environment["KANNA_MOVE_TO"] else { return }
        app.staticTexts[target].firstMatch.tap()
        let run = app.buttons["run-here"]
        XCTAssertTrue(run.waitForExistence(timeout: 10))
        run.tap()
        app.tabBars.buttons["Chat"].tap()
        XCTAssertTrue(seen("The planner is on \(target) now").waitForExistence(timeout: 30), app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("moved")
    }

    func testQuizRound() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let repo = environment["KANNA_QUIZ_REPO"] else {
            throw XCTSkip("a real phone paired with a real daemon, and a round ready in that repo")
        }
        app.launch()
        XCTAssertTrue(app.buttons["repo-switcher"].waitForExistence(timeout: 15))
        XCTAssertTrue(allow(for: 20) { !seen("Can't reach").exists }, "no link")
        app.buttons["repo-switcher"].tap()
        let row = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", repo)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 10), "no repo \(repo)")
        row.tap()
        app.tabBars.buttons["Quiz"].tap()
        XCTAssertTrue(app.buttons["next-round"].waitForExistence(timeout: 300), "no round: " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("map")
        let started = Date()
        app.buttons["next-round"].tap()
        XCTAssertTrue(seen("1 of 5").waitForExistence(timeout: 3), "a round starts with no wait")
        print("round started in \(Date().timeIntervalSince(started)) s")
        for _ in 0..<5 {
            let first = [app.buttons["drawing-0"], app.buttons.containing(NSPredicate(format: "label BEGINSWITH %@", "1")).firstMatch, app.buttons.containing(NSPredicate(format: "label BEGINSWITH %@", "A")).firstMatch].first { $0.waitForExistence(timeout: 2) }
            XCTAssertNotNil(first, app.buttons.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
            first?.tap()
            XCTAssertTrue(app.buttons["next-question"].waitForExistence(timeout: 5))
            shot("answered")
            app.buttons["next-question"].tap()
        }
        XCTAssertTrue(seen(" of 5").waitForExistence(timeout: 5))
        XCTAssertTrue(seen("The map moved").waitForExistence(timeout: 20), "the map didn't move: " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("end")
    }

    func testShelf() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let task = environment["KANNA_TASK"], let repo = environment["KANNA_REPO"] else {
            throw XCTSkip("a real phone paired with a real daemon, and a task on its Shelf")
        }
        app.launch()
        XCTAssertTrue(app.buttons["repo-switcher"].waitForExistence(timeout: 15))
        XCTAssertTrue(allow(for: 20) { !seen("Can't reach").exists }, "no link")
        app.buttons["repo-switcher"].tap()
        let row = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", repo)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 10), "no repo \(repo)")
        row.tap()
        app.tabBars.buttons["Shelf"].tap()
        let listed = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", task)).firstMatch
        XCTAssertTrue(listed.waitForExistence(timeout: 15), "not on the Shelf: \(task): " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("shelf")
        listed.tap()
        let composer = app.textFields["composer"].exists ? app.textFields["composer"] : app.textViews["composer"]
        XCTAssertTrue(composer.waitForExistence(timeout: 10), app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        Thread.sleep(forTimeInterval: 2)
        shot("task")
        if let say = environment["KANNA_TELL"] {
            let field = app.textFields["composer"].exists ? app.textFields["composer"] : app.textViews["composer"]
            field.tap()
            field.typeText(say)
            app.buttons["send"].tap()
            XCTAssertTrue(seen("Waits until it starts").waitForExistence(timeout: 15) || seen("Read").waitForExistence(timeout: 30))
            shot("told")
        }
        if let desk = environment["KANNA_DESK"] {
            app.buttons["pick"].tap()
            let choice = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", desk)).firstMatch
            if choice.waitForExistence(timeout: 3) { choice.tap() }
            XCTAssertTrue(seen("On \(desk)'s Desk").waitForExistence(timeout: 30), app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
            shot("picked")
        }
    }

    private var chip: String { app.buttons["chip"].exists ? app.buttons["chip"].label : "" }

    private func linked(within seconds: TimeInterval) -> Int? {
        let began = Date()
        while Date().timeIntervalSince(began) < seconds {
            if !chip.isEmpty, !chip.contains("Offline"), app.buttons["chip"].value as? String == "live" { return Int(Date().timeIntervalSince(began) * 1000) }
            Thread.sleep(forTimeInterval: 0.05)
        }
        return nil
    }

    func testPair() throws {
        guard ProcessInfo.processInfo.environment["KANNA_PAIR"] != nil else { throw XCTSkip("a real phone and a Mac showing a code") }
        app.launch()
        XCTAssertTrue(app.buttons["scan"].waitForExistence(timeout: 15))
        _ = allow(for: 4) { false }
        app.buttons["scan"].tap()
        XCTAssertTrue(allow(for: 270) { seen("Your repos").exists }, "never paired: " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("paired")
    }

    func testAway() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let ask = environment["KANNA_AWAY_ASK"], let answer = environment["KANNA_ANSWER"], let repo = environment["KANNA_REPO"] else {
            throw XCTSkip("a paired phone on cellular, with its Wi-Fi off")
        }
        XCTAssertFalse(ask.contains(answer), "the answer has to be something the question doesn't say")
        app.launch()
        let dial = linked(within: 45)
        XCTAssertNotNil(dial, "no link")
        print("kanna-away launch to link: \(dial ?? -1) ms, \(chip)")
        self.repo(repo)
        XCTAssertFalse(chip.contains("LAN"), "away from the LAN, the chip says \(chip)")
        app.buttons["chip"].tap()
        Thread.sleep(forTimeInterval: 1)
        print("kanna-away sheet: " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("chip")
        app.buttons["chip"].tap()
        let field = app.textFields["composer"].exists ? app.textFields["composer"] : app.textViews["composer"]
        XCTAssertTrue(field.waitForExistence(timeout: 10))
        field.tap()
        field.typeText(ask)
        app.buttons["send"].tap()
        let asked = Date()
        XCTAssertTrue(seen(answer).waitForExistence(timeout: 300), "no answer: " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        print("kanna-away answered in \(Int(Date().timeIntervalSince(asked))) s, \(chip)")
        shot("answer")
        for away in [5.0, 15.0, 45.0] {
            XCUIDevice.shared.press(.home)
            Thread.sleep(forTimeInterval: away)
            let left = Date()
            app.activate()
            let waited = Int(Date().timeIntervalSince(left) * 1000)
            print("kanna-away back after \(Int(away)) s: \(waited + (linked(within: 30) ?? -1)) ms, \(chip)")
        }
        let watched = Date()
        var last = chip
        while Date().timeIntervalSince(watched) < 90 {
            if chip != last {
                print("kanna-away chip moved at \(Int(Date().timeIntervalSince(watched))) s: \(last) to \(chip)")
                last = chip
            }
            Thread.sleep(forTimeInterval: 1)
        }
        print("kanna-away chip at the end: \(chip)")
        XCTAssertFalse(chip.contains("LAN"))
        shot("end")
    }

    func testScanAskAnswer() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let ask = environment["KANNA_ASK"], let repo = environment["KANNA_REPO"] else {
            throw XCTSkip("a real phone and a Mac showing a code")
        }
        let answer = environment["KANNA_ANSWER"] ?? ""
        XCTAssertTrue(answer.isEmpty || !ask.contains(answer), "the answer has to be something the question doesn't say")
        app.launch()
        if environment["KANNA_REPAIR"] != nil, app.buttons["repo-switcher"].waitForExistence(timeout: 8) {
            app.buttons["repo-switcher"].tap()
            XCTAssertTrue(app.buttons["pair-again"].waitForExistence(timeout: 5))
            app.buttons["pair-again"].tap()
        }
        if app.buttons["scan"].waitForExistence(timeout: 8) {
            app.buttons["scan"].tap()
            XCTAssertTrue(allow(for: 270) { seen("Your repos").exists }, "never paired: " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
            shot("paired")
            app.buttons.containing(NSPredicate(format: "label CONTAINS %@", repo)).firstMatch.tap()
        }
        let field = app.textFields["composer"].exists ? app.textFields["composer"] : app.textViews["composer"]
        XCTAssertTrue(field.waitForExistence(timeout: 10))
        XCTAssertTrue(allow(for: 20) { !seen("Can't reach").exists })
        if app.buttons["repo-switcher"].waitForExistence(timeout: 5) {
            app.buttons["repo-switcher"].tap()
            let row = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", repo)).firstMatch
            XCTAssertTrue(row.waitForExistence(timeout: 10), "no repo \(repo)")
            row.tap()
            Thread.sleep(forTimeInterval: 1)
        }
        shot("chat")
        field.tap()
        field.typeText(ask)
        shot("typed")
        app.buttons["send"].tap()
        XCTAssertTrue(answer.isEmpty || seen(answer).waitForExistence(timeout: 300), "no answer: " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        XCTAssertTrue(app.buttons["code-card"].waitForExistence(timeout: 300), "no code card: " + app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
        shot("answer")
        app.buttons["code-card"].tap()
        Thread.sleep(forTimeInterval: 1)
        shot("code")
        app.buttons["close-code"].tap()
    }
}
