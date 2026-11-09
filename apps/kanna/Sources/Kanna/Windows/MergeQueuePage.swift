import AppKit
import KannaWindows

struct QueueEntry: Equatable {
    let id: String
    let task: String
    let name: String
    let branch: String?
    let state: String
    let since: Double?
    let endedAt: Double?
    let main: String?
    let base: String?
    let landed: String?
    let why: String?
    let terminal: String?
    var after: String?

    init(_ json: JSON) {
        let doc = json["doc"]
        id = json["id"]?.string ?? ""
        task = json["task"]?.string ?? ""
        name = doc?["name"]?.string ?? task
        branch = doc?["branch"]?.string
        state = json["state"]?.string ?? ""
        since = doc?["since"]?.number
        endedAt = json["endedAt"]?.number
        main = doc?["main"]?.string
        base = doc?["base"]?.string
        landed = doc?["landed"]?.string
        why = doc?["why"]?.string
        terminal = json["terminal"]?.string
    }

    var live: Bool { ["queued", "testing", "landing"].contains(state) }

    static func short(_ commit: String?) -> String? { commit.map { String($0.prefix(8)) } }

    @MainActor static func clock(from start: Double?, to end: Double? = nil) -> String? {
        guard let start else { return nil }
        let seconds = Int(max(0, (end ?? Date().timeIntervalSince1970 + DeskTask.skew) - start))
        return seconds < 60 ? "\(seconds)s" : "\(seconds / 60)m\(String(format: "%02d", seconds % 60))s"
    }

    @MainActor var said: String {
        let main = self.main ?? "main"
        switch state {
        case "testing":
            let on = ["testing on \(main)", Self.short(base)].compactMap { $0 }.joined(separator: " ")
            return [on + (branch.map { " + \($0)" } ?? ""), Self.clock(from: since)].compactMap { $0 }.joined(separator: " · ")
        case "landing": return why.map { "landing · waits: \($0)" } ?? "landing on \(main)"
        case "failed": return ["failed", why, DeskTask.ago(endedAt), "back on the Shelf"].compactMap { $0 }.joined(separator: " · ")
        case "landed": return [Self.short(landed).map { "landed as \($0)" } ?? "landed", DeskTask.ago(endedAt), Self.clock(from: since, to: endedAt).map { "tested in \($0)" }].compactMap { $0 }.joined(separator: " · ")
        default: return ([after.map { "waiting · tested on top of \u{201C}\($0)\u{201D}" } ?? "waiting"] + [branch].compactMap { $0 }).joined(separator: " · ")
        }
    }
}

struct QueueLog: Equatable {
    let id: String
    let note: String?
    let text: String?
}

@MainActor
final class MergeQueuePage: NSView {
    static let title = "Merge queue"
    private let scroller = NSScrollView()
    private let document = QueueDocument()
    private let rule = QueueRule()
    private let head = NSTextField(labelWithString: "")
    private let tail = NSTextField(labelWithString: "")
    private let waiting = NSTextField(wrappingLabelWithString: "")
    private let logScroller = NSScrollView()
    private let logText = NSTextView()
    private var shown: [QueueEntry]?
    private var picked: String?
    private var log: QueueLog?
    private var hosted: PaneView?
    private var asked: String?
    private var body = NSRect.zero
    var onTakeOut: ((String) -> Void)?
    var onPick: (() -> Void)?
    var onLog: ((String) -> Void)?

    override var isFlipped: Bool { true }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        scroller.drawsBackground = false
        scroller.hasVerticalScroller = true
        scroller.documentView = document
        addSubview(scroller)
        addSubview(rule)
        for field in [head, tail] {
            field.font = .systemFont(ofSize: 12)
            field.textColor = .secondaryLabelColor
            field.lineBreakMode = .byTruncatingTail
            addSubview(field)
        }
        waiting.alignment = .center
        waiting.textColor = .secondaryLabelColor
        waiting.font = .systemFont(ofSize: 13)
        addSubview(waiting)
        logText.isEditable = false
        logText.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        logText.textContainerInset = NSSize(width: 12, height: 10)
        logText.drawsBackground = false
        logText.autoresizingMask = [.width]
        logText.isVerticallyResizable = true
        logScroller.drawsBackground = false
        logScroller.hasVerticalScroller = true
        logScroller.documentView = logText
        addSubview(logScroller)
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    private func paint() { layer?.backgroundColor = resolved(.textBackgroundColor) }

