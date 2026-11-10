import AppKit
import Markdown
import Testing
@testable import KannaMarkdown

@MainActor
struct MarkdownTableTests {
    struct Cell {
        let table: NSTextTable
        let row: Int
        let column: Int
        let range: NSRange
        let block: NSTextTableBlock
    }

    struct Drawn: Equatable {
        let bounds: NSRect
        let text: NSRect
    }

    static func doc(tables: Int = 14) -> String {
        var out = "# Rows\n\nTables whose cells wrap to many lines, between paragraphs.\n\n"
        for t in 1...tables {
            out += "## Table \(t)\n\nA paragraph before table \(t), long enough to wrap once in a narrow pane and say what the table compares.\n\n"
            out += "| # | Claim | Verdict | Notes |\n|---|---|---|---|\n"
            for r in 1...(4 + t % 5) {
                let long = r % 3 == 0 ? ", with a much longer note that goes on to wrap over several more lines than any other cell in this row, so the row is as tall as this cell" : ""
                out += "| \(r) | The server keeps every terminal in a process of its own, row \(r) of table \(t), and says so in a sentence that wraps | ✓, except the socket is owner-only and `runtime.json` sits under `servers/<hash>`\(long) | short |\n"
            }
            out += "\n"
        }
        return out
    }

    static func render(_ source: String) -> NSAttributedString {
        var renderer = MarkdownRenderer(folder: URL(fileURLWithPath: NSTemporaryDirectory()), diagrams: DiagramCache())
        return renderer.render(Document(parsing: source))
    }

    static func cells(_ storage: NSTextStorage) -> [Cell] {
        var out: [Cell] = []
        storage.enumerateAttribute(.paragraphStyle, in: NSRange(location: 0, length: storage.length)) { value, range, _ in
            guard let block = (value as? NSParagraphStyle)?.textBlocks.last as? NSTextTableBlock else { return }
            if let last = out.last, last.block === block {
                out[out.count - 1] = Cell(table: last.table, row: last.row, column: last.column, range: NSUnionRange(last.range, range), block: block)
            } else {
                out.append(Cell(table: block.table, row: block.startingRow, column: block.startingColumn, range: range, block: block))
            }
        }
        return out
    }

    final class Laid {
        let storage: NSTextStorage
        let manager = MarkdownLayoutManager()
        let container: NSTextContainer
        init(_ text: NSAttributedString, width: CGFloat) {
            storage = NSTextStorage(attributedString: text)
            storage.addLayoutManager(manager)
            container = NSTextContainer(size: NSSize(width: width, height: CGFloat.greatestFiniteMagnitude))
            manager.addTextContainer(container)
        }

        func drawn(_ cell: Cell) -> Drawn {
            let glyphs = manager.glyphRange(forCharacterRange: cell.range, actualCharacterRange: nil)
            var text = NSRect.null
            manager.enumerateLineFragments(forGlyphRange: glyphs) { _, used, _, _, _ in text = text.union(used) }
            return Drawn(bounds: manager.boundsRect(for: cell.block, glyphRange: glyphs), text: text)
        }

        func scroll(from top: CGFloat, to bottom: CGFloat, step: CGFloat, height: CGFloat = 600) {
            var y = top
            while y < bottom {
                manager.ensureLayout(forBoundingRect: NSRect(x: 0, y: y, width: container.size.width, height: height), in: container)
                y += step
            }
        }
    }

