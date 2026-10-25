import AppKit

package enum DocTheme {
    package static let bar = color(dark: 0x202226, light: 0xf4f4f6)
    package static let panel = color(dark: 0x1c1d22, light: 0xf7f7f9)
    package static let card = color(dark: 0x23252b, light: 0xffffff)
    package static let line = color(dark: 0x2c2e34, light: 0xdedfe3)
    package static let edge = color(dark: 0x3a3d44, light: 0xc9cbd1)
    package static let text = color(dark: 0xe6e6e6, light: 0x1d1f23)
    package static let soft = color(dark: 0xc9ccd3, light: 0x3a3d44)
    package static let dim = color(dark: 0x8b8f98, light: 0x7a7e87)
    package static let accent = color(dark: 0x3478f6, light: 0x2f6fe4)
    package static let ok = color(dark: 0x4cb782, light: 0x2f9a63)
    package static let warn = color(dark: 0xd9a441, light: 0xb9801b)
    package static let agent = color(dark: 0xd18ae6, light: 0x9a45c4)
    package static let tint = color(dark: 0x3478f6, light: 0x2f6fe4, alpha: (0.2, 0.16))
    package static let tintOn = color(dark: 0x3478f6, light: 0x2f6fe4, alpha: (0.42, 0.34))
    package static let button = color(dark: 0x2a2c31, light: 0xffffff)
    package static let field = color(dark: 0x1b1c21, light: 0xf3f3f5)
    package static let resolvedTint = NSColor(srgbRed: 0.5, green: 0.5, blue: 0.5, alpha: 0.16)

    package static var sans: NSFont { NSFont.systemFont(ofSize: 13) }
    package static var small: NSFont { NSFont.systemFont(ofSize: 12) }
    package static var tiny: NSFont { NSFont.systemFont(ofSize: 11.5) }
    package static var mono: NSFont { NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular) }

    private static func color(dark: UInt32, light: UInt32, alpha: (CGFloat, CGFloat) = (1, 1)) -> NSColor {
        NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            let value = isDark ? dark : light
            return NSColor(srgbRed: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255,
                           blue: CGFloat(value & 0xff) / 255, alpha: isDark ? alpha.0 : alpha.1)
        }
    }

    package static func paragraph(_ lineHeight: CGFloat, size: CGFloat) -> NSParagraphStyle {
        let style = NSMutableParagraphStyle()
        style.minimumLineHeight = (size * lineHeight).rounded()
        style.maximumLineHeight = (size * lineHeight).rounded()
        return style
    }
}

@MainActor
package class FlippedView: NSView {
    package override var isFlipped: Bool { true }
    package var fill: NSColor? { didSet { needsDisplay = true } }
    package var stroke: NSColor? { didSet { needsDisplay = true } }
    package var radius: CGFloat = 0

    package override func draw(_ dirtyRect: NSRect) {
        if let fill {
            fill.setFill()
            NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
        }
        if let stroke {
            stroke.setStroke()
            NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius).stroke()
        }
    }
}