    var entries: [QueueEntry] { shown ?? [] }
    var selected: QueueEntry? { entries.first { $0.id == picked } ?? entries.first }
    var wantedTerminal: String? { isHidden ? nil : selected?.terminal }
    var hostedTerminal: String? { hosted?.superview === self ? hosted?.id : nil }
    var detail: String { head.stringValue }
    var logShown: String? { logScroller.isHidden ? nil : logText.string }
    var waitingText: String? { waiting.isHidden ? nil : waiting.stringValue }

    func center(of entry: String, takeOut: Bool = false) -> NSPoint? {
        guard let row = document.subviews.compactMap({ $0 as? QueueRowView }).first(where: { $0.entry.id == entry || $0.entry.task == entry }) else { return nil }
        document.scrollToVisible(row.frame)
        return row.convert(takeOut ? row.takeOutCenter : NSPoint(x: 40, y: row.bounds.midY), to: nil)
    }

    func show(_ entries: [QueueEntry]?) {
        var entries = entries
        if var list = entries {
            var before: String?
            for index in list.indices where list[index].live {
                list[index].after = before
                before = list[index].name
            }
            entries = list
        }
        shown = entries
        lay()
    }

    func pick(_ entry: String) {
        picked = entries.first { $0.id == entry || $0.task == entry }?.id ?? entry
        lay()
        onPick?()
    }

    func show(log: QueueLog) {
        self.log = log
        asked = nil
        layDetail()
    }

    func host(_ view: PaneView?) {
        if hosted !== view, hosted?.superview === self { hosted?.removeFromSuperview() }
        hosted = view
        guard let view else { return }
        if view.superview !== self { addSubview(view) }
        view.grip.isHidden = true
        view.focused = false
        view.autoresizingMask = []
        view.frame = body
    }

    override func resizeSubviews(withOldSize oldSize: NSSize) {
        super.resizeSubviews(withOldSize: oldSize)
        if oldSize != bounds.size { lay() }
    }

