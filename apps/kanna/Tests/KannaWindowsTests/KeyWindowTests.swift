import Testing
@testable import KannaWindows

struct KeyWindowTests {
    @Test func appKitsKeyWindowWinsOverTheAppsRecord() {
        #expect(KeyWindow.resolve(appKit: "win-57", front: ["d1"], windows: ["d1", "win-57"]) == "win-57")
    }

    @Test func withNoAppKitKeyWindowTheWindowLastInFrontIsKey() {
        #expect(KeyWindow.resolve(appKit: nil, front: ["win-1", "d1"], windows: ["d1", "win-1"]) == "win-1")
    }

    @Test func aKeyWindowThatIsntKannasFallsBackToTheRecord() {
        #expect(KeyWindow.resolve(appKit: "alert", front: ["d1"], windows: ["d1", "win-1"]) == "d1")
    }

    @Test func aClosedWindowInTheRecordIsSkipped() {
        #expect(KeyWindow.resolve(appKit: nil, front: ["gone", "d1"], windows: ["d1", "win-1"]) == "d1")
    }

    @Test func withNeitherTheLastWindowIsKey() {
        #expect(KeyWindow.resolve(appKit: nil, front: ["gone"], windows: ["d1", "win-1"]) == "win-1")
        #expect(KeyWindow.resolve(appKit: nil, front: [], windows: []) == nil)
    }
}