@MainActor
package final class Chip: FlippedView {
    package let label = NSTextField(labelWithString: "")
    package var onPress: (() -> Void)?
    package var on = false { didSet { paint() } }
    package var warn = false { didSet { paint() } }

    package init(_ text: String) {
        super.init(frame: .zero)
        radius = 5
        label.font = DocTheme.small
        label.lineBreakMode = .byTruncatingTail
        addSubview(label)
        set(text)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package func set(_ text: String) {
        label.stringValue = text
        label.sizeToFit()
        frame.size = NSSize(width: ceil(label.frame.width) + 18, height: 22)
        label.frame.origin = NSPoint(x: 9, y: ((22 - label.frame.height) / 2).rounded())
        paint()
    }

    private func paint() {
        stroke = warn ? DocTheme.warn : on ? DocTheme.accent : DocTheme.edge
        label.textColor = warn ? DocTheme.warn : on ? DocTheme.text : DocTheme.soft
    }

    package override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    package override func mouseDown(with event: NSEvent) {
        if let onPress { onPress() } else { super.mouseDown(with: event) }
    }
}

@MainActor
package final class DocBar: FlippedView {
    package let path = NSTextField(labelWithString: "")
    package let agent = Chip("")
    package let threads = Chip("Threads")
    package let diff = Chip("")

    package init() {
        super.init(frame: .zero)
        fill = DocTheme.bar
        path.font = DocTheme.mono
        path.textColor = DocTheme.soft
        path.lineBreakMode = .byTruncatingTail
        for view in [path, agent, threads, diff] { addSubview(view) }
        threads.on = true
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package override func layout() {
        super.layout()
        var x = bounds.width - 10
        for chip in [threads, agent] where !chip.isHidden {
            x -= chip.frame.width
            chip.frame.origin = NSPoint(x: x, y: 6)
            x -= 10
        }
        let room = diff.isHidden ? 0 : diff.frame.width + 10
        path.sizeToFit()
        path.frame = NSRect(x: 10, y: ((bounds.height - path.frame.height) / 2).rounded(), width: max(0, min(path.frame.width, x - 10 - room)), height: path.frame.height)
        diff.frame.origin = NSPoint(x: path.frame.maxX + 10, y: 6)
    }

    package override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        DocTheme.line.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }
}

@MainActor
package final class BoxField: NSTextView {
    package var onKey: ((NSEvent) -> Bool)?
    package var onText: (() -> Void)?
    package var placeholder = ""

    package convenience init(placeholder: String) {
        self.init(frame: NSRect(x: 0, y: 0, width: 100, height: 28))
        self.placeholder = placeholder
        font = DocTheme.sans
        textColor = DocTheme.text
        insertionPointColor = DocTheme.text
        drawsBackground = false
        isRichText = false
        allowsUndo = true
        textContainerInset = .zero
        textContainer?.lineFragmentPadding = 0
        isAutomaticQuoteSubstitutionEnabled = false
        isAutomaticDashSubstitutionEnabled = false
    }

    package override func keyDown(with event: NSEvent) {
        if onKey?(event) == true { return }
        super.keyDown(with: event)
    }

    package override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if window?.firstResponder === self, event.modifierFlags.contains(.command), onKey?(event) == true { return true }
        return super.performKeyEquivalent(with: event)
    }

    package override func didChangeText() {
        super.didChangeText()
        needsDisplay = true
        onText?()
    }

    package override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if string.isEmpty {
            (placeholder as NSString).draw(at: NSPoint(x: 0, y: 0), withAttributes: [.font: font ?? DocTheme.sans, .foregroundColor: DocTheme.dim])
        }
    }

    package var trimmed: String { string.trimmingCharacters(in: .whitespacesAndNewlines) }
}

@MainActor
package final class GrowField: NSScrollView {
    package static let most: CGFloat = 160
    package let field: BoxField
    package let least: CGFloat
    package var onGrow: (() -> Void)?

    package init(_ field: BoxField, width: CGFloat, least: CGFloat) {
        self.field = field
        self.least = least
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: least))
        drawsBackground = false
        borderType = .noBorder
        hasVerticalScroller = true
        hasHorizontalScroller = false
        autohidesScrollers = true
        scrollerStyle = .overlay
        field.frame = NSRect(x: 0, y: 0, width: width, height: least)
        field.minSize = NSSize(width: 0, height: least)
        field.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        field.isVerticallyResizable = true
        field.isHorizontallyResizable = false
        field.autoresizingMask = [.width]
        field.textContainer?.widthTracksTextView = true
        field.textContainer?.containerSize = NSSize(width: width, height: .greatestFiniteMagnitude)
        documentView = field
        field.onText = { [weak self] in self?.fit() }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package var wanted: CGFloat {
        guard let manager = field.layoutManager, let container = field.textContainer else { return least }
        manager.ensureLayout(for: container)
        let used = manager.usedRect(for: container).height + field.textContainerInset.height * 2
        return min(Self.most, max(least, ceil(used)))
    }

    package func fit() {
        let height = wanted
        guard height != frame.height else { return }
        frame.size.height = height
        field.scrollRangeToVisible(field.selectedRange())
        onGrow?()
    }
}

