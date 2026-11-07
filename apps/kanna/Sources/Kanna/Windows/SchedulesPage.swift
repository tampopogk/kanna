import AppKit
import KannaWindows

struct ScheduleFiring: Equatable {
    let at: String
    let how: String
    let state: String
    let delivered: String?
    let detail: String?
    let reply: String?

    var stateText: String {
        switch state {
        case "delivered": return "delivered \(delivered?.split(separator: " ").last.map(String.init) ?? "")".trimmingCharacters(in: .whitespaces)
        case "skipped": return "no message"
        case "failed": return "not left"
        case "undelivered": return "not delivered"
        default: return state
        }
    }

    var note: String {
        if let reply { return reply }
        if let detail { return detail }
        if state == "skipped" { return "its last message still waited" }
        return how == "run" ? "run by hand" : ""
    }

    var tone: NSColor {
        switch state {
        case "delivered": return .systemGreen
        case "waiting": return .systemOrange
        case "failed", "undelivered": return .systemRed
        default: return .secondaryLabelColor
        }
    }
}

struct ScheduleRow: Equatable {
    let id: String
    let name: String
    let repo: String?
    let rule: String
    let target: String
    let next: String
    let waiting: Bool
    let paused: Bool
    let prompt: String
    let firings: [ScheduleFiring]

    init(_ json: JSON) {
        id = json["schedule"]?.string ?? ""
        name = json["name"]?.string ?? id
        repo = json["repo"]?.string
        rule = json["rule"]?.string ?? ""
        target = json["targetName"]?.string ?? json["target"]?.string ?? ""
        next = json["nextText"]?.string ?? ""
        waiting = json["waiting"]?.bool == true
        paused = json["paused"]?.bool == true
        prompt = json["prompt"]?.string ?? ""
        firings = (json["firings"]?.array ?? []).map {
            ScheduleFiring(at: $0["atText"]?.string ?? "", how: $0["how"]?.string ?? "rule", state: $0["state"]?.string ?? "", delivered: $0["deliveredText"]?.string,
                           detail: $0["detail"]?.string, reply: $0["reply"]?.string)
        }
    }
}

@MainActor
final class SchedulesPage: NSView {
    private let scroller = NSScrollView()
    private let document = SchedulesDocument()
    private var shown: [ScheduleRow]?
    private var here: String?
    private var opened: Set<String> = []
    private var confirming: String?
    private var controls: [String: NSView] = [:]
    var onAction: ((String, String) -> Void)?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        scroller.drawsBackground = false
        scroller.hasVerticalScroller = true
        scroller.autoresizingMask = [.width, .height]
        scroller.frame = bounds
        scroller.documentView = document
        addSubview(scroller)
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    private func paint() { layer?.backgroundColor = resolved(.textBackgroundColor) }

    var rows: [ScheduleRow] { shown ?? [] }
    var groupOrder: [String?] { ScheduleOrder.groups(rows, repo: \.repo, here: here).map(\.repo) }
    func isOpen(_ id: String) -> Bool { opened.contains(id) }

    func center(of control: String) -> NSPoint? {
        guard let view = controls[control] else { return nil }
        document.scrollToVisible(view.frame)
        return view.convert(NSPoint(x: view.bounds.midX, y: view.bounds.midY), to: nil)
    }

    func enabled(_ control: String) -> Bool { (controls[control] as? SchedulesButton)?.isEnabled ?? false }

    func show(_ rows: [ScheduleRow]?, here: String?) {
        guard rows != shown || here != self.here else { return }
        shown = rows
        self.here = here
        lay()
    }

    override func resizeSubviews(withOldSize oldSize: NSSize) {
        super.resizeSubviews(withOldSize: oldSize)
        if oldSize.width != bounds.width { lay() }
    }

    @discardableResult
    private func label(_ text: String, size: CGFloat = 12, weight: NSFont.Weight = .regular, color: NSColor = .labelColor, wraps: CGFloat? = nil) -> NSTextField {
        let field = wraps == nil ? NSTextField(labelWithString: text) : NSTextField(wrappingLabelWithString: text)
        field.font = .systemFont(ofSize: size, weight: weight)
        field.textColor = color
        if let wraps {
            field.preferredMaxLayoutWidth = wraps
            field.frame.size = field.sizeThatFits(NSSize(width: wraps, height: .greatestFiniteMagnitude))
        } else {
            field.lineBreakMode = .byTruncatingTail
            field.sizeToFit()
        }
        document.addSubview(field)
        return field
    }

