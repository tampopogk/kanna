import AppKit
import Testing
@testable import KannaOverview

struct OverviewLayoutTests {
    let font = NSFont.monospacedSystemFont(ofSize: 12.5, weight: .regular)
    let files = OverviewPanel(name: "files", width: 231, columns: 110)

    func viewport(content: CGFloat, height: CGFloat = 600) -> OverviewViewport {
        OverviewViewport(documentHeight: max(content, height) + height, contentHeight: content, top: 0, height: height)
    }

    func cutoff(_ size: CGFloat) -> CGFloat {
        OverviewMetrics.width + OverviewMetrics.columns * OverviewLayout.columnWidth(.monospacedSystemFont(ofSize: size, weight: .regular))
    }

    func shown(_ width: CGFloat, size: CGFloat = 12.5) -> Bool {
        let column = OverviewLayout.columnWidth(.monospacedSystemFont(ofSize: size, weight: .regular))
        return OverviewLayout.shown([.overview], width: width, columnWidth: column).contains(.overview)
    }

    @Test func aFileThatFitsWantsNoOverview() {
        #expect(!OverviewLayout.wanted(enabled: true, viewport: viewport(content: 600)))
        #expect(!OverviewLayout.wanted(enabled: true, viewport: viewport(content: 300)))
        #expect(OverviewLayout.wanted(enabled: true, viewport: viewport(content: 601)))
    }

    @Test func turnedOffWantsNone() {
        #expect(!OverviewLayout.wanted(enabled: false, viewport: viewport(content: 5000)))
    }

    @Test func itShowsOnlyWhenTheTextKeepsEightyColumns() {
        #expect(shown(cutoff(12.5)))
        #expect(!shown(cutoff(12.5) - 1))
    }

    @Test func theCutoffFollowsZoom() {
        let width = cutoff(12.5)
        #expect(shown(width, size: 9))
        #expect(!shown(width, size: 16))
        #expect(cutoff(16) > cutoff(12.5) && cutoff(12.5) > cutoff(9))
    }

    @Test func theListGivesWayFirst() {
        let column = OverviewLayout.columnWidth(font)
        let both = files.width + OverviewMetrics.width + 110 * column
        #expect(OverviewLayout.shown([files, .overview], width: both, columnWidth: column) == [files, .overview])
        #expect(OverviewLayout.shown([files, .overview], width: both - 1, columnWidth: column) == [.overview])
        #expect(OverviewLayout.shown([files, .overview], width: cutoff(12.5) - 1, columnWidth: column) == [])
    }

    @Test func narrowingOnlyHides() {
        let column = OverviewLayout.columnWidth(font)
        var before = 2
        for width in stride(from: 2000, through: 200, by: -1) {
            let count = OverviewLayout.shown([files, .overview], width: CGFloat(width), columnWidth: column).count
            #expect(count <= before, "\(width)")
            before = count
        }
    }

    @Test func withoutTheOverviewTheListKeepsItsOwnRule() {
        let column = OverviewLayout.columnWidth(font)
        let alone = files.width + 110 * column
        #expect(OverviewLayout.shown([files], width: alone, columnWidth: column) == [files])
        #expect(OverviewLayout.shown([files], width: alone - 1, columnWidth: column) == [])
    }
}

struct OverviewGeometryTests {
    func geometry(document: CGFloat, top: CGFloat, view: CGFloat = 600, height: CGFloat = 600, lineHeight: CGFloat = 19) -> OverviewGeometry {
        OverviewGeometry(viewport: OverviewViewport(documentHeight: document, contentHeight: document - view, top: top, height: view), scale: 2 / lineHeight, height: height)
    }

    @Test func aShortFileDoesNotScrollTheThumbnail() {
        let g = geometry(document: 3000, top: 900)
        #expect(g.thumbHeight < g.height)
        #expect(g.offset == 0)
        #expect(abs(g.boxY - 900 * 2 / 19) < 0.001)
        #expect(abs(g.boxHeight - 600 * 2 / 19) < 0.001)
    }

    @Test func aTallFileScrollsTheThumbnailWithTheText() {
        let top = geometry(document: 100_000, top: 0)
        #expect(top.offset == 0 && top.boxY == 0)
        let end = geometry(document: 100_000, top: 100_000 - 600)
        #expect(abs(end.offset - (end.thumbHeight - end.height)) < 0.001)
        #expect(abs(end.boxY + end.boxHeight - end.height) < 0.001)
        let middle = geometry(document: 100_000, top: (100_000 - 600) / 2)
        #expect(abs(middle.boxY + middle.boxHeight / 2 - middle.height / 2) < 0.001)
    }

    @Test func clickingPutsThatLineInTheMiddle() {
        let g = geometry(document: 3000, top: 0)
        let y: CGFloat = 200
        #expect(abs(g.top(clickingThumbAt: y) - (y * 19 / 2 - 300)) < 0.001)
        #expect(g.top(clickingThumbAt: 0) == 0)
        #expect(g.top(clickingThumbAt: 10_000) == g.maxTop)
    }

    @Test func clickingATallFileCountsTheOffset() {
        let g = geometry(document: 100_000, top: 50_000)
        let y = g.boxY + g.boxHeight / 2
        #expect(abs(g.top(clickingThumbAt: y) - 50_000) < 1)
    }

    @Test func draggingTheBoxAcrossItsTrackScrollsTheWholeFile() {
        for document: CGFloat in [3000, 100_000] {
            let g = geometry(document: document, top: 0)
            let track = min(g.thumbHeight, g.height) - g.boxHeight
            #expect(abs(g.top(dragFrom: 0, by: track, rate: g.dragRate) - g.maxTop) < 0.001, "\(document)")
            #expect(g.top(dragFrom: 0, by: -50, rate: g.dragRate) == 0)
        }
        let short = geometry(document: 3000, top: 0)
        #expect(abs(short.dragRate - 19 / 2) < 0.001)
    }

    @Test func theMarksColumnMapsTheWholeFile() {
        let g = geometry(document: 6000, top: 0)
        #expect(abs(g.marksScale - 0.1) < 0.0001)
        #expect(abs(g.top(clickingMarksAt: 300) - (3000 - 300)) < 0.001)
        #expect(abs(g.marksDragRate - 10) < 0.0001)
    }

    @Test func marksOnNeighbouringLinesMerge() {
        let color = NSColor.red
        let runs = OverviewMark.runs([OverviewMark(y: 0, height: 19, kind: .added, color: color), OverviewMark(y: 19, height: 19, kind: .added, color: color),
                                      OverviewMark(y: 95, height: 19, kind: .added, color: color), OverviewMark(y: 19, height: 19, kind: .hit, color: color)])
        #expect(runs.count == 3)
        #expect(runs.first { $0.kind == .added && $0.y == 0 }?.height == 38)
    }
}
