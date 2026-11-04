import AppKit
import KannaCode

@MainActor
final class LogRowsView: NSView, NSTextViewDelegate {
    weak var pane: LogPaneView?
    private var foldRects: [Int: NSRect] = [:]

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    override func becomeFirstResponder() -> Bool {
        pane?.onFocus?()
        return true
    }

    override func draw(_ dirtyRect: NSRect) {
        LogTheme.background.setFill()
        dirtyRect.fill()
        guard let pane, let result = pane.result else { return }
        let graph = pane.graph
        let m = pane.metrics
        guard !graph.rows.isEmpty else { return }
        let first = max(0, Int(dirtyRect.minY / m.row) - 1)
        let last = min(graph.rows.count - 1, Int(dirtyRect.maxY / m.row) + 1)
        guard first <= last else { return }
        let hits = Set(pane.hits)
        let now = pane.now()
        for index in first...last {
            let lane = graph.rows[index]
            let row = result.rows[lane.row]
            let rect = NSRect(x: 0, y: CGFloat(index) * m.row, width: bounds.width, height: m.row)
            if pane.fresh.contains(row.hash) { LogTheme.fresh.setFill(); rect.fill() }
            if hits.contains(row.hash) { LogTheme.hit.setFill(); rect.fill() }
            if row.hash == pane.selected {
                LogTheme.tint.setFill()
                rect.fill()
                LogTheme.accent.setFill()
                NSRect(x: 0, y: rect.minY, width: 2, height: rect.height).fill()
            }
            drawLines(lane, at: rect.minY, m)
        }
        for index in first...last {
            let lane = graph.rows[index]
            drawNode(lane, row: result.rows[lane.row], at: CGFloat(index) * m.row, m)
            drawText(lane, row: result.rows[lane.row], index: index, at: CGFloat(index) * m.row, m, now: now)
        }
    }

    private func stroke(_ path: NSBezierPath, _ color: Int, _ m: LogMetrics, dashed: Bool = false) {
        path.lineWidth = 1.8 * m.scale
        path.lineCapStyle = .round
        if dashed { path.setLineDash([2 * m.scale, 3 * m.scale], count: 2, phase: 0) }
        LogTheme.lane(color).setStroke()
        path.stroke()
    }

    private func drawLines(_ lane: LaneRow, at top: CGFloat, _ m: LogMetrics) {
        let mid = top + m.row / 2, bottom = top + m.row
        let x = m.x(lane.lane)
        for line in lane.through {
            let path = NSBezierPath()
            path.move(to: NSPoint(x: m.x(line.lane), y: top))
            path.line(to: NSPoint(x: m.x(line.lane), y: bottom))
            stroke(path, line.color, m)
        }
        for line in lane.incoming {
            let path = NSBezierPath()
            let from = m.x(line.lane)
            path.move(to: NSPoint(x: from, y: top))
            if line.lane == lane.lane {
                path.line(to: NSPoint(x: x, y: mid))
            } else {
                let bend = min(abs(x - from), m.row / 2) * (x > from ? 1 : -1)
                path.curve(to: NSPoint(x: from + bend, y: mid), controlPoint1: NSPoint(x: from, y: mid), controlPoint2: NSPoint(x: from, y: mid))
                path.line(to: NSPoint(x: x, y: mid))
            }
            stroke(path, line.color, m)
        }
        for edge in lane.outgoing {
            let path = NSBezierPath()
            path.move(to: NSPoint(x: x, y: mid))
            if edge.stub {
                path.line(to: NSPoint(x: x, y: bottom))
                stroke(path, edge.color, m, dashed: true)
                continue
            }
            if edge.hook {
                let end = NSPoint(x: x - min(m.lane, 10 * m.scale), y: bottom - 3 * m.scale)
                path.curve(to: end, controlPoint1: NSPoint(x: end.x, y: mid), controlPoint2: NSPoint(x: end.x, y: mid))
                stroke(path, 0, m, dashed: true)
                LogTheme.lane(0).setFill()
                NSBezierPath(ovalIn: NSRect(x: end.x - 1.8 * m.scale, y: end.y - 1.8 * m.scale, width: 3.6 * m.scale, height: 3.6 * m.scale)).fill()
                continue
            }
            let to = m.x(edge.lane)
            if edge.lane == lane.lane {
                path.line(to: NSPoint(x: x, y: bottom))
            } else {
                let bend = min(abs(to - x), m.row / 2) * (to > x ? 1 : -1)
                path.line(to: NSPoint(x: to - bend, y: mid))
                path.curve(to: NSPoint(x: to, y: bottom), controlPoint1: NSPoint(x: to, y: mid), controlPoint2: NSPoint(x: to, y: mid))
            }
            stroke(path, edge.color, m)
        }
    }