package enum DocWait: Equatable {
    case typing
    case note(String, warn: Bool)

    package static let grace: TimeInterval = 5

    package static func of(_ thread: DocThread, agent: String?, status: String?, now: Date = Date()) -> DocWait? {
        guard thread.isOpen, let last = thread.messages.last, last.who == "you" else { return nil }
        if thread.status == "unsent" { return .note("Not sent · \(thread.error ?? "no agent")", warn: false) }
        guard agent != nil else { return .note("Not answered · no agent", warn: false) }
        switch status {
        case "working": return .typing
        case "needs-you": return .note("Not answered · the agent is waiting on you", warn: true)
        default:
            if let at = DocStore.date(last.at), now.timeIntervalSince(at) < grace { return .typing }
            return .note("Not answered · the agent stopped", warn: false)
        }
    }

    package static func all(_ threads: [DocThread], agent: String?, status: String?, now: Date = Date()) -> [String: DocWait] {
        var out: [String: DocWait] = [:]
        for thread in threads { out[thread.id] = of(thread, agent: agent, status: status, now: now) }
        return out
    }
}

@MainActor
package final class TypingDots: NSView {
    private let dots = (0..<3).map { _ in CALayer() }
    private let size: CGFloat
    private let gap: CGFloat
    private let color: NSColor

    package init(size: CGFloat = 4, gap: CGFloat = 3, color: NSColor = DocTheme.dim) {
        self.size = size
        self.gap = gap
        self.color = color
        super.init(frame: NSRect(x: 0, y: 0, width: size * 3 + gap * 2, height: size))
        wantsLayer = true
        for (index, dot) in dots.enumerated() {
            dot.frame = CGRect(x: CGFloat(index) * (size + gap), y: 0, width: size, height: size)
            dot.cornerRadius = size / 2
            layer?.addSublayer(dot)
        }
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package static var still: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }
    package var animating: Bool { dots.first?.animation(forKey: "fade") != nil }

    private func paint() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            for dot in dots { dot.backgroundColor = color.cgColor }
        }
    }

    package override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    package override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        animate()
    }

    package func animate() {
        for (index, dot) in dots.enumerated() {
            dot.removeAnimation(forKey: "fade")
            dot.opacity = 0.6
            guard window != nil, !Self.still else { continue }
            let fade = CAKeyframeAnimation(keyPath: "opacity")
            fade.values = [0.25, 0.9, 0.25, 0.25]
            fade.keyTimes = [0, 0.3, 0.6, 1]
            fade.duration = 1.4
            fade.repeatCount = .infinity
            fade.beginTime = CACurrentMediaTime() + Double(index) * 0.15
            fade.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            dot.add(fade, forKey: "fade")
        }
    }
}

@MainActor
package func threadViews(_ thread: DocThread, wait: DocWait?, note: String?, width: CGFloat) -> [NSView] {
    var views: [NSView] = []
    if !thread.quote.isEmpty { views.append(quoteView(thread.quote, width: width)) }
    for message in thread.messages {
        let agent = message.who == "agent"
        let text = NSMutableAttributedString(string: agent ? "agent" : "you", attributes: [
            .font: NSFont.systemFont(ofSize: 12.5, weight: .semibold), .foregroundColor: agent ? DocTheme.agent : DocTheme.text,
        ])
        text.append(NSAttributedString(string: " " + message.text, attributes: [.font: NSFont.systemFont(ofSize: 12.5), .foregroundColor: DocTheme.soft]))
        text.addAttribute(.paragraphStyle, value: DocTheme.paragraph(1.4, size: 12.5), range: NSRange(location: 0, length: text.length))
        views.append(wrapped(text, width: width))
    }
    switch wait {
    case .typing:
        let holder = FlippedView(frame: NSRect(x: 0, y: 0, width: width, height: 12))
        let dots = TypingDots()
        dots.identifier = NSUserInterfaceItemIdentifier("typing")
        dots.frame.origin = NSPoint(x: 0, y: 4)
        dots.toolTip = "The agent is working"
        holder.addSubview(dots)
        views.append(holder)
    case .note(let text, let warn):
        let label = noteLabel(text, width: width)
        label.textColor = warn ? DocTheme.warn : DocTheme.dim
        label.identifier = NSUserInterfaceItemIdentifier("waiting")
        views.append(label)
    case nil: break
    }
    if let note { views.append(noteLabel(note, width: width)) }
    return views
}

