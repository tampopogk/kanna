import AppKit
import KannaPerf
import KannaWindows

struct DeskTask: Equatable {
    let id: String
    let title: String
    let leftAt: Double?
    let status: String?
    let detail: String
    var agent: String?
    var place: String?
    var offline = false
    var made = 0
    var task: String?

    var identity: String { task ?? id }

    @MainActor var meta: String { offline ? "offline" : status == "needs-you" ? "needs you" : Self.since(leftAt) }

    @MainActor var sub: String {
        let status = status.map { $0 == "needs-you" ? "needs you" : $0 }.map { offline ? "last known: \($0)" : $0 }
        return [status, detail.isEmpty ? nil : detail, offline ? "offline" : Self.since(leftAt)].compactMap { $0 }.joined(separator: " · ")
    }

    @MainActor static var skew: Double = 0

    @MainActor static func since(_ at: Double?) -> String {
        let ago = ago(at)
        return ago == "now" ? "just now" : ago
    }

    @MainActor static func ago(_ at: Double?) -> String {
        guard let at else { return "now" }
        let minutes = Int(max(0, Date().timeIntervalSince1970 + skew - at) / 60)
        return minutes < 1 ? "now" : minutes < 60 ? "\(minutes)m ago" : minutes < 60 * 24 ? "\(minutes / 60)h ago" : "\(minutes / 60 / 24)d ago"
    }

    @MainActor
    static func dot(_ task: DeskTask, in rect: NSRect) {
        guard task.offline else {
            color(task.status).setFill()
            return NSBezierPath(ovalIn: rect).fill()
        }
        NSColor.systemRed.setStroke()
        let ring = NSBezierPath(ovalIn: rect.insetBy(dx: 0.75, dy: 0.75))
        ring.lineWidth = 1.5
        ring.stroke()
    }

    static func color(_ status: String?) -> NSColor {
        switch status {
        case "working": return .controlAccentColor
        case "needs-you": return .systemOrange
        case "idle": return .systemGreen
        default: return .tertiaryLabelColor
        }
    }
}

struct ShelfTask: Equatable {
    let id: String
    let name: String
    let message: String?
    let from: String?
    let madeAt: Double?
    var waits: [String] = []
    var agent: String?
    var running = false
    var status: String?

    static func program(_ command: String?) -> String? {
        guard let first = command?.split(whereSeparator: \.isWhitespace).first else { return nil }
        return first.split(separator: "/").last.map(String.init)
    }

    var blocked: Bool { !waits.isEmpty }

    @MainActor var after: String {
        let made = "made " + DeskTask.ago(madeAt).replacingOccurrences(of: "now", with: "just now")
        return [from.map { "from \($0)" }, made].compactMap { $0 }.joined(separator: " · ")
    }

    static func words(_ query: String) -> [String] { query.split(whereSeparator: \.isWhitespace).map(String.init) }

    func matches(_ query: String) -> Bool {
        Self.words(query).allSatisfy { word in [name, message ?? ""].contains { $0.range(of: word, options: .caseInsensitive) != nil } }
    }

    static func marked(_ text: String, _ query: String, font: NSFont, color: NSColor) -> NSMutableAttributedString {
        let out = NSMutableAttributedString(string: text, attributes: [.font: font, .foregroundColor: color])
        let whole = text as NSString
        for word in words(query) {
            var from = 0
            while from < whole.length {
                let found = whole.range(of: word, options: .caseInsensitive, range: NSRange(location: from, length: whole.length - from))
                if found.location == NSNotFound { break }
                out.addAttribute(.backgroundColor, value: NSColor.controlAccentColor.withAlphaComponent(0.22), range: found)
                from = found.location + max(1, found.length)
            }
        }
        return out
    }
}

struct DeskPage: Equatable {
    var active: String?
    var tasks: [DeskTask] = []
    var keys: [String: String] = [:]
    var shelf: [ShelfTask] = []
    var running = 0
    var project = ""
    var cheer = ""
    var note = ""

    static let limit = 3

    var full: Bool { running >= Self.limit }
}

