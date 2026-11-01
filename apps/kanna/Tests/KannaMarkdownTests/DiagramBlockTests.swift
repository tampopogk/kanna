import AppKit
import Markdown
import Testing
@testable import KannaMarkdown

@MainActor struct DiagramBlockTests {
    @Test func aDiagramThatFailsShowsItsErrorAndSource() {
        let source = "sequenceDiagram\n    Note over A: one; two"
        var renderer = MarkdownRenderer(folder: URL(fileURLWithPath: "/"), diagrams: DiagramCache())
        let text = renderer.render(Document(parsing: "```mermaid\n\(source)\n```\n")).string
        #expect(text.contains("Diagram error: Diagram parse error (sequence)"))
        #expect(text.contains("write #59; for a semicolon"))
        #expect(text.contains("Note over A: one; two"))
    }
}
