import AppKit
import Testing
@testable import KannaCode

@MainActor
struct CodeZoomAnchorTests {
    func pane() -> CodePaneView {
        let pane = CodePaneView(path: "/nowhere/long.swift", line: nil)
        pane.frame = NSRect(x: 0, y: 0, width: 900, height: 600)
        pane.show(text: (1...800).map { "let value\($0) = \($0)" }.joined(separator: "\n"))
        pane.layoutSubtreeIfNeeded()
        return pane
    }

    func top(_ pane: CodePaneView) -> CGFloat { pane.scrollView.contentView.bounds.minY }

    func scroll(_ pane: CodePaneView, to y: CGFloat) {
        let clip = pane.scrollView.contentView
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: y))
        pane.scrollView.reflectScrolledClipView(clip)
    }

    @Test func aZoomKeepsTheLineAtTheTop() throws {
        let pane = pane()
        scroll(pane, to: 4007)
        let start = try #require(pane.topSpot)
        #expect(start.fraction > 0.05)
        let y = top(pane)
        for _ in 0..<10 {
            pane.zoom(by: 1)
            let zoomed = try #require(pane.topSpot)
            #expect(zoomed.line == start.line)
            #expect(abs(zoomed.fraction - start.fraction) < 0.01)
            pane.zoom(by: -1)
        }
        #expect(abs(top(pane) - y) < 0.5)
    }

    @Test func theTopStaysAtTheTop() {
        let pane = pane()
        let y = top(pane)
        pane.zoom(by: 2)
        pane.zoom(by: -2)
        #expect(top(pane) == y)
    }
}
