import Testing
@testable import KannaWindows

struct KeyboardTests {
    @Test func aWindowWithNoResponderGivesTheFocusedPaneTheKeyboard() {
        #expect(Keyboard.moves(from: .nothing, keyed: false))
        #expect(Keyboard.moves(from: .nothing, keyed: true))
    }

    @Test func theKeyboardFollowsTheFocusedPaneFromAnotherPane() {
        #expect(Keyboard.moves(from: .pane, keyed: true))
    }

    @Test func theFocusedPaneKeepsIt() {
        #expect(!Keyboard.moves(from: .target, keyed: false))
        #expect(!Keyboard.moves(from: .target, keyed: true))
    }

    @Test func aFieldBeingTypedInKeepsIt() {
        #expect(!Keyboard.moves(from: .text, keyed: false))
        #expect(!Keyboard.moves(from: .text, keyed: true))
    }

    @Test func aNewWindowTakesItFromTheKeyViewAppKitPicked() {
        #expect(Keyboard.moves(from: .other, keyed: false))
    }

    @Test func onceAPaneHadItAViewTheUserChoseKeepsIt() {
        #expect(!Keyboard.moves(from: .other, keyed: true))
    }
}