    private func circle(_ center: NSPoint, _ radius: CGFloat) -> NSBezierPath {
        NSBezierPath(ovalIn: NSRect(x: center.x - radius, y: center.y - radius, width: radius * 2, height: radius * 2))
    }

    private func drawNode(_ lane: LaneRow, row: LogRow, at top: CGFloat, _ m: LogMetrics) {
        let center = NSPoint(x: m.x(lane.lane), y: top + m.row / 2)
        let color = LogTheme.lane(lane.color)
        let s = m.scale
        if lane.folded {
            let ring = circle(center, 5 * s)
            LogTheme.background.setFill()
            ring.fill()
            ring.lineWidth = 2 * s
            color.setStroke()
            ring.stroke()
            color.setFill()
            circle(center, 2 * s).fill()
        } else if lane.labels.contains(where: { !$0.merged }) {
            let dot = circle(center, 5.5 * s)
            color.setFill()
            dot.fill()
            dot.lineWidth = 2 * s
            LogTheme.background.setStroke()
            dot.stroke()
        } else if row.isMerge {
            let dot = circle(center, 3.6 * s)
            color.setFill()
            dot.fill()
            dot.lineWidth = 1.5 * s
            LogTheme.background.setStroke()
            dot.stroke()
        } else {
            let ring = circle(center, 4.2 * s)
            LogTheme.background.setFill()
            ring.fill()
            ring.lineWidth = 2 * s
            color.setStroke()
            ring.stroke()
        }
    }

