import AppKit
import KannaCode
import KannaOverview

@MainActor
public final class LogPaneView: NSView {
    public static let detailWidth: CGFloat = 340
    public static let detailBelow: CGFloat = 820

    public var onFocus: (() -> Void)?
    public var onZoom: (() -> Void)?
    public var onOpenDiff: ((String, [String]) -> Void)?
    public var onCommit: ((String, @escaping (Data?, String?) -> Void) -> Void)?
    public var contextMenu: (() -> NSMenu?)?
    public var keyView: NSView { rows }
    public var now: () -> Int = { Int(Date().timeIntervalSince1970) }

    public private(set) var result: LogResult?
    public private(set) var graph = LogGraph(rows: [], lanes: 0)
    public private(set) var serial = 0
    public private(set) var pending: (result: LogResult, serial: Int)?
    public private(set) var notice: String?
    public private(set) var open: Set<String>
    public private(set) var selected: String?
    public private(set) var fresh: Set<String> = []
    public private(set) var query = ""
    public private(set) var finding = false
    public private(set) var hits: [String] = []
    public private(set) var detail: CommitDetail?
    public private(set) var detailShown = false
    public var stale: Bool { pending != nil }
    public var zoom: CGFloat { fontSize / CodeTheme.fontSize }

    let header = LogHeader()
    let rows = LogRowsView()
    private let scroller = NSScrollView()
    private let detailScroll = NSScrollView()
    private let detailText = NSTextView()
    private let findBar = LogFindBar()
    private let message = NSTextField(labelWithString: "")
    private var fontSize = CodeTheme.fontSize
    private var asked: String?
    private var freshTimer: Timer?