@MainActor
final class DeskStrip: NSView {
    static let height: CGFloat = 30
    static let titleFloor: CGFloat = 60
    var onActivate: ((String) -> Void)?
    var onDesk: (() -> Void)?
    private var shown: [DeskTask] = []
    private var keys: [String: String] = [:]
    private var waiting = 0
    private let count = NSTextField(labelWithString: "")
    private let hint = DeskHint()
    private var ticker: Timer?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        count.font = .systemFont(ofSize: 12)
        count.textColor = .tertiaryLabelColor
        hint.onPress = { [weak self] in self?.onDesk?() }
        paint()
    }

    var waitingText: String { count.isHidden ? "" : count.stringValue }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        ticker?.invalidate()
        ticker = window == nil ? nil : Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        relayout()
    }

    private func paint() { layer?.backgroundColor = resolved(.windowBackgroundColor) }

    private var stripItems: [DeskStripItem] { subviews.compactMap { $0 as? DeskStripItem } }

    var items: [(task: String, center: NSPoint)] {
        stripItems.map { ($0.task.id, $0.convert(NSPoint(x: 20, y: $0.bounds.midY), to: nil)) }
    }

    var geometry: JSON {
        ["width": .number(Double(bounds.width)),
         "hint": .string(hint.text), "hintRight": .number(Double(hint.frame.maxX)),
         "items": .array(stripItems.map { ["task": .string($0.task.id), "meta": .string($0.metaText), "right": .number(Double($0.frame.maxX)), "key": $0.keyText.map { .string($0) } ?? .null] })]
    }

    func show(_ tasks: [DeskTask], keys: [String: String], waiting: Int) {
        guard tasks != shown || keys != self.keys || waiting != self.waiting else { return }
        shown = tasks
        self.keys = keys
        self.waiting = waiting
        subviews.forEach { $0.removeFromSuperview() }
        count.stringValue = "\(waiting) on the Shelf"
        count.sizeToFit()
        addSubview(count)
        addSubview(hint)
        for task in tasks {
            let item = DeskStripItem(task: task, key: keys[task.id])
            item.onPress = { [weak self] in self?.onActivate?(task.id) }
            item.onHover = { [weak self] in self?.relayout() }
            addSubview(item)
        }
        relayout()
    }

    func tick() {
        stripItems.forEach { $0.refresh() }
        relayout()
    }

    fileprivate func relayout() {
        let items = stripItems
        let gaps = CGFloat(max(0, items.count - 1)) * 4
        let chrome = items.reduce(0) { $0 + $1.chrome }
        let metas = items.map(\.metaWidth)
        let titles = items.map(\.titleWidth)
        hint.frame.origin = NSPoint(x: 8, y: (Self.height - hint.frame.height) / 2)
        let start = hint.frame.maxX + (items.isEmpty ? 0 : 9)
        var room = bounds.width - start - gaps - chrome - metas.reduce(0, +)
        let titleFloors = titles.map { min($0, Self.titleFloor) }.reduce(0, +)
        count.isHidden = waiting == 0
        if !count.isHidden, room - count.frame.width - 20 < titleFloors {
            count.isHidden = true
        }
        if !count.isHidden { room -= count.frame.width + 20 }
        let fitted = Self.shrink(titles, to: room, floor: Self.titleFloor)
        let leftover = room - fitted.reduce(0, +)
        let fittedMetas = leftover < 0 ? Self.shrink(metas, to: metas.reduce(0, +) + leftover, floor: 0) : metas
        var x: CGFloat = start
        for (index, item) in items.enumerated() {
            item.fit(title: fitted[index], meta: fittedMetas[index])
            item.frame.origin = NSPoint(x: x, y: (Self.height - item.frame.height) / 2)
            x += item.frame.width + 4
        }
        count.frame.origin = NSPoint(x: bounds.width - count.frame.width - 12, y: (Self.height - count.frame.height) / 2)
    }

    static func shrink(_ widths: [CGFloat], to total: CGFloat, floor: CGFloat) -> [CGFloat] {
        let floors = widths.map { min($0, floor) }
        let spare = zip(widths, floors).map { $0 - $1 }.reduce(0, +)
        let cut = widths.reduce(0, +) - total
        guard cut > 0 else { return widths }
        guard cut < spare else { return floors }
        return zip(widths, floors).map { ($0 - ($0 - $1) * cut / spare).rounded(.down) }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
        if !stripItems.isEmpty { NSRect(x: hint.frame.maxX + 4, y: (Self.height - 14) / 2, width: 1, height: 14).fill() }
    }
}

@MainActor
private final class DeskHint: NSView {
    var onPress: (() -> Void)?
    private let label = NSTextField(labelWithString: "⌘. Desk")
    private var hover = false

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        label.font = .systemFont(ofSize: 12)
        label.sizeToFit()
        label.frame.origin = NSPoint(x: 8, y: (22 - label.frame.height) / 2)
        addSubview(label)
        frame.size = NSSize(width: label.frame.width + 16, height: 22)
        toolTip = "The Desk view: the planning agent, the Desk's tasks and the Shelf"
        wantsLayer = true
        layer?.cornerRadius = 6
        paint()
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var text: String { label.stringValue }

    private func paint() {
        label.textColor = hover ? .labelColor : .tertiaryLabelColor
        layer?.backgroundColor = hover ? resolved(.quaternaryLabelColor) : nil
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    override func mouseEntered(with event: NSEvent) { hover = true; paint() }
    override func mouseExited(with event: NSEvent) { hover = false; paint() }
    override func mouseDown(with event: NSEvent) { onPress?() }
}

@MainActor
private final class DeskStripItem: NSView {
    let task: DeskTask
    var onPress: (() -> Void)?
    var onHover: (() -> Void)?
    private let title: NSTextField
    private let meta: NSTextField
    private let key: NSTextField?
    private var hover = false
    private(set) var titleWidth: CGFloat = 0
    private(set) var metaWidth: CGFloat = 0