    private func line(_ text: String, font: NSFont, color: NSColor) -> NSAttributedString {
        let style = NSMutableParagraphStyle()
        style.lineBreakMode = .byTruncatingTail
        return NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color, .paragraphStyle: style])
    }

    private func put(_ text: NSAttributedString, x: CGFloat, top: CGFloat, width: CGFloat, _ m: LogMetrics) {
        guard width > 4 else { return }
        let height = ceil(text.size().height)
        text.draw(with: NSRect(x: x, y: top + (m.row - height) / 2, width: width, height: height), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }

    private func drawText(_ lane: LaneRow, row: LogRow, index: Int, at top: CGFloat, _ m: LogMetrics, now: Int) {
        let s = m.scale
        let right = bounds.width - 12 * s
        let whenX = right - m.when
        let authorX = whenX - m.author
        put(line(LogAge.text(row.time, now: now), font: LogTheme.sans(size: 12 * s), color: LogTheme.muted).rightAligned, x: whenX, top: top, width: m.when, m)
        put(line(row.author, font: LogTheme.sans(size: 12 * s), color: LogTheme.muted), x: authorX, top: top, width: m.author - 8 * s, m)
        var x = m.graph
        put(line(row.short, font: LogTheme.mono(size: 11.5 * s), color: LogTheme.muted), x: x, top: top, width: m.hash, m)
        x += m.hash
        let end = authorX - 10 * s
        for chip in chips(lane, row: row) {
            guard x < end - 40 * s else { break }
            x += draw(chip, x: x, top: top, limit: end - x, m) + 6 * s
        }
        let foldText = lane.folded || lane.open ? "\(lane.open ? "▾" : "▸") \(row.fold)" : nil
        let foldWidth = foldText.map { ceil(line($0, font: LogTheme.mono(size: 10.5 * s), color: LogTheme.muted).size().width) + 14 * s } ?? 0
        let spine = row.spine || !row.isMerge
        let subject = line(row.subject, font: LogTheme.sans(size: 12.5 * s), color: spine ? LogTheme.text : LogTheme.muted)
        let room = max(0, end - x - (foldText == nil ? 0 : foldWidth + 6 * s))
        put(subject, x: x, top: top, width: room, m)
        foldRects[index] = nil
        if let foldText {
            let fx = x + min(ceil(subject.size().width), room) + 6 * s
            let rect = NSRect(x: fx, y: top + (m.row - 17 * s) / 2, width: foldWidth, height: 17 * s)
            let pill = NSBezierPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5), xRadius: rect.height / 2, yRadius: rect.height / 2)
            pill.lineWidth = 1
            LogTheme.line.setStroke()
            pill.stroke()
            put(line(foldText, font: LogTheme.mono(size: 10.5 * s), color: LogTheme.muted), x: fx + 7 * s, top: top, width: foldWidth - 8 * s, m)
            foldRects[index] = rect
        }
    }

    struct Chip {
        var parts: [(String, NSColor, Bool)]
        var border: NSColor
        var fill: NSColor
        var dot: NSColor?
        var dashed = false
        var mono = false
    }

    func chips(_ lane: LaneRow, row: LogRow) -> [Chip] {
        var out: [Chip] = []
        for task in lane.labels {
            if task.merged {
                out.append(Chip(parts: [(task.name, LogTheme.text, false), ("✓ merged", LogTheme.ok, false)], border: LogTheme.ok.withAlphaComponent(0.45), fill: LogTheme.ok.withAlphaComponent(0.1), dot: nil))
            } else {
                let color = LogTheme.lane(lane.color)
                var parts: [(String, NSColor, Bool)] = [(task.name, LogTheme.text, false), (task.place == "desk" ? "Desk" : task.place == "shelf" ? "Shelf" : "Done", LogTheme.muted, false), ("\(task.ahead) ahead", LogTheme.muted, true)]
                if task.behind > 0 { parts.append(("\(task.behind) behind", LogTheme.bad, true)) }
                out.append(Chip(parts: parts, border: color.withAlphaComponent(0.55), fill: color.withAlphaComponent(0.14), dot: color))
            }
        }
        let branches = Set(lane.labels.map(\.branch))
        for ref in row.refs where ref != "HEAD" && !branches.contains(ref) {
            if ref.hasPrefix("tag: ") {
                out.append(Chip(parts: [(String(ref.dropFirst(5)), LogTheme.warn, false)], border: LogTheme.warn.withAlphaComponent(0.6), fill: .clear, dot: nil, dashed: true, mono: true))
            } else if ref == pane?.result?.main {
                out.append(Chip(parts: [(ref, LogTheme.accent, false)], border: LogTheme.accent, fill: .clear, dot: nil, mono: true))
            } else if ref.contains("/") {
                out.append(Chip(parts: [(ref, LogTheme.muted, false)], border: LogTheme.line, fill: LogTheme.panel, dot: nil, mono: true))
            } else {
                out.append(Chip(parts: [(ref, LogTheme.text, false)], border: LogTheme.line, fill: LogTheme.panel, dot: nil, mono: true))
            }
        }
        return out
    }

    private func draw(_ chip: Chip, x: CGFloat, top: CGFloat, limit: CGFloat, _ m: LogMetrics) -> CGFloat {
        let s = m.scale
        let font = chip.mono ? LogTheme.mono(size: 11 * s) : LogTheme.sans(size: 11 * s)
        let small = LogTheme.mono(size: 10.5 * s)
        let pieces = chip.parts.enumerated().map { offset, part in
            NSAttributedString(string: part.0, attributes: [.font: offset == 0 ? font : (part.2 ? small : LogTheme.sans(size: 10.5 * s)), .foregroundColor: part.1])
        }
        let gap = 5 * s
        let lead = (chip.dot == nil ? 0 : 12 * s) + 7 * s
        let nameMax = 220 * s
        var widths = pieces.map { ceil($0.size().width) }
        widths[0] = min(widths[0], nameMax)
        var width = lead + widths.reduce(0, +) + gap * CGFloat(max(0, widths.count - 1)) + 7 * s
        if width > limit {
            widths[0] = max(30 * s, widths[0] - (width - limit))
            width = lead + widths.reduce(0, +) + gap * CGFloat(max(0, widths.count - 1)) + 7 * s
        }
        let rect = NSRect(x: x, y: top + (m.row - m.chip) / 2, width: width, height: m.chip)
        let shape = NSBezierPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5), xRadius: m.chip / 2, yRadius: m.chip / 2)
        chip.fill.setFill()
        shape.fill()
        shape.lineWidth = 1
        if chip.dashed { shape.setLineDash([3 * s, 2 * s], count: 2, phase: 0) }
        chip.border.setStroke()
        shape.stroke()
        if let dot = chip.dot {
            dot.setFill()
            circle(NSPoint(x: x + 7 * s + 3.5 * s, y: rect.midY), 3.5 * s).fill()
        }
        var at = x + lead
        for (offset, piece) in pieces.enumerated() {
            let styled = NSMutableAttributedString(attributedString: piece)
            let style = NSMutableParagraphStyle()
            style.lineBreakMode = .byTruncatingTail
            styled.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: styled.length))
            put(styled, x: at, top: top, width: widths[offset] + 1, m)
            at += widths[offset] + gap
        }
        return width
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        guard let pane else { return }
        let point = convert(event.locationInWindow, from: nil)
        let index = Int(point.y / pane.metrics.row)
        guard let lane = pane.graph.rows[safe: index] else { return }
        if let rect = foldRects[index], rect.insetBy(dx: -4, dy: -4).contains(point) {
            pane.toggle(lane.hash)
            return
        }
        pane.select(lane.hash, scroll: false)
        if event.clickCount == 2 { pane.openDiff(lane.hash) }
    }

    override func keyDown(with event: NSEvent) {
        let flags = event.modifierFlags.intersection([.command, .control, .option])
        if flags.isEmpty, let chars = event.charactersIgnoringModifiers, let pane, pane.press(chars == "\u{3}" ? "\r" : chars) { return }
        super.keyDown(with: event)
    }

    override func cancelOperation(_ sender: Any?) {
        if pane?.finding == true { pane?.closeFind() }
    }

    @objc func copy(_ sender: Any?) { pane?.copyHash() }

    @objc override func performTextFinderAction(_ sender: Any?) {
        guard let pane, let item = sender as? NSValidatedUserInterfaceItem, let action = NSTextFinder.Action(rawValue: item.tag) else { return }
        switch action {
        case .showFindInterface: pane.showFind()
        case .nextMatch: pane.findStep(forward: true)
        case .previousMatch: pane.findStep(forward: false)
        default: break
        }
    }

    override func validateProposedFirstResponder(_ responder: NSResponder, for event: NSEvent?) -> Bool { true }

    @objc func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(copy(_:)) { return pane?.selected != nil }
        if item.action == #selector(performTextFinderAction(_:)) { return true }
        return false
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let pane else { return nil }
        let point = convert(event.locationInWindow, from: nil)
        if let lane = pane.graph.rows[safe: Int(point.y / pane.metrics.row)] { pane.select(lane.hash, scroll: false) }
        let menu = pane.contextMenu?() ?? NSMenu()
        var items = [NSMenuItem(title: "Open Diff", action: #selector(openDiff), keyEquivalent: ""), NSMenuItem(title: "Copy Hash", action: #selector(copyHash), keyEquivalent: ""),
                     NSMenuItem(title: "Copy Subject", action: #selector(copySubject), keyEquivalent: "")]
        if let selected = pane.selected, let row = pane.result?.rows.first(where: { $0.hash == selected }), row.fold > 0 {
            items.append(NSMenuItem(title: pane.open.contains(selected) ? "Fold Merge" : "Unfold Merge", action: #selector(toggleFold), keyEquivalent: ""))
        }
        items.forEach { $0.target = self }
        if !menu.items.isEmpty { items.append(.separator()) }
        for (offset, item) in items.enumerated() { menu.insertItem(item, at: offset) }
        return menu
    }

    @objc private func openDiff() { pane?.openDiff() }
    @objc private func copyHash() { pane?.copyHash() }
    @objc private func copySubject() { pane?.copySubject() }
    @objc private func toggleFold() { if let selected = pane?.selected { pane?.toggle(selected) } }

    func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        let text = (link as? URL)?.absoluteString ?? (link as? String) ?? ""
        guard text.hasPrefix("kanna-log-") else { return false }
        pane?.follow(link: text)
        return true
    }
}

extension NSAttributedString {
    var rightAligned: NSAttributedString {
        let out = NSMutableAttributedString(attributedString: self)
        let style = NSMutableParagraphStyle()
        style.alignment = .right
        style.lineBreakMode = .byTruncatingTail
        out.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: out.length))
        return out
    }
}