    public init(zoom: CGFloat = 1) {
        open = []
        fontSize = Self.clamp((CodeTheme.fontSize * zoom * 2).rounded() / 2)
        super.init(frame: .zero)
        rows.pane = self
        scroller.documentView = rows
        scroller.hasVerticalScroller = true
        scroller.hasHorizontalScroller = false
        scroller.autohidesScrollers = true
        scroller.drawsBackground = true
        scroller.backgroundColor = LogTheme.background
        addSubview(scroller)
        detailText.isEditable = false
        detailText.isSelectable = true
        detailText.drawsBackground = true
        detailText.backgroundColor = LogTheme.background
        detailText.textContainerInset = NSSize(width: 12, height: 12)
        detailText.isVerticallyResizable = true
        detailText.autoresizingMask = [.width]
        detailText.textContainer?.widthTracksTextView = true
        detailText.delegate = rows
        detailScroll.documentView = detailText
        detailScroll.hasVerticalScroller = true
        detailScroll.autohidesScrollers = true
        detailScroll.drawsBackground = true
        detailScroll.backgroundColor = LogTheme.background
        addSubview(detailScroll)
        message.textColor = LogTheme.muted
        message.alignment = .center
        message.font = LogTheme.sans(size: 13)
        message.isHidden = true
        message.maximumNumberOfLines = 4
        addSubview(message)
        header.onRefresh = { [weak self] in self?.refresh() }
        addSubview(header)
        findBar.isHidden = true
        findBar.onChange = { [weak self] text in self?.find(text) }
        findBar.onStep = { [weak self] forward in self?.findStep(forward: forward) }
        findBar.onClose = { [weak self] in self?.closeFind() }
        addSubview(findBar)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    public func stop() {
        freshTimer?.invalidate()
        freshTimer = nil
    }

    static func clamp(_ size: CGFloat) -> CGFloat { min(max(size, 9), 32) }

    var metrics: LogMetrics { LogMetrics(fontSize: fontSize, lanes: graph.lanes) }

    public override var isFlipped: Bool { true }

    public override func layout() {
        super.layout()
        let headerHeight: CGFloat = 34
        header.frame = NSRect(x: 0, y: 0, width: bounds.width, height: headerHeight)
        detailShown = bounds.width >= Self.detailBelow * zoom
        let side = detailShown ? Self.detailWidth * max(1, zoom * 0.9) : 0
        let body = NSRect(x: 0, y: headerHeight, width: bounds.width - side, height: max(0, bounds.height - headerHeight))
        scroller.frame = body
        detailScroll.isHidden = !detailShown
        detailScroll.frame = NSRect(x: body.maxX + 1, y: headerHeight, width: max(0, side - 1), height: body.height)
        detailText.frame.size.width = detailScroll.contentSize.width
        rows.frame = NSRect(x: 0, y: 0, width: scroller.contentSize.width, height: max(scroller.contentSize.height, CGFloat(graph.rows.count) * metrics.row))
        message.frame = NSRect(x: 20, y: headerHeight + body.height / 2 - 40, width: max(0, body.width - 40), height: 80)
        let findWidth: CGFloat = 330
        findBar.frame = NSRect(x: body.maxX - findWidth - 12, y: headerHeight + 6, width: findWidth, height: 30)
        needsDisplay = true
    }

    public override func draw(_ dirtyRect: NSRect) {
        LogTheme.line.setFill()
        NSRect(x: 0, y: 33, width: bounds.width, height: 1).fill()
        if detailShown { NSRect(x: scroller.frame.maxX, y: 34, width: 1, height: bounds.height - 34).fill() }
    }

    public func receive(_ data: Data, serial: Int) {
        guard let result = LogResult.decode(data) else { return }
        receive(result, serial: serial)
    }

    public func receive(_ result: LogResult, serial: Int) {
        let reading = self.result.map { $0.error == nil && !$0.rows.isEmpty } ?? false
        if !reading {
            return show(result, serial: serial)
        }
        if let error = result.error {
            notice = error
        } else {
            notice = nil
            pending = result == self.result ? nil : (result, serial)
        }
        header.mark(held: heldText, notice: notice)
    }

    public var heldText: String? {
        guard let pending, let result else { return nil }
        let shown = Set(result.rows.map(\.hash))
        let new = pending.result.rows.filter { !shown.contains($0.hash) }.count
        return new > 0 ? "\(new) new commit\(new == 1 ? "" : "s")" : "changed"
    }

    public func refresh() {
        guard let pending else { return }
        show(pending.result, serial: pending.serial)
    }

    public func show(_ result: LogResult, serial: Int) {
        let before = self.result.map { Set($0.rows.map(\.hash)) }
        let top = topHash
        pending = nil
        notice = nil
        self.result = result
        self.serial = serial
        let folds = Set(result.rows.filter { $0.fold > 0 }.map(\.hash))
        open = open.intersection(folds)
        if let before {
            fresh = Set(result.rows.map(\.hash).filter { !before.contains($0) })
            freshTimer?.invalidate()
            if !fresh.isEmpty {
                freshTimer = Timer.scheduledTimer(withTimeInterval: 2.6, repeats: false) { [weak self] _ in
                    MainActor.assumeIsolated {
                        self?.fresh = []
                        self?.rows.needsDisplay = true
                    }
                }
            }
        }
        rebuild()
        if !graph.rows.contains(where: { $0.hash == selected }) { selected = graph.rows.first?.hash }
        if let top, let index = graph.rows.firstIndex(where: { $0.hash == top }) { scroll(toRow: index, top: true) }
        if finding { hits = matches(query) }
        header.show(result, shown: graph.rows.count)
        header.mark(held: nil, notice: nil)
        message.isHidden = result.error == nil && !result.rows.isEmpty
        message.stringValue = result.error ?? "No commits\n\(result.command)"
        message.textColor = result.error == nil ? LogTheme.muted : LogTheme.bad
        askDetail()
    }

    private func rebuild() {
        guard let result else { return }
        graph = LogGraph.make(result, open: open)
        needsLayout = true
        layoutSubtreeIfNeeded()
        rows.needsDisplay = true
        header.show(result, shown: graph.rows.count)
    }

    var topHash: String? {
        guard !graph.rows.isEmpty else { return nil }
        let index = Int(scroller.contentView.bounds.minY / metrics.row)
        return graph.rows[safe: max(0, index)]?.hash
    }

    public var selectedIndex: Int? { selected.flatMap { hash in graph.rows.firstIndex { $0.hash == hash } } }

    func row(of hash: String) -> LogRow? { result?.rows.first { $0.hash == hash } }

    public func select(_ hash: String, scroll scrolling: Bool = true) {
        guard let index = graph.rows.firstIndex(where: { $0.hash == hash }) else { return }
        selected = hash
        if scrolling { scroll(toRow: index, top: false) }
        rows.needsDisplay = true
        askDetail()
        findBar.count = findLabel
    }

    func scroll(toRow index: Int, top: Bool) {
        let height = metrics.row
        let y = CGFloat(index) * height
        let clip = scroller.contentView
        var origin = clip.bounds.origin
        if top {
            origin.y = y
        } else if y < clip.bounds.minY + height {
            origin.y = y - height * 2
        } else if y + height > clip.bounds.maxY - height {
            origin.y = y + height * 3 - clip.bounds.height
        } else {
            return
        }
        origin.y = max(0, min(origin.y, rows.frame.height - clip.bounds.height))
        clip.scroll(to: origin)
        scroller.reflectScrolledClipView(clip)
    }

    public func toggle(_ merge: String, open wanted: Bool? = nil) {
        guard let row = row(of: merge), row.fold > 0 else { return }
        let on = wanted ?? !open.contains(merge)
        guard on != open.contains(merge) else { return }
        if on { open.insert(merge) } else { open.remove(merge) }
        let top = topHash
        rebuild()
        if let top, let index = graph.rows.firstIndex(where: { $0.hash == top }) { scroll(toRow: index, top: true) }
        if let selected, !graph.rows.contains(where: { $0.hash == selected }) { select(merge) }
    }

    public func reveal(_ hash: String) {
        guard let side = row(of: hash)?.side, !open.contains(side) else { return }
        toggle(side, open: true)
    }

    public func press(_ key: String) -> Bool {
        let list = graph.rows
        guard !list.isEmpty else { return false }
        let at = selectedIndex
        switch key {
        case "j", "\u{F701}": select(list[at.map { min($0 + 1, list.count - 1) } ?? 0].hash)
        case "k", "\u{F700}": select(list[at.map { max($0 - 1, 0) } ?? 0].hash)
        case "g": select(list[0].hash)
        case "G": select(list[list.count - 1].hash)
        case "l", "\u{F703}":
            if let selected { toggle(selected, open: true) }
        case "h", "\u{F702}":
            guard let selected else { return true }
            if open.contains(selected) {
                toggle(selected, open: false)
            } else if let side = row(of: selected)?.side, open.contains(side) {
                toggle(side, open: false)
                select(side)
            }
        case "\r": openDiff()
        default: return false
        }
        return true
    }

    public func openDiff(_ hash: String? = nil) {
        guard let hash = hash ?? selected, let row = row(of: hash) else { return }
        let short = String(hash.prefix(12))
        onOpenDiff?(hash, row.parents.isEmpty ? ["4b825dc642cb6eb9a060e54bf8d69288fbee4904", short] : ["\(short)^!"])
    }

    public func copyHash() {
        guard let selected else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(selected, forType: .string)
    }

    public func copySubject() {
        guard let subject = selected.flatMap(row(of:))?.subject else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(subject, forType: .string)
    }

    public func zoom(by step: Int) {
        let target = step == 0 ? CodeTheme.fontSize : Self.clamp(fontSize + CGFloat(step))
        guard target != fontSize else { return }
        let top = topHash
        fontSize = target
        rebuild()
        if let top, let index = graph.rows.firstIndex(where: { $0.hash == top }) { scroll(toRow: index, top: true) }
        if let detail { showDetail(detail) }
        onZoom?()
    }

    public func showFind() {
        finding = true
        findBar.isHidden = false
        findBar.text = query
        window?.makeFirstResponder(findBar.field)
        hits = matches(query)
        findBar.count = findLabel
        rows.needsDisplay = true
    }

    public func closeFind() {
        finding = false
        query = ""
        hits = []
        findBar.isHidden = true
        window?.makeFirstResponder(rows)
        rows.needsDisplay = true
    }

    public func find(_ text: String) {
        query = text
        finding = true
        findBar.isHidden = false
        if findBar.text != text { findBar.text = text }
        hits = matches(text)
        findBar.count = findLabel
        rows.needsDisplay = true
    }

    public func findStep(forward: Bool) {
        guard !hits.isEmpty else { return NSSound.beep() }
        let order = result?.rows.map(\.hash) ?? []
        let here = selected.flatMap { order.firstIndex(of: $0) } ?? -1
        let positions = hits.compactMap { order.firstIndex(of: $0) }
        let target = forward ? positions.first { $0 > here } ?? positions.first : positions.last { $0 < here } ?? positions.last
        guard let target else { return }
        let hash = order[target]
        reveal(hash)
        select(hash)
    }

    func matches(_ text: String) -> [String] {
        let query = FindQuery(text.trimmingCharacters(in: .whitespaces))
        guard !query.isEmpty, let result else { return [] }
        return result.rows.filter { query.matches($0.subject) || $0.hash.hasPrefix(query.text.lowercased()) }.map(\.hash)
    }

    var findLabel: String {
        guard !query.trimmingCharacters(in: .whitespaces).isEmpty else { return "" }
        guard !hits.isEmpty else { return "no match" }
        let index = selected.flatMap { hits.firstIndex(of: $0) }
        return "\(index.map { String($0 + 1) } ?? "–") of \(hits.count)"
    }

    private func askDetail() {
        guard let hash = selected, asked != hash else { return }
        asked = hash
        if detail?.hash == hash { return showDetail(detail!) }
        guard let onCommit else { return }
        onCommit(hash) { [weak self] data, error in
            guard let self, self.selected == hash else { return }
            if let data, let found = CommitDetail.decode(data) {
                detail = found
                showDetail(found)
            } else {
                detail = nil
                detailText.textStorage?.setAttributedString(NSAttributedString(string: error ?? "", attributes: [.foregroundColor: LogTheme.bad, .font: LogTheme.sans(size: 12)]))
            }
        }
    }

    func showDetail(_ detail: CommitDetail) {
        let text = LogDetailText.make(detail, row: row(of: detail.hash), labels: graph.rows.first { $0.hash == detail.hash }?.labels ?? [], fold: row(of: detail.hash).map { ($0.fold, open.contains(detail.hash)) },
                                      now: now(), zoom: zoom)
        detailText.textStorage?.setAttributedString(text)
        detailText.scroll(.zero)
    }

    func follow(link: String) {
        if link.hasPrefix("kanna-log-commit:") {
            let hash = String(link.dropFirst("kanna-log-commit:".count))
            reveal(hash)
            select(hash)
        } else if link == "kanna-log-copy:" {
            copyHash()
        } else if link == "kanna-log-diff:" {
            openDiff()
        } else if link.hasPrefix("kanna-log-fold:") {
            toggle(String(link.dropFirst("kanna-log-fold:".count)))
            if let detail { showDetail(detail) }
        }
    }

    public var debugRows: [[String: Any]] {
        guard let result else { return [] }
        return graph.rows.map { lane in
            let row = result.rows[lane.row]
            return ["hash": row.hash, "subject": row.subject, "lane": lane.lane, "color": lane.color, "parents": lane.outgoing.map(\.lane), "hooks": lane.outgoing.filter(\.hook).count,
                    "fold": row.fold, "folded": lane.folded, "open": lane.open, "spine": row.spine, "refs": row.refs,
                    "labels": lane.labels.map { ["id": $0.id, "name": $0.name, "place": $0.place, "ahead": $0.ahead, "behind": $0.behind, "merged": $0.merged] as [String: Any] }]
        }
    }
}

struct LogMetrics {
    let fontSize: CGFloat
    let lanes: Int

