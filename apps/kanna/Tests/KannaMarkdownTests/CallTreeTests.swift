import AppKit
import Testing
@testable import KannaMarkdown

@MainActor
struct CallTreeTests {
    static let doc = """
    # Rename a Shelf task

    ```calltree
    kanna CLI
    main          crates/kanna/src/lib.rs:17                kanna task name t12 "New name"
      parse       crates/kanna/src/commands.rs:440          task name TASK NAME → request
      wire        crates/kanna/src/lib.rs:316               task name with a TASK → tasks
        -> socket {"cmd":"tasks","action":"name"}
    kanna-daemon
        Tasks::name    crates/kanna-daemon/src/tasks.rs:1043  finds the task on the Shelf
          rename_pane  crates/kanna-mux/src/core.rs:1371      its held agent's pane
          store        crates/kanna-daemon/src/tasks.rs:1067  on the reply: the new name
    ```

    Text after.
    """

    static func repo() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("calltree-\(ProcessInfo.processInfo.processIdentifier)")
        try FileManager.default.createDirectory(at: root.appendingPathComponent(".git"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: root.appendingPathComponent("docs"), withIntermediateDirectories: true)
        try doc.write(to: root.appendingPathComponent("docs/rename.md"), atomically: true, encoding: .utf8)
        return root
    }

    @Test func theTreeParsesThroughTheFFI() {
        let tree = CallTree.parse(Self.doc.components(separatedBy: "```")[1].replacingOccurrences(of: "calltree\n", with: ""), doc: "/repo/docs")
        #expect(tree.processes == ["kanna CLI", "kanna-daemon"])
        #expect(tree.rows.map(\.name) == ["main", "parse", "wire", "", "Tasks::name", "rename_pane", "store"])
        #expect(tree.rows[3].kind == .hop)
        #expect(tree.rows[1].number == 1)
        #expect(tree.rows[4].guides == [.none, .end])
        #expect(tree.rows[4].location == "tasks.rs:1043")
        #expect(tree.rows[4].package == "kanna-daemon")
        #expect(tree.errors.isEmpty)
    }

    @Test func rowsShowTheirCallsAndLinkToTheCode() throws {
        let root = try Self.repo()
        defer { try? FileManager.default.removeItem(at: root) }
        let pane = MarkdownPaneView(path: root.appendingPathComponent("docs/rename.md").path)
        pane.frame = NSRect(x: 0, y: 0, width: 1100, height: 700)
        pane.show(text: Self.doc)
        let text = pane.text
        #expect(text.contains("Tasks::name  finds the task on the Shelf"))
        #expect(text.contains("kanna-daemon tasks.rs:1043"))
        #expect(text.contains("⇣ socket {\"cmd\":\"tasks\",\"action\":\"name\"}"))
        #expect(text.contains("1 rename_pane"))
        let storage = try #require(pane.textView.textStorage)
        let at = (storage.string as NSString).range(of: "tasks.rs:1043").location
        let link = storage.attribute(.link, at: at, effectiveRange: nil) as? String
        #expect(link == MarkdownRenderer.codeLink(path: root.appendingPathComponent("crates/kanna-daemon/src/tasks.rs").path, line: 1043))
        #expect(storage.attribute(.kannaLine, at: at, effectiveRange: nil) as? Int == 10)
        if let out = ProcessInfo.processInfo.environment["KANNA_CALLTREE_PNG"] {
            for dark in [true, false] {
                pane.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                pane.layoutSubtreeIfNeeded()
                let rep = try #require(pane.bitmapImageRepForCachingDisplay(in: pane.bounds))
                pane.cacheDisplay(in: pane.bounds, to: rep)
                try rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: out + (dark ? "-dark.png" : "-light.png")))
            }
        }
    }

    @Test func aCodeLinkOpensTheCodeAtItsLine() throws {
        let root = try Self.repo()
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("docs/rename.md").path
        let target = MarkdownRenderer.codeLink(path: file, line: 3)
        #expect(MarkdownPaneView.action(for: target, from: file) == .openCode(file, 3))
        #expect(MarkdownPaneView.action(for: "rename.md#L3", from: file) == .openCode(file, 3))
        #expect(MarkdownPaneView.action(for: "rename.md", from: file) == .showMarkdown(file))
    }
}