@MainActor
private func quoteView(_ quote: String, width: CGFloat) -> NSView {
    let text = NSAttributedString(string: quote, attributes: [
        .font: DocTheme.small, .foregroundColor: DocTheme.dim, .paragraphStyle: DocTheme.paragraph(1.4, size: 12),
    ])
    let label = wrapped(text, width: width - 9)
    let holder = FlippedView(frame: NSRect(x: 0, y: 0, width: width, height: label.frame.height))
    label.frame.origin = NSPoint(x: 9, y: 0)
    holder.addSubview(label)
    let bar = FlippedView(frame: NSRect(x: 0, y: 0, width: 2, height: label.frame.height))
    bar.fill = DocTheme.accent
    holder.addSubview(bar)
    return holder
}

@MainActor
private func wrapped(_ text: NSAttributedString, width: CGFloat) -> NSTextField {
    let label = NSTextField(wrappingLabelWithString: "")
    label.attributedStringValue = text
    label.isSelectable = true
    label.frame.size = NSSize(width: width, height: ceil(label.sizeThatFits(NSSize(width: width, height: .greatestFiniteMagnitude)).height))
    return label
}

@MainActor
package func stack(_ views: [NSView], in host: NSView, x: CGFloat, y: CGFloat, gap: CGFloat) -> CGFloat {
    var y = y
    for (index, view) in views.enumerated() {
        if index > 0 { y += gap }
        view.frame.origin = NSPoint(x: x, y: y)
        host.addSubview(view)
        y += view.frame.height
    }
    return y
}

@MainActor
package func noteLabel(_ text: String, width: CGFloat) -> NSTextField {
    let label = NSTextField(wrappingLabelWithString: text)
    label.font = DocTheme.tiny
    label.textColor = DocTheme.warn
    label.frame.size = NSSize(width: width, height: ceil(label.sizeThatFits(NSSize(width: width, height: .greatestFiniteMagnitude)).height))
    return label
}