    private func rule(_ left: CGFloat, _ y: CGFloat, _ width: CGFloat) {
        document.addSubview(SchedulesRule(frame: NSRect(x: left, y: y, width: width, height: 1)))
    }

    private func lay() {
        document.subviews.forEach { $0.removeFromSuperview() }
        controls = [:]
        let left: CGFloat = 32, width = max(420, min(900, scroller.contentSize.width - left * 2))
        var y: CGFloat = 28
        let title = label("Schedules", size: 22, weight: .bold)
        title.frame.origin = NSPoint(x: left, y: y)
        y += title.frame.height + 4
        let sub = label("Every repo on this machine", size: 13, color: .secondaryLabelColor)
        sub.frame.origin = NSPoint(x: left, y: y)
        y += sub.frame.height
        if let shown, shown.isEmpty {
            y += 28
            let none = label("No schedules yet. Ask an agent, or run kanna schedule add 'daily 09:00' planner \"…\".", color: .secondaryLabelColor, wraps: width)
            none.frame.origin = NSPoint(x: left, y: y)
            y += none.frame.height
        }
        for group in ScheduleOrder.groups(rows, repo: \.repo, here: here) {
            y += 28
            let name = label(group.repo.map { ($0 as NSString).lastPathComponent } ?? "No repo", size: 13, weight: .semibold)
            name.frame.origin = NSPoint(x: left, y: y)
            if let repo = group.repo {
                let path = (repo as NSString).abbreviatingWithTildeInPath + (repo == here ? " · this Desk" : "")
                let where_ = label(path, color: .secondaryLabelColor)
                where_.frame = NSRect(x: name.frame.maxX + 10, y: y + 1, width: left + width - name.frame.maxX - 10, height: where_.frame.height)
            }
            y += name.frame.height + 8
            rule(left, y, width)
            for row in group.items {
                y = lay(row, left: left, width: width, y: y + 12)
                rule(left, y, width)
            }
        }
        y += 32
        document.frame = NSRect(x: 0, y: 0, width: scroller.contentSize.width, height: max(y, scroller.contentSize.height))
    }

    private func lay(_ row: ScheduleRow, left: CGFloat, width: CGFloat, y start: CGFloat) -> CGFloat {
        var y = start, right = left + width
        let dim: NSColor = row.paused ? .tertiaryLabelColor : .secondaryLabelColor
        for (action, text) in [("remove", confirming == row.id ? "Remove?" : "Remove"), ("run", "Run now"), (row.paused ? "resume" : "pause", row.paused ? "Resume" : "Pause")] {
            let button = SchedulesButton(title: text) { [weak self] in self?.act(action, row) }
            button.frame.origin = NSPoint(x: right - button.frame.width, y: y)
            if action == "run" && row.waiting {
                button.isEnabled = false
                button.toolTip = "Its last message is still waiting"
            }
            if action == "remove" && confirming == row.id { button.tint = .systemRed }
            document.addSubview(button)
            controls["\(row.id):\(action)"] = button
            right = button.frame.minX - 6
        }
        for (text, tone) in [(row.paused, "paused"), (row.waiting, "waiting")].filter(\.0).map({ ($0.1, $0.1 == "waiting" ? NSColor.systemOrange : NSColor.secondaryLabelColor) }) {
            let badge = SchedulesBadge(text: text, tone: tone)
            badge.frame.origin = NSPoint(x: right - badge.frame.width - 4, y: y + 3)
            document.addSubview(badge)
            right = badge.frame.minX - 6
        }
        let name = label(row.name, size: 13, weight: .semibold, color: row.paused ? .secondaryLabelColor : .labelColor)
        name.frame.origin = NSPoint(x: left, y: y + 3)
        let what = label(row.rule, weight: .medium, color: row.paused ? .secondaryLabelColor : .labelColor)
        what.frame.origin = NSPoint(x: name.frame.maxX + 10, y: y + 4)
        let goes = label("→ \(row.target) · next \(row.next)", color: dim)
        goes.frame = NSRect(x: what.frame.maxX + 4, y: y + 4, width: max(0, right - what.frame.maxX - 8), height: goes.frame.height)
        y += 28
        let open = opened.contains(row.id)
        let text = "\(open ? "▾" : "▸") \(row.prompt)"
        let prompt = label(text, color: open ? .labelColor : .secondaryLabelColor, wraps: open ? width : nil)
        prompt.frame = NSRect(x: left, y: y, width: width, height: prompt.frame.height)
        let toggle = SchedulesToggle(frame: prompt.frame) { [weak self] in self?.toggle(row.id) }
        toggle.setAccessibilityLabel(open ? "Collapse the prompt" : "Expand the prompt")
        document.addSubview(toggle)
        controls["\(row.id):prompt"] = toggle
        y += prompt.frame.height + 6
        let inset = left + 12
        if row.firings.isEmpty {
            let none = label("Hasn't fired yet", color: .secondaryLabelColor)
            none.frame.origin = NSPoint(x: inset, y: y)
            y += none.frame.height + 3
        }
        for firing in row.firings {
            let at = label(firing.at, color: .secondaryLabelColor)
            at.frame.origin = NSPoint(x: inset, y: y)
            let state = label(firing.stateText, color: firing.tone)
            state.frame.origin = NSPoint(x: inset + 134, y: y)
            let x = inset + 262, room = left + width - x
            let replied = firing.reply != nil
            let note = label(firing.note, color: replied ? .labelColor : .secondaryLabelColor, wraps: open ? room : nil)
            note.frame = NSRect(x: x, y: y, width: room, height: note.frame.height)
            y += max(at.frame.height, note.frame.height) + 3
        }
        return y + 8
    }

