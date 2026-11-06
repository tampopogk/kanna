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
        XCTAssertTrue(seen("First message").waitForExistence(timeout: 10), app.staticTexts.allElementsBoundByIndex.map(\.label).joined(separator: " | "))
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