@MainActor
package final class ActButton: FlippedView {
    package let label = NSTextField(labelWithString: "")
    package var onPress: (() -> Void)?
    private let resolve: Bool
    private var hover = false

    package init(resolve: Bool) {
        self.resolve = resolve
        super.init(frame: NSRect(x: 0, y: 0, width: 26, height: 22))
        radius = 5
        label.stringValue = resolve ? "✓" : "↺"
        label.font = DocTheme.small
        label.alignment = .center
        label.frame = NSRect(x: 0, y: 3, width: 26, height: 16)
        addSubview(label)
        toolTip = resolve ? "Resolve" : "Reopen"
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package override func mouseEntered(with event: NSEvent) { hover = true; paint() }
    package override func mouseExited(with event: NSEvent) { hover = false; paint() }
    private func paint() {
        fill = DocTheme.button
        stroke = hover ? (resolve ? DocTheme.ok : DocTheme.accent) : DocTheme.edge
        label.textColor = hover ? (resolve ? DocTheme.ok : DocTheme.accent) : DocTheme.dim
    }

    package override func mouseDown(with event: NSEvent) { onPress?() }
}

@MainActor
package final class ReplyRow: FlippedView {
    package let field: BoxField
    package let grow: GrowField
    package let send: Chip
    package let act: ActButton
    package var onResize: (() -> Void)?
    private let box = FlippedView()

    package init(resolve: Bool, width: CGFloat) {
        let act = ActButton(resolve: resolve), send = Chip("⌘↵"), field = BoxField(placeholder: "Reply")
        send.fill = DocTheme.button
        send.label.textColor = DocTheme.text
        send.identifier = NSUserInterfaceItemIdentifier("send")
        act.identifier = NSUserInterfaceItemIdentifier("act")
        field.font = DocTheme.small
        let boxWidth = width - act.frame.width - 6 - send.frame.width - 6
        self.act = act
        self.send = send
        self.field = field
        grow = GrowField(field, width: boxWidth - 14, least: 16)
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 24))
        box.fill = DocTheme.field
        box.stroke = DocTheme.line
        box.radius = 5
        box.frame = NSRect(x: 0, y: 0, width: boxWidth, height: 24)
        grow.frame.origin = NSPoint(x: 7, y: 4)
        box.addSubview(grow)
        for view in [box, send, act] { addSubview(view) }
        grow.onGrow = { [weak self] in
            self?.place()
            self?.onResize?()
        }
        place()
    }

    private func place() {
        box.frame.size.height = grow.frame.height + 8
        frame.size.height = box.frame.height
        act.frame.origin = NSPoint(x: frame.width - act.frame.width, y: frame.height - 23)
        send.frame.origin = NSPoint(x: act.frame.minX - 6 - send.frame.width, y: frame.height - 23)
        box.needsDisplay = true
    }

    package var fieldFrame: NSRect { box.convert(grow.frame, to: self) }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package override func mouseDown(with event: NSEvent) {
        if box.frame.contains(convert(event.locationInWindow, from: nil)) { window?.makeFirstResponder(field) } else { super.mouseDown(with: event) }
    }
}

@MainActor
package final class CommentBox: FlippedView {
    package static let width: CGFloat = 360
    package let field: BoxField
    package let grow: GrowField
    package var threadID: String?
    package var onResize: ((CommentBox) -> Void)?
    private var row: ReplyRow?
    private var below: [NSView] = []
    private var fieldTop: CGFloat = 8

    package init(composer: Void, width: CGFloat = CommentBox.width) {
        field = BoxField(placeholder: "Comment")
        grow = GrowField(field, width: width - 16, least: 54)
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 100))
        decorate()
        grow.frame.origin = NSPoint(x: 8, y: 8)
        addSubview(grow)
        let hint = NSTextField(labelWithString: "⌘↵ posts · Esc cancels")
        hint.identifier = NSUserInterfaceItemIdentifier("hint")
        hint.font = DocTheme.tiny
        hint.textColor = DocTheme.dim
        hint.sizeToFit()
        let post = Chip("Post")
        post.fill = DocTheme.accent
        post.stroke = DocTheme.accent
        post.label.textColor = .white
        post.identifier = NSUserInterfaceItemIdentifier("post")
        post.frame.origin.x = width - 8 - post.frame.width
        hint.frame.origin.x = post.frame.minX - 6 - hint.frame.width
        hint.isHidden = hint.frame.minX < 8
        addSubview(hint)
        addSubview(post)
        below = [hint, post]
        grow.onGrow = { [weak self] in self?.regrow() }
        place()
    }

    package init(thread: DocThread, wait: DocWait?, note: String?, width: CGFloat = CommentBox.width) {
        let row = ReplyRow(resolve: thread.isOpen, width: width - 24)
        field = row.field
        grow = row.grow
        self.row = row
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 100))
        threadID = thread.id
        decorate()
        let y = stack(threadViews(thread, wait: wait, note: note, width: width - 24), in: self, x: 12, y: 10, gap: 6)
        row.frame.origin = NSPoint(x: 12, y: y + 10)
        addSubview(row)
        row.onResize = { [weak self] in self?.regrow() }
        place()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func place() {
        if let row {
            frame.size.height = row.frame.maxY + 10
            return
        }
        let bottom = fieldTop + grow.frame.height
        for view in below {
            view.frame.origin.y = bottom + 4 + ((22 - view.frame.height) / 2).rounded()
        }
        frame.size.height = bottom + 4 + 22 + 8
    }

    private func regrow() {
        place()
        needsDisplay = true
        onResize?(self)
    }

    private func decorate() {
        fill = DocTheme.card
        stroke = DocTheme.accent
        radius = 8
        wantsLayer = true
        shadow = {
            let shadow = NSShadow()
            shadow.shadowColor = NSColor.black.withAlphaComponent(0.28)
            shadow.shadowBlurRadius = 14
            shadow.shadowOffset = NSSize(width: 0, height: -8)
            return shadow
        }()
    }

    package func button(_ name: String) -> NSView? {
        subviews.first { $0.identifier?.rawValue == name } ?? row?.subviews.first { $0.identifier?.rawValue == name }
    }

    package var parts: [String: NSRect] {
        let fieldFrame = row.map { $0.convert($0.fieldFrame, to: self) } ?? grow.frame
        var parts = ["box": bounds, "field": fieldFrame, "text": NSRect(origin: fieldFrame.origin, size: field.frame.size)]
        for name in ["hint", "post"] { parts[name] = subviews.first { $0.identifier?.rawValue == name }.flatMap { $0.isHidden ? nil : $0.frame } }
        return parts
    }
}

