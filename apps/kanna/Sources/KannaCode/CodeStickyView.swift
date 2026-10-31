import AppKit

@MainActor
final class CodeStickyView: NSView {
    struct Row {
        let line: Int
        let text: NSAttributedString
        let marks: CodeMarks
        let selection: [NSRect]
        let matches: [NSRect]
    }

    var rows: [Row] = [] { didSet { needsDisplay = true } }
    var gutterWidth: CGFloat = 0
    var textX: CGFloat = 0
    var lineHeight: CGFloat = 16
    var fontSize = CodeTheme.fontSize
    var selectionColor = NSColor.selectedTextBackgroundColor
    var onPick: ((Int) -> Void)?

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard !rows.isEmpty else { return }
        CodeTheme.background.setFill()
        bounds.fill()
        let font = CodeTheme.font(size: fontSize)
        for (index, row) in rows.enumerated() {
            let y = CGFloat(index) * lineHeight
            row.marks.drawGutter(line: row.line, in: NSRect(x: 0, y: y, width: gutterWidth, height: lineHeight), font: font)
            let text = NSRect(x: gutterWidth, y: y, width: bounds.width - gutterWidth, height: lineHeight)
            NSGraphicsContext.saveGraphicsState()
            text.clip()
            row.marks.fillLine(text)
            row.marks.drawMatches(row.matches.map { place($0, y: y) })
            selectionColor.setFill()
            row.selection.forEach { place($0, y: y).fill() }
            row.text.draw(at: NSPoint(x: textX, y: y))
            row.marks.drawDim(text)
            NSGraphicsContext.restoreGraphicsState()
        }
        CodeTheme.line.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }

    private func place(_ span: NSRect, y: CGFloat) -> NSRect {
        NSRect(x: textX + span.minX, y: y, width: span.width, height: lineHeight)
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let index = Int(point.y / lineHeight)
        if rows.indices.contains(index) { onPick?(rows[index].line) }
    }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .pointingHand)
    }
}
