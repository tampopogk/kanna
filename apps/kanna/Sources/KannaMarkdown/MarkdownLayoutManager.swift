import AppKit
import KannaDocs

struct DocMark {
    enum Kind { case open, resolved, active }
    let range: NSRange
    let kind: Kind
}

final class MarkdownLayoutManager: NSLayoutManager {
    var marks: [DocMark] = []

    func markRects(_ range: NSRange) -> [NSRect] {
        guard let storage = textStorage, NSMaxRange(range) <= storage.length, range.length > 0 else { return [] }
        let glyphs = glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        var rects: [NSRect] = []
        enumerateLineFragments(forGlyphRange: glyphs) { fragment, _, container, line, _ in
            let part = NSIntersectionRange(line, glyphs)
            guard part.length > 0 else { return }
            let box = self.boundingRect(forGlyphRange: part, in: container)
            let first = self.characterIndexForGlyph(at: part.location)
            let font = storage.attribute(.font, at: first, effectiveRange: nil) as? NSFont ?? NSFont.systemFont(ofSize: MarkdownTheme.bodySize)
            let baseline = fragment.minY + self.location(forGlyphAt: part.location).y
            let top = baseline - ceil(font.ascender), bottom = baseline + ceil(-font.descender)
            rects.append(NSRect(x: box.minX, y: top, width: box.width, height: bottom - top))
        }
        return rects
    }

    private func drawMarks(_ shown: NSRange, at origin: NSPoint) {
        for kind in [DocMark.Kind.resolved, .open, .active] {
            for mark in marks where mark.kind == kind && NSIntersectionRange(mark.range, shown).length > 0 {
                for rect in markRects(mark.range) {
                    let at = rect.offsetBy(dx: origin.x, dy: origin.y)
                    switch kind {
                    case .resolved: DocTheme.resolvedTint.setFill()
                    case .open: DocTheme.tint.setFill()
                    case .active: DocTheme.tintOn.setFill()
                    }
                    at.fill(using: .sourceOver)
                    if kind == .open {
                        DocTheme.accent.setFill()
                        NSRect(x: at.minX, y: at.maxY - 1.5, width: at.width, height: 1.5).fill(using: .sourceOver)
                    }
                }
            }
        }
    }

    override func drawBackground(forGlyphRange glyphsToShow: NSRange, at origin: NSPoint) {
        super.drawBackground(forGlyphRange: glyphsToShow, at: origin)
        defer { drawMarks(characterRange(forGlyphRange: glyphsToShow, actualGlyphRange: nil), at: origin) }
        guard let storage = textStorage, let container = textContainers.first, storage.length > 0 else { return }
        let shown = characterRange(forGlyphRange: glyphsToShow, actualGlyphRange: nil)
        let whole = NSRange(location: 0, length: storage.length)
        var drawn = Set<Int>()
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        var location = shown.location
        while location < NSMaxRange(shown) {
            var range = NSRange()
            let value = storage.attribute(.kannaBlock, at: location, longestEffectiveRange: &range, in: whole)
            location = max(location + 1, NSMaxRange(range))
            guard let block = value as? BlockDecoration, drawn.insert(block.id).inserted else { continue }
            let used = usedRect(for: range)
            let left = origin.x + container.lineFragmentPadding + block.indent
            let right = origin.x + container.size.width - container.lineFragmentPadding
            switch block.kind {
            case .code:
                let pad: CGFloat = 8
                let panel = NSRect(x: left, y: origin.y + used.minY - pad, width: right - left, height: used.height + pad * 2)
                MarkdownTheme.panel.setFill()
                NSBezierPath(roundedRect: panel, xRadius: 6, yRadius: 6).fill()
            case .quote:
                MarkdownTheme.line.setFill()
                NSRect(x: left + 2, y: origin.y + used.minY, width: 3, height: used.height).fill()
            case .rule:
                MarkdownTheme.line.setFill()
                NSRect(x: left, y: origin.y + used.midY.rounded(), width: right - left, height: 1).fill()
            }
        }
    }

    func drawChangeBars(in rect: NSRect, origin: NSPoint) {
        guard let storage = textStorage, let container = textContainers.first, storage.length > 0 else { return }
        let glyphs = glyphRange(forBoundingRect: rect.offsetBy(dx: -origin.x, dy: -origin.y), in: container)
        let shown = characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        let whole = NSRange(location: 0, length: storage.length)
        let x = origin.x + container.lineFragmentPadding - 16
        var location = shown.location
        while location < NSMaxRange(shown) {
            var range = NSRange()
            let value = storage.attribute(.kannaChange, at: location, longestEffectiveRange: &range, in: whole)
            location = max(location + 1, NSMaxRange(range))
            guard let kind = (value as? String).flatMap(ChangeKind.init) else { continue }
            let used = usedRect(for: range)
            switch kind {
            case .added: MarkdownTheme.addedBar.setFill()
            case .removed: MarkdownTheme.removedText.setFill()
            case .changed: MarkdownTheme.changedBar.setFill()
            }
            NSRect(x: x, y: origin.y + used.minY, width: 3, height: used.height).fill()
        }
    }

    private func usedRect(for range: NSRange) -> NSRect {
        let glyphs = glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        var rect = NSRect.null
        enumerateLineFragments(forGlyphRange: glyphs) { _, used, _, _, _ in rect = rect.union(used) }
        return rect.isNull ? .zero : rect
    }
}
