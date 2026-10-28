import Testing
@testable import KannaWindows

struct KeyWindowTests {
    @Test func appKitsKeyWindowWinsOverAStaleLayoutKey() {
        #expect(KeyWindow.resolve(appKit: "win-57", layout: "d1", windows: ["d1", "win-57"]) == "win-57")
    }

    @Test func theLayoutKeyIsTheFallbackWhenAppKitHasNoKeyWindow() {
        #expect(KeyWindow.resolve(appKit: nil, layout: "win-1", windows: ["d1", "win-1"]) == "win-1")
    }

    @Test func aKeyWindowThatIsntKannasFallsBackToTheLayoutKey() {
        #expect(KeyWindow.resolve(appKit: "alert", layout: "d1", windows: ["d1", "win-1"]) == "d1")
    }

    @Test func withNeitherTheLastWindowIsKey() {
        #expect(KeyWindow.resolve(appKit: nil, layout: "gone", windows: ["d1", "win-1"]) == "win-1")
        #expect(KeyWindow.resolve(appKit: nil, layout: nil, windows: []) == nil)
    }
}
