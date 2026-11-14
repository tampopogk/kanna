import AppKit
import KannaWindows

final class StatsSwatch: NSView {
    var color = NSColor.clear

    override func draw(_ dirtyRect: NSRect) {
        color.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 2, yRadius: 2).fill()
    }
}

final class StatsCard: NSView {
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 9, yRadius: 9)
        StatsInk.panel.setFill()
        path.fill()
        StatsInk.line.setStroke()
        path.stroke()
    }
}

@MainActor
final class StatsChip: NSView {
    let title: String
    let on: Bool
    let plain: Bool
    var onClick: (() -> Void)?
    private let text: NSAttributedString

    override var isFlipped: Bool { true }

    init(title: String, on: Bool, plain: Bool = false) {
        self.title = title
        self.on = on
        self.plain = plain
        text = NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: plain ? 11.5 : 12), .foregroundColor: plain ? StatsInk.going : NSColor.labelColor])
        let size = text.size()
        super.init(frame: NSRect(x: 0, y: 0, width: ceil(size.width) + (plain ? 0 : 18), height: plain ? ceil(size.height) + 6 : 24))
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        setAccessibilityLabel(title)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func hitTest(_ point: NSPoint) -> NSView? { frame.contains(point) ? self : nil }

    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }

    override func mouseDown(with event: NSEvent) {
        guard let click = onClick else { return }
        DispatchQueue.main.async { click() }
    }

    override func draw(_ dirtyRect: NSRect) {
        let size = text.size()
        if plain {
            text.draw(at: NSPoint(x: 0, y: (bounds.height - size.height) / 2))
            return
        }
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 6, yRadius: 6)
        StatsInk.panel.setFill()
        path.fill()
        (on ? StatsInk.going : StatsInk.line).setStroke()
        path.stroke()
        text.draw(at: NSPoint(x: 9, y: (bounds.height - size.height) / 2))
    }
}

@MainActor
final class StatsTable: NSView {
    let columns: [StatsColumn]
    let shown: [StatsTableRow]
    private var widths: [CGFloat] = []
    private let rowHeight: CGFloat = 26, headerHeight: CGFloat, pad: CGFloat = 8, barWidth: CGFloat = 56

    override var isFlipped: Bool { true }

    private static func ink(_ cell: StatsCell, dim: Bool) -> NSColor {
        if dim { return .secondaryLabelColor }
        switch cell.tone {
        case "pass": return StatsInk.landed
        case "now": return StatsInk.going
        default: return .labelColor
        }
    }

    private static func text(_ cell: StatsCell, dim: Bool) -> NSAttributedString {
        let font = cell.mono ? NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular) : NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .regular)
        return NSAttributedString(string: cell.text, attributes: [.font: font, .foregroundColor: ink(cell, dim: dim)])
    }

    private static func head(_ title: String, right: Bool) -> NSAttributedString {
        let style = NSMutableParagraphStyle()
        style.alignment = right ? .right : .left
        return NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: 12, weight: .medium), .foregroundColor: NSColor.secondaryLabelColor, .paragraphStyle: style])
    }

    init(chart: StatsChart, hidden: Set<String>, width: CGFloat) {
        columns = chart.columns
        shown = chart.table.filter { !hidden.contains($0.pipeline) }
        headerHeight = chart.columns.contains { $0.title.contains("\n") } ? 40 : 26
        super.init(frame: .zero)
        widths = columns.indices.map { [pad, barWidth] column in
            let head = StatsTable.head(columns[column].title, right: false).size().width
            let cells = shown.map { row -> CGFloat in
                guard column < row.cells.count else { return 0 }
                let cell = row.cells[column]
                return StatsTable.text(cell, dim: false).size().width + (cell.swatch ? 15 : 0) + (cell.bar == nil ? 0 : barWidth + 6)
            }
            return ceil(max(head, cells.max() ?? 0)) + pad * 2
        }
        let natural = widths.reduce(0, +)
        if natural < width, !widths.isEmpty {
            if let fill = columns.firstIndex(where: \.fill) {
                widths[fill] += width - natural
            } else {
                let more = (width - natural) / CGFloat(widths.count)
                widths = widths.map { $0 + more }
            }
        }
        frame = NSRect(x: 0, y: 0, width: max(natural, width), height: headerHeight + rowHeight * CGFloat(shown.count))
        setAccessibilityElement(true)
        setAccessibilityRole(.table)
        setAccessibilityLabel(chart.what + ": " + shown.map { $0.cells.map(\.text).joined(separator: ", ") }.joined(separator: "; "))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draw(_ dirtyRect: NSRect) {
        var x: CGFloat = 0
        for (column, width) in zip(columns, widths) {
            StatsTable.head(column.title, right: column.right).draw(in: NSRect(x: x + pad, y: 6, width: width - pad * 2, height: headerHeight - 8))
            x += width
        }
        StatsInk.line.setFill()
        NSRect(x: 0, y: headerHeight - 1, width: bounds.width, height: 1).fill()
        for (index, row) in shown.enumerated() {
            let top = headerHeight + rowHeight * CGFloat(index)
            x = 0
            for (column, cell) in row.cells.enumerated() where column < widths.count {
                let (width, right) = (widths[column], columns[column].right)
                let text = StatsTable.text(cell, dim: row.dim)
                let size = text.size()
                let y = top + (rowHeight - size.height) / 2
                var left = right ? x + width - pad - size.width : x + pad
                if cell.swatch {
                    StatsInk.of(series: row.series).setFill()
                    NSBezierPath(roundedRect: NSRect(x: left, y: top + 8.5, width: 9, height: 9), xRadius: 2, yRadius: 2).fill()
                    left += 15
                }
                if let bar = cell.bar {
                    let length = max(2, barWidth * CGFloat(min(max(bar, 0), 1)))
                    let rect = NSRect(x: left - 6 - length, y: top + 9, width: length, height: 8)
                    StatsInk.of(series: row.series).setFill()
                    NSBezierPath(roundedRect: rect, xRadius: 3, yRadius: 3).fill()
                }
                text.draw(at: NSPoint(x: left, y: y))
                x += width
            }
            StatsInk.line.withAlphaComponent(0.6).setFill()
            NSRect(x: 0, y: top + rowHeight - 1, width: bounds.width, height: 1).fill()
        }
    }
}

