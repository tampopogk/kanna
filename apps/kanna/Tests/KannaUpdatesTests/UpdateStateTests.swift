import KannaUpdates
import XCTest

final class UpdateStateTests: XCTestCase {
    private let one = UpdateOffer(version: "4.2.0", build: "900", speaks: 4)
    private let two = UpdateOffer(version: "4.2.1", build: "910", speaks: 4)

    func testAScheduledCheckAsksOnce() {
        var state = UpdateState()
        XCTAssertEqual(state.found(one, byHand: false), .ask)
        XCTAssertEqual(state.phase, .asking(one))
        state.later()
        XCTAssertEqual(state.phase, .waiting(one))
        XCTAssertEqual(state.found(one, byHand: false), .dismiss)
        XCTAssertEqual(state.phase, .waiting(one))
    }

    func testLaterChangesNothingUntilANewerBuild() {
        var state = UpdateState()
        _ = state.found(one, byHand: false)
        state.later()
        XCTAssertEqual(state.menuTitle, "Install Update 4.2.0…")
        XCTAssertEqual(state.found(two, byHand: false), .ask)
    }

    func testALaunchAsksAgain() {
        var state = UpdateState(pending: one)
        XCTAssertEqual(state.phase, .waiting(one))
        XCTAssertEqual(state.found(one, byHand: false), .ask)
    }

    func testCheckingByHandAlwaysAsks() {
        var state = UpdateState()
        _ = state.found(one, byHand: false)
        state.later()
        XCTAssertEqual(state.found(one, byHand: true), .ask)
    }

    func testInstallFromTheMenuAsksNoSecondTime() {
        var state = UpdateState(pending: one)
        XCTAssertTrue(state.wantsInstallFromMenu)
        state.requestInstall()
        XCTAssertEqual(state.found(one, byHand: true), .install)
        XCTAssertEqual(state.phase, .downloading(one))
        XCTAssertFalse(state.menuEnabled)
    }

    func testYesGoesThroughDownloadToInstall() {
        var state = UpdateState()
        _ = state.found(one, byHand: false)
        state.accepted()
        XCTAssertEqual(state.phase, .downloading(one))
        state.installing()
        XCTAssertEqual(state.phase, .installing(one))
    }

    func testCancelOrAFailureKeepsItOffered() {
        var state = UpdateState()
        _ = state.found(one, byHand: false)
        state.accepted()
        state.stopped()
        XCTAssertEqual(state.phase, .waiting(one))
        XCTAssertEqual(state.pending, one)
    }

    func testAFeedThatNoLongerOffersItClearsIt() {
        var state = UpdateState(pending: one)
        state.notFound()
        XCTAssertEqual(state.phase, .idle)
        XCTAssertNil(state.pending)
        XCTAssertEqual(state.menuTitle, "Check for Updates…")
    }

    func testMachinesThatFallBehind() {
        XCTAssertEqual(UpdateText.machinesBehind(speaks: 4, mine: 4, machines: ["linux-1": 4]), [])
        XCTAssertEqual(UpdateText.machinesBehind(speaks: nil, mine: 4, machines: ["linux-1": 4]), [])
        XCTAssertEqual(UpdateText.machinesBehind(speaks: 5, mine: 4, machines: ["b": 4, "a": nil, "c": 5]), ["a", "b"])
    }

    func testTheWarningLine() {
        XCTAssertNil(UpdateText.fallsBehind([]))
        XCTAssertEqual(UpdateText.fallsBehind(["linux-1"]), "linux-1 will show Incompatible until you update it.")
        XCTAssertEqual(UpdateText.fallsBehind(["a", "b"]), "a and b will show Incompatible until you update them.")
        XCTAssertEqual(UpdateText.fallsBehind(["a", "b", "c"]), "3 machines will show Incompatible until you update them.")
    }

    func testSizes() {
        XCTAssertEqual(UpdateText.size(received: 18_000_000, total: 45_000_000), "18 of 45 MB")
        XCTAssertEqual(UpdateText.size(received: 18_000_000, total: 0), "18 MB")
    }
}