    init(task: DeskTask, key: String?) {
        self.task = task
        title = NSTextField(labelWithString: task.title)
        meta = NSTextField(labelWithString: "")
        self.key = key.map { NSTextField(labelWithString: $0) }
        super.init(frame: NSRect(x: 0, y: 0, width: 0, height: 22))
        for label in [title, meta] + [self.key].compactMap({ $0 }) {
            label.font = .systemFont(ofSize: 12)
            label.lineBreakMode = .byTruncatingTail
            label.sizeToFit()
            addSubview(label)
        }
        self.key?.font = .systemFont(ofSize: 11)
        self.key?.sizeToFit()
        self.key?.textColor = .tertiaryLabelColor
        self.key?.isHidden = true
        meta.textColor = .tertiaryLabelColor
        titleWidth = min(title.frame.width, 260)
        paintTitle()
        refresh()
        toolTip = [task.title, task.place].compactMap { $0 }.joined(separator: "\n")
        wantsLayer = true
        layer?.cornerRadius = 6
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var metaText: String { meta.isHidden ? "" : meta.stringValue }

    var keyText: String? { key?.stringValue }

    var chrome: CGFloat { 23 + 6 + 8 + (hover ? (key.map { $0.frame.width + 6 } ?? 0) : 0) }

    func refresh() {
        meta.stringValue = "· \(task.meta)"
        meta.sizeToFit()
        metaWidth = meta.frame.width
    }

    func fit(title width: CGFloat, meta room: CGFloat) {
        title.frame = NSRect(x: 23, y: (22 - title.frame.height) / 2, width: width, height: title.frame.height)
        meta.isHidden = room < 12
        meta.frame = NSRect(x: title.frame.maxX + 6, y: title.frame.minY, width: meta.isHidden ? 0 : room, height: meta.frame.height)
        key?.isHidden = !hover
        key?.frame.origin = NSPoint(x: meta.frame.maxX + 6, y: (22 - (key?.frame.height ?? 0)) / 2)
        frame.size.width = (hover ? key?.frame.maxX ?? meta.frame.maxX : meta.frame.maxX) + 8
    }

    override func draw(_ dirtyRect: NSRect) {
        DeskTask.dot(task, in: NSRect(x: 9, y: bounds.midY - 4, width: 8, height: 8))
    }

    override func mouseEntered(with event: NSEvent) { setHover(true) }
    override func mouseExited(with event: NSEvent) { setHover(false) }

    private func paintTitle() {
        title.textColor = hover ? .labelColor : task.status == "needs-you" ? .systemOrange : .secondaryLabelColor
    }

    private func setHover(_ on: Bool) {
        hover = on
        paintTitle()
        layer?.backgroundColor = on ? resolved(.quaternaryLabelColor) : nil
        onHover?()
    }

    override func mouseDown(with event: NSEvent) { onPress?() }
}
@MainActor
final class DeskView: NSView, NSSearchFieldDelegate {
    static let cheers = [
        "Clear desk, clear mind. Pick what's next.",
        "Nice work. What's next?",
        "One thing at a time, and you're doing great.",
        "Fresh start. Pick a task when you're ready.",
        "Good progress today. Keep it going.",
        "Breathe. Then pick the next one.",
    ]
    var onActivate: ((String) -> Void)?
    var onStart: ((String) -> Void)?
    var onLeaveSearch: (() -> Void)?
    var onShelve: ((String) -> Void)?
    var onRun: ((String) -> Void)?
    var onKeyed: ((Bool) -> Void)?
    private var shown: DeskPage?
    private let scroller = NSScrollView()
    private let document = FlippedView()
    private let search = NSSearchField()
    private let searchKey = NSTextField(labelWithString: "⌘F")
    private(set) var query = ""
    private var chosen: String?
    private var chosenAt = 0
    private(set) var keyed = false
    private var started: String?
    private var responderWatch: NSKeyValueObservation?
    private var ticker: Timer?
    private var deskKept: [String: DeskRow] = [:]
    private var shelfKept: [String: ShelfRow] = [:]

    override var isFlipped: Bool { true }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        scroller.frame = bounds
        scroller.autoresizingMask = [.width, .height]
        scroller.drawsBackground = false
        scroller.automaticallyAdjustsContentInsets = false
        scroller.hasVerticalScroller = true
        scroller.hasHorizontalScroller = false
        scroller.scrollerStyle = .overlay
        scroller.autohidesScrollers = true
        scroller.verticalScrollElasticity = .allowed
        scroller.horizontalScrollElasticity = .none
        scroller.documentView = document
        addSubview(scroller)
        search.placeholderString = "Search the Shelf"
        search.font = .systemFont(ofSize: 12)
        search.controlSize = .small
        search.delegate = self
        search.target = self
        search.action = #selector(searched)
        search.identifier = NSUserInterfaceItemIdentifier("shelf-search")
        searchKey.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        searchKey.textColor = .tertiaryLabelColor
        searchKey.sizeToFit()
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    private func paint() { layer?.backgroundColor = resolved(.textBackgroundColor) }

