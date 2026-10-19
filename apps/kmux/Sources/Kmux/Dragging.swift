import AppKit
import KmuxCore

// Dragging, as in the model (reference/kmux/index.html): dividers resize
// with snapping, the ⋯ grip moves a pane, and tabs reorder or move.

/// Where an event happened, in screen coordinates.
@MainActor func screenPoint(_ event: NSEvent) -> NSPoint {
    event.window.map { $0.convertPoint(toScreen: event.locationInWindow) } ?? event.locationInWindow
}

/// Mouse positions for the next drag to follow instead of the mouse (debug.drag).
@MainActor var scriptedDrag: [NSPoint]?

/// Follows the mouse until the button comes up: calls `moved` with each
/// position (screen coordinates) and returns where the button was released.
@MainActor func trackMouse(_ moved: (NSPoint) -> Void) -> NSPoint {
    if let points = scriptedDrag, let end = points.last {
        scriptedDrag = nil
        points.dropLast().forEach(moved)
        return end
    }
    while let event = NSApp.nextEvent(matching: [.leftMouseDragged, .leftMouseUp], until: .distantFuture, inMode: .eventTracking, dequeue: true) {
        let point = screenPoint(event)
        if event.type == .leftMouseUp { return point }
        moved(point)
    }
    return NSEvent.mouseLocation
}

/// A translucent highlight over where a drag will drop: a pane's half, a tab,
/// or the outline of a new window. It floats above every window.
@MainActor
final class DropHint {
    private let panel: NSPanel
    private let box = NSView()
    private let label = NSTextField(labelWithString: "")

    /// Above every window being dropped onto (tests raise both).
    static var level = NSWindow.Level.floating
    /// Tests drag without showing anything.
    static var quiet = false

    init() {
        panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        panel.level = Self.level
        box.wantsLayer = true
        box.layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.22).cgColor
        box.layer?.borderColor = NSColor.controlAccentColor.cgColor
        box.layer?.borderWidth = 2
        label.font = .monospacedSystemFont(ofSize: 12, weight: .semibold)
        label.textColor = .controlAccentColor
        label.alignment = .center
        box.addSubview(label)
        panel.contentView = box
    }

    var windowNumber: Int { panel.windowNumber }

    func show(_ rect: NSRect, text: String = "", rounded: Bool = false) {
        guard !Self.quiet else { return }
        panel.setFrame(rect, display: false)
        box.layer?.cornerRadius = rounded ? 10 : 2
        label.stringValue = text
        label.sizeToFit()
        label.frame.origin = NSPoint(x: (rect.width - label.frame.width) / 2, y: (rect.height - label.frame.height) / 2)
        panel.orderFront(nil)
    }

    func hide() { panel.orderOut(nil) }
}

/// The handle over a divider between two panes. Dragging it moves the split
/// between them, snapping to ¼ ⅓ ½ ⅔ ¾, with their shares shown beside the mouse.
@MainActor
final class DividerView: NSView {
    static let grab: CGFloat = 3 // extra reach either side of the line
    static let nice: [Double] = [1 / 4, 1 / 3, 1 / 2, 2 / 3, 3 / 4]
    static let minimum: CGFloat = 80

    let split: Split
    /// The tab, and the way to the split from its root, for `resize`.
    let tab: String
    let path: [Int]
    let index: Int
    /// Where the pair of panes starts, and its length, along the split.
    private let start: CGFloat
    private let span: CGFloat
    /// The first pane's share while the divider is dragged.
    var onPreview: ((Double) -> Void)?
    /// Dropped: the `resize` request's arguments.
    var onResize: ((JSON) -> Void)?

    init(split: Split, tab: String, path: [Int], index: Int, line: NSRect, first: NSRect, second: NSRect) {
        self.split = split
        self.tab = tab
        self.path = path
        self.index = index
        let row = split.axis == .row
        start = row ? first.minX : first.maxY
        span = row ? second.maxX - first.minX : first.maxY - second.minY
        super.init(frame: row ? line.insetBy(dx: -Self.grab, dy: 0) : line.insetBy(dx: 0, dy: -Self.grab))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: split.axis == .row ? .resizeLeftRight : .resizeUpDown)
    }

    override func mouseDown(with event: NSEvent) {
        guard let stage = superview else { return }
        onResize?(drag(in: stage))
    }

    /// The point `fraction` of the way along the pair of panes (stage coordinates).
    func position(_ fraction: CGFloat) -> NSPoint {
        split.axis == .row ? NSPoint(x: start + span * fraction, y: frame.midY) : NSPoint(x: frame.midX, y: start - span * fraction)
    }

    /// The share the first pane gets when the divider is at `position` (stage coordinates).
    func share(at position: NSPoint) -> Double {
        let row = split.axis == .row
        let pair = split.kids[index].size + split.kids[index + 1].size
        let low = min(Self.minimum, span / 2)
        let offset = min(max(row ? position.x - start : start - position.y, low), span - low)
        let share = pair * Double(offset / span)
        return Self.nice.first { abs(share - $0) < 0.015 && $0 < pair } ?? share
    }

    /// Follows the mouse until it is let go, and returns the `resize` that
    /// puts the divider there.
    func drag(in stage: NSView) -> JSON {
        var last = split.kids[index].size
        let tip = NSTextField(labelWithString: "")
        tip.font = .monospacedSystemFont(ofSize: 11, weight: .medium)
        tip.textColor = .white
        tip.drawsBackground = true
        tip.backgroundColor = NSColor.black.withAlphaComponent(0.75)
        stage.addSubview(tip)
        let move = { [self] (point: NSPoint) in
            guard let window = stage.window else { return }
            let local = stage.convert(window.convertPoint(fromScreen: point), from: nil)
            let pair = split.kids[index].size + split.kids[index + 1].size
            let share = share(at: local)
            last = share
            onPreview?(share)
            tip.stringValue = " \(Fraction.format(share)) \(split.axis == .row ? "|" : "/") \(Fraction.format(pair - share)) "
            tip.sizeToFit()
            tip.frame.origin = NSPoint(x: local.x + 12, y: local.y - 12 - tip.frame.height)
            stage.addSubview(tip) // keep it above panes placed since
        }
        move(trackMouse(move))
        tip.removeFromSuperview()
        return ["tab": .string(tab), "path": .array(path.map { .number(Double($0)) }), "index": .number(Double(index)), "share": .number(last)]
    }
}