    private func label(_ text: String, weight: NSFont.Weight = .regular, color: NSColor = .labelColor) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = .systemFont(ofSize: 12, weight: weight)
        field.textColor = color
        field.sizeToFit()
        document.addSubview(field)
        return field
    }

    private func header(_ title: String, detail: String, count: String, y: CGFloat, left: CGFloat, width: CGFloat) -> CGFloat {
        let name = label(title, weight: .semibold)
        name.frame.origin = NSPoint(x: left, y: y)
        let more = label(detail, color: .secondaryLabelColor)
        more.frame.origin = NSPoint(x: name.frame.maxX + 4, y: y)
        let tally = label(count, color: .secondaryLabelColor)
        tally.frame.origin = NSPoint(x: left + width - tally.frame.width, y: y)
        return y + name.frame.height + 8
    }

    private func lay() {
        document.subviews.forEach { $0.removeFromSuperview() }
        let left: CGFloat = 32, width = max(320, bounds.width - left * 2)
        let live = entries.filter(\.live), past = entries.filter { !$0.live }
        let chosen = selected?.id
        var y: CGFloat = 22
        let main = live.first?.main ?? past.first?.main ?? "main"
        let at = live.first.flatMap { QueueEntry.short($0.base) }.map { " \($0)" } ?? ""
        y = header("Merging", detail: "into \(main)\(at)", count: live.isEmpty ? "" : "\(live.count) in line", y: y, left: left, width: width)
        if live.isEmpty {
            let none = label(shown == nil ? "Loading…" : "Nothing is merging.", color: .secondaryLabelColor)
            none.frame.origin = NSPoint(x: left, y: y + 4)
            y += none.frame.height + 12
        }
        for entry in live + past {
            if entry.id == past.first?.id {
                y = header("Recent", detail: "", count: "landed or failed", y: y + 16, left: left, width: width)
            }
            let row = QueueRowView(entry: entry, width: width + 20, selected: entry.id == chosen)
            row.onPick = { [weak self] in self?.pick(entry.id) }
            row.onTakeOut = { [weak self] in self?.onTakeOut?(entry.task) }
            row.frame.origin = NSPoint(x: left - 10, y: y)
            document.addSubview(row)
            y += row.frame.height
        }
        y += 12
        let list = min(y, (bounds.height * 0.56).rounded())
        scroller.frame = NSRect(x: 0, y: 0, width: bounds.width, height: list)
        document.frame = NSRect(x: 0, y: 0, width: scroller.contentSize.width, height: max(y, list))
        rule.frame = NSRect(x: 0, y: list, width: bounds.width, height: 1)
        layDetail()
    }

    private func layDetail() {
        let top = rule.frame.maxY
        let entry = selected
        let bar: CGFloat = entry == nil ? 0 : 30
        body = NSRect(x: 0, y: top + bar, width: bounds.width, height: max(0, bounds.height - top - bar))
        var left = "", right = "", wait: String?, text: String?
        if let entry {
            switch entry.state {
            case "testing", "landing":
                left = "Tester · " + [entry.name, entry.branch].compactMap { $0 }.joined(separator: " · ")
                right = entry.state == "landing" ? "landing" : ["running", QueueEntry.clock(from: entry.since)].compactMap { $0 }.joined(separator: " · ")
                if entry.terminal == nil { wait = entry.state == "landing" ? (entry.why.map { "Its tests are done. It waits to land: \($0)" } ?? "Its tests are done. It's landing.") : "Its tester is starting." }
            case "queued":
                left = [entry.name, entry.branch].compactMap { $0 }.joined(separator: " · ")
                right = "waiting"
                wait = entry.after.map { "Waiting. Its tester starts once \u{201C}\($0)\u{201D} lands or fails, and its terminal shows here." } ?? "Waiting. Its tester starts next, and its terminal shows here."
            default:
                left = "Tester's answer and test.log · " + [entry.name, entry.branch].compactMap { $0 }.joined(separator: " · ")
                right = entry.state == "failed" ? "failed \(DeskTask.ago(entry.endedAt))" : "landed \(DeskTask.ago(entry.endedAt))"
                if log?.id == entry.id {
                    let parts = [log?.note ?? (entry.state == "failed" ? entry.why : nil), log?.text.map { "──────── test.log ────────\n\($0)" }].compactMap { $0 }
                    text = parts.isEmpty ? "It kept no answer and no log." : parts.joined(separator: "\n\n")
                } else {
                    wait = "Loading its log…"
                    if asked != entry.id {
                        asked = entry.id
                        onLog?(entry.id)
                    }
                }
            }
        } else {
            wait = shown == nil ? "" : "Nothing is merging. Tell a ready task's agent to merge and it shows here."
        }
        head.stringValue = left
        tail.stringValue = right
        tail.sizeToFit()
        tail.frame.origin = NSPoint(x: bounds.width - tail.frame.width - 14, y: top + 7)
        head.frame = NSRect(x: 14, y: top + 7, width: max(0, tail.frame.minX - 26), height: 16)
        head.isHidden = entry == nil
        tail.isHidden = entry == nil
        waiting.isHidden = wait == nil
        if let wait {
            waiting.stringValue = wait
            let size = waiting.sizeThatFits(NSSize(width: max(100, body.width - 120), height: .greatestFiniteMagnitude))
            waiting.frame = NSRect(x: ((body.width - size.width) / 2).rounded(), y: body.minY + ((body.height - size.height) / 2).rounded(), width: size.width, height: size.height)
        }
        logScroller.isHidden = text == nil
        logScroller.frame = body
        if let text, logText.string != text {
            logText.string = text
            logText.textColor = .labelColor
            logText.frame.size.width = logScroller.contentSize.width
        }
        if entry?.terminal == nil, hosted?.superview === self {
            hosted?.removeFromSuperview()
            hosted = nil
        }
        hosted?.frame = body
    }
}