    var rows: [(task: String, center: NSPoint)] {
        rowViews.compactMap { $0 as? DeskRow }.map { ($0.task.id, $0.convert(NSPoint(x: 20, y: $0.bounds.midY), to: nil)) }
    }

    var shelfRows: [(task: String, center: NSPoint)] {
        rowViews.compactMap { $0 as? ShelfRow }.map { ($0.task.id, $0.convert(NSPoint(x: 20, y: $0.bounds.midY), to: nil)) }
    }

    var blockedShelf: [JSON] {
        rowViews.compactMap { $0 as? ShelfRow }.filter(\.task.blocked).map { ["task": .string($0.task.id), "waits": .array($0.task.waits.map { .string($0) }), "go": .string($0.label)] }
    }

    var visibleShelf: [String] {
        let seen = document.visibleRect
        return document.subviews.compactMap { $0 as? ShelfRow }.filter { seen.contains($0.frame) }.map(\.task.id)
    }

    var scrollTop: CGFloat { scroller.contentView.bounds.minY }
    var contentHeight: CGFloat { document.frame.height }
    var visibleHeight: CGFloat { scroller.contentView.bounds.height }

    func scroll(toY y: CGFloat) {
        scroller.contentView.scroll(to: NSPoint(x: 0, y: min(max(0, y), max(0, document.frame.height - scroller.contentView.bounds.height))))
        scroller.reflectScrolledClipView(scroller.contentView)
    }

    var found: [String] { shown.map { page in page.shelf.filter { $0.matches(query) }.map(\.id) } ?? [] }

    var order: [String] { (shown?.tasks.map(\.identity) ?? []) + found }

    var selected: Int { selection.flatMap { found.firstIndex(of: $0) } ?? 0 }

    var selection: String? {
        guard let chosen else { return nil }
        let rows = order
        if rows.contains(chosen) { return chosen }
        return rows.isEmpty ? nil : rows[min(chosenAt, rows.count - 1)]
    }

    private func choose(_ task: String?) {
        chosen = task
        chosenAt = task.flatMap { order.firstIndex(of: $0) } ?? 0
    }

    var selectedTask: (id: String, onDesk: Bool, blocked: Bool, running: Bool, active: Bool)? {
        guard keyed, let selection, let page = shown else { return nil }
        if let task = page.tasks.first(where: { $0.identity == selection }) { return (task.id, true, false, false, task.id == page.active) }
        guard let task = page.shelf.first(where: { $0.id == selection }), task.matches(query) else { return nil }
        return (selection, false, task.blocked, task.running, false)
    }

    var hints: JSON {
        .array(rowViews.compactMap { view -> JSON? in
            if let row = view as? DeskRow { return ["task": .string(row.task.identity), "alt": .string(row.altText), "go": .string(row.goText), "selected": .bool(row.selected)] }
            if let row = view as? ShelfRow { return ["task": .string(row.task.id), "alt": .string(row.altText), "go": .string(row.label), "selected": .bool(row.selected)] }
            return nil
        })
    }

    var searching: Bool { search.currentEditor() != nil && window?.firstResponder === search.currentEditor() }

    var searchShown: Bool { search.superview != nil && !search.isHidden }

    var searchFrame: NSRect { search.frame }

    func focusSearch() {
        if searchShown {
            window?.makeFirstResponder(search)
            search.currentEditor()?.selectAll(nil)
            document.scrollToVisible(search.frame.insetBy(dx: 0, dy: -24))
        } else if shown?.tasks.isEmpty == false {
            window?.makeFirstResponder(self)
        }
        if selection == nil { choose(firstRow) }
        updateKeyed()
        relay()
        reveal()
    }

    private var firstRow: String? { found.first ?? (query.isEmpty ? shown?.tasks.first?.identity : nil) }

    func clearSearch() {
        started = nil
        guard !query.isEmpty || !search.stringValue.isEmpty else { return }
        search.stringValue = ""
        searched()
    }

    @objc private func searched() {
        guard search.stringValue != query else { return }
        query = search.stringValue
        choose(firstRow)
        searchKey.isHidden = true
        relay()
    }

    private func relay() {
        if let shown { lay(shown, top: scrollTop) }
    }

    private func reveal() {
        guard let row = rowViews.first(where: { ($0 as? DeskRow)?.selected ?? ($0 as? ShelfRow)?.selected ?? false }) else { return }
        document.scrollToVisible(row.frame)
    }

