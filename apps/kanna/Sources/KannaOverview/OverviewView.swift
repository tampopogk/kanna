import AppKit

@MainActor
public final class OverviewView: NSView {
    public var onScroll: ((CGFloat) -> Void)?
    public weak var scrollTarget: NSView?
    public var background: NSColor = .textBackgroundColor { didSet { needsDisplay = true } }
    public var edge: NSColor = .separatorColor { didSet { needsDisplay = true } }

    public var content: OverviewContent = .empty {
        didSet {
            tiles = [:]
            needsDisplay = true
        }
    }

    public var marks: [OverviewMark] = [] {
        didSet { if marks != oldValue { needsDisplay = true } }
    }

    public var viewport = OverviewViewport(documentHeight: 0, contentHeight: 0, top: 0, height: 0) {
        didSet {
            guard viewport != oldValue else { return }
            if viewport.top != oldValue.top { scrolled() }
            needsDisplay = true
        }
    }

    public private(set) var hovering = false { didSet { if hovering != oldValue { needsDisplay = true } } }
    public private(set) var dragging = false { didSet { if dragging != oldValue { needsDisplay = true } } }
    public private(set) var renderedTiles = 0

    static let tileHeight: CGFloat = 256
    private var tiles: [Int: NSImage] = [:]
    private var lastScroll: Date = .distantPast
    private var fade: DispatchWorkItem?
    private var drag: (start: CGFloat, y: CGFloat, rate: CGFloat)?
    private var tracking: NSTrackingArea?

    public override var isFlipped: Bool { true }

    public var geometry: OverviewGeometry { OverviewGeometry(viewport: viewport, scale: content.scale, height: bounds.height) }

    var thumbX: CGFloat { 1 }
    var marksX: CGFloat { 1 + OverviewMetrics.thumbWidth + 1 }

    public var thumbShown: Bool { hovering || dragging || Date().timeIntervalSince(lastScroll) < 1 }

    public var debugState: [String: Double] {
        let g = geometry
        return ["offset": Double(g.offset), "box_y": Double(g.boxY), "box_height": Double(g.boxHeight), "scale": Double(g.scale),
                "height": Double(bounds.height), "width": Double(bounds.width), "marks": Double(marks.count), "tiles": Double(tiles.count),
                "rendered_tiles": Double(renderedTiles), "box_shown": hovering || dragging ? 1 : 0, "document_height": Double(viewport.documentHeight),
                "top": Double(viewport.top)]
    }

    public var debugMarks: [String: Int] {
        Dictionary(grouping: marks, by: \.kind.rawValue).mapValues(\.count)
    }

