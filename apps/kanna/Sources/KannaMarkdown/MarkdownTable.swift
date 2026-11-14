import AppKit

enum ColumnPlan {
    static let minimum: CGFloat = 72

    static func widths(natural: [CGFloat], room: CGFloat) -> [CGFloat] {
        let count = natural.count
        guard count > 0, room > 0 else { return natural.map { _ in 0 } }
        if natural.reduce(0, +) <= room { return natural }
        let floors = natural.map { min($0, minimum) }
        let floorSum = floors.reduce(0, +)
        if floorSum >= room { return floors.map { $0 * room / floorSum } }
        var widths = [CGFloat?](repeating: nil, count: count)
        var changed = true
        while changed {
            changed = false
            let open = (0..<count).filter { widths[$0] == nil }
            guard !open.isEmpty else { break }
            let left = room - widths.compactMap { $0 }.reduce(0, +)
            let fair = left / CGFloat(open.count)
            for index in open where natural[index] <= fair {
                widths[index] = natural[index]
                changed = true
            }
            if changed { continue }
            let weight = open.map { natural[$0] }.reduce(0, +)
            for index in open where left * natural[index] / weight < floors[index] {
                widths[index] = floors[index]
                changed = true
            }
            if changed { continue }
            for index in open { widths[index] = left * natural[index] / weight }
        }
        return widths.map { $0 ?? 0 }
    }
}

final class MarkdownTable: NSTextTable {
    static let inset: CGFloat = 12.5
    var natural: [CGFloat] = []
    private var planned: (room: CGFloat, offsets: [CGFloat], widths: [CGFloat])?

    func columns(room: CGFloat) -> (offsets: [CGFloat], widths: [CGFloat]) {
        if let planned, planned.room == room { return (planned.offsets, planned.widths) }
        let widths = ColumnPlan.widths(natural: natural, room: room)
        var offsets: [CGFloat] = []
        var x: CGFloat = 0
        for width in widths {
            offsets.append(x)
            x += width
        }
        planned = (room, offsets, widths)
        return (offsets, widths)
    }

    override func rect(for block: NSTextTableBlock, layoutAt startingPoint: NSPoint, in rect: NSRect, textContainer: NSTextContainer, characterRange: NSRange) -> NSRect {
        var out = super.rect(for: block, layoutAt: startingPoint, in: rect, textContainer: textContainer, characterRange: characterRange)
        let plan = columns(room: rect.width - 1)
        let first = block.startingColumn, last = min(plan.widths.count, first + block.columnSpan) - 1
        guard first <= last else { return out }
        let width = plan.widths[first...last].reduce(0, +)
        out.origin.x = (rect.minX + plan.offsets[first] + Self.inset + 0.5).rounded()
        out.size.width = max(1, (width - Self.inset * 2).rounded(.down))
        return out
    }
}

struct TableRow {
    let range: NSRange
    let first: NSRange
    let block: NSTextTableBlock
}

extension MarkdownLayoutManager {
    static func tableRows(in storage: NSAttributedString) -> [TableRow] {
        var rows: [TableRow] = []
        storage.enumerateAttribute(.paragraphStyle, in: NSRange(location: 0, length: storage.length)) { value, range, _ in
            guard let block = (value as? NSParagraphStyle)?.textBlocks.last as? NSTextTableBlock else { return }
            if let last = rows.last, last.block.table === block.table, last.block.startingRow == block.startingRow, NSMaxRange(last.range) == range.location {
                rows[rows.count - 1] = TableRow(range: NSUnionRange(last.range, range), first: last.first, block: last.block)
            } else {
                rows.append(TableRow(range: range, first: range, block: block))
            }
        }
        return rows
    }

    func widenToRows(_ range: NSRange, crossing bounds: NSRect) -> NSRange {
        guard let storage = textStorage else { return range }
        if tableRows == nil { tableRows = Self.tableRows(in: storage) }
        guard let rows = tableRows, !rows.isEmpty else { return range }
        let laid = allowsNonContiguousLayout ? storage.length : firstUnlaidCharacterIndex()
        let frame = { (row: TableRow) -> NSRect in
            self.boundsRect(for: row.block, glyphRange: self.glyphRange(forCharacterRange: row.first, actualCharacterRange: nil))
        }
        var low = 0, high = rows.count
        while low < high {
            let mid = (low + high) / 2
            if NSMaxRange(rows[mid].range) <= laid, frame(rows[mid]).maxY <= bounds.minY { low = mid + 1 } else { high = mid }
        }
        var out = range
        var index = low
        while index < rows.count, NSMaxRange(rows[index].range) <= laid {
            let rect = frame(rows[index])
            if rect.minY >= bounds.maxY { break }
            if rect.intersects(bounds) || rect.maxY > bounds.minY {
                let glyphs = glyphRange(forCharacterRange: rows[index].range, actualCharacterRange: nil)
                out = out.length == 0 ? glyphs : NSUnionRange(out, glyphs)
            }
            index += 1
        }
        return out
    }
}
