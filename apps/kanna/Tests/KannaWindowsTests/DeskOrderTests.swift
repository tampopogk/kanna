import Testing
@testable import KannaWindows

struct DeskOrderTests {
    struct Task: Equatable {
        let id: String
        let madeAt: Double?
        var needsYou = false
    }

    struct Desk {
        var active: String
        var recent: [String]
        var needs: Set<String> = []

        var keyed: [String] { DeskOrder.byRecency(recent) { needs.contains($0) } }

        mutating func press(_ key: Int) {
            let task = keyed[key - 1]
            recent.removeAll { $0 == task }
            recent.insert(active, at: 0)
            active = task
        }
    }

    @Test func theDesksTasksShowOldestFirstWithTheActiveOneInItsPlace() {
        let tasks = [Task(id: "b", madeAt: 20), Task(id: "c", madeAt: 30, needsYou: true), Task(id: "a", madeAt: 10)]
        #expect(DeskOrder.byAge(tasks, made: \.madeAt).map(\.id) == ["a", "b", "c"])
    }

    @Test func aTaskWithNoBirthdayGoesLastAndTiesKeepTheirOrder() {
        let tasks = [Task(id: "x", madeAt: nil), Task(id: "b", madeAt: 5), Task(id: "a", madeAt: 5), Task(id: "y", madeAt: nil)]
        #expect(DeskOrder.byAge(tasks, made: \.madeAt).map(\.id) == ["b", "a", "x", "y"])
    }

    @Test func keysNumberTheOthersLeftMostRecentlyFirstWithOnesThatNeedYouAhead() {
        let desk = Desk(active: "a", recent: ["c", "b", "d"], needs: ["d"])
        #expect(desk.keyed == ["d", "c", "b"])
        #expect(DeskOrder.keys(desk.keyed) == ["d": "⌘1", "c": "⌘2", "b": "⌘3"])
    }

    @Test func onlyThreeKeys() {
        #expect(DeskOrder.keys(["a", "b", "c", "d"]).count == 3)
    }

    @Test func commandOneTwiceGoesBackToTheFirstTask() {
        var desk = Desk(active: "a", recent: ["b", "c"])
        desk.press(1)
        #expect(desk.active == "b")
        desk.press(1)
        #expect(desk.active == "a")
    }

    @Test func commandTwoThreeTimesVisitsEveryTask() {
        var desk = Desk(active: "a", recent: ["b", "c"])
        var seen: [String] = []
        for _ in 0..<3 {
            desk.press(2)
            seen.append(desk.active)
        }
        #expect(seen == ["c", "b", "a"])
    }
}
