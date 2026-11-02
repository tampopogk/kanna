import AppKit
import Testing
@testable import KannaRun

@Suite struct RunGraphTests {
    let nodes = [
        RunNode(id: "check", kind: "run", who: "check.sh", col: 0, row: 0),
        RunNode(id: "review", kind: "agent", who: "codex", col: 1, row: 0),
        RunNode(id: "e2e", kind: "run", who: "e2e", col: 2, row: 0),
        RunNode(id: "done", kind: "done", who: "", col: 3, row: 0),
        RunNode(id: "fix", kind: "agent", who: "claude", col: 1, row: 1),
    ]
    let edges = [
        RunEdge(from: "check", to: "review", route: "next"),
        RunEdge(from: "review", to: "e2e", route: "next"),
        RunEdge(from: "e2e", to: "done", route: "next"),
        RunEdge(from: "check", to: "fix", route: "down", label: "fail · max 3", fail: true, lane: 0, lanes: 3),
        RunEdge(from: "review", to: "fix", route: "down", label: "fail · max 3", fail: true, lane: 1, lanes: 3),
        RunEdge(from: "e2e", to: "fix", route: "down", label: "fail · max 2", fail: true, lane: 2, lanes: 3),
        RunEdge(from: "fix", to: "check", route: "up"),
    ]

    @Test func columnsLineUpAndAStageOffTheLineSitsUnderItsColumn() {
        let graph = RunGraph.lay(nodes, edges) { _ in 72 }
        let check = graph.node("check")!.frame, review = graph.node("review")!.frame, fix = graph.node("fix")!.frame
        #expect(review.minX == check.maxX + RunGraph.gap)
        #expect(fix.minX == review.minX)
        #expect(fix.minY == review.maxY + RunGraph.rowGap)
        #expect(graph.size.width == graph.node("done")!.frame.maxX + 8)
    }

    @Test func failArrowsIntoOneStageFanOutAndTheOneAboveItGoesStraightDown() {
        let graph = RunGraph.lay(nodes, edges) { _ in 72 }
        let downs = graph.edges.filter { $0.edge.route == "down" }
        let fix = graph.node("fix")!.frame
        let ends = downs.map { $0.points.last!.x - fix.midX }
        #expect(ends == [-16, 0, 16])
        #expect(downs[1].points.count == 2, "review is right above fix")
        #expect(downs[0].points.count == 4 && downs[2].points.count == 4)
        #expect(downs[0].points[1].y != downs[2].points[1].y, "the two that turn cross at different levels")
        #expect(downs[0].label?.trailing == true && downs[2].label?.trailing == false, "a label sits on the side away from the stage it goes to")
    }

    @Test func aWayBackUpLeavesTheStageFromItsSideAndRisesIntoTheTarget() {
        let graph = RunGraph.lay(nodes, edges) { _ in 72 }
        let up = graph.edges.first { $0.edge.route == "up" }!
        let fix = graph.node("fix")!.frame, check = graph.node("check")!.frame
        #expect(up.points.first == CGPoint(x: fix.minX, y: fix.midY))
        #expect(up.points.last == CGPoint(x: check.midX - 6, y: check.maxY + 3))
    }

    @Test func theBadgeIsTheStagesPlaceAndDoneOnceHandedBack() {
        var notice = RunNotice()
        notice.place = ("review", 5, 7)
        notice.state = "running"
        #expect(notice.badge == "5/7")
        notice.state = "done"
        #expect(notice.badge == "done")
    }

    @Test func numbersNotRecordedYetShowAsADash() {
        #expect(RunFormat.count(nil) == "—")
        #expect(RunFormat.count(2_600_000) == "2.6M")
        #expect(RunFormat.count(71_000) == "71k")
        #expect(RunFormat.clock(4_920) == "1h 22m")
    }
}
