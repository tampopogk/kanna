import AppKit

public struct CodeMarks: Equatable, Sendable {
    public var selection: [NSRange] = []
    public var matches: [NSRange] = []
    public var current = false
    public var commented = false
    public var dimmed = false
    public var changed = false
    public var flashing = false

    @MainActor
    func fillLine(_ rect: NSRect) {
        if commented {
            CodeTheme.tint.setFill()
            rect.fill()
        }
        if current {
            CodeTheme.currentLine.setFill()
            rect.fill()
        }
        if flashing {
            CodeTheme.flash.setFill()
            rect.fill()
        }
    }

    @MainActor
    func drawMatches(_ rects: [NSRect]) {
        for rect in rects { NSTextFinder.drawIncrementalMatchHighlight(in: rect) }
    }

    @MainActor
    func drawDim(_ rect: NSRect) {
        guard dimmed else { return }
        CodeTheme.dim.setFill()
        rect.fill(using: .sourceOver)
    }

    @MainActor
    func drawGutter(line: Int, in rect: NSRect, font: NSFont) {
        if commented {
            CodeTheme.tint.setFill()
            rect.fill()
        }
        if current {
            CodeTheme.currentLine.setFill()
            rect.fill()
        }
        if commented {
            CodeTheme.tintEdge.setFill()
            NSRect(x: rect.minX, y: rect.minY, width: 2, height: rect.height).fill()
        }
        if changed {
            CodeTheme.changed.setFill()
            NSBezierPath(roundedRect: NSRect(x: rect.minX + 3, y: rect.minY + 2, width: 3, height: rect.height - 4), xRadius: 1.5, yRadius: 1.5).fill()
        }
        let label = String(line) as NSString
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: current || commented ? CodeTheme.currentNumber : CodeTheme.number]
        let size = label.size(withAttributes: attributes)
        label.draw(at: NSPoint(x: rect.maxX - CodeTheme.gutterPadding - size.width, y: rect.minY + (rect.height - size.height) / 2), withAttributes: attributes)
    }
}
