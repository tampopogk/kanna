import AppKit

@MainActor
final class TabBar: NSView {
    static let height: CGFloat = 34

    var editing: Bool { items.contains { $0.isEditing } }
    var onSelect: ((String) -> Void)?
    var onClose: ((String) -> Void)?
    var onNew: (() -> Void)?
    var onRename: ((String, String) -> Void)?
    var onEditEnded: (() -> Void)?
    var onDrag: ((String, CGFloat) -> Void)?
    var onMoveToNewWindow: ((String) -> Void)?
    var onPage: ((String) -> Void)?
    var onCycle: ((Int) -> Void)?
    private var shown: [(id: String, title: String, active: Bool)] = []
    private var shownPages: [String] = []
    private var shownPage: String?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        paint()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
        needsDisplay = true
        subviews.forEach { $0.needsDisplay = true }
    }

    private func paint() { layer?.backgroundColor = resolved(.windowBackgroundColor) }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func show(_ window: JSON, pages: [String] = [], page: String? = nil) {
        guard case .array(let list)? = window["tabs"] else { return }
        let active = page == nil ? window["tab"]?.string : nil
        let tabs = list.compactMap { tab in tab["id"]?.string.map { (id: $0, title: tab["title"]?.string ?? $0, active: $0 == active) } }
        guard tabs.map(\.id) != shown.map(\.id) || tabs.map(\.title) != shown.map(\.title) || tabs.map(\.active) != shown.map(\.active)
            || pages != shownPages || page != shownPage else { return }
        shown = tabs
        shownPages = pages
        shownPage = page
        subviews.forEach { $0.removeFromSuperview() }
        for name in pages {
            let item = PageItem(title: name, active: name == page)
            item.onSelect = { [weak self] in self?.onPage?(name) }
            item.onCycle = { [weak self] step in self?.onCycle?(step) }
            addSubview(item)
        }
        for tab in tabs {
            let item = TabItem(id: tab.id, title: tab.title, active: tab.active)
            item.onSelect = { [weak self] in self?.onSelect?(tab.id) }
            item.onClose = { [weak self] in self?.onClose?(tab.id) }
            item.onRename = { [weak self] title in self?.onRename?(tab.id, title) }
            item.onEditEnded = { [weak self] in self?.onEditEnded?() }
            item.onDrag = { [weak self] grab in self?.onDrag?(tab.id, grab) }
            item.onMoveToNewWindow = { [weak self] in self?.onMoveToNewWindow?(tab.id) }
            addSubview(item)
        }
        let plus = NSButton(title: "+", target: self, action: #selector(newTab))
        plus.isBordered = false
        plus.font = .systemFont(ofSize: 15)
        plus.contentTintColor = .secondaryLabelColor
        plus.frame.size = NSSize(width: 22, height: 22)
        plus.toolTip = "New tab"
        addSubview(plus)
        self.plus = plus
        place(animated: false)
    }

    struct Gap: Equatable {
        let owner: ObjectIdentifier
        let hiding: String
        let width: CGFloat
        let index: Int?
    }

    private(set) var gap: Gap?

    func setGap(_ gap: Gap?, animated: Bool) {
        guard gap != self.gap else { return }
        self.gap = gap
        place(animated: animated)
    }

    func clearGap(owner: ObjectIdentifier, animated: Bool) {
        guard gap?.owner == owner else { return }
        setGap(nil, animated: animated)
    }

    private var tabsStart: CGFloat {
        subviews.compactMap { $0 as? PageItem }.reduce(8) { $0 + $1.frame.width + 2 }
    }

    private func others(hiding: String?) -> [TabItem] { items.filter { $0.id != hiding } }

    func gapIndex(center: CGFloat, width: CGFloat, hiding: String?) -> Int {
        var x = tabsStart
        for (i, item) in others(hiding: hiding).enumerated() {
            if center <= x + (item.frame.width + width) / 2 { return i }
            x += item.frame.width + 2
        }
        return others(hiding: hiding).count
    }

    func gapRect(_ index: Int, width: CGFloat, hiding: String?) -> NSRect {
        let x = others(hiding: hiding).prefix(index).reduce(tabsStart) { $0 + $1.frame.width + 2 }
        return NSRect(x: x, y: 0, width: width, height: TabBar.height)
    }

    func tabsSpan(hiding: String?) -> ClosedRange<CGFloat> {
        tabsStart...others(hiding: hiding).reduce(tabsStart) { $0 + $1.frame.width + 2 }
    }

    func index(of id: String) -> Int? { items.firstIndex { $0.id == id } }

    func frame(of id: String) -> NSRect? { items.first { $0.id == id }?.frame }

    func image(of id: String) -> NSImage? {
        guard let item = items.first(where: { $0.id == id }), let rep = item.bitmapImageRepForCachingDisplay(in: item.bounds) else { return nil }
        item.cacheDisplay(in: item.bounds, to: rep)
        let image = NSImage(size: item.bounds.size)
        image.addRepresentation(rep)
        return image
    }

    private func place(animated: Bool) {
        var x: CGFloat = 8
        var moves: [(NSView, NSPoint)] = []
        for page in subviews.compactMap({ $0 as? PageItem }) {
            moves.append((page, NSPoint(x: x, y: 0)))
            x += page.frame.width + 2
        }
        var shown = 0
        for item in items {
            item.isHidden = item.id == gap?.hiding
            if item.isHidden { continue }
            if let gap, shown == gap.index { x += gap.width + 2 }
            moves.append((item, NSPoint(x: x, y: 0)))
            x += item.frame.width + 2
            shown += 1
        }
        if let gap, gap.index == shown { x += gap.width + 2 }
        if let plus { moves.append((plus, NSPoint(x: x + 4, y: (TabBar.height - 22) / 2))) }
        guard animated else { return moves.forEach { $0.0.setFrameOrigin($0.1) } }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.16
            context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0.8, 0.2, 1)
            context.allowsImplicitAnimation = true
            for (view, origin) in moves where view.frame.origin != origin { view.animator().setFrameOrigin(origin) }
        }
    }

    var pageItems: [(title: String, center: NSPoint)] {
        subviews.compactMap { $0 as? PageItem }.map { ($0.title, $0.convert(NSPoint(x: $0.bounds.midX, y: $0.bounds.midY), to: nil)) }
    }

    private weak var plus: NSButton?
    var plusCenter: NSPoint { plus.map { NSPoint(x: $0.frame.midX, y: $0.frame.midY) } ?? .zero }
    private var items: [TabItem] { subviews.compactMap { $0 as? TabItem } }

    func target(at point: NSPoint) -> (tab: String?, frame: NSRect)? {
        if let item = items.first(where: { $0.frame.contains(point) }) { return (item.id, item.frame) }
        if let plus, plus.frame.insetBy(dx: -4, dy: -3).contains(point) { return (nil, plus.frame) }
        return nil
    }

    func center(of id: String) -> NSPoint? {
        guard let item = subviews.compactMap({ $0 as? TabItem }).first(where: { $0.id == id }) else { return nil }
        return item.convert(NSPoint(x: item.bounds.midX - 8, y: item.bounds.midY), to: nil)
    }

    @objc private func newTab() { onNew?() }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
    }
}