    private func updateKeyed() {
        let now = !isHidden && window != nil && (searching || window?.firstResponder === self)
        guard now != keyed else { return }
        keyed = now
        relay()
        onKeyed?(now)
    }

    override var acceptsFirstResponder: Bool { true }

    override func keyDown(with event: NSEvent) {
        guard window?.firstResponder === self else { return super.keyDown(with: event) }
        interpretKeyEvents([event])
    }

    override func doCommand(by selector: Selector) {
        if !perform(selector) { super.doCommand(by: selector) }
    }

    override func insertText(_ insertString: Any) {}

    private func step(_ by: Int) {
        let rows = order
        guard !rows.isEmpty else { return }
        let at = selection.flatMap { rows.firstIndex(of: $0) }
        let next: Int
        switch by {
        case Int.min: next = 0
        case Int.max: next = rows.count - 1
        default: next = at.map { max(0, min(rows.count - 1, $0 + by)) } ?? (by > 0 ? 0 : rows.count - 1)
        }
        choose(rows[next])
        relay()
        reveal()
    }

    func enter() {
        guard let task = selectedTask else { return }
        if task.onDesk { onActivate?(task.id) } else { start(task.id) }
    }

    func run() {
        guard let task = selectedTask, !task.onDesk else { return }
        onRun?(task.id)
    }

    func shelveSelected() -> Bool {
        guard let task = selectedTask, task.onDesk else { return false }
        onShelve?(task.id)
        return true
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        guard keyed, flags == .command, event.keyCode == 36 || event.keyCode == 76 else { return super.performKeyEquivalent(with: event) }
        run()
        return true
    }

    func perform(_ selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveDown(_:)): step(1)
        case #selector(NSResponder.moveUp(_:)): step(-1)
        case #selector(NSResponder.moveToBeginningOfDocument(_:)), #selector(NSResponder.scrollToBeginningOfDocument(_:)): step(Int.min)
        case #selector(NSResponder.moveToEndOfDocument(_:)), #selector(NSResponder.scrollToEndOfDocument(_:)): step(Int.max)
        case #selector(NSResponder.insertNewline(_:)): enter()
        case #selector(NSResponder.cancelOperation(_:)):
            if query.isEmpty { onLeaveSearch?() } else { clearSearch() }
        default: return false
        }
        return true
    }

    func controlTextDidChange(_ obj: Notification) { searched() }

    func controlTextDidBeginEditing(_ obj: Notification) { searchKey.isHidden = true }

    func controlTextDidEndEditing(_ obj: Notification) { searchKey.isHidden = !query.isEmpty }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool { perform(selector) }

    private func start(_ task: String) {
        if !query.isEmpty { started = task }
        onStart?(task)
    }

    var note: String { document.subviews.compactMap { $0 as? NSTextField }.first { $0.identifier?.rawValue == "note" }?.stringValue ?? "" }