@MainActor
final class DockDividerView: NSView {
    let state: Window
    private let span: CGFloat
    private let extent: CGFloat
    /// The dock's width while its divider is dragged.
    var onPreview: ((Double) -> Void)?
    /// Dropped: the `resize` request's arguments.
    var onResize: ((JSON) -> Void)?

    init(window: Window, line: NSRect, span: CGFloat, extent: CGFloat) {
        state = window
        self.span = span
        self.extent = extent
        super.init(frame: line.insetBy(dx: -DividerView.grab, dy: 0))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var paneID: String? { state.dock?.pane }

    override func resetCursorRects() { addCursorRect(bounds, cursor: .resizeLeftRight) }

    override func mouseDown(with event: NSEvent) {
        guard let content = superview else { return }
        onResize?(drag(in: content))
    }

    func position(_ fraction: CGFloat) -> NSPoint {
        let left = state.dock?.edge != .right
        let offset = span * fraction + (left ? 0 : WindowController.divider)
        return NSPoint(x: left ? offset : extent - offset, y: frame.midY)
    }

    func width(at point: NSPoint) -> Double {
        let left = state.dock?.edge != .right
        let offset = left ? point.x : extent - WindowController.divider - point.x
        let low = min(DividerView.minimum, span / 2)
        let width = Double(min(max(offset, low), span - low) / span)
        return (DividerView.nice + [Dock.defaultWidth]).first { abs(width - $0) < 0.015 } ?? width
    }

    /// Follows the mouse until it is let go, and returns the `resize` that
    /// gives the dock that width.
    func drag(in content: NSView) -> JSON {
        var last = state.dock?.width ?? Dock.defaultWidth
        let tip = NSTextField(labelWithString: "")
        tip.font = .monospacedSystemFont(ofSize: 11, weight: .medium)
        tip.textColor = .white
        tip.drawsBackground = true
        tip.backgroundColor = NSColor.black.withAlphaComponent(0.75)
        content.addSubview(tip)
        let move = { [self] (point: NSPoint) in
            guard let window = content.window, state.dock != nil else { return }
            let local = content.convert(window.convertPoint(fromScreen: point), from: nil)
            let width = width(at: local)
            last = width
            onPreview?(width)
            let left = state.dock?.edge != .right
            tip.stringValue = " \(Fraction.format(left ? width : 1 - width)) | \(Fraction.format(left ? 1 - width : width)) "
            tip.sizeToFit()
            tip.frame.origin = NSPoint(x: local.x + 12, y: local.y - 12 - tip.frame.height)
            content.addSubview(tip)
        }
        move(trackMouse(move))
        tip.removeFromSuperview()
        return ["pane": .string(state.dock?.pane ?? ""), "size": .number(last)]
    }
}

/// The ⋯ handle at the top of a pane, shown while the mouse is over the pane.
/// Dragging it moves the pane.
@MainActor
final class PaneGrip: NSView {
    static let size = NSSize(width: 34, height: 14)
    var onDrag: ((NSEvent) -> Void)?
    /// The grip is drawn only while the mouse is over it (or dragging it).
    private var hovered = false { didSet { needsDisplay = true } }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        toolTip = "Drag to move this pane"
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        // A tracking area made while the mouse is already inside never reports
        // the exit unless told so, and the grip would stay up (e.g. after the
        // pane's layout was rebuilt under the mouse).
        let inside = window.map { bounds.contains(convert($0.mouseLocationOutsideOfEventStream, from: nil)) } ?? false
        var options: NSTrackingArea.Options = [.mouseEnteredAndExited, .activeAlways, .inVisibleRect]
        if inside { options.insert(.assumeInside) }
        addTrackingArea(NSTrackingArea(rect: bounds, options: options, owner: self))
        hovered = inside
    }

    override func mouseEntered(with event: NSEvent) { hovered = true }
    override func mouseExited(with event: NSEvent) { hovered = false }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .openHand) }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        NSCursor.closedHand.push()
        defer { NSCursor.pop() }
        onDrag?(event)
    }

    override func draw(_ dirtyRect: NSRect) {
        guard hovered else { return }
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 0, dy: -6).offsetBy(dx: 0, dy: 6), xRadius: 6, yRadius: 6)
        NSColor.controlAccentColor.setFill()
        shape.fill()
        NSColor.white.setFill()
        for i in -1...1 {
            NSBezierPath(ovalIn: NSRect(x: bounds.midX + CGFloat(i) * 6 - 1.5, y: bounds.midY - 1, width: 3, height: 3)).fill()
        }
    }
}
