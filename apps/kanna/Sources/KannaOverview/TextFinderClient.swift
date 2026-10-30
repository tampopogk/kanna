import AppKit

@MainActor
public final class TextFinderClient: NSObject, @preconcurrency NSTextFinderClient {
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