    private func label(_ text: String, size: CGFloat = 12, weight: NSFont.Weight = .regular, color: NSColor = .labelColor) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = .systemFont(ofSize: size, weight: weight)
        field.textColor = color
        field.sizeToFit()
        document.addSubview(field)
        return field
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        responderWatch = window?.observe(\.firstResponder, options: [.new]) { [weak self] _, _ in
            MainActor.assumeIsolated { self?.updateKeyed() }
        }
        updateKeyed()
        ticker?.invalidate()
        ticker = window == nil ? nil : Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
    }

    func tick() {
        document.subviews.forEach { ($0 as? DeskRow)?.refresh() ?? ($0 as? ShelfRow)?.refresh() }
    }

    private var rowViews: [NSView] { document.subviews.filter { $0 is DeskRow || $0 is ShelfRow }.sorted { $0.frame.minY < $1.frame.minY } }

    var times: JSON {
        .array(rowViews.compactMap { view -> JSON? in
            if let row = view as? DeskRow { return ["task": .string(row.task.id), "sub": .string(row.subText)] }
            if let row = view as? ShelfRow { return ["task": .string(row.task.id), "sub": .string(row.subText)] }
            return nil
        })
    }

    var deskRows: JSON {
        .array(rowViews.compactMap { $0 as? DeskRow }.map { row in
            ["task": .string(row.task.id), "go": .string(row.goText), "goShown": .bool(row.goShown), "active": .bool(row.active), "highlit": .bool(row.highlit)]
        })
    }

    override var isHidden: Bool {
        didSet { updateKeyed() }
    }

    func show(_ page: DeskPage) {
        if page == shown { return }
        shown = page
        if let started, !page.shelf.contains(where: { $0.id == started }) { clearSearch() }
        if let chosen, let at = order.firstIndex(of: chosen) { chosenAt = at }
        lay(page, top: scrollTop)
    }

    private func lay(_ page: DeskPage, top: CGFloat) {
        let span = Perf.begin(.desk, "DeskView.lay")
        document.subviews.filter { $0 !== search && $0 !== searchKey && !($0 is DeskRow) && !($0 is ShelfRow) }.forEach { $0.removeFromSuperview() }
        let (deskBefore, shelfBefore) = (deskKept, shelfKept)
        (deskKept, shelfKept) = ([:], [:])
        var made = 0
        let left: CGFloat = 48, width = min(760, scroller.contentSize.width - left * 2)
        var y: CGFloat = 40
        let line = label(page.cheer, size: 13, color: .secondaryLabelColor)
        line.frame.origin = NSPoint(x: left, y: y)
        y += line.frame.height + 22
        y = header("Desk", detail: page.project, count: "\(page.running) of \(DeskPage.limit) tasks", y: y, left: left, width: width).bottom
        if page.tasks.isEmpty {
            let none = label("Nothing on the Desk.", color: .secondaryLabelColor)
            none.frame.origin = NSPoint(x: left, y: y + 4)
            y += none.frame.height + 12
        }
        for task in page.tasks {
            let chosen = keyed && task.identity == selection
            let row = deskBefore[task.id].flatMap { $0.shows(task, key: page.keys[task.id], width: width + 20, active: task.id == page.active, selected: chosen) ? $0 : nil } ?? {
                made += 1
                let row = DeskRow(task: task, key: page.keys[task.id], width: width + 20, active: task.id == page.active, selected: chosen)
                row.onPress = { [weak self] in self?.onActivate?(task.id) }
                row.onAlt = { [weak self] in self?.onShelve?(task.id) }
                return row
            }()
            deskKept[task.id] = row
            row.frame.origin = NSPoint(x: left - 10, y: y)
            if row.superview !== document { document.addSubview(row) }
            y += row.frame.height
        }
        y += 28
        let hits = page.shelf.filter { $0.matches(query) }
        let selection = self.selection
        let head = header("Shelf", detail: nil, count: query.isEmpty ? "\(page.shelf.count) waiting" : "\(hits.count) of \(page.shelf.count)", y: y, left: left, width: width)
        let widest = ["\(page.shelf.count) waiting", "\(page.shelf.count) of \(page.shelf.count)"].map { ($0 as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 12)]).width }.max() ?? 0
        placeSearch(shown: !page.shelf.isEmpty, right: head.tally.maxX - ceil(widest), middle: head.tally.midY, left: left)
        y = head.bottom
        if page.shelf.isEmpty {
            let none = label("Empty. Ask the planning agent for tasks.", color: .secondaryLabelColor)
            none.frame.origin = NSPoint(x: left, y: y + 4)
            y += none.frame.height + 12
        } else if hits.isEmpty {
            let none = label("No Shelf task matches “\(query)”.", color: .secondaryLabelColor)
            none.frame.origin = NSPoint(x: left, y: y + 4)
            y += none.frame.height + 12
        }
        for task in hits {
            let chosen = keyed && task.id == selection
            let row = shelfBefore[task.id].flatMap { $0.shows(task, full: page.full, width: width + 20, query: query, selected: chosen) ? $0 : nil } ?? {
                made += 1
                let row = ShelfRow(task: task, full: page.full, width: width + 20, query: query, selected: chosen)
                row.onPress = { [weak self] in self?.start(task.id) }
                row.onAlt = { [weak self] in self?.onRun?(task.id) }
                return row
            }()
            shelfKept[task.id] = row
            row.frame.origin = NSPoint(x: left - 10, y: y)
            if row.superview !== document { document.addSubview(row) }
            y += row.frame.height
        }
        for (id, row) in deskBefore where deskKept[id] !== row { row.removeFromSuperview() }
        for (id, row) in shelfBefore where shelfKept[id] !== row { row.removeFromSuperview() }
        let note = label(page.note, color: .systemOrange)
        note.identifier = NSUserInterfaceItemIdentifier("note")
        note.frame.origin = NSPoint(x: left, y: y + 8)
        document.frame = NSRect(x: 0, y: 0, width: scroller.contentSize.width, height: y + 8 + note.frame.height + 40)
        scroll(toY: top)
        Perf.end(span, "\(page.tasks.count + hits.count) rows, \(made) new")
        Clicks.laid(desk: page.tasks.map(\.id), shelf: page.shelf.map(\.id))
    }

    private func placeSearch(shown: Bool, right edge: CGFloat, middle: CGFloat, left: CGFloat) {
        guard shown else {
            search.removeFromSuperview()
            searchKey.removeFromSuperview()
            return
        }
        if search.superview == nil {
            document.addSubview(search)
            document.addSubview(searchKey)
        }
        let right = edge - 12
        let field = min(320, max(120, right - left - 100))
        search.frame = NSRect(x: right - field, y: middle - 11, width: field, height: 22)
        searchKey.frame.origin = NSPoint(x: search.frame.maxX - searchKey.frame.width - 24, y: middle - searchKey.frame.height / 2)
        searchKey.isHidden = !query.isEmpty || search.currentEditor() != nil
    }

    private func header(_ title: String, detail: String?, count: String, y: CGFloat, left: CGFloat, width: CGFloat) -> (bottom: CGFloat, tally: NSRect) {
        let head = label(title, weight: .semibold)
        head.frame.origin = NSPoint(x: left, y: y)
        if let detail {
            let more = label("· " + detail, color: .secondaryLabelColor)
            more.frame.origin = NSPoint(x: head.frame.maxX + 4, y: y)
        }
        let tally = label(count, color: .secondaryLabelColor)
        tally.frame.origin = NSPoint(x: left + width - tally.frame.width, y: y)
        return (y + head.frame.height + 8, tally.frame)
    }

    override func resizeSubviews(withOldSize oldSize: NSSize) {
        let top = scrollTop
        super.resizeSubviews(withOldSize: oldSize)
        guard let shown else { return }
        if oldSize.width != bounds.width { lay(shown, top: top) } else { scroll(toY: top) }
    }
}

