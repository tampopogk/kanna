import AppKit
import KannaPerf

struct ShelfTask: Equatable {
    let id: String
    let title: String
    let parkedAt: Double?
    let status: String?
    let detail: String
    var agent: String?

    var meta: String { status == "needs-you" ? "needs you" : "parked " + Self.ago(parkedAt) }

    static func ago(_ at: Double?) -> String {
        guard let at else { return "now" }
        let minutes = Int(max(0, Date().timeIntervalSince1970 - at) / 60)
        return minutes < 1 ? "now" : minutes < 60 ? "\(minutes)m ago" : minutes < 60 * 24 ? "\(minutes / 60)h ago" : "\(minutes / 60 / 24)d ago"
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

struct BacklogTask: Equatable {
    let id: String
    let name: String
    let message: String?
    let from: String?
    let madeAt: Double?
    var waits: [String] = []
    var agent: String?

    static func program(_ command: String?) -> String? {
        guard let first = command?.split(whereSeparator: \.isWhitespace).first else { return nil }
        return first.split(separator: "/").last.map(String.init)
    }

    var blocked: Bool { !waits.isEmpty }

    var sub: String { [waits.isEmpty ? nil : "waits for " + waits.joined(separator: ", "), agent, message, after].compactMap { $0 }.joined(separator: " · ") }

    var after: String {
        let made = "made " + ShelfTask.ago(madeAt).replacingOccurrences(of: "now", with: "just now")
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
    var tasks: [ShelfTask] = []
    var backlog: [BacklogTask] = []
    var running = 0
    var project = ""
    var cheer = ""
    var note = ""

    static let limit = 3

    var full: Bool { running >= Self.limit }
}

@MainActor
final class ShelfStrip: NSView {
    static let height: CGFloat = 30
    static let titleFloor: CGFloat = 60
    var onPull: ((String) -> Void)?
    private var shown: [ShelfTask] = []
    private var waiting = 0
    private let count = NSTextField(labelWithString: "")
    private var ticker: Timer?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        count.font = .systemFont(ofSize: 12)
        count.textColor = .tertiaryLabelColor
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

    private var shelfItems: [ShelfItem] { subviews.compactMap { $0 as? ShelfItem } }

    var items: [(task: String, center: NSPoint)] {
        shelfItems.map { ($0.task.id, $0.convert(NSPoint(x: 20, y: $0.bounds.midY), to: nil)) }
    }

    var geometry: JSON {
        ["width": .number(Double(bounds.width)),
         "items": .array(shelfItems.map { ["task": .string($0.task.id), "meta": .string($0.metaText), "right": .number(Double($0.frame.maxX))] })]
    }

    func show(_ tasks: [ShelfTask], waiting: Int) {
        guard tasks != shown || waiting != self.waiting else { return }
        shown = tasks
        self.waiting = waiting
        subviews.forEach { $0.removeFromSuperview() }
        count.stringValue = "\(waiting) in backlog"
        count.sizeToFit()
        addSubview(count)
        for (index, task) in tasks.enumerated() {
            let item = ShelfItem(task: task, key: index < 3 ? "⌘\(index + 1)" : nil)
            item.onPress = { [weak self] in self?.onPull?(task.id) }
            item.onHover = { [weak self] in self?.relayout() }
            addSubview(item)
        }
        relayout()
    }

    private func tick() {
        shelfItems.forEach { $0.refresh() }
        relayout()
    }

    fileprivate func relayout() {
        let items = shelfItems
        let gaps = CGFloat(max(0, items.count - 1)) * 4
        let chrome = items.reduce(0) { $0 + $1.chrome }
        let metas = items.map(\.metaWidth)
        let titles = items.map(\.titleWidth)
        var room = bounds.width - 8 - gaps - chrome - metas.reduce(0, +)
        count.isHidden = waiting == 0
        if !count.isHidden, room - count.frame.width - 20 < titles.map { min($0, Self.titleFloor) }.reduce(0, +) {
            count.isHidden = true
        }
        if !count.isHidden { room -= count.frame.width + 20 }
        let fitted = Self.shrink(titles, to: room, floor: Self.titleFloor)
        let leftover = room - fitted.reduce(0, +)
        let fittedMetas = leftover < 0 ? Self.shrink(metas, to: metas.reduce(0, +) + leftover, floor: 0) : metas
        var x: CGFloat = 8
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
    }
}

@MainActor
private final class ShelfItem: NSView {
    let task: ShelfTask
    var onPress: (() -> Void)?
    var onHover: (() -> Void)?
    private let title: NSTextField
    private let meta: NSTextField
    private let key: NSTextField?
    private var hover = false
    private(set) var titleWidth: CGFloat = 0
    private(set) var metaWidth: CGFloat = 0

    init(task: ShelfTask, key: String?) {
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
        toolTip = task.title
        wantsLayer = true
        layer?.cornerRadius = 6
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var metaText: String { meta.isHidden ? "" : meta.stringValue }

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
        ShelfTask.color(task.status).setFill()
        NSBezierPath(ovalIn: NSRect(x: 9, y: bounds.midY - 4, width: 8, height: 8)).fill()
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
final class ShelfPage: NSView, NSSearchFieldDelegate {
    static let cheers = [
        "Clear desk, clear mind. Pick what's next.",
        "Nice work. What's next?",
        "One thing at a time, and you're doing great.",
        "Fresh start. Pull a task when you're ready.",
        "Good progress today. Keep it going.",
        "Breathe. Then pick the next one.",
    ]
    var onPull: ((String) -> Void)?
    var onStart: ((String) -> Void)?
    var onLeaveSearch: (() -> Void)?
    private var shown: DeskPage?
    private let scroller = NSScrollView()
    private let document = FlippedView()
    private let search = NSSearchField()
    private let searchKey = NSTextField(labelWithString: "⌘F")
    private(set) var query = ""
    private(set) var selected = 0
    private var started: String?

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
        search.placeholderString = "Search the backlog"
        search.font = .systemFont(ofSize: 12)
        search.controlSize = .small
        search.delegate = self
        search.target = self
        search.action = #selector(searched)
        search.identifier = NSUserInterfaceItemIdentifier("backlog-search")
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
        document.subviews.compactMap { $0 as? ShelfRow }.map { ($0.task.id, $0.convert(NSPoint(x: 20, y: $0.bounds.midY), to: nil)) }
    }

    var backlogRows: [(task: String, center: NSPoint)] {
        document.subviews.compactMap { $0 as? BacklogRow }.map { ($0.task.id, $0.convert(NSPoint(x: 20, y: $0.bounds.midY), to: nil)) }
    }

    var blockedBacklog: [JSON] {
        document.subviews.compactMap { $0 as? BacklogRow }.filter(\.task.blocked).map { ["task": .string($0.task.id), "waits": .array($0.task.waits.map { .string($0) }), "go": .string($0.label)] }
    }

    var visibleBacklog: [String] {
        let seen = document.visibleRect
        return document.subviews.compactMap { $0 as? BacklogRow }.filter { seen.contains($0.frame) }.map(\.task.id)
    }

    var scrollTop: CGFloat { scroller.contentView.bounds.minY }
    var contentHeight: CGFloat { document.frame.height }
    var visibleHeight: CGFloat { scroller.contentView.bounds.height }

    func scroll(toY y: CGFloat) {
        scroller.contentView.scroll(to: NSPoint(x: 0, y: min(max(0, y), max(0, document.frame.height - scroller.contentView.bounds.height))))
        scroller.reflectScrolledClipView(scroller.contentView)
    }

    var found: [String] { shown.map { page in page.backlog.filter { $0.matches(query) }.map(\.id) } ?? [] }

    var searching: Bool { search.currentEditor() != nil && window?.firstResponder === search.currentEditor() }

    var searchShown: Bool { search.superview != nil && !search.isHidden }

    var searchFrame: NSRect { search.frame }

    func focusSearch() {
        guard searchShown else { return }
        window?.makeFirstResponder(search)
        search.currentEditor()?.selectAll(nil)
        document.scrollToVisible(search.frame.insetBy(dx: 0, dy: -24))
    }

    func clearSearch() {
        started = nil
        guard !query.isEmpty || !search.stringValue.isEmpty else { return }
        search.stringValue = ""
        searched()
    }

    @objc private func searched() {
        guard search.stringValue != query else { return }
        query = search.stringValue
        selected = 0
        searchKey.isHidden = true
        if let shown { lay(shown, top: scrollTop) }
    }

    func controlTextDidChange(_ obj: Notification) { searched() }

    func controlTextDidBeginEditing(_ obj: Notification) { searchKey.isHidden = true }

    func controlTextDidEndEditing(_ obj: Notification) { searchKey.isHidden = !query.isEmpty }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        let hits = found
        switch selector {
        case #selector(NSResponder.moveDown(_:)), #selector(NSResponder.moveUp(_:)):
            guard !hits.isEmpty else { return true }
            selected = max(0, min(hits.count - 1, selected + (selector == #selector(NSResponder.moveDown(_:)) ? 1 : -1)))
            if let shown { lay(shown, top: scrollTop) }
            if let row = document.subviews.compactMap({ $0 as? BacklogRow }).first(where: { $0.selected }) { document.scrollToVisible(row.frame) }
            return true
        case #selector(NSResponder.insertNewline(_:)):
            guard !query.isEmpty, !hits.isEmpty else { return true }
            start(hits[min(selected, hits.count - 1)])
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            if query.isEmpty { onLeaveSearch?() } else { clearSearch() }
            return true
        default:
            return false
        }
    }

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

    func show(_ page: DeskPage) {
        if page == shown { return }
        shown = page
        if let started, !page.backlog.contains(where: { $0.id == started }) { clearSearch() }
        lay(page, top: scrollTop)
    }

    private func lay(_ page: DeskPage, top: CGFloat) {
        let span = Perf.begin(.shelf, "ShelfPage.lay")
        document.subviews.filter { $0 !== search && $0 !== searchKey }.forEach { $0.removeFromSuperview() }
        let left: CGFloat = 48, width = min(760, scroller.contentSize.width - left * 2)
        var y: CGFloat = 40
        let line = label(page.cheer, size: 13, color: .secondaryLabelColor)
        line.frame.origin = NSPoint(x: left, y: y)
        y += line.frame.height + 22
        y = header("Shelf", detail: page.project, count: "\(page.running) of \(DeskPage.limit) tasks", y: y, left: left, width: width).bottom
        if page.tasks.isEmpty {
            let none = label("Nothing on the Shelf.", color: .secondaryLabelColor)
            none.frame.origin = NSPoint(x: left, y: y + 4)
            y += none.frame.height + 12
        }
        for (index, task) in page.tasks.enumerated() {
            let row = ShelfRow(task: task, key: index < 3 ? "⌘\(index + 1)" : nil, width: width + 20)
            row.onPress = { [weak self] in self?.onPull?(task.id) }
            row.frame.origin = NSPoint(x: left - 10, y: y)
            document.addSubview(row)
            y += row.frame.height
        }
        y += 28
        let hits = page.backlog.filter { $0.matches(query) }
        selected = min(selected, max(0, hits.count - 1))
        let head = header("Backlog", detail: nil, count: query.isEmpty ? "\(page.backlog.count) waiting" : "\(hits.count) of \(page.backlog.count)", y: y, left: left, width: width)
        let widest = ["\(page.backlog.count) waiting", "\(page.backlog.count) of \(page.backlog.count)"].map { ($0 as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 12)]).width }.max() ?? 0
        placeSearch(shown: !page.backlog.isEmpty, right: head.tally.maxX - ceil(widest), middle: head.tally.midY, left: left)
        y = head.bottom
        if page.backlog.isEmpty {
            let none = label("Empty. Ask the planning agent for tasks.", color: .secondaryLabelColor)
            none.frame.origin = NSPoint(x: left, y: y + 4)
            y += none.frame.height + 12
        } else if hits.isEmpty {
            let none = label("No backlog task matches “\(query)”.", color: .secondaryLabelColor)
            none.frame.origin = NSPoint(x: left, y: y + 4)
            y += none.frame.height + 12
        }
        for (index, task) in hits.enumerated() {
            let row = BacklogRow(task: task, full: page.full, width: width + 20, query: query, selected: !query.isEmpty && index == selected)
            row.onPress = { [weak self] in self?.start(task.id) }
            row.frame.origin = NSPoint(x: left - 10, y: y)
            document.addSubview(row)
            y += row.frame.height
        }
        let note = label(page.note, color: .systemOrange)
        note.identifier = NSUserInterfaceItemIdentifier("note")
        note.frame.origin = NSPoint(x: left, y: y + 8)
        document.frame = NSRect(x: 0, y: 0, width: scroller.contentSize.width, height: y + 8 + note.frame.height + 40)
        scroll(toY: top)
        Perf.end(span, "\(page.tasks.count + hits.count) rows")
        Clicks.laid(shelf: page.tasks.map(\.id), backlog: page.backlog.map(\.id))
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
private final class BacklogRow: NSView {
    let task: BacklogTask
    let selected: Bool
    var onPress: (() -> Void)?
    private let go: NSTextField
    private let full: Bool

    override var isFlipped: Bool { true }

    init(task: BacklogTask, full: Bool, width: CGFloat, query: String, selected: Bool) {
        self.task = task
        self.full = full
        self.selected = selected
        go = NSTextField(labelWithString: task.blocked ? "Blocked" : full ? "Shelf full" : selected ? "↩ Move to Shelf" : "Move to Shelf →")
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 52))
        wantsLayer = true
        layer?.cornerRadius = 8
        let name = NSTextField(labelWithAttributedString: BacklogTask.marked(task.name, query, font: .systemFont(ofSize: 13, weight: .medium), color: .labelColor))
        name.lineBreakMode = .byTruncatingTail
        name.frame = NSRect(x: 30, y: 8, width: width - 160, height: 18)
        let small = NSFont.systemFont(ofSize: 12)
        let detail = NSMutableAttributedString(string: task.waits.isEmpty ? "" : "waits for " + task.waits.joined(separator: ", ") + " · ", attributes: [.font: small, .foregroundColor: NSColor.systemOrange])
        detail.append(NSAttributedString(string: task.agent.map { $0 + " · " } ?? "", attributes: [.font: small, .foregroundColor: NSColor.secondaryLabelColor]))
        detail.append(BacklogTask.marked(task.message.map { $0 + " · " } ?? "", query, font: small, color: .secondaryLabelColor))
        detail.append(NSAttributedString(string: task.after, attributes: [.font: small, .foregroundColor: NSColor.secondaryLabelColor]))
        let sub = NSTextField(labelWithAttributedString: detail)
        sub.lineBreakMode = .byTruncatingTail
        sub.frame = NSRect(x: 30, y: 27, width: width - 160, height: 17)
        go.font = .systemFont(ofSize: 12)
        go.sizeToFit()
        go.frame.origin = NSPoint(x: width - go.frame.width - 10, y: (52 - go.frame.height) / 2)
        [name, sub, go].forEach(addSubview)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var label: String { go.stringValue }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.tertiaryLabelColor.setStroke()
        let ring = NSBezierPath(ovalIn: NSRect(x: 10.5, y: bounds.midY - 4, width: 8, height: 8))
        ring.lineWidth = 1.4
        if task.blocked { ring.setLineDash([2, 2], count: 2, phase: 0) }
        ring.stroke()
    }

    private var hover = false

    private func paint() {
        layer?.backgroundColor = hover || selected ? resolved(.quaternaryLabelColor) : nil
        go.textColor = (hover || selected) && !full && !task.blocked ? .controlAccentColor : .tertiaryLabelColor
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

    override func mouseDown(with event: NSEvent) { onPress?() }
}

@MainActor
private final class ShelfRow: NSView {
    let task: ShelfTask
    var onPress: (() -> Void)?
    private let go: NSTextField

    override var isFlipped: Bool { true }

    init(task: ShelfTask, key: String?, width: CGFloat) {
        self.task = task
        go = NSTextField(labelWithString: (key.map { "\($0) · " } ?? "") + "Pull →")
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 52))
        wantsLayer = true
        layer?.cornerRadius = 8
        let name = NSTextField(labelWithString: task.title)
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        name.lineBreakMode = .byTruncatingTail
        name.frame = NSRect(x: 30, y: 8, width: width - 140, height: 18)
        let status = task.status.map { $0 == "needs-you" ? "needs you" : $0 }
        let sub = NSTextField(labelWithString: [task.agent, status, task.detail.isEmpty ? nil : task.detail, "parked " + ShelfTask.ago(task.parkedAt)].compactMap { $0 }.joined(separator: " · "))
        sub.font = .systemFont(ofSize: 12)
        sub.textColor = .secondaryLabelColor
        sub.lineBreakMode = .byTruncatingTail
        sub.frame = NSRect(x: 30, y: 27, width: width - 140, height: 17)
        go.font = .systemFont(ofSize: 12)
        go.textColor = .controlAccentColor
        go.sizeToFit()
        go.frame.origin = NSPoint(x: width - go.frame.width - 10, y: (52 - go.frame.height) / 2)
        go.isHidden = true
        [name, sub, go].forEach(addSubview)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draw(_ dirtyRect: NSRect) {
        ShelfTask.color(task.status).setFill()
        NSBezierPath(ovalIn: NSRect(x: 10, y: bounds.midY - 4, width: 8, height: 8)).fill()
    }

    override func mouseEntered(with event: NSEvent) {
        go.isHidden = false
        layer?.backgroundColor = resolved(.quaternaryLabelColor)
    }

    override func mouseExited(with event: NSEvent) {
        go.isHidden = true
        layer?.backgroundColor = nil
    }

    override func mouseDown(with event: NSEvent) { onPress?() }
}
