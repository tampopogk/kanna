import Testing
@testable import KannaWindows

struct ScheduleOrderTests {
    private func order(_ repos: [String?], here: String?) -> [String?] {
        ScheduleOrder.groups(repos.indices.map { $0 }, repo: { repos[$0] }, here: here).map(\.repo)
    }

    @Test func theDesksRepoComesFirstThenByNameThenNoRepo() {
        #expect(order([nil, "/w/alpha", "/w/kanna", "/w/Beta"], here: "/w/kanna") == ["/w/kanna", "/w/alpha", "/w/Beta", nil])
    }

    @Test func aDeskWhoseRepoHasNoSchedulesChangesNothing() {
        #expect(order(["/w/b", "/w/a"], here: "/w/kanna") == ["/w/a", "/w/b"])
    }

    @Test func aRepoKeepsItsSchedulesInOrder() {
        let groups = ScheduleOrder.groups(["/a", "/b", "/a"].enumerated().map { $0 }, repo: { $0.element }, here: nil)
        #expect(groups.map { $0.items.map(\.offset) } == [[0, 2], [1]])
    }
}