public struct DocTourStep: Equatable, Sendable {
    public let thread: String
    public let at: Int
    public let of: Int

    public init(thread: String, at: Int, of: Int) {
        self.thread = thread
        self.at = at
        self.of = max(of, at)
    }

    public var last: Bool { at >= of }
}

@MainActor
package final class TourRow: FlippedView {
    package let step: DocTourStep
    package let back = Chip("‹ Back")
    package let next: Chip

    package init(step: DocTourStep, width: CGFloat, onStep: @escaping (String) -> Void) {
        self.step = step
        next = Chip(step.last ? "Done" : "Next ›")
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 30))
        identifier = NSUserInterfaceItemIdentifier("tour")
        let place = NSMutableAttributedString(string: "\(step.at) of \(step.of)", attributes: [.font: NSFont.systemFont(ofSize: 11, weight: .semibold), .foregroundColor: DocTheme.text])
        place.append(NSAttributedString(string: " · tour", attributes: [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: DocTheme.dim]))
        let label = NSTextField(labelWithAttributedString: place)
        label.sizeToFit()
        label.frame.origin = NSPoint(x: 0, y: 8 + ((22 - label.frame.height) / 2).rounded())
        next.on = true
        next.onPress = { onStep("next") }
        if step.at > 1 { back.onPress = { onStep("back") } } else { back.alphaValue = 0.35 }
        next.frame.origin = NSPoint(x: width - next.frame.width, y: 8)
        back.frame.origin = NSPoint(x: next.frame.minX - 6 - back.frame.width, y: 8)
        for view in [label, back, next] as [NSView] { addSubview(view) }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        DocTheme.line.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
    }
}