@MainActor
private final class PageItem: NSView {
    let title: String
    var onSelect: (() -> Void)?
    var onCycle: ((Int) -> Void)?
    private let active: Bool

    override var acceptsFirstResponder: Bool { true }
    override var canBecomeKeyView: Bool { true }

    override func keyDown(with event: NSEvent) {
        switch event.keyCode {
        case 123: onCycle?(-1)
        case 124: onCycle?(1)
        case 36, 49, 76: onSelect?()
        default: super.keyDown(with: event)
        }
    }

    init(title: String, active: Bool) {
        self.title = title
        self.active = active
        let label = NSTextField(labelWithString: title)
        label.font = .systemFont(ofSize: 12, weight: active ? .semibold : .regular)
        label.textColor = active ? .labelColor : .secondaryLabelColor
        label.sizeToFit()
        super.init(frame: NSRect(x: 0, y: 0, width: label.frame.width + 24, height: TabBar.height))
        label.frame.origin = NSPoint(x: 12, y: (TabBar.height - label.frame.height) / 2)
        addSubview(label)
        setAccessibilityRole(.radioButton)
        setAccessibilityLabel(title)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draw(_ dirtyRect: NSRect) {
        guard active else { return }
        NSColor.controlAccentColor.setFill()
        NSRect(x: 6, y: 1, width: bounds.width - 12, height: 2).fill()
    }

    override func mouseDown(with event: NSEvent) { onSelect?() }
}

@MainActor
private final class TabItem: NSView, NSTextFieldDelegate {
    let id: String
    var onSelect: (() -> Void)?
    var onClose: (() -> Void)?
    var onRename: ((String) -> Void)?
    var onEditEnded: (() -> Void)?
    var onDrag: ((CGFloat) -> Void)?
    var onMoveToNewWindow: (() -> Void)?
    private let active: Bool
    private let label: NSTextField
    private var editor: NSTextField?
    var isEditing: Bool { editor != nil }
    private var cancelled = false

