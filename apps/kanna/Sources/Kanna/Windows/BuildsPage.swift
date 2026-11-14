import AppKit

struct BuildRow: Equatable {
    let job: String
    let mark: String
    let label: String
    let detail: String
    let state: String
    let note: String
    let tone: BuildTone
    let tip: String
    let cancellable: Bool
}

enum BuildTone: Equatable {
    case running, queued, done, failed

    var color: NSColor {
        switch self {
        case .running: return .systemGreen
        case .queued: return .systemOrange
        case .done: return .tertiaryLabelColor
        case .failed: return .systemRed
        }
    }
}

struct BuildMac: Equatable {
    let name: String
    let summary: String
    let unreachable: Bool
    let rows: [BuildRow]
    let recent: [BuildRow]

    static func time(_ seconds: Double) -> String {
        let total = Int(max(seconds, 0))
        if total < 60 { return "\(total)s" }
        if total < 3600 { return "\(total / 60)m \(total % 60)s" }
        return "\(total / 3600)h \(total / 60 % 60)m"
    }

    static func result(_ status: JSON?) -> (String, BuildTone) {
        switch status?["phase"]?.string {
        case "exited":
            let code = Int(status?["code"]?.number ?? 1)
            return code == 0 ? ("Passed", .done) : ("Failed · exit \(code)", .failed)
        case "signalled": return ("Failed · signal \(Int(status?["signal"]?.number ?? 0))", .failed)
        case "cancelled": return ("Cancelled", .done)
        default: return ("Failed", .failed)
        }
    }

    static func rows(from reply: JSON, now: Double) -> [BuildMac] {
        (reply["macs"]?.array ?? []).compactMap { mac in
            let name = mac["mac"]?.string ?? ""
            if mac["unreachable"]?.string != nil {
                return BuildMac(name: name, summary: "Unreachable", unreachable: true, rows: [], recent: [])
            }
            let guests = mac["guests"]?.array ?? []
            let jobs = mac["jobs"]?.array ?? []
            guard !guests.isEmpty || !jobs.isEmpty else { return nil }
            var seen: [String] = []
            for guest in guests {
                let line = ["\(guest["arch"]?.string ?? "")", "macOS \(guest["macos"]?.string ?? "")", "Xcode \(guest["xcode"]?.string ?? "")"].joined(separator: " · ")
                if !seen.contains(line) { seen.append(line) }
            }
            let summary = (seen + ["\(guests.count) guest\(guests.count == 1 ? "" : "s")"]).joined(separator: " · ")
            let rows = jobs.map { job -> BuildRow in
                let queued = job["stage"]?.string == "queued"
                let label = job["label"]?.string ?? ""
                let from = job["machine"]?.string ?? ""
                let place = Int(job["ahead"]?.number ?? 0) + 1
                let since = ((job["started"]?.number ?? job["submitted"]?.number ?? now * 1000) / 1000)
                let stopping = job["cancel"]?.bool == true
                return BuildRow(job: job["job"]?.string ?? "", mark: queued ? "\(place)" : "▶", label: label, detail: job["command"]?.string ?? "",
                                state: stopping ? "Cancelling" : queued ? "Queued · \(place)\(ordinal(place))" : job["stage"]?.string == "sending" ? "Sending" : "Running",
                                note: queued ? "waiting \(time(now - since))" : "\(time(now - since)) · \(job["guest"]?.string ?? "")",
                                tone: queued ? .queued : .running, tip: from, cancellable: !stopping)
            }
            let recent = (mac["recent"]?.array ?? []).prefix(8).map { job -> BuildRow in
                let (state, tone) = result(job["status"])
                let ago = now - (job["finished"]?.number ?? now * 1000) / 1000
                return BuildRow(job: job["job"]?.string ?? "", mark: "", label: job["label"]?.string ?? "", detail: job["command"]?.string ?? "", state: state, note: "\(time(ago)) ago", tone: tone, tip: job["machine"]?.string ?? "", cancellable: false)
            }
            return BuildMac(name: name, summary: summary, unreachable: false, rows: rows, recent: Array(recent))
        }
    }

    private static func ordinal(_ place: Int) -> String {
        switch (place % 10, place % 100) {
        case (1, let t) where t != 11: return "st"
        case (2, let t) where t != 12: return "nd"
        case (3, let t) where t != 13: return "rd"
        default: return "th"
        }
    }
}

@MainActor
final class BuildsPage: NSView {
    private let scroller = NSScrollView()
    private let document = BuildsDocument()
    private var shown: [BuildMac]?
    var onCancel: ((String) -> Void)?

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

    var macs: [BuildMac] { shown ?? [] }

    func show(_ macs: [BuildMac]) {
        guard macs != shown else { return }
        shown = macs
        lay()
    }

    override func resizeSubviews(withOldSize oldSize: NSSize) {
        super.resizeSubviews(withOldSize: oldSize)
        if oldSize.width != bounds.width { lay() }
    }

