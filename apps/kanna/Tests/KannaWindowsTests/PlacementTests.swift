import Testing
@testable import KannaWindows

struct PlacementTests {
    @Test func whileKannaIsActiveEverythingComesToTheFront() {
        for origin in [Origin.app, .user, .agent] {
            #expect(Placement.of(origin, active: true, background: false) == .front)
            #expect(Placement.of(origin, active: true, background: true) == .front)
        }
    }

    @Test func theUsersOwnActionsBringKannaForward() {
        #expect(Placement.of(.app, active: false, background: false) == .front)
        #expect(Placement.of(.user, active: false, background: false) == .front)
    }

    @Test func anAgentOrTheDaemonOrdersNothingWhileKannaIsntActive() {
        #expect(Placement.of(.agent, active: false, background: false) == .waiting)
    }

    @Test func aBackgroundAppKeepsItsWindowsBehind() {
        #expect(Placement.of(.app, active: false, background: true) == .behindApp)
        #expect(Placement.of(.user, active: false, background: true) == .behindApp)
        #expect(Placement.of(.agent, active: false, background: true) == .behindKanna)
    }

    @Test func onlyWhatComesForwardIsRecorded() {
        #expect(Placement.front.recorded)
        #expect(Placement.behindApp.recorded)
        #expect(!Placement.behindKanna.recorded)
        #expect(!Placement.waiting.recorded)
    }
}

struct WindowOrderTests {
    @Test func aRenderWhileTheAppIsInactiveOrdersNoWindow() {
        var order = WindowOrder()
        for window in ["win-1", "win-2", "d1"] {
            #expect(order.place(window, .agent, active: false, background: false) == .waiting)
        }
        #expect(order.waiting == ["win-1", "win-2", "d1"])
    }

    @Test func waitingWindowsComeInWhenKannaIsActiveInTheSavedOrder() {
        var order = WindowOrder()
        for window in ["win-1", "win-2", "d1", "win-9"] { _ = order.place(window, .agent, active: false, background: false) }
        #expect(order.activated(front: ["d1", "win-1"]) == ["d1", "win-1", "win-2", "win-9"])
        #expect(order.waiting.isEmpty)
    }

    @Test func aWindowTheUserAsksForStopsWaiting() {
        var order = WindowOrder()
        _ = order.place("win-1", .agent, active: false, background: false)
        _ = order.place("win-2", .agent, active: false, background: false)
        #expect(order.place("win-1", .user, active: false, background: false) == .front)
        #expect(order.waiting == ["win-2"])
        order.forget("win-2")
        #expect(order.waiting.isEmpty)
    }
}
