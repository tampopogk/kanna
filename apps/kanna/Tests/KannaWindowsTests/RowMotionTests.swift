import Testing
@testable import KannaWindows

struct RowMotionTests {
    @Test func aRowArrivingAnimates() {
        let plan = RowMotion.plan(from: ["a", "b", "c"], to: ["a", "n", "b", "c"])
        #expect(plan.why == .animated)
        #expect(plan.arrived == ["n"])
        #expect(plan.left.isEmpty)
        #expect(plan.moved.isEmpty)
    }

    @Test func aRowLeavingAnimates() {
        let plan = RowMotion.plan(from: ["a", "b", "c"], to: ["a", "c"])
        #expect(plan.why == .animated)
        #expect(plan.left == ["b"])
        #expect(plan.arrived.isEmpty)
        #expect(plan.moved.isEmpty)
    }

    @Test func rowsShiftedByAnotherAreNotMoved() {
        let plan = RowMotion.plan(from: ["a", "b", "c", "d"], to: ["b", "c", "d"])
        #expect(plan.moved.isEmpty)
        #expect(plan.changed == 1)
    }

    @Test func aRowChangingPlaceIsTheOneThatMoved() {
        let plan = RowMotion.plan(from: ["a", "b", "c", "d"], to: ["b", "c", "d", "a"])
        #expect(plan.why == .animated)
        #expect(plan.moved == ["a"])
        #expect(plan.changed == 1)
    }

    @Test func twoRowsSwappingCountOneMove() {
        let plan = RowMotion.plan(from: ["a", "b"], to: ["b", "a"])
        #expect(plan.why == .animated)
        #expect(plan.moved.count == 1)
    }

    @Test func aReversedListMovesAllButOne() {
        let plan = RowMotion.plan(from: ["a", "b", "c", "d", "e"], to: ["e", "d", "c", "b", "a"])
        #expect(plan.moved.count == 4)
        #expect(plan.why == .burst)
    }

    @Test func theStripSwitchingTasksIsALeaveAndAnArrival() {
        let plan = RowMotion.plan(from: ["b", "c"], to: ["a", "c"])
        #expect(plan.why == .animated)
        #expect(plan.arrived == ["a"])
        #expect(plan.left == ["b"])
    }

    @Test func threeChangesAnimateAndFourDont() {
        #expect(RowMotion.plan(from: ["a"], to: ["a", "x", "y", "z"]).why == .animated)
        #expect(RowMotion.plan(from: ["a"], to: ["a", "w", "x", "y", "z"]).why == .burst)
        #expect(RowMotion.plan(from: ["a", "b", "c"], to: ["x", "y"]).why == .burst)
    }

    @Test func twentyTasksMovingInDontAnimate() {
        let before = ["a", "b"]
        let plan = RowMotion.plan(from: before, to: before + (1...20).map { "m\($0)" })
        #expect(plan.why == .burst)
        #expect(!plan.animated)
        #expect(plan.arrived.count == 20)
    }

    @Test func theFirstShowingDoesntAnimate() {
        #expect(RowMotion.plan(from: nil, to: ["a", "b"]).why == .first)
        #expect(RowMotion.plan(from: nil, to: []).why == .first)
    }

    @Test func anEmptyListGettingItsFirstRowAnimates() {
        #expect(RowMotion.plan(from: [], to: ["a"]).why == .animated)
    }

    @Test func reduceMotionTurnsItOff() {
        let plan = RowMotion.plan(from: ["a"], to: ["a", "b"], still: true)
        #expect(plan.why == .still)
        #expect(!plan.animated)
        #expect(plan.arrived == ["b"])
        #expect(RowMotion.Why.still.rawValue == "reduce-motion")
    }

    @Test func aListOffScreenDoesntAnimate() {
        #expect(RowMotion.plan(from: ["a"], to: ["b"], shown: false).why == .hidden)
    }

    @Test func aSearchDoesntAnimate() {
        #expect(RowMotion.plan(from: ["a", "b", "c"], to: ["b"], quiet: true).why == .quiet)
    }

    @Test func nothingChangingIsUnchangedWhateverElse() {
        #expect(RowMotion.plan(from: ["a", "b"], to: ["a", "b"], shown: false, still: true, quiet: true).why == .unchanged)
    }

    @Test func steadyKeepsTheLongestRunInOrder() {
        #expect(RowMotion.steady([0, 1, 2]) == [0, 1, 2])
        #expect(RowMotion.steady([3, 0, 1, 2]) == [1, 2, 3])
        #expect(RowMotion.steady([]) == [])
    }

    @Test func oneSharedTiming() {
        #expect(RowMotion.duration == 0.2)
        #expect(RowMotion.most == 3)
    }
}