    private func label(_ text: String, size: CGFloat = 12, weight: NSFont.Weight = .regular, color: NSColor = .labelColor, mono: Bool = false) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = mono ? .monospacedSystemFont(ofSize: size, weight: weight) : .systemFont(ofSize: size, weight: weight)
        field.textColor = color
        field.lineBreakMode = .byTruncatingTail
        field.sizeToFit()
        document.addSubview(field)
        return field
    }

    private func row(_ row: BuildRow, left: CGFloat, width: CGFloat, y: CGFloat) {
        document.addSubview(BuildsRule(frame: NSRect(x: left, y: y, width: width, height: 1)))
        let top = y + 10
        let stateX = left + width - 262, cancelX = left + width - 72
        let mark = label(row.mark, color: .secondaryLabelColor)
        mark.frame = NSRect(x: left, y: top, width: 22, height: mark.frame.height)
        let name = label(row.label, size: 13, weight: .semibold)
        name.frame = NSRect(x: left + 26, y: top - 1, width: stateX - left - 38, height: name.frame.height)
        name.toolTip = row.tip
        let detail = label(row.detail, size: 11, color: .secondaryLabelColor, mono: true)
        detail.frame = NSRect(x: left + 26, y: top + 20, width: stateX - left - 38, height: detail.frame.height)
        let dot = BuildsDot(frame: NSRect(x: stateX, y: top + 4, width: 8, height: 8))
        dot.tone = row.tone
        document.addSubview(dot)
        let state = label(row.state)
        state.frame = NSRect(x: stateX + 14, y: top - 1, width: 170, height: state.frame.height)
        let note = label(row.note, size: 11, color: .secondaryLabelColor)
        note.frame = NSRect(x: stateX + 14, y: top + 20, width: 170, height: note.frame.height)
        if row.cancellable {
            let cancel = BuildsCancel(title: "Cancel", job: row.job) { [weak self] job in self?.onCancel?(job) }
            cancel.frame = NSRect(x: cancelX, y: top + 2, width: 72, height: 24)
            document.addSubview(cancel)
        }
    }

    private func lay() {
        document.subviews.forEach { $0.removeFromSuperview() }
        let left: CGFloat = 32, width = max(480, min(900, scroller.contentSize.width - left * 2))
        var y: CGFloat = 28
        let title = label("Builds", size: 22, weight: .bold)
        title.frame.origin = NSPoint(x: left, y: y)
        y += title.frame.height + 4
        let sub = label("Your machines' jobs on build Macs, in the order they run", size: 13, color: .secondaryLabelColor)
        sub.frame.origin = NSPoint(x: left, y: y)
        y += sub.frame.height + 26
        let macs = shown ?? []
        if macs.isEmpty {
            label("No build Macs with guests.", color: .secondaryLabelColor).frame.origin = NSPoint(x: left, y: y)
            y += 30
        }
        for mac in macs {
            let name = label(mac.name, size: 13, weight: .semibold)
            name.frame.origin = NSPoint(x: left, y: y)
            let summary = label(mac.summary, color: mac.unreachable ? .systemRed : .secondaryLabelColor)
            summary.frame = NSRect(x: left + name.frame.width + 10, y: y + 1, width: width - name.frame.width - 10, height: summary.frame.height)
            y += 24
            if !mac.unreachable, mac.rows.isEmpty {
                document.addSubview(BuildsRule(frame: NSRect(x: left, y: y, width: width, height: 1)))
                let empty = label("Nothing running or queued.", color: .secondaryLabelColor)
                empty.frame.origin = NSPoint(x: left, y: y + 12)
                y += 40
            }
            for entry in mac.rows {
                row(entry, left: left, width: width, y: y)
                y += 54
            }
            if !mac.recent.isEmpty {
                y += 12
                let head = label("RECENT", size: 11, weight: .medium, color: .secondaryLabelColor)
                head.frame.origin = NSPoint(x: left, y: y)
                y += 20
                for entry in mac.recent {
                    row(entry, left: left, width: width, y: y)
                    y += 54
                }
            }
            y += 22
        }
        document.frame = NSRect(x: 0, y: 0, width: scroller.contentSize.width, height: max(y, scroller.contentSize.height))
    }
}

private final class BuildsDocument: NSView {
    override var isFlipped: Bool { true }
}

private final class BuildsRule: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        bounds.fill()
    }
}

private final class BuildsDot: NSView {
    var tone: BuildTone = .queued

    override func draw(_ dirtyRect: NSRect) {
        tone.color.setFill()
        NSBezierPath(ovalIn: bounds).fill()
    }
}

@MainActor
private final class BuildsCancel: NSButton {
    private let job: String
    private let done: (String) -> Void

    init(title: String, job: String, done: @escaping (String) -> Void) {
        self.job = job
        self.done = done
        super.init(frame: .zero)
        self.title = title
        bezelStyle = .rounded
        controlSize = .small
        target = self
        action = #selector(press)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    @objc private func press() { done(job) }
}
