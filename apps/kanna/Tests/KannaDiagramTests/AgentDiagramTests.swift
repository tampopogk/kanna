import AppKit
import Foundation
import Testing
@testable import KannaDiagram

@Suite struct AgentDiagramTests {
    static let ssh = """
    sequenceDiagram
        participant You as Your Mac (terminal app)
        participant R as Remote (shell, agent)
        You->>R: keystroke "l"
        R-->>You: bytes "l"
        Note over You: nothing on screen until the echo comes back
        R-->>You: every byte the program writes, in order
        Note over You,R: lose the link and the session dies#59; a slow link means typing lags by the round trip
    """
    static let mosh = """
    sequenceDiagram
        participant You as Your Mac (mosh client: a screen copy)
        participant R as Remote (mosh server: the real screen)
        You->>R: keystroke "l"
        Note over You: shows "l" at once (a guess, underlined until confirmed)
        R->>R: program writes bytes, server updates its screen
        R-->>You: diff: screen #35;41 → #35;42 (only the cells that changed)
        Note over R: link slow? it skips straight to the newest screen
        Note over You,R: laptop sleeps or changes Wi-Fi: it carries on (UDP, no connection to break)
    """
    static let pipeline = """
    flowchart LR
        P["shell / agent"] -->|bytes| K["keeper (pty)"] -->|bytes| D["daemon: headless Ghostty<br/>the real screen"] -->|bytes| A["Kanna.app draws"]
    """

    static func render(_ name: String, _ scene: DiagramScene) {
        let context = CGContext(data: nil, width: Int(scene.size.width), height: Int(scene.size.height), bitsPerComponent: 8, bytesPerRow: 0,
                                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        #expect(scene.draw(in: context, dark: false) == 0, "no label in \(name) is shrunk to fit")
        guard let dir = ProcessInfo.processInfo.environment["KANNA_DIAGRAM_PNG"] else { return }
        for dark in [false, true] {
            try? scene.png(dark: dark)?.write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(name)-\(dark ? "dark" : "light").png"))
        }
    }

    static func labels(_ scene: DiagramScene) -> [DiagramScene.Label] {
        scene.items.compactMap { if case .text(let label) = $0 { label } else { nil } }
    }

    @Test func sshSequenceDraws() throws {
        let scene = try #require(try Diagram.layout(source: Self.ssh).scene)
        Self.render("ssh", scene)
        #expect(Set(scene.texts).isSuperset(of: ["Your Mac (terminal app)", "Remote (shell, agent)", "keystroke \"l\"", "bytes \"l\""]))
        let note = try #require(Self.labels(scene).first { $0.text.hasPrefix("lose the link") })
        #expect(LabelText.plainLines(note.text) == ["lose the link and the session dies; a slow link means typing lags by the round trip"])
    }

    @Test func moshSequenceDraws() throws {
        let scene = try #require(try Diagram.layout(source: Self.mosh).scene)
        Self.render("mosh", scene)
        #expect(Set(scene.texts).isSuperset(of: ["Your Mac (mosh client: a screen copy)", "Remote (mosh server: the real screen)",
                                                 "program writes bytes, server updates its screen"]))
        let diff = try #require(Self.labels(scene).first { $0.text.hasPrefix("diff:") })
        #expect(LabelText.plainLines(diff.text) == ["diff: screen #41 → #42 (only the cells that changed)"])
    }

    @Test func chainedLabelledEdgesDraw() throws {
        let scene = try #require(try Diagram.layout(source: Self.pipeline).scene)
        Self.render("pipeline", scene)
        #expect(scene.texts.filter { $0 == "bytes" }.count == 3)
        #expect(Set(scene.texts).isSuperset(of: ["shell / agent", "keeper (pty)", "Kanna.app draws"]))
        let daemon = try #require(Self.labels(scene).first { $0.text.hasPrefix("daemon") })
        #expect(LabelText.plainLines(daemon.text) == ["daemon: headless Ghostty", "the real screen"])
    }

    @Test func entityCodesDecode() {
        #expect(LabelText.plainLines("I \u{FB02}\u{B0}\u{B0}9829\u{B6}\u{DF} you") == ["I ♥ you"])
        #expect(LabelText.plainLines("say \u{FB02}\u{B0}quot\u{B6}\u{DF}hi\u{FB02}\u{B0}quot\u{B6}\u{DF}") == ["say \"hi\""])
        #expect(LabelText.plainLines("&#35;1 &#x2192; &amp;") == ["#1 → &"])
    }

    @Test func sequenceSeparatorsAndCommentsAreMermaids() throws {
        #expect(throws: DiagramError.self) { try Diagram.layout(source: "sequenceDiagram\n    Note over A: one; two") }
        let scene = try #require(try Diagram.layout(source: "sequenceDiagram\n    A->>B: issue #41 fixed").scene)
        #expect(scene.texts.contains("issue"))
    }
}