    private func scrolled() {
        lastScroll = Date()
        fade?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.needsDisplay = true }
        fade = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.05, execute: work)
    }

    public override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        tiles = [:]
        needsDisplay = true
    }

    public override func updateTrackingAreas() {
        super.updateTrackingAreas()
        tracking.map(removeTrackingArea)
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(area)
        tracking = area
    }

    public func hover(_ on: Bool) { hovering = on }

    public override func mouseEntered(with event: NSEvent) { hovering = true }
    public override func mouseExited(with event: NSEvent) { hovering = false }

    public override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    public override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        press(at: point)
    }

    public override func mouseDragged(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        move(to: point)
    }

    public override func mouseUp(with event: NSEvent) { release() }

    public func press(at point: NSPoint) {
        let g = geometry
        dragging = true
        if point.x < marksX {
            let start = g.boxContains(point.y) ? viewport.top : g.top(clickingThumbAt: point.y)
            if start != viewport.top { onScroll?(start) }
            drag = (start, point.y, g.dragRate)
        } else {
            let start = g.top(clickingMarksAt: point.y)
            onScroll?(start)
            drag = (start, point.y, g.marksDragRate)
        }
    }

    public func move(to point: NSPoint) {
        guard let drag else { return }
        onScroll?(geometry.top(dragFrom: drag.start, by: point.y - drag.y, rate: drag.rate))
    }

    public func release() {
        drag = nil
        dragging = false
    }

    public override func scrollWheel(with event: NSEvent) {
        if let scrollTarget { scrollTarget.scrollWheel(with: event) } else { super.scrollWheel(with: event) }
    }

    public override func draw(_ dirtyRect: NSRect) {
        background.setFill()
        bounds.fill()
        edge.setFill()
        NSRect(x: 0, y: 0, width: 1, height: bounds.height).fill()
        NSRect(x: marksX - 1, y: 0, width: 1, height: bounds.height).fill()
        let g = geometry
        drawThumb(g)
        if hovering || dragging {
            NSColor(white: isDark ? 1 : 0, alpha: dragging ? (isDark ? 0.2 : 0.16) : (isDark ? 0.12 : 0.1)).setFill()
            NSRect(x: thumbX, y: g.boxY, width: OverviewMetrics.thumbWidth, height: max(2, g.boxHeight)).fill(using: .sourceOver)
        }
        drawMarks(g)
    }

    private var isDark: Bool { effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }

    private func drawThumb(_ g: OverviewGeometry) {
        NSGraphicsContext.saveGraphicsState()
        NSRect(x: thumbX, y: 0, width: OverviewMetrics.thumbWidth, height: bounds.height).clip()
        switch content {
        case .empty:
            break
        case .image(let image, let scale):
            let size = NSSize(width: OverviewMetrics.thumbWidth, height: viewport.documentHeight * scale)
            NSImage(cgImage: image, size: size).draw(in: NSRect(x: thumbX, y: -g.offset, width: size.width, height: size.height), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        case .text:
            let first = Int((g.offset / Self.tileHeight).rounded(.down))
            let last = Int(((g.offset + bounds.height) / Self.tileHeight).rounded(.down))
            let limit = Int((g.thumbHeight / Self.tileHeight).rounded(.down))
            for index in max(0, first)...max(0, min(last, limit)) {
                let image = tile(index)
                image.draw(in: NSRect(x: thumbX, y: CGFloat(index) * Self.tileHeight - g.offset, width: OverviewMetrics.thumbWidth, height: Self.tileHeight),
                           from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            }
        }
        NSGraphicsContext.restoreGraphicsState()
    }

    private func tile(_ index: Int) -> NSImage {
        if let made = tiles[index] { return made }
        let made = render(tile: index)
        tiles[index] = made
        renderedTiles += 1
        return made
    }

    private func render(tile index: Int) -> NSImage {
        let size = NSSize(width: OverviewMetrics.thumbWidth, height: Self.tileHeight)
        let backing = window?.backingScaleFactor ?? 2
        let image = NSImage(size: size)
        guard case .text(let sides, _) = content,
              let context = CGContext(data: nil, width: Int(size.width * backing), height: Int(size.height * backing), bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return image }
        context.translateBy(x: 0, y: size.height * backing)
        context.scaleBy(x: backing, y: -backing)
        let scale = content.scale
        let top = CGFloat(index) * Self.tileHeight
        effectiveAppearance.performAsCurrentDrawingAppearance {
            var colors: [ObjectIdentifier: CGColor] = [:]
            func cg(_ color: NSColor) -> CGColor {
                if let found = colors[ObjectIdentifier(color)] { return found }
                let made = color.cgColor
                colors[ObjectIdentifier(color)] = made
                return made
            }
            let sideWidth = (size.width / CGFloat(max(sides.count, 1))).rounded(.down)
            for (number, side) in sides.enumerated() {
                let left = CGFloat(number) * sideWidth
                let right = number == sides.count - 1 ? size.width : left + sideWidth - 1
                let text = side.string.string as NSString
                var at = Self.firstLine(in: side.lines, below: top / scale)
                while at < side.lines.count {
                    let line = side.lines[at]
                    at += 1
                    let y = line.y * scale - top
                    if y >= size.height { break }
                    let height = max(line.height * scale, OverviewMetrics.lineHeight)
                    if let background = line.background {
                        context.setFillColor(cg(background))
                        context.fill(CGRect(x: left, y: y, width: right - left, height: height))
                    }
                    guard line.range.length > 0, NSMaxRange(line.range) <= text.length else { continue }
                    var column: CGFloat = 0
                    side.string.enumerateAttribute(.foregroundColor, in: line.range) { value, run, stop in
                        let color = cg(value as? NSColor ?? NSColor.textColor)
                        for offset in run.location..<NSMaxRange(run) {
                            let x = left + 1 + column
                            if x >= right { stop.pointee = true; return }
                            let unit = text.character(at: offset)
                            switch unit {
                            case 0x20: column += 1
                            case 0x09: column += 4
                            case 0x0a, 0x0d: break
                            case 0xDC00...0xDFFF: break
                            default:
                                let strong = (unit >= 0x30 && unit <= 0x39) || (unit >= 0x41 && unit <= 0x5a) || (unit >= 0x61 && unit <= 0x7a) || unit > 0x7f
                                context.setFillColor(color.copy(alpha: color.alpha * (strong ? 0.85 : 0.5)) ?? color)
                                context.fill(CGRect(x: x, y: y + 0.25, width: 1, height: OverviewMetrics.lineHeight - 0.5))
                                column += 1
                            }
                        }
                    }
                }
            }
        }
        if let made = context.makeImage() { image.addRepresentation(NSBitmapImageRep(cgImage: made)) }
        image.size = size
        return image
    }

    static func firstLine(in lines: [OverviewLine], below y: CGFloat) -> Int {
        var low = 0
        var high = lines.count
        while low < high {
            let mid = (low + high) / 2
            if lines[mid].y + lines[mid].height <= y { low = mid + 1 } else { high = mid }
        }
        return low
    }

    private func drawMarks(_ g: OverviewGeometry) {
        let scale = g.marksScale
        let lane = (OverviewMetrics.marksWidth - 2) / 2
        for mark in marks {
            mark.color.setFill()
            let x = marksX + 1 + CGFloat(mark.kind.lane) * lane
            NSRect(x: x, y: mark.y * scale, width: lane, height: max(2, mark.height * scale)).fill()
        }
        guard thumbShown, viewport.documentHeight > viewport.height else { return }
        NSColor(white: isDark ? 1 : 0, alpha: isDark ? 0.22 : 0.2).setFill()
        let rect = NSRect(x: marksX + 1, y: viewport.top * scale, width: OverviewMetrics.marksWidth - 2, height: max(8, viewport.height * scale))
        NSBezierPath(roundedRect: rect, xRadius: 2, yRadius: 2).fill()
    }
}
