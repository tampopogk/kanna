import Testing
@testable import KannaMachines

struct ProtocolsTests {
    @Test func protocolsMeetAtTheHighestBothSpeak() {
        #expect(Protocols.common([1, 3], [2, 5]) == 3)
        #expect(Protocols.common([1, 2], [3, 4]) == nil)
    }

    @Test func theMessageSaysWhichSideIsOlderAndLeadsWithTheFix() {
        let old = Protocols.otherProtocol(machine: "r720-1", protocols: [1, 1], version: "4.0.1", commit: String(repeating: "d7cb0a1e", count: 5), mine: [2, 3])
        #expect(old.title == "r720-1 runs an older Kanna")
        #expect(old.detail == "Update Kanna on r720-1. It runs 4.0.1 (d7cb0a1e), protocol 1; this Kanna speaks 2–3.")
        let new = Protocols.otherProtocol(machine: "vm", protocols: [4, 5], version: "4.2.0", commit: nil, mine: [2, 3])
        #expect(new.title == "vm runs a newer Kanna")
        #expect(new.detail == "Update this Kanna. It runs 4.2.0, protocol 4–5; this Kanna speaks 2–3.")
    }
}
