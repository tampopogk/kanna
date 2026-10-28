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

    @Test func anAgentOrTheDaemonStaysBehindTheAppInFront() {
        #expect(Placement.of(.agent, active: false, background: false) == .behindKanna)
        #expect(Placement.of(.agent, active: false, background: true) == .behindKanna)
    }

    @Test func aBackgroundAppKeepsTheUsersWindowsJustBehindTheFrontApp() {
        #expect(Placement.of(.app, active: false, background: true) == .behindApp)
        #expect(Placement.of(.user, active: false, background: true) == .behindApp)
    }

    @Test func onlyWhatStaysBehindKannaLeavesTheRecordAlone() {
        #expect(Placement.front.recorded)
        #expect(Placement.behindApp.recorded)
        #expect(!Placement.behindKanna.recorded)
    }
}