@MainActor
final class StatsRunChart: NSView {
    let tile: StatsTile
    private let points: [StatsRunPoint]
    private let medians: [StatsRunPoint]
    private let gutter: CGFloat = 32, under: CGFloat = 18, top: CGFloat = 6
    var onHover: ((NSAttributedString?, NSPoint) -> Void)?

    override var isFlipped: Bool { true }

    init(tile: StatsTile, hidden: Set<String>) {
        self.tile = tile
        points = tile.chart.runPoints.filter { !hidden.contains($0.pipeline) }
        medians = tile.chart.medians.filter { !hidden.contains($0.pipeline) }
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
        setAccessibilityLabel("\(tile.label): " + (medians + points).map(\.label).joined(separator: ", "))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self))
    }

    private var plot: NSRect { NSRect(x: gutter, y: top, width: max(1, bounds.width - gutter - 8), height: max(1, bounds.height - top - under)) }
    private var scale: Double { max(tile.chart.max ?? 1, 0.0001) }
    private func y(_ value: Double) -> CGFloat { plot.maxY - CGFloat(min(value / scale, 1)) * plot.height }
    private func x(_ at: Double) -> CGFloat { plot.minX + CGFloat(StatsFormat.across(at, from: tile.chart.from, to: tile.chart.to)) * plot.width }

    override func draw(_ dirtyRect: NSRect) {
        let plot = self.plot
        let small: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .regular), .foregroundColor: NSColor.secondaryLabelColor]
        func text(_ string: String, _ point: NSPoint, align: CGFloat) {
            let size = (string as NSString).size(withAttributes: small)
            (string as NSString).draw(at: NSPoint(x: point.x - size.width * align, y: point.y - size.height / 2), withAttributes: small)
        }
        for (value, shown) in zip([0, scale / 2, scale], ["0"] + tile.chart.ticks) {
            StatsInk.line.setFill()
            NSRect(x: plot.minX, y: y(value), width: plot.width, height: 1).fill()
            text(shown, NSPoint(x: plot.minX - 6, y: y(value)), align: 1)
        }
        text(StatsFormat.windowStart(from: tile.chart.from, to: tile.chart.to), NSPoint(x: plot.minX, y: bounds.height - 7), align: 0)
        text("today", NSPoint(x: plot.maxX, y: bounds.height - 7), align: 1)
        for median in medians {
            let path = NSBezierPath()
            path.lineWidth = 2
            path.setLineDash([4, 3], count: 2, phase: 0)
            path.move(to: NSPoint(x: plot.minX, y: y(median.value)))
            path.line(to: NSPoint(x: plot.maxX, y: y(median.value)))
            StatsInk.of(series: median.series).withAlphaComponent(0.8).setStroke()
            path.stroke()
        }
        for point in points {
            let dot = NSBezierPath(ovalIn: NSRect(x: x(point.at) - 4.5, y: y(point.value) - 4.5, width: 9, height: 9))
            dot.lineWidth = 2
            StatsInk.panel.setStroke()
            StatsInk.of(series: point.series).setFill()
            dot.stroke()
            dot.fill()
        }
    }

    override func mouseMoved(with event: NSEvent) {
        let at = convert(event.locationInWindow, from: nil)
        let near = points.min { hypot(x($0.at) - at.x, y($0.value) - at.y) < hypot(x($1.at) - at.x, y($1.value) - at.y) }
        guard let near, hypot(x(near.at) - at.x, y(near.value) - at.y) < 14 else { return onHover?(nil, .zero) ?? () }
        onHover?(NSAttributedString(string: near.label, attributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.labelColor]), at)
    }

    override func mouseExited(with event: NSEvent) { onHover?(nil, .zero) }
}
