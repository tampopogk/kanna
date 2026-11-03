import Testing
@testable import KannaMachines

struct ProtocolsTests {
    @Test func theMessageSaysWhichSideIsOlderAndLeadsWithTheFix() {
        let old = Protocols.otherProtocol(machine: "r720-1", protocol: 3, version: "4.0.1", commit: String(repeating: "d7cb0a1e", count: 5), mine: 4)
        #expect(old.title == "r720-1 runs an older Kanna")
        #expect(old.detail == "Update Kanna on r720-1. It runs 4.0.1 (d7cb0a1e), protocol 3; this Kanna speaks 4.")
        let new = Protocols.otherProtocol(machine: "vm", protocol: 5, version: "4.2.0", commit: nil, mine: 4)
        #expect(new.title == "vm runs a newer Kanna")
        #expect(new.detail == "Update this Kanna. It runs 4.2.0, protocol 5; this Kanna speaks 4.")
        let before = Protocols.otherProtocol(machine: "vm", protocol: nil, version: "4.0.0", commit: nil, mine: 4)
        #expect(before.detail == "Update Kanna on vm. It runs 4.0.0, an older protocol; this Kanna speaks 4.")
    }
}