@MainActor
package final class ThreadCard: FlippedView {
    package let thread: DocThread
    package let row: ReplyRow
    package private(set) var tourRow: TourRow?
    package var field: BoxField { row.field }
    package var act: ActButton { row.act }
    package var grow: GrowField { row.grow }
    package var onSelect: (() -> Void)?
    package var onResize: (() -> Void)?
    private let active: Bool

    package init(thread: DocThread, active: Bool, wait: DocWait?, note: String?, width: CGFloat, tour: DocTourStep? = nil, onTour: @escaping (String) -> Void = { _ in }) {
        self.thread = thread
        self.active = active
        let inner = width - 20
        row = ReplyRow(resolve: thread.isOpen, width: inner)
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 10))
        radius = 8
        fill = DocTheme.card
        stroke = active ? DocTheme.accent : DocTheme.line
        var views = threadViews(thread, wait: wait, note: note, width: inner)
        if let tour {
            let made = TourRow(step: tour, width: inner, onStep: onTour)
            tourRow = made
            views.append(made)
        }
        let y = stack(views, in: self, x: 10, y: 9, gap: 6)
        row.frame.origin = NSPoint(x: 10, y: y + 8)
        addSubview(row)
        frame.size.height = row.frame.maxY + 9
        row.onResize = { [weak self] in
            guard let self else { return }
            frame.size.height = row.frame.maxY + 9
            needsDisplay = true
            onResize?()
        }
        alphaValue = thread.isOpen ? 1 : 0.55
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package var typing: Bool { descendant("typing") != nil }
    package var waitingText: String? { (descendant("waiting") as? NSTextField)?.stringValue }

    private func descendant(_ name: String) -> NSView? {
        var queue: [NSView] = subviews
        while !queue.isEmpty {
            let view = queue.removeFirst()
            if view.identifier?.rawValue == name { return view }
            queue += view.subviews
        }
        return nil
    }

    package override func hitTest(_ point: NSPoint) -> NSView? {
        let hit = super.hitTest(point)
        guard !active, let hit else { return hit }
        if let tourRow, hit.isDescendant(of: tourRow) { return hit }
        return hit === act || hit.isDescendant(of: act) ? hit : self
    }

    package override func mouseDown(with event: NSEvent) {
        if !active { onSelect?() }
    }
}

public struct CardState {
    public var rect: NSRect
    public var field: NSRect
    public var typing: Bool
    public var waiting: String?
    public var reply: Bool
}

