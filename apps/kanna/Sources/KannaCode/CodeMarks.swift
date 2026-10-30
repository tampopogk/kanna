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

@MainActor
public final class CodeFinderClient: NSObject, @preconcurrency NSTextFinderClient {
    weak var textView: NSTextView?

    public init(textView: NSTextView) {
        self.textView = textView
    }

    public var string: String { textView?.string ?? "" }
    public var isSelectable: Bool { true }
    public var isEditable: Bool { false }
    public var allowsMultipleSelection: Bool { false }
    public var firstSelectedRange: NSRange { textView?.selectedRange() ?? NSRange(location: 0, length: 0) }

    public var selectedRanges: [NSValue] {
        get { textView?.selectedRanges ?? [] }
        set { textView?.selectedRanges = newValue }
    }

    public func scrollRangeToVisible(_ range: NSRange) { textView?.scrollRangeToVisible(range) }

    public func contentView(at index: Int, effectiveCharacterRange outRange: NSRangePointer) -> NSView {
        outRange.pointee = NSRange(location: 0, length: (string as NSString).length)
        return textView ?? NSView()
    }

    public func rects(forCharacterRange range: NSRange) -> [NSValue]? {
        guard let textView, let manager = textView.layoutManager, let container = textView.textContainer else { return nil }
        let origin = textView.textContainerOrigin
        var rects: [NSValue] = []
        manager.enumerateEnclosingRects(forGlyphRange: manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil),
                                        withinSelectedGlyphRange: NSRange(location: NSNotFound, length: 0), in: container) { rect, _ in
            rects.append(NSValue(rect: rect.offsetBy(dx: origin.x, dy: origin.y)))
        }
        return rects
    }

    public var visibleCharacterRanges: [NSValue] {
        guard let textView, let manager = textView.layoutManager, let container = textView.textContainer else { return [] }
        let origin = textView.textContainerOrigin
        let glyphs = manager.glyphRange(forBoundingRect: textView.visibleRect.offsetBy(dx: -origin.x, dy: -origin.y), in: container)
        return [NSValue(range: manager.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil))]
    }

    public func drawCharacters(in range: NSRange, forContentView view: NSView) {
        guard let textView, let manager = textView.layoutManager else { return }
        manager.drawGlyphs(forGlyphRange: manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil), at: textView.textContainerOrigin)
    }
}
