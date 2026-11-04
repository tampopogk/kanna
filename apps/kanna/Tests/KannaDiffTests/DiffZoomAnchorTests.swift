import AppKit
import Testing
@testable import KannaDiff

@MainActor
struct DiffZoomAnchorTests {
    func pane() -> DiffPaneView {
        let lines = (1...600).map { "let value\($0) = \($0)" }
        let hunk = DiffHunk(old: 1, new: 1, scope: "", lines: lines.enumerated().map { DiffLine(kind: .added, text: $1, old: nil, new: $0 + 1) })
        let file = DiffFile(path: "a.rs", oldPath: nil, status: "M", added: lines.count, removed: 0, binary: nil, untracked: nil, fold: nil, note: nil, hunks: [hunk])
        let pane = DiffPaneView(view: "unified")
        pane.frame = NSRect(x: 0, y: 0, width: 900, height: 600)
        pane.receive(DiffResult(folder: "/r", args: [], command: "git diff", top: "/r", files: [file], added: lines.count, removed: 0, error: nil), serial: 1)
        pane.layoutSubtreeIfNeeded()
        return pane
    }

    func top(_ pane: DiffPaneView) -> CGFloat { pane.columns[0].scrollView.contentView.bounds.minY }

    func scroll(_ pane: DiffPaneView, to y: CGFloat) {
        let clip = pane.columns[0].scrollView.contentView
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: y))
        pane.columns[0].scrollView.reflectScrolledClipView(clip)
    }

    @Test func aZoomKeepsTheRowAtTheTop() throws {
        let pane = pane()
        scroll(pane, to: 3001)
        let start = try #require(pane.topSpot)
        let y = top(pane)
        for _ in 0..<10 {
            pane.zoom(by: 1)
            let zoomed = try #require(pane.topSpot)
            #expect(zoomed.row == start.row)
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