    init(id: String, title: String, active: Bool) {
        self.id = id
        self.active = active
        label = NSTextField(labelWithString: title)
        label.font = .systemFont(ofSize: 12, weight: active ? .semibold : .regular)
        label.textColor = active ? .labelColor : .secondaryLabelColor
        label.lineBreakMode = .byTruncatingTail
        label.sizeToFit()
        let width = min(max(label.frame.width, 40), 220)
        super.init(frame: NSRect(x: 0, y: 0, width: width + 34, height: TabBar.height))
        label.frame = NSRect(x: 10, y: (TabBar.height - label.frame.height) / 2, width: width, height: label.frame.height)
        addSubview(label)
        let close = NSButton(title: "×", target: self, action: #selector(closeClicked))
        close.isBordered = false
        close.font = .systemFont(ofSize: 13)
        close.contentTintColor = .tertiaryLabelColor
        close.frame = NSRect(x: width + 14, y: (TabBar.height - 18) / 2, width: 16, height: 18)
        close.toolTip = "Close tab"
        addSubview(close)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draw(_ dirtyRect: NSRect) {
        guard active else { return }
        NSColor.controlAccentColor.setFill()
        NSRect(x: 6, y: 1, width: bounds.width - 12, height: 2).fill()
    }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { return startEditing() }
        let origin = screenPoint(event)
        let grab = convert(event.locationInWindow, from: nil).x
        while let next = NSApp.nextEvent(matching: [.leftMouseDragged, .leftMouseUp], until: .distantFuture, inMode: .eventTracking, dequeue: true) {
            if next.type == .leftMouseUp { break }
            let point = screenPoint(next)
            if hypot(point.x - origin.x, point.y - origin.y) >= 5 {
                onDrag?(grab)
                return
            }
        }
        onSelect?()
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let menu = NSMenu()
        menu.addItem(withTitle: "Move Tab to New Window", action: #selector(moveToNewWindow), keyEquivalent: "").target = self
        menu.addItem(withTitle: "Close Tab", action: #selector(closeClicked), keyEquivalent: "").target = self
        return menu
    }

    @objc private func moveToNewWindow() { onMoveToNewWindow?() }

    private func startEditing() {
        guard editor == nil else { return }
        let field = NSTextField(string: label.stringValue)
        field.font = label.font
        field.frame = label.frame.insetBy(dx: -3, dy: -2)
        field.frame.size.width = max(field.frame.width, 90)
        field.focusRingType = .exterior
        field.delegate = self
        cancelled = false
        label.isHidden = true
        addSubview(field)
        editor = field
        window?.makeFirstResponder(field)
        field.currentEditor()?.selectAll(nil)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        guard selector == #selector(NSResponder.cancelOperation(_:)) else { return false }
        cancelled = true
        window?.makeFirstResponder(nil)
        return true
    }

    func controlTextDidEndEditing(_ notification: Notification) {
        guard let field = editor else { return }
        editor = nil
        field.removeFromSuperview()
        label.isHidden = false
        let title = field.stringValue.trimmingCharacters(in: .whitespaces)
        if !cancelled, !title.isEmpty, title != label.stringValue { onRename?(title) }
        window?.makeFirstResponder(nil)
        onEditEnded?()
    }

    @objc private func closeClicked() { onClose?() }
}