private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}


@MainActor
private final class RowHints {
    let alt = NSTextField(labelWithString: "")
    let go = NSTextField(labelWithString: "")

    init(in row: NSView) {
        for field in [alt, go] {
            field.font = .systemFont(ofSize: 12)
            field.lineBreakMode = .byTruncatingTail
            row.addSubview(field)
        }
    }

    func place(alt altText: String, go goText: String, width: CGFloat, height: CGFloat) -> CGFloat {
        alt.stringValue = altText
        go.stringValue = goText
        go.sizeToFit()
        go.frame.size.width = min(go.frame.width, 300)
        go.frame.origin = NSPoint(x: width - go.frame.width - 10, y: (height - go.frame.height) / 2)
        alt.sizeToFit()
        alt.frame.origin = NSPoint(x: go.frame.minX - alt.frame.width - 14, y: (height - alt.frame.height) / 2)
        return altText.isEmpty ? go.frame.minX : alt.frame.minX
    }

    func pressedAlt(_ event: NSEvent, in row: NSView) -> Bool {
        !alt.isHidden && !alt.stringValue.isEmpty && alt.frame.insetBy(dx: -4, dy: -6).contains(row.convert(event.locationInWindow, from: nil))
    }
}

@MainActor
private final class ShelfRow: NSView {
    let task: ShelfTask
    let selected: Bool
    var onPress: (() -> Void)?
    var onAlt: (() -> Void)?
    private let name: NSTextField
    private let sub = NSTextField(labelWithString: "")
    private var hints: RowHints!
    private let full: Bool
    private let query: String

    override var isFlipped: Bool { true }

    func shows(_ task: ShelfTask, full: Bool, width: CGFloat, query: String, selected: Bool) -> Bool {
        task == self.task && full == self.full && width == frame.width && query == self.query && selected == self.selected
    }

    init(task: ShelfTask, full: Bool, width: CGFloat, query: String, selected: Bool) {
        self.task = task
        self.full = full
        self.selected = selected
        self.query = query
        name = NSTextField(labelWithAttributedString: ShelfTask.marked(task.name, query, font: .systemFont(ofSize: 13, weight: .medium), color: .labelColor))
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 52))
        wantsLayer = true
        layer?.cornerRadius = 8
        name.lineBreakMode = .byTruncatingTail
        refresh()
        sub.lineBreakMode = .byTruncatingTail
        [name, sub].forEach(addSubview)
        hints = RowHints(in: self)
        paint()
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var label: String { hints.go.stringValue }

    var altText: String { hints.alt.stringValue }

    var subText: String { sub.stringValue }

    func refresh() {
        let small = NSFont.systemFont(ofSize: 12)
        let state = task.running ? (task.status.map { $0 == "needs-you" ? "needs you" : $0 } ?? "running") + " on the Shelf · " : ""
        let detail = NSMutableAttributedString(string: state, attributes: [.font: small, .foregroundColor: NSColor.secondaryLabelColor])
        detail.append(NSAttributedString(string: task.waits.isEmpty ? "" : "waits for " + task.waits.joined(separator: ", ") + " · ", attributes: [.font: small, .foregroundColor: NSColor.systemOrange]))
        detail.append(ShelfTask.marked(task.message.map { $0 + " · " } ?? "", query, font: small, color: .secondaryLabelColor))
        detail.append(NSAttributedString(string: task.after, attributes: [.font: small, .foregroundColor: NSColor.secondaryLabelColor]))
        sub.attributedStringValue = detail
    }

    override func draw(_ dirtyRect: NSRect) {
        let rect = NSRect(x: 10.5, y: bounds.midY - 4, width: 8, height: 8)
        if task.running {
            DeskTask.color(task.status ?? "working").setFill()
            return NSBezierPath(ovalIn: rect.insetBy(dx: -0.5, dy: -0.5)).fill()
        }
        NSColor.tertiaryLabelColor.setStroke()
        let ring = NSBezierPath(ovalIn: rect)
        ring.lineWidth = 1.4
        if task.blocked { ring.setLineDash([2, 2], count: 2, phase: 0) }
        ring.stroke()
    }

    private var hover = false

    private func paint() {
        let lit = hover || selected
        let alt: String, go: String
        if task.blocked {
            alt = ""
            go = lit ? "Blocked: waits for " + task.waits.joined(separator: ", ") : "Blocked"
        } else {
            alt = "⌘↩ " + (task.running ? "Stop" : "Start")
            go = full ? "Desk full" : selected ? "↩ Move to Desk" : "Move to Desk →"
        }
        let edge = hints.place(alt: alt, go: go, width: bounds.width, height: bounds.height)
        hints.alt.isHidden = !lit
        hints.alt.textColor = .secondaryLabelColor
        hints.go.textColor = lit && !full && !task.blocked ? .controlAccentColor : .tertiaryLabelColor
        let room = max(40, edge - 30 - 16)
        name.frame = NSRect(x: 30, y: 8, width: room, height: 18)
        sub.frame = NSRect(x: 30, y: 27, width: room, height: 17)
        layer?.backgroundColor = lit ? resolved(.quaternaryLabelColor) : nil
    }

    override func viewDidMoveToWindow() { paint() }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    override func mouseEntered(with event: NSEvent) {
        hover = true
        paint()
    }

    override func mouseExited(with event: NSEvent) {
        hover = false
        paint()
    }

    override func mouseDown(with event: NSEvent) {
        if hints.pressedAlt(event, in: self) { onAlt?() } else { onPress?() }
    }
}