private final class QueueDocument: NSView {
    override var isFlipped: Bool { true }
}

private final class QueueRule: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        bounds.fill()
    }
}

@MainActor
private final class QueueRowView: NSView {
    let entry: QueueEntry
    var onPick: (() -> Void)?
    var onTakeOut: (() -> Void)?
    private let go = NSTextField(labelWithString: "Take out")
    private let selected: Bool
    private var hover = false

    override var isFlipped: Bool { true }

    init(entry: QueueEntry, width: CGFloat, selected: Bool) {
        self.entry = entry
        self.selected = selected
        let tall: CGFloat = entry.live ? 52 : 30
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: tall))
        wantsLayer = true
        layer?.cornerRadius = 8
        let name = NSTextField(labelWithString: entry.name)
        name.font = .systemFont(ofSize: 13, weight: entry.state == "queued" || !entry.live ? .medium : .semibold)
        name.lineBreakMode = .byTruncatingTail
        let sub = NSTextField(labelWithString: "")
        let small = NSFont.systemFont(ofSize: 12)
        let said = NSMutableAttributedString(string: entry.said, attributes: [.font: small, .foregroundColor: entry.state == "landing" && entry.why != nil ? NSColor.systemOrange : NSColor.secondaryLabelColor])
        if entry.state == "failed" { said.addAttribute(.foregroundColor, value: NSColor.systemRed, range: NSRange(location: 0, length: 6)) }
        sub.attributedStringValue = said
        sub.lineBreakMode = .byTruncatingTail
        if entry.live {
            name.frame = NSRect(x: 30, y: 8, width: width - 140, height: 18)
            sub.frame = NSRect(x: 30, y: 27, width: width - 140, height: 17)
            go.font = small
            go.sizeToFit()
            go.frame.origin = NSPoint(x: width - go.frame.width - 10, y: (tall - go.frame.height) / 2)
            addSubview(go)
        } else {
            name.sizeToFit()
            name.frame = NSRect(x: 30, y: 6, width: min(name.frame.width, (width - 50) * 0.5), height: 18)
            sub.frame = NSRect(x: name.frame.maxX + 10, y: 7, width: max(0, width - name.frame.maxX - 20), height: 17)
        }
        [name, sub].forEach(addSubview)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var takeOutCenter: NSPoint { NSPoint(x: go.frame.midX, y: go.frame.midY) }

    override func draw(_ dirtyRect: NSRect) {
        let ring = NSBezierPath(ovalIn: NSRect(x: 10.5, y: bounds.midY - 4, width: 8, height: 8))
        switch entry.state {
        case "queued":
            NSColor.controlAccentColor.setStroke()
            ring.lineWidth = 1.4
            ring.stroke()
        case "failed":
            NSColor.systemRed.setFill()
            ring.fill()
        case "landed":
            NSColor.systemGreen.setFill()
            ring.fill()
        default:
            DeskTask.color("working").setFill()
            ring.fill()
        }
    }

    private func paint() {
        layer?.backgroundColor = selected ? resolved(.quaternaryLabelColor) : hover ? resolved(.quaternaryLabelColor.withAlphaComponent(0.5)) : nil
        go.textColor = hover ? .controlAccentColor : .tertiaryLabelColor
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
        let point = convert(event.locationInWindow, from: nil)
        if entry.live, go.frame.insetBy(dx: -8, dy: -8).contains(point) { return onTakeOut?() ?? () }
        onPick?()
    }
}
