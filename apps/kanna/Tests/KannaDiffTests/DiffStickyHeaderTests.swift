import AppKit
import KannaCode
import Testing
@testable import KannaDiff

@MainActor
struct DiffStickyHeaderTests {
    func file(_ status: String) -> DiffFile {
        DiffFile(path: "apps/kanna/Sources/KannaDiff/DiffFileList.swift", oldPath: nil, status: status, added: 3, removed: 2, binary: nil, untracked: nil, fold: nil, note: nil, hunks: [])
    }

    func render(_ view: NSView) -> NSBitmapImageRep {
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return bitmap
    }

    @Test func theHeaderDrawsAtEveryZoom() {
        let header = DiffStickyHeader(frame: NSRect(x: 0, y: 0, width: 600, height: 18))
        header.gutterWidth = 60
        for status in ["A", "D", "M", "R"] {
            header.file = file(status)
            for size in stride(from: DiffPaneView.clamp(0), through: DiffPaneView.clamp(.infinity), by: 0.5) {
                header.fontSize = size
                #expect(render(header).size.width == 600)
            }
        }
    }

    @Test func theFileListDraws() {
        let list = DiffFileList(frame: NSRect(x: 0, y: 0, width: DiffFileList.width, height: 200))
        list.show(["A", "D", "M"].map(file))
        list.selected = 0
        list.fresh = [1]
        #expect(render(list).size.width == DiffFileList.width)
    }
}