@MainActor
package final class ThreadPanel: FlippedView {
    package static let width: CGFloat = 280
    package let scroll = NSScrollView()
    private let content = FlippedView()
    package var onSelect: ((String) -> Void)?
    package var onAct: ((DocThread) -> Void)?
    package var onReply: ((String, String) -> Void)?
    package var showResolved = false
    package var tour: DocTourStep?
    package var onTour: ((String) -> Void)?
    package var emptyText = "No comments yet. Select text and press ⌘↵, or ⌘-click a word."
    private(set) var cards: [ThreadCard] = []
    private var focusNext: String?
    private var last: (threads: [DocThread], active: String?, waits: [String: DocWait], notes: [String: String], showResolved: Bool, width: CGFloat, tour: DocTourStep?)?

    package init() {
        super.init(frame: .zero)
        fill = DocTheme.panel
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.documentView = content
        addSubview(scroll)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    package override func layout() {
        super.layout()
        scroll.frame = NSRect(x: 1, y: 0, width: bounds.width - 1, height: bounds.height)
        if let last, last.width != bounds.width { show(last.threads, active: last.active, waits: last.waits, notes: last.notes) }
    }

    package override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        DocTheme.line.setFill()
        NSRect(x: 0, y: 0, width: 1, height: bounds.height).fill()
    }

    package func card(_ id: String) -> ThreadCard? { cards.first { $0.thread.id == id } }

    package var cardStates: [String: CardState] {
        Dictionary(cards.map { card in
            (card.thread.id, CardState(rect: card.convert(card.bounds, to: nil), field: card.field.convert(card.field.bounds, to: nil), typing: card.typing,
                                       waiting: card.waitingText, reply: card.window?.firstResponder === card.field))
        }, uniquingKeysWith: { a, _ in a })
    }

    package func show(_ threads: [DocThread], active: String?, waits: [String: DocWait], notes: [String: String]) {
        let width = bounds.width - 1
        if let last, last.threads == threads, last.active == active, last.waits == waits, last.notes == notes, last.showResolved == showResolved, last.width == bounds.width, last.tour == tour { return }
        last = (threads, active, waits, notes, showResolved, bounds.width, tour)
        let drafts = Dictionary(cards.map { ($0.thread.id, $0.field.string) }, uniquingKeysWith: { a, _ in a })
        let focused = focusNext ?? cards.first { window?.firstResponder === $0.field }?.thread.id
        focusNext = nil
        content.subviews.forEach { $0.removeFromSuperview() }
        cards = []
        let open = threads.filter(\.isOpen), resolved = threads.filter { !$0.isOpen }
        let title = NSTextField(labelWithString: "Threads")
        title.font = DocTheme.tiny
        title.textColor = DocTheme.dim
        title.sizeToFit()
        title.frame.origin = NSPoint(x: 10, y: 10)
        content.addSubview(title)
        if !resolved.isEmpty {
            let link = NSTextField(labelWithString: "\(showResolved ? "Hide" : "Show") resolved (\(resolved.count))")
            link.font = DocTheme.tiny
            link.textColor = DocTheme.dim
            link.sizeToFit()
            let button = Clickable(frame: NSRect(x: width - 10 - link.frame.width, y: 10, width: link.frame.width, height: link.frame.height))
            link.frame.origin = .zero
            button.addSubview(link)
            button.onPress = { [weak self] in
                guard let self else { return }
                showResolved.toggle()
                last = nil
                show(threads, active: active, waits: waits, notes: notes)
            }
            content.addSubview(button)
        }
        var y = title.frame.maxY + 8
        let shown = open + (showResolved ? resolved : [])
        if shown.isEmpty {
            let empty = NSTextField(wrappingLabelWithString: emptyText)
            empty.font = DocTheme.small
            empty.textColor = DocTheme.dim
            empty.frame = NSRect(x: 10, y: y + 4, width: width - 20, height: ceil(empty.sizeThatFits(NSSize(width: width - 20, height: .greatestFiniteMagnitude)).height))
            content.addSubview(empty)
            y = empty.frame.maxY
        }
        for thread in shown {
            let step = tour.flatMap { $0.thread == thread.id ? $0 : nil }
            let card = ThreadCard(thread: thread, active: thread.id == active, wait: waits[thread.id], note: notes[thread.id], width: width - 20, tour: step) { [weak self] to in self?.onTour?(to) }
            card.frame.origin = NSPoint(x: 10, y: y)
            card.onSelect = { [weak self] in self?.onSelect?(thread.id) }
            card.act.onPress = { [weak self] in self?.onAct?(thread) }
            card.field.string = drafts[thread.id] ?? ""
            card.grow.fit()
            card.onResize = { [weak self] in self?.restack() }
            let submit: () -> Void = { [weak self, weak card] in
                guard let card else { return }
                let text = card.field.trimmed
                guard !text.isEmpty else { return }
                card.field.string = ""
                self?.focusNext = thread.id
                self?.onReply?(thread.id, text)
            }
            card.row.send.onPress = submit
            card.field.onKey = { event in
                guard event.keyCode == 36, !event.modifierFlags.contains(.shift) else { return false }
                submit()
                return true
            }
            content.addSubview(card)
            cards.append(card)
            y += card.frame.height + 8
        }
        content.frame = NSRect(x: 0, y: 0, width: width, height: max(y + 2, scroll.contentSize.height))
        if let focused, let card = cards.first(where: { $0.thread.id == focused }) {
            window?.makeFirstResponder(card.field)
            card.field.setSelectedRange(NSRange(location: (card.field.string as NSString).length, length: 0))
        }
    }

    private func restack() {
        guard var y = cards.first?.frame.minY else { return }
        for card in cards {
            card.frame.origin.y = y
            y += card.frame.height + 8
        }
        content.frame.size.height = max(y + 2, scroll.contentSize.height)
        if let card = cards.first(where: { window?.firstResponder === $0.field }) { content.scrollToVisible(card.frame) }
    }

    package func reveal(_ id: String) {
        guard let card = card(id) else { return }
        content.scrollToVisible(card.frame)
    }
}

@MainActor
package final class Clickable: FlippedView {
    package var onPress: (() -> Void)?
    package override func mouseDown(with event: NSEvent) { onPress?() }
    package override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }
}