@MainActor
private final class DeskRow: NSView {
    let task: DeskTask
    let selected: Bool
    var onPress: (() -> Void)?
    var onAlt: (() -> Void)?
    private let name = NSTextField(labelWithString: "")
    private let sub = NSTextField(labelWithString: "")
    private var hints: RowHints!

    override var isFlipped: Bool { true }

    let active: Bool
    private let key: String?

    func shows(_ task: DeskTask, key: String?, width: CGFloat, active: Bool, selected: Bool) -> Bool {
        task == self.task && key == self.key && width == frame.width && active == self.active && selected == self.selected
    }

    init(task: DeskTask, key: String?, width: CGFloat, active: Bool, selected: Bool) {
        self.task = task
        self.active = active
        self.key = key
        self.selected = selected
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 52))
        wantsLayer = true
        layer?.cornerRadius = 8
        name.stringValue = task.title
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        name.lineBreakMode = .byTruncatingTail
        refresh()
        sub.font = .systemFont(ofSize: 12)
        sub.textColor = .secondaryLabelColor
        sub.lineBreakMode = .byTruncatingTail
        toolTip = task.place
        [name, sub].forEach(addSubview)
        hints = RowHints(in: self)
        paint()
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var subText: String { sub.stringValue }
    var goText: String { hints.go.stringValue }
    var altText: String { hints.alt.stringValue }
    var goShown: Bool { !hints.go.isHidden }
    var highlit: Bool { layer?.backgroundColor != nil }

    func refresh() { sub.stringValue = task.sub }

    private var hover = false

    private func paint() {
        let lit = hover || selected
        let go = active ? (selected ? "↩ Back to it" : "⌘. · Back to it") : selected ? "↩ Make active" : (key.map { "\($0) · " } ?? "") + "Make active →"
        let edge = hints.place(alt: "⇧⌘⌫ Move to Shelf", go: go, width: bounds.width, height: bounds.height)
        hints.alt.isHidden = !lit
        hints.go.isHidden = !lit
        hints.alt.textColor = .secondaryLabelColor
        hints.go.textColor = active ? .secondaryLabelColor : .controlAccentColor
        name.sizeToFit()
        name.frame = NSRect(x: 30, y: 8, width: min(name.frame.width, bounds.width - 140), height: 18)
        sub.frame = NSRect(x: 30, y: 27, width: bounds.width - 140, height: 17)
        if lit {
            let room = max(40, edge - 30 - 16)
            name.frame.size.width = min(name.frame.width, room)
            sub.frame.size.width = room
        }
        layer?.backgroundColor = lit ? resolved(.quaternaryLabelColor) : active ? resolved(.quaternaryLabelColor.withAlphaComponent(0.5)) : nil
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    override func draw(_ dirtyRect: NSRect) {
        DeskTask.dot(task, in: NSRect(x: 10, y: bounds.midY - 4, width: 8, height: 8))
    }

    override func mouseEntered(with event: NSEvent) {
        hover = true
        paint()
    }

    override func mouseExited(with event: NSEvent) {
        hover = false
        paint()
    }

    override func mouseDown(with event: NSEvent) {
        if hints.pressedAlt(event, in: self) { onAlt?() } else { onPress?() }
    }
}
