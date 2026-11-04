import Testing
@testable import KannaLog

struct LogLanesTests {
    func row(_ hash: String, _ parents: [String], spine: Bool = false, side: String? = nil, fold: Int = 0) -> LogRow {
        LogRow(hash: hash, parents: parents, subject: hash, spine: spine, side: side, fold: fold)
    }

    func graph(_ rows: [LogRow], tasks: [LogTask] = [], open: Set<String> = []) -> LogGraph {
        LogGraph.make(LogResult(rows: rows, tasks: tasks), open: open)
    }

    func at(_ graph: LogGraph, _ hash: String) -> LaneRow? { graph.rows.first { $0.hash == hash } }

    @Test func mainsLineIsLaneZeroAndABranchTakesTheNextLane() {
        let g = graph([row("t2", ["t1"]), row("m2", ["m1"], spine: true), row("t1", ["m1"]), row("m1", ["m0"], spine: true), row("m0", [], spine: true)])
        #expect(g.rows.filter { $0.hash.hasPrefix("m") }.allSatisfy { $0.lane == 0 && $0.color == 0 })
        #expect(at(g, "t2")?.lane == 1 && at(g, "t1")?.lane == 1)
        #expect(at(g, "t1")?.outgoing == [LaneEdge(lane: 0, color: 0)])
        #expect(at(g, "m1")?.incoming.map(\.lane) == [0])
        #expect(g.lanes == 2)
    }

    @Test func aMergeCurvesOutToItsSecondParentsLane() {
        let g = graph([row("m2", ["m1", "b1"], spine: true), row("b1", ["m0"]), row("m1", ["m0"], spine: true), row("m0", [], spine: true)])
        let merge = at(g, "m2")!
        #expect(merge.outgoing.map(\.lane) == [0, 1])
        #expect(at(g, "b1")?.lane == 1 && at(g, "b1")?.incoming.map(\.lane) == [1])
        #expect(at(g, "m1")?.through.map(\.lane) == [1])
    }

    @Test func anOctopusMergeOpensALaneForEachOtherParent() {
        let g = graph([row("m1", ["m0", "a", "b", "c"], spine: true), row("a", ["m0"]), row("b", ["m0"]), row("c", ["m0"]), row("m0", [], spine: true)])
        #expect(at(g, "m1")?.outgoing.map(\.lane) == [0, 1, 2, 3])
        #expect(Set(g.rows.filter { ["a", "b", "c"].contains($0.hash) }.map(\.color)).count == 3)
        #expect(g.rows.filter { ["a", "b", "c"].contains($0.hash) }.allSatisfy { $0.outgoing.map(\.lane) == [0] })
        #expect(g.lanes == 4)
    }

    @Test func aFreedLaneIsReusedByTheNextTip() {
        let g = graph([
            row("a1", ["m2"]), row("m3", ["m2"], spine: true), row("m2", ["m1"], spine: true),
            row("b1", ["m1"]), row("m1", ["m0"], spine: true), row("m0", [], spine: true),
        ])
        #expect(at(g, "a1")?.lane == 1 && at(g, "b1")?.lane == 1)
        #expect(at(g, "a1")?.color != at(g, "b1")?.color)
    }

    @Test func aMergeOfAFarMainCommitIntoABranchIsAHookNotALane() {
        var rows = [row("t2", ["t1", "m1"])]
        rows += (3...9).reversed().map { row("m\($0)", ["m\($0 - 1)"], spine: true) }
        rows += [row("t1", ["m2"]), row("m2", ["m1"], spine: true), row("m1", ["m0"], spine: true), row("m0", [], spine: true)]
        let g = graph(rows)
        #expect(at(g, "t2")?.outgoing == [LaneEdge(lane: 1, color: at(g, "t2")!.color), LaneEdge(lane: 0, color: 0, hook: true)])
        #expect(g.lanes == 2)
        let near = graph([row("t2", ["t1", "m1"]), row("t1", ["m1"]), row("m1", ["m0"], spine: true), row("m0", [], spine: true)])
        #expect(at(near, "t2")?.outgoing.contains { $0.hook } == false)
    }

    @Test func aFoldedMergeHidesItsSideAndUnfoldingShowsItInItsOwnLane() {
        let rows = [
            row("m2", ["m1", "a4"], spine: true, fold: 4), row("a4", ["a3"], side: "m2"), row("a3", ["a2"], side: "m2"), row("a2", ["a1"], side: "m2"),
            row("a1", ["m1"], side: "m2"), row("m1", ["m0"], spine: true), row("m0", [], spine: true),
        ]
        let folded = graph(rows)
        #expect(folded.rows.map(\.hash) == ["m2", "m1", "m0"])
        #expect(at(folded, "m2")?.folded == true && at(folded, "m2")?.outgoing.map(\.lane) == [0])
        #expect(folded.lanes == 1)
        let open = graph(rows, open: ["m2"])
        #expect(open.rows.count == 7 && at(open, "m2")?.open == true)
        #expect(at(open, "a4")?.lane == 1 && at(open, "a1")?.lane == 1)
    }

    @Test func aTasksLabelSitsOnItsTipOrOnTheMergeItIsFoldedInto() {
        let rows = [
            row("t1", ["m2"]), row("m2", ["m1", "a4"], spine: true, fold: 4), row("a4", ["a3"], side: "m2"), row("a3", ["a2"], side: "m2"),
            row("a2", ["a1"], side: "m2"), row("a1", ["m1"], side: "m2"), row("m1", [], spine: true),
        ]
        let tasks = [LogTask(id: "task-1", name: "Open", place: "desk", branch: "t", tip: "t1", ahead: 1), LogTask(id: "task-2", name: "Landed", place: "done", branch: "a", tip: "a4", merged: true)]
        let folded = graph(rows, tasks: tasks)
        #expect(at(folded, "t1")?.labels.map(\.name) == ["Open"])
        #expect(at(folded, "m2")?.labels.map(\.name) == ["Landed"])
        let open = graph(rows, tasks: tasks, open: ["m2"])
        #expect(at(open, "m2")?.labels.isEmpty == true && at(open, "a4")?.labels.map(\.name) == ["Landed"])
    }

    @Test func aParentOutsideTheLoadedHistoryEndsInAStub() {
        let g = graph([row("m1", ["m0"], spine: true)])
        #expect(at(g, "m1")?.outgoing == [LaneEdge(lane: 0, color: 0, stub: true)])
    }

    @Test func agesReadAsTheMockupWritesThem() {
        #expect(LogAge.text(1000, now: 1030) == "just now")
        #expect(LogAge.text(0, now: 6 * 60) == "6m ago")
        #expect(LogAge.text(0, now: 3 * 3600) == "3h ago")
        #expect(LogAge.text(0, now: 14 * 86400) == "14d ago")
        #expect(LogAge.text(0, now: 150 * 86400) == "5mo ago")
    }
}