    static func checkRows(_ laid: Laid, against full: Laid) {
        let cells = Self.cells(laid.storage)
        #expect(cells.count > 200)
        let rows = Dictionary(grouping: cells) { "\(ObjectIdentifier($0.table))/\($0.row)" }
        for (key, row) in rows {
            let drawn = row.map(laid.drawn)
            let top = drawn[0].bounds.minY, height = drawn[0].bounds.height
            #expect(drawn.allSatisfy { abs($0.bounds.minY - top) < 0.01 && abs($0.bounds.height - height) < 0.01 }, "row \(key): cells differ in top or height")
            let tallest = drawn.map { $0.text.maxY }.max()!
            #expect(abs(tallest + 6 + 0.5 - drawn[0].bounds.maxY) <= 1, "row \(key): \(drawn[0].bounds.maxY - tallest) below its tallest cell's text")
            #expect(drawn.allSatisfy { $0.text.maxY <= $0.bounds.maxY && $0.text.minY >= $0.bounds.minY }, "row \(key): text drawn outside its cell")
            #expect(drawn == row.map(full.drawn), "row \(key): drawn unlike a full layout")
        }
        let byTable = Dictionary(grouping: cells.filter { $0.column == 0 }) { ObjectIdentifier($0.table) }
        for (_, column) in byTable {
            let boxes = column.sorted { $0.row < $1.row }.map { laid.drawn($0).bounds }
            for (above, below) in zip(boxes, boxes.dropFirst()) { #expect(abs(above.maxY - below.minY) < 0.01, "a gap or overlap between rows") }
        }
    }

    @Test func everyRowIsAsTallAsItsTallestCellHoweverItIsReached() {
        let text = Self.render(Self.doc())
        let full = Laid(text, width: 700)
        full.manager.ensureLayout(for: full.container)
        let height = full.manager.usedRect(for: full.container).height
        #expect(height > 6000)

        let scrolled = Laid(text, width: 700)
        scrolled.scroll(from: 0, to: height, step: 37)
        Self.checkRows(scrolled, against: full)

        let jumped = Laid(text, width: 700)
        jumped.manager.allowsNonContiguousLayout = true
        jumped.manager.ensureLayout(forBoundingRect: NSRect(x: 0, y: height * 0.6, width: 700, height: 600), in: jumped.container)
        jumped.scroll(from: height * 0.6, to: height, step: 50)
        jumped.scroll(from: 0, to: height * 0.6, step: 50)
        jumped.manager.ensureLayout(for: jumped.container)
        Self.checkRows(jumped, against: full)

        let narrow = Laid(text, width: 480)
        narrow.scroll(from: 0, to: 2000, step: 100)
        narrow.container.size = NSSize(width: 700, height: CGFloat.greatestFiniteMagnitude)
        narrow.scroll(from: 0, to: height, step: 90)
        Self.checkRows(narrow, against: full)
    }

    @Test func aStripRedrawnWhileScrollingHoldsEveryLineInIt() {
        let laid = Laid(Self.render(Self.doc()), width: 700)
        laid.scroll(from: 0, to: 20000, step: 300)
        let manager = laid.manager, container = laid.container
        var fragments: [(glyph: Int, rect: NSRect)] = []
        manager.enumerateLineFragments(forGlyphRange: NSRange(location: 0, length: manager.numberOfGlyphs)) { rect, _, _, glyphs, _ in
            fragments.append((glyphs.location, rect))
        }
        var missed = 0
        for strip in [7, 23, 61] as [CGFloat] {
            var y: CGFloat = 0
            let height = manager.usedRect(for: container).height
            while y < height {
                let rect = NSRect(x: 0, y: y, width: 700, height: strip)
                let drawn = manager.glyphRange(forBoundingRect: rect, in: container)
                missed += fragments.filter { $0.rect.intersects(rect) && !NSLocationInRange($0.glyph, drawn) }.count
                y += strip
            }
        }
        #expect(missed == 0, "\(missed) lines left out of the strips that show them")
    }

    @Test func columnsFollowTheirContentAndNeverOverflow() {
        let text = Self.render(Self.doc(tables: 1))
        for width: CGFloat in [420, 700, 1000] {
            let laid = Laid(text, width: width)
            laid.manager.ensureLayout(for: laid.container)
            let header = Self.cells(laid.storage).filter { $0.row == 0 }.sorted { $0.column < $1.column }.map { laid.drawn($0).bounds }
            #expect(header.count == 4)
            #expect(header[0].width < 60, "the # column is \(header[0].width) wide at \(width)")
            #expect(header[3].width < header[1].width)
            #expect(header.last!.maxX <= width, "the table overflows \(width): \(header.last!.maxX)")
            #expect(header[1].width >= ColumnPlan.minimum && header[2].width >= ColumnPlan.minimum)
            for (left, right) in zip(header, header.dropFirst()) { #expect(abs(left.maxX - right.minX) <= 1) }
        }
    }

    @Test func aShortColumnKeepsItsWidthAndTheRestShareInProportion() {
        let widths = ColumnPlan.widths(natural: [40, 600, 900, 70], room: 700)
        #expect(widths[0] == 40)
        #expect(widths[3] == 70)
        #expect(abs(widths[2] / widths[1] - 1.5) < 0.001)
        #expect(abs(widths.reduce(0, +) - 700) < 0.001)
    }

    @Test func aTableThatFitsKeepsItsNaturalWidths() {
        #expect(ColumnPlan.widths(natural: [40, 120, 90], room: 700) == [40, 120, 90])
    }

    @Test func aLongColumnLeavesTheOthersTheirMinimum() {
        let widths = ColumnPlan.widths(natural: [200, 5000], room: 300)
        #expect(widths[0] == ColumnPlan.minimum)
        #expect(abs(widths[1] - (300 - ColumnPlan.minimum)) < 0.001)
    }

    @Test func tooManyColumnsShrinkToFit() {
        let widths = ColumnPlan.widths(natural: Array(repeating: 400, count: 10), room: 500)
        #expect(abs(widths.reduce(0, +) - 500) < 0.001)
    }
}
