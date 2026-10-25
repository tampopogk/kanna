import Testing
@testable import KannaFleet

struct ProtocolsTests {
    @Test func protocolsMeetAtTheHighestBothSpeak() {
        #expect(Protocols.common([1, 3], [2, 5]) == 3)
        #expect(Protocols.common([1, 2], [3, 4]) == nil)
    }

    @Test func theMessageSaysWhichSideIsOlderAndHowToFixIt() {
        #expect(Protocols.otherProtocol(machine: "r720-1", protocols: [1, 1], version: "4.0.1", commit: String(repeating: "d7cb0a1e", count: 5), mine: [2, 3]) == "r720-1 runs Kanna 4.0.1 (d7cb0a1e), protocol 1, and this Kanna speaks 2–3; it's older: kanna fleet update r720-1")
        #expect(Protocols.otherProtocol(machine: "vm", protocols: [4, 5], version: "4.2.0", commit: nil, mine: [2, 3]) == "vm runs Kanna 4.2.0, protocol 4–5, and this Kanna speaks 2–3; this Kanna is older: update it, or run kanna fleet update on vm")
    }
}
