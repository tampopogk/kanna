import AppKit
import Testing
@testable import KannaMarkdown

@MainActor
struct ScrollAnchorTests {
    static let doc: String = (1...40).map { section in
        let words = (1...90).map { "word\(section)x\($0)" }.joined(separator: " ")
        let diagram = section % 4 == 0 ? "\n\n```mermaid\nflowchart TD\n  A\(section)[Start] --> B\(section)[Middle] --> C\(section)[End]\n```\n" : ""
        return "## Section \(section)\n\n\(words)\(diagram)\n"
    }.joined(separator: "\n")

    func pane() -> MarkdownPaneView {
        let pane = MarkdownPaneView(path: "/nowhere/long.md")
        pane.frame = NSRect(x: 0, y: 0, width: 520, height: 600)
        pane.show(text: Self.doc)
        settle(pane)
        return pane
    }

    func settle(_ pane: MarkdownPaneView) {
        pane.layoutSubtreeIfNeeded()
        pane.layoutManager.ensureLayout(for: pane.textView.textContainer!)
        pane.textView.sizeToFit()
        #expect(pane.textView.frame.height > 9000)
    }

    func resize(_ pane: MarkdownPaneView, width: CGFloat) {
        pane.frame.size.width = width
        pane.layoutSubtreeIfNeeded()
    }

    func topText(_ pane: MarkdownPaneView) -> String {
        let position = pane.readingPosition()!
        let string = pane.textView.textStorage!.string as NSString
        return string.substring(with: string.lineRange(for: NSRange(location: position.index, length: 0)))
    }

    @Test func aResizeKeepsTheTextAtTheTop() {
        let pane = pane()
        pane.scroll(toY: 3000)
        #expect(pane.scrollTop == 3000)
        let index = pane.readingPosition()!.index
        resize(pane, width: 1100)
        #expect(pane.readingPosition()!.index == index)
        #expect(pane.scrollTop < 3000)
    }

    @Test func zoomingAndUnzoomingTwentyTimesComesBackToTheSamePlace() {
        for y: CGFloat in [1500, 3333, 5210, 8000] {
            let pane = pane()
            pane.scroll(toY: y)
            let start = pane.scrollTop
            let text = topText(pane)
            for _ in 0..<20 {
                resize(pane, width: 1300)
                resize(pane, width: 520)
            }
            #expect(abs(pane.scrollTop - start) < 0.5)
            #expect(topText(pane) == text)
        }
    }

    @Test func aDiagramAtTheTopKeepsItsFraction() throws {
        let pane = pane()
        let storage = try #require(pane.textView.textStorage)
        var at: Int?
        storage.enumerateAttribute(.attachment, in: NSRange(location: 0, length: storage.length)) { value, range, stop in
            if value != nil, range.location > 2000 { at = range.location; stop.pointee = true }
        }
        let index = try #require(at)
        let glyph = pane.layoutManager.glyphIndexForCharacter(at: index)
        let line = pane.layoutManager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil).offsetBy(dx: 0, dy: pane.textView.textContainerOrigin.y)
        pane.scroll(toY: line.minY + line.height * 0.4)
        resize(pane, width: 1300)
        let position = try #require(pane.readingPosition())
        #expect(position.index == index)
        #expect(abs(position.fraction - 0.4) < 0.01)
    }

    @Test func fontZoomInAndOutComesBack() {
        let pane = pane()
        pane.scroll(toY: 4321)
        let start = pane.scrollTop
        let text = topText(pane)
        for _ in 0..<10 {
            pane.zoom(by: 1)
            #expect(topText(pane) == text)
            pane.zoom(by: -1)
        }
        #expect(abs(pane.scrollTop - start) < 0.5)
        #expect(topText(pane) == text)
    }

    @Test func showingThePanelsKeepsThePlace() {
        let pane = pane()
        resize(pane, width: 1500)
        settle(pane)
        pane.scroll(toY: 4000)
        let start = pane.scrollTop
        pane.set(markup: true)
        resize(pane, width: 900)
        resize(pane, width: 1500)
        pane.set(markup: false)
        #expect(abs(pane.scrollTop - start) < 0.5)
    }

    @Test func theTopStaysAtTheTop() {
        let pane = pane()
        resize(pane, width: 1300)
        pane.zoom(by: 1)
        resize(pane, width: 520)
        #expect(pane.scrollTop == 0)
    }
}