    var scale: CGFloat { fontSize / CodeTheme.fontSize }
    var row: CGFloat { (26 * scale).rounded() }
    var pad: CGFloat { 12 * scale }
    var lane: CGFloat { lanes > 16 ? max(8, (16 * 14 / CGFloat(lanes)).rounded(.down)) * scale : 14 * scale }
    var graph: CGFloat { pad * 2 + CGFloat(max(0, lanes - 1)) * lane }
    var hash: CGFloat { 72 * scale }
    var author: CGFloat { 110 * scale }
    var when: CGFloat { 70 * scale }
    var chip: CGFloat { (18 * scale).rounded() }

    func x(_ lane: Int) -> CGFloat { pad + CGFloat(lane) * self.lane }
}

@MainActor
final class LogHeader: NSView {
    var onRefresh: (() -> Void)?
    private let command = NSTextField(labelWithString: "")
    private let counts = NSTextField(labelWithString: "")
    private let held = NSButton(title: "", target: nil, action: nil)
    private let notice = NSTextField(labelWithString: "")
    private(set) var heldText: String?

    override init(frame: NSRect) {
        super.init(frame: frame)
        command.font = LogTheme.mono(size: 11.5, weight: .medium)
        command.textColor = LogTheme.text
        command.lineBreakMode = .byTruncatingTail
        counts.font = LogTheme.sans(size: 12)
        counts.textColor = LogTheme.muted
        held.isBordered = false
        held.target = self
        held.action = #selector(refresh)
        held.toolTip = "⌘R shows them"
        held.isHidden = true
        notice.font = LogTheme.sans(size: 12)
        notice.textColor = LogTheme.bad
        notice.lineBreakMode = .byTruncatingTail
        notice.isHidden = true
        [command, counts, held, notice].forEach(addSubview)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }

    @objc private func refresh() { onRefresh?() }

    func show(_ result: LogResult, shown: Int) {
        command.stringValue = result.command
        let folded = result.rows.count - shown
        var text = "\(result.rows.count) commit\(result.rows.count == 1 ? "" : "s")"
        if folded > 0 { text += " · \(folded) folded" }
        if result.more { text += " · older not shown" }
        counts.stringValue = text
        needsLayout = true
    }

    func mark(held text: String?, notice error: String?) {
        heldText = text
        held.isHidden = text == nil
        held.attributedTitle = NSAttributedString(string: text ?? "", attributes: [.font: LogTheme.sans(size: 11.5, weight: .semibold), .foregroundColor: LogTheme.accent, .underlineStyle: NSUnderlineStyle.single.rawValue])
        notice.stringValue = error.map { $0.split(separator: "\n").first.map(String.init) ?? $0 } ?? ""
        notice.toolTip = error
        notice.isHidden = error == nil
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let y = (bounds.height - 18) / 2
        counts.sizeToFit()
        var right = bounds.width - 10 - counts.frame.width
        counts.frame.origin = NSPoint(x: right, y: y)
        if !held.isHidden {
            held.sizeToFit()
            right -= held.frame.width + 10
            held.frame = NSRect(x: right, y: y - 1, width: held.frame.width, height: 20)
        }
        if !notice.isHidden {
            let width = min(260, max(0, right - 200))
            right -= width + 10
            notice.frame = NSRect(x: right, y: y, width: width, height: 18)
        }
        command.frame = NSRect(x: 12, y: y, width: max(0, right - 22), height: 18)
    }
}

@MainActor
final class LogFindBar: NSView, NSSearchFieldDelegate {
    var onChange: ((String) -> Void)?
    var onStep: ((Bool) -> Void)?
    var onClose: (() -> Void)?
    let field = NSSearchField()
    private let label = NSTextField(labelWithString: "")
    var text: String {
        get { field.stringValue }
        set { field.stringValue = newValue }
    }
    var count: String {
        get { label.stringValue }
        set { label.stringValue = newValue }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 8
        layer?.borderWidth = 1
        field.placeholderString = "Find subjects and hashes"
        field.delegate = self
        field.sendsSearchStringImmediately = true
        field.target = self
        field.action = #selector(changed)
        label.font = LogTheme.sans(size: 11.5)
        label.textColor = LogTheme.muted
        label.alignment = .right
        [field, label].forEach(addSubview)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func updateLayer() {
        layer?.backgroundColor = LogTheme.panel.cgColor
        layer?.borderColor = LogTheme.line.cgColor
    }

    override var wantsUpdateLayer: Bool { true }

    override func layout() {
        super.layout()
        field.frame = NSRect(x: 6, y: 4, width: bounds.width - 96, height: 22)
        label.frame = NSRect(x: bounds.width - 86, y: 7, width: 78, height: 16)
    }

    @objc private func changed() { onChange?(field.stringValue) }

    func controlTextDidChange(_ notification: Notification) { onChange?(field.stringValue) }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            onStep?(!(NSApp.currentEvent?.modifierFlags.contains(.shift) ?? false))
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            onClose?()
            return true
        default:
            return false
        }
    }
}