    private func toggle(_ id: String) {
        if opened.remove(id) == nil { opened.insert(id) }
        lay()
    }

    private func act(_ action: String, _ row: ScheduleRow) {
        guard action == "remove", confirming != row.id else {
            confirming = nil
            onAction?(action, row.id)
            return
        }
        confirming = row.id
        lay()
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
            guard let self, confirming == row.id else { return }
            confirming = nil
            lay()
        }
    }
}

private final class SchedulesDocument: NSView {
    override var isFlipped: Bool { true }
}

private final class SchedulesRule: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        bounds.fill()
    }
}

@MainActor
private final class SchedulesButton: NSView {
    private let field: NSTextField
    private let run: () -> Void
    var isEnabled = true { didSet { paint() } }
    var tint: NSColor? { didSet { paint() } }

    init(title: String, run: @escaping () -> Void) {
        self.run = run
        field = NSTextField(labelWithString: title)
        field.font = .systemFont(ofSize: 11)
        field.sizeToFit()
        super.init(frame: NSRect(x: 0, y: 0, width: field.frame.width + 18, height: 22))
        field.frame.origin = NSPoint(x: 9, y: ((22 - field.frame.height) / 2).rounded())
        addSubview(field)
        setAccessibilityRole(.button)
        setAccessibilityLabel(title)
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func paint() {
        field.textColor = !isEnabled ? .tertiaryLabelColor : tint ?? .labelColor
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 5, yRadius: 5)
        NSColor.controlBackgroundColor.setFill()
        shape.fill()
        NSColor.separatorColor.setStroke()
        shape.stroke()
    }

    override func mouseDown(with event: NSEvent) {
        if isEnabled { run() }
    }
}

@MainActor
private final class SchedulesToggle: NSView {
    private let run: () -> Void

    init(frame: NSRect, run: @escaping () -> Void) {
        self.run = run
        super.init(frame: frame)
        setAccessibilityRole(.button)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func mouseDown(with event: NSEvent) { run() }

    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }
}

@MainActor
private final class SchedulesBadge: NSView {
    private let tone: NSColor

    init(text: String, tone: NSColor) {
        self.tone = tone
        let field = NSTextField(labelWithString: text)
        field.font = .systemFont(ofSize: 11, weight: .medium)
        field.textColor = tone
        field.sizeToFit()
        super.init(frame: NSRect(x: 0, y: 0, width: field.frame.width + 12, height: field.frame.height + 2))
        field.frame.origin = NSPoint(x: 6, y: 1)
        addSubview(field)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draw(_ dirtyRect: NSRect) {
        tone.withAlphaComponent(0.5).setStroke()
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 4, yRadius: 4)
        outline.lineWidth = 1
        outline.stroke()
    }
}
