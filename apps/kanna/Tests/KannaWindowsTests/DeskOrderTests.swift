import Testing
@testable import KannaWindows

struct DeskOrderTests {
    struct Task: Equatable {
        let id: String
        let madeAt: Double
        var needsYou = false
    }

    struct Desk {
        var active: String
        var recent: [String]

        var keyed: [String] { recent }

        mutating func press(_ key: Int) {
            guard let task = DeskOrder.task(keyed, key: key) else { return }
            recent.removeAll { $0 == task }
            recent.insert(active, at: 0)
            active = task
        }
    }

    @Test func theDesksTasksShowOldestFirstWithTheActiveOneInItsPlace() {
        let tasks = [Task(id: "b", madeAt: 20), Task(id: "c", madeAt: 30, needsYou: true), Task(id: "a", madeAt: 10)]
        #expect(DeskOrder.byAge(tasks, made: \.madeAt).map(\.id) == ["a", "b", "c"])
    }

    @Test func tiesKeepTheirOrder() {
        let tasks = [Task(id: "c", madeAt: 9), Task(id: "b", madeAt: 5), Task(id: "a", madeAt: 5)]
        #expect(DeskOrder.byAge(tasks, made: \.madeAt).map(\.id) == ["b", "a", "c"])
    }

    @Test func tasksMadeAtTheSameMomentOnDifferentMachinesKeepOneOrderHoweverTheyArrive() {
        let tasks = [Task(id: "c", madeAt: 5), Task(id: "a", madeAt: 5), Task(id: "b", madeAt: 5), Task(id: "z", madeAt: 1)]
        let order = ["z", "a", "b", "c"]
        #expect(DeskOrder.byAge(tasks, made: \.madeAt, tie: \.id).map(\.id) == order)
        #expect(DeskOrder.byAge(tasks.reversed(), made: \.madeAt, tie: \.id).map(\.id) == order)
    }

    @Test func aTieInBothKeepsItsOrder() {
        let tasks = [Task(id: "a", madeAt: 5, needsYou: true), Task(id: "a", madeAt: 5)]
        #expect(DeskOrder.byAge(tasks, made: \.madeAt, tie: \.id) == tasks)
    }

    @Test func commandOneIsTheTaskLeftLastAndCommandTwoTheOneLeftLongestAgo() {
        let desk = Desk(active: "a", recent: ["c", "b"])
        #expect(DeskOrder.task(desk.keyed, key: 1) == "c")
        #expect(DeskOrder.task(desk.keyed, key: 2) == "b")
        #expect(DeskOrder.task(desk.keyed, key: 3) == nil)
        #expect(DeskOrder.keys(desk.keyed) == ["c": "⌘1", "b": "⌘2"])
    }

    @Test func theStripListsTheOtherTasksInKeyOrder() {
        var desk = Desk(active: "a", recent: ["c", "b"])
        #expect(DeskOrder.strip(desk.keyed) == ["c", "b"])
        desk.press(1)
        #expect(DeskOrder.strip(desk.keyed) == ["a", "b"])
    }

    @Test func withNoTaskActiveTheStripListsCommandOneThenCommandTwoThenTheUnkeyedTask() {
        #expect(DeskOrder.strip(["a", "b", "c"]) == ["a", "c", "b"])
        #expect(DeskOrder.strip(["b"]) == ["b"])
        #expect(DeskOrder.strip([String]()) == [])
    }

    @Test func withNoTaskActiveCommandTwoIsTheTaskLeftLongestAgo() {
        #expect(DeskOrder.task(["a", "b", "c"], key: 2) == "c")
        #expect(DeskOrder.keys(["a", "b", "c"]) == ["a": "⌘1", "c": "⌘2"])
    }

    @Test func withOneOtherTaskBothKeysGoToItAndItShowsCommandOne() {
        #expect(DeskOrder.task(["b"], key: 1) == "b")
        #expect(DeskOrder.task(["b"], key: 2) == "b")
        #expect(DeskOrder.keys(["b"]) == ["b": "⌘1"])
    }

    @Test func withNoOtherTaskNeitherKeyDoesAnything() {
        #expect(DeskOrder.task([String](), key: 1) == nil)
        #expect(DeskOrder.task([String](), key: 2) == nil)
        #expect(DeskOrder.keys([]) == [:])
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

    @Test func withTwoTasksCommandTwoGoesBackAndForth() {
        var desk = Desk(active: "a", recent: ["b"])
        var seen: [String] = []
        for _ in 0..<3 {
            desk.press(2)
            seen.append(desk.active)
        }
        #expect(seen == ["b", "a", "b"])
    }
}
