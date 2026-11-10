import AppKit

@MainActor
public final class RunPaneView: NSView, NSTextFieldDelegate {
    public static let actions: [(action: String, title: String)] = [("watch", "Watch"), ("pause", "Pause"), ("skip", "Skip stage"), ("retry", "Retry stage"), ("edit", "Edit pipeline"), ("stop", "Stop")]

    public var onAction: ((String, [String: String]) -> Void)?
    public var onFocus: (() -> Void)?
    public var contextMenu: (() -> NSMenu?)?
    public var keyView: NSView { document }
    public private(set) var notice: RunNotice?
    public private(set) var failure: String?
    public private(set) var flashed: String?
    public private(set) var said: String?

    private let scroll = NSScrollView()
    private let document = RunDocument()
    private var ticker: Timer?

    public init() {
        super.init(frame: NSRect(x: 0, y: 0, width: 900, height: 600))
        wantsLayer = true
        scroll.frame = bounds
        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = true
        scroll.borderType = .noBorder
        scroll.documentView = document
        addSubview(scroll)
        document.owner = self
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    public override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    private func paint() {
        scroll.backgroundColor = RunTheme.background
        document.needsDisplay = true
    }

    public override func layout() {
        super.layout()
        document.rebuild(width: scroll.contentSize.width)
    }

    public func load(_ notice: RunNotice) {
        failure = nil
        self.notice = notice
        document.notice = notice
        document.failure = nil
        document.rebuild(width: scroll.contentSize.width)
        ticker?.invalidate()
        ticker = nil
        if notice.live {
            ticker = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.document.needsDisplay = true }
            }
        }
    }

    public func show(failure: String) {
        self.failure = failure
        document.failure = failure
        document.rebuild(width: scroll.contentSize.width)
    }

    public func flash(_ message: String) {
        flashed = message
        said = nil
        document.flash = message
        document.said = nil
        document.needsDisplay = true
    }

    public func say(_ message: String) {
        said = message
        document.said = message
        document.needsDisplay = true
    }

    public func toggleDiff(_ n: Int) {
        document.toggleDiff(n)
    }

    public func stop() {
        ticker?.invalidate()
        ticker = nil
    }

    public func press(_ action: String, note: String? = nil) {
        flashed = nil
        said = nil
        document.flash = nil
        document.said = nil
        var args: [String: String] = [:]
        let note = note ?? document.noteField.stringValue
        switch action {
        case "yes", "no":
            args["answer"] = action
            if !note.isEmpty { args["note"] = note }
            onAction?("answer", args)
        case "resume":
            if !note.isEmpty { args["text"] = note }
            onAction?("resume", args)
        default:
            onAction?(action, args)
        }
    }

    public var enabled: [String: Bool] { document.buttons.mapValues(\.isEnabled) }
    public var subtitle: String { document.subtitle }
    public var tiles: [(value: String, label: String)] { document.tiles }
    public var rows: [[String]] { document.rows }
    public var workerLines: [String] { document.workerLines }
    public var revisionLines: [String] { document.revisionLines }
    public var diffLines: [String] { document.diffLines }
    public var legend: [String] { document.legendItems }
    public var graph: RunGraph { document.graph }
    public var cardShown: Bool { document.cardButtons.contains { !$0.isHidden } }

    public override func mouseDown(with event: NSEvent) {
        onFocus?()
        super.mouseDown(with: event)
    }

    public override func menu(for event: NSEvent) -> NSMenu? { contextMenu?() }
}

@MainActor
final class RunDocument: NSView {
    weak var owner: RunPaneView?
    var notice: RunNotice?
    var failure: String?
    var flash: String?
    var said: String?
    private var expanded: Set<Int> = []
    private var toggles: [(frame: CGRect, n: Int)] = []
    private(set) var buttons: [String: NSButton] = [:]
    private(set) var cardButtons: [NSButton] = []
    let noteField = NSTextField()
    private(set) var graph = RunGraph()
    private var graphOrigin = CGPoint.zero
    private(set) var graphScale: CGFloat = 1
    private var legendLines = 1
    private var wrapped = false
    private var spots: [String: CGFloat] = [:]
    private var tileFrames: [CGRect] = []
    private var cardFrame = CGRect.zero
    private var cardText: CGFloat = 0
    private var noteWidth: CGFloat = 0
    private var width: CGFloat = 900

    private let side: CGFloat = 24
    private let rowHeight: CGFloat = 26

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: 900, height: 600))
        for (action, title) in RunPaneView.actions {
            let button = NSButton(title: title, target: self, action: #selector(pressed(_:)))
            button.bezelStyle = .rounded
            button.controlSize = .small
            button.font = RunTheme.sans(12)
            button.identifier = NSUserInterfaceItemIdentifier(action)
            button.sizeToFit()
            buttons[action] = button
            addSubview(button)
        }
        noteField.font = RunTheme.sans(12)
        noteField.bezelStyle = .roundedBezel
        noteField.controlSize = .small
        addSubview(noteField)
        for (action, title) in [("yes", "Yes"), ("no", "No"), ("resume", "Resume")] {
            let button = NSButton(title: title, target: self, action: #selector(pressed(_:)))
            button.bezelStyle = .rounded
            button.controlSize = .small
            button.font = RunTheme.sans(12)
            button.identifier = NSUserInterfaceItemIdentifier(action)
            button.sizeToFit()
            cardButtons.append(button)
            addSubview(button)
        }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    @objc private func pressed(_ sender: NSButton) {
        guard let action = sender.identifier?.rawValue else { return }
        owner?.press(action)
        if ["yes", "no", "resume"].contains(action) { noteField.stringValue = "" }
    }

    private var now: Double { Date().timeIntervalSince1970 }

    private func attributed(_ text: String, _ font: NSFont, _ color: NSColor) -> NSAttributedString {
        NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color])
    }

    private func measure(_ text: String, _ font: NSFont) -> CGFloat { ceil(attributed(text, font, .labelColor).size().width) }

    var subtitle: String {
        guard let n = notice else { return "" }
        var parts = [n.run, "pipeline \(n.pipeline)" + (n.revision.map { " rev \($0.n)" } ?? ""), "started \(RunFormat.time(n.startedAt)) by \(n.startedBy)", RunFormat.clock((n.endedAt ?? now) - n.startedAt)]
        switch n.state {
        case "done": parts.append("handed back")
        case "stopped": parts.append(n.outcome == "stuck" ? "stuck" : "stopped" + (n.why.map { ": \($0)" } ?? ""))
        case "paused": parts.append("paused")
        case "waiting": parts.append("waiting on you")
        default: break
        }
        return parts.joined(separator: " · ")
    }

    var tiles: [(value: String, label: String)] {
        guard let n = notice else { return [] }
        let numbers = n.numbers
        let sessions = numbers.budget.map { "\(numbers.sessions) of \($0) sessions" } ?? "\(numbers.sessions) session\(numbers.sessions == 1 ? "" : "s")"
        let waiting = numbers.waited + (numbers.waitingSince.map { max(0, now - $0) } ?? 0)
        let lines = numbers.lines.map { ("+\($0.added) −\($0.removed)", "lines · \($0.files) file\($0.files == 1 ? "" : "s")") } ?? ("—", "lines")
        return [
            ("\(numbers.visits)", "stage visits · \(sessions)"),
            (RunFormat.clock((n.endedAt ?? now) - n.startedAt), "wall · \(RunFormat.clock(waiting)) waiting on you"),
            (RunFormat.count(numbers.turns), "agent turns"),
            (RunFormat.count(numbers.tokens), "tokens" + (RunFormat.money(numbers.cost).map { " · \($0)" } ?? "")),
            ("\(numbers.rework)", "rework loops"),
            ("\(numbers.interventions)", "your interventions"),
            lines,
        ]
    }

    var rows: [[String]] {
        guard let n = notice else { return [] }
        return n.visits.map { v in
            let took = v.endedAt.map { RunFormat.clock($0 - v.startedAt) } ?? RunFormat.clock(now - v.startedAt)
            let waiting = n.state == "paused" || n.state == "waiting"
            let note = v.endedAt == nil && v.note.isEmpty ? "\(waiting ? "waiting" : "running") \(took)" : v.note
            return ["\(v.n)", v.stage + (v.visit > 1 ? " ·\(v.visit)" : ""), v.who, v.endedAt == nil ? "" : took, RunFormat.verdict(v, state: n.state),
                    v.kind == "agent" ? RunFormat.count(v.turns) : "", v.kind == "agent" ? RunFormat.count(v.tokens) : "", note]
        }
    }

    var workerLines: [String] {
        guard let n = notice else { return [] }
        return n.workers.map { w in
            let folder = w.worktree.map { ($0 as NSString).lastPathComponent }
            return [w.stage + (w.visit > 1 ? " ·\(w.visit)" : ""), w.who, w.terminal, folder, "running \(RunFormat.clock(now - w.startedAt))"].compactMap { $0 }.joined(separator: " · ")
        }
    }

    var legendItems: [String] {
        guard let n = notice else { return [] }
        let kinds = Set(n.nodes.map(\.kind))
        var items: [String] = []
        if kinds.contains("agent") { items.append("agent session") }
        if kinds.contains("run") { items.append("command") }
        if kinds.contains("ask") { items.append("your answer") }
        if kinds.contains("merge") { items.append("merge") }
        return items + ["passed", "failed", "running", "waiting on you", "×N visits"]
    }

    private func nodeWidth(_ node: RunNode) -> CGFloat {
        let name = measure(node.id, node.kind == "run" ? RunTheme.mono(11.5, .medium) : RunTheme.sans(11.5, .semibold)) + 36
        let who = measure(node.who, node.kind == "run" ? RunTheme.mono(10) : RunTheme.sans(10)) + 18
        return max(56, name, who)
    }

    func rebuild(width: CGFloat) {
        self.width = max(width, 520)
        spots = [:]
        var y: CGFloat = 18
        let hasNotice = notice != nil && failure == nil
        for button in buttons.values { button.isHidden = !hasNotice }
        guard let n = notice, failure == nil else {
            cardButtons.forEach { $0.isHidden = true }
            noteField.isHidden = true
            frame = NSRect(x: 0, y: 0, width: self.width, height: max(superview?.bounds.height ?? 0, 200))
            needsDisplay = true
            return
        }
        spots["title"] = y
        let titleWidth = measure(n.name ?? n.run, RunTheme.sans(17, .semibold))
        wrapped = side + titleWidth + 14 + measure(subtitle, RunTheme.sans(13)) > self.width - side
        if wrapped { y += 24 }
        var x = self.width - side
        for (action, _) in RunPaneView.actions.reversed() {
            guard let button = buttons[action] else { continue }
            x -= button.frame.width
            button.frame.origin = NSPoint(x: x, y: y + 30)
            x -= 6
        }
        buttons["watch"]?.isEnabled = n.live && !n.workers.isEmpty
        for action in ["pause", "skip", "retry", "stop"] { buttons[action]?.isEnabled = n.live }
        buttons["edit"]?.isEnabled = n.live
        y += 30 + 30 + 10
        if let card = n.card {
            let asks = card.kind == "ask", needs = card.kind == "need"
            let inputs = asks || needs
            let cardWidth = self.width - 2 * side
            let field = inputs ? min(260, max(140, cardWidth * 0.3)) : 0
            let buttonsWidth = cardButtons.filter { (asks && ["yes", "no"].contains($0.identifier?.rawValue)) || (needs && $0.identifier?.rawValue == "resume") }.map { $0.frame.width + 6 }.reduce(0, +)
            cardText = cardWidth - 36 - 14 - (inputs ? field + buttonsWidth + 14 : 6)
            let titleHeight = ceil(attributed(card.title, RunTheme.sans(13, .semibold), RunTheme.text).boundingRect(with: CGSize(width: cardText, height: 400), options: [.usesLineFragmentOrigin]).height)
            let height = max(62, titleHeight + 42)
            cardFrame = CGRect(x: side, y: y, width: cardWidth, height: height)
            noteWidth = field
            noteField.isHidden = !(asks || needs)
            noteField.placeholderString = asks ? "a note for \(card.stage) (optional)" : "your answer for \(card.stage)"
            var bx = cardFrame.maxX - 14
            for button in cardButtons.reversed() {
                let shown = (asks && ["yes", "no"].contains(button.identifier?.rawValue)) || (needs && button.identifier?.rawValue == "resume")
                button.isHidden = !shown
                guard shown else { continue }
                bx -= button.frame.width
                button.frame.origin = NSPoint(x: bx, y: cardFrame.midY - button.frame.height / 2)
                bx -= 6
            }
            noteField.frame = NSRect(x: bx - noteWidth, y: cardFrame.midY - 11, width: noteWidth, height: 22)
            y += height + 14
        } else {
            cardFrame = .zero
            noteField.isHidden = true
            cardButtons.forEach { $0.isHidden = true }
        }
        graph = RunGraph.lay(n.nodes, n.edges, width: nodeWidth)
        let room = self.width - 2 * side
        graphScale = graph.size.width > 0 ? max(1, min(1.5, room / graph.size.width)) : 1
        let left = max(side, (self.width - graph.size.width * graphScale) / 2)
        graphOrigin = CGPoint(x: left, y: y + 10)
        y += graph.size.height * graphScale + 22
        spots["legend"] = y
        legendLines = legendRows().count
        y += 20 * CGFloat(legendLines) + 6
        tileFrames = []
        var tx = side, ty = y
        for tile in tiles {
            let w = max(measure(tile.value, RunTheme.sans(16, .semibold)), measure(tile.label, RunTheme.sans(11))) + 22
            if tx + w > self.width - side, tx > side {
                tx = side
                ty += 56
            }
            tileFrames.append(CGRect(x: tx, y: ty, width: max(96, w), height: 48))
            tx += max(96, w) + 8
        }
        y = ty + 48 + 16
        spots["revision"] = y
        y += revisionsHeight(n)
        spots["stages"] = y
        y += 22
        spots["table"] = y
        y += rowHeight * CGFloat(n.visits.count + 1) + 10
        spots["workers"] = y
        y += 22 + CGFloat(max(1, n.workers.count)) * 20 + 20
        frame = NSRect(x: 0, y: 0, width: self.width, height: max(y, superview?.bounds.height ?? 0))
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        owner?.onFocus?()
        let point = convert(event.locationInWindow, from: nil)
        if let toggle = toggles.first(where: { $0.frame.insetBy(dx: -4, dy: -3).contains(point) }) { toggleDiff(toggle.n) }
    }

    func toggleDiff(_ n: Int) {
        if !expanded.insert(n).inserted { expanded.remove(n) }
        rebuild(width: width)
    }

    private let revisionLine: CGFloat = 20
    private let diffLine: CGFloat = 17

    private func shownRevisions(_ n: RunNotice) -> [RunRevision] {
        if !n.revisions.isEmpty { return n.revisions }
        return n.revision.map { [RunRevision(n: $0.n, by: $0.by, at: $0.at)] } ?? []
    }

    private func open(_ revision: RunRevision) -> Bool { revision.n.map { expanded.contains($0) } == true && revision.changed }

    private func revisionsHeight(_ n: RunNotice) -> CGFloat {
        let revisions = shownRevisions(n)
        var height: CGFloat = revisions.isEmpty ? revisionLine : 0
        for revision in revisions {
            height += revisionLine + CGFloat(revision.errors.count) * diffLine
            if open(revision) { height += CGFloat(revision.diff.count) * diffLine + 14 }
        }
        return height + 2
    }

    var revisionLines: [String] {
        guard let n = notice else { return [] }
        return shownRevisions(n).map { revision in
            ([revision.title + " " + revision.detail] + revision.errors).joined(separator: "\n") + (revision.changed ? (open(revision) ? " · hide diff" : " · show diff") : "")
        }
    }

    var diffLines: [String] {
        guard let n = notice else { return [] }
        return shownRevisions(n).filter(open).flatMap { $0.diff.map { $0.sign + $0.text } }
    }

    override func menu(for event: NSEvent) -> NSMenu? { owner?.contextMenu?() }

    override func draw(_ dirtyRect: NSRect) {
        RunTheme.background.setFill()
        bounds.fill()
        if let failure {
            attributed(failure, RunTheme.sans(13), RunTheme.muted).draw(at: NSPoint(x: side, y: 24))
            return
        }
        guard let n = notice else {
            attributed("Waiting for the run…", RunTheme.sans(13), RunTheme.muted).draw(at: NSPoint(x: side, y: 24))
            return
        }
        drawHeader(n)
        if n.card != nil { drawCard(n) }
        drawGraph(n)
        drawLegend()
        drawTiles()
        drawRevision(n)
        drawTable(n)
        drawWorkers(n)
        if let message = flash ?? said {
            let text = attributed(message, RunTheme.sans(12), flash == nil ? RunTheme.muted : RunTheme.bad)
            let size = text.size()
            text.draw(at: NSPoint(x: width - side - size.width, y: (spots["title"] ?? 18) + 62))
        }
    }

    private func drawHeader(_ n: RunNotice) {
        let y = spots["title"] ?? 18
        let title = attributed(n.name ?? n.run, RunTheme.sans(17, .semibold), RunTheme.text)
        title.draw(at: NSPoint(x: side, y: y))
        let sub = NSMutableAttributedString(attributedString: attributed(subtitle, RunTheme.sans(13), RunTheme.muted))
        if let range = subtitle.range(of: "pipeline \(n.pipeline)") {
            let start = subtitle.distance(from: subtitle.startIndex, to: range.lowerBound) + "pipeline ".count
            sub.addAttributes([.foregroundColor: RunTheme.text, .font: RunTheme.sans(13, .semibold)], range: NSRange(location: start, length: n.pipeline.utf16.count))
        }
        sub.draw(with: CGRect(x: wrapped ? side : side + title.size().width + 14, y: wrapped ? y + 28 : y + 4, width: width - 2 * side, height: 18), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }

    private func drawCard(_ n: RunNotice) {
        guard let card = n.card else { return }
        let path = NSBezierPath(roundedRect: cardFrame, xRadius: 9, yRadius: 9)
        RunTheme.warnFill.setFill()
        path.fill()
        RunTheme.warn.setStroke()
        path.lineWidth = 1
        path.stroke()
        let dot = NSBezierPath(ovalIn: CGRect(x: cardFrame.minX + 16, y: cardFrame.midY - 4, width: 8, height: 8))
        RunTheme.warn.setFill()
        dot.fill()
        let title = attributed(card.title, RunTheme.sans(13, .semibold), RunTheme.text)
        let titleHeight = ceil(title.boundingRect(with: CGSize(width: cardText, height: 400), options: [.usesLineFragmentOrigin]).height)
        title.draw(with: CGRect(x: cardFrame.minX + 36, y: cardFrame.minY + 10, width: cardText, height: titleHeight), options: [.usesLineFragmentOrigin])
        attributed(card.detail, RunTheme.sans(12), RunTheme.muted).draw(with: CGRect(x: cardFrame.minX + 36, y: cardFrame.minY + 14 + titleHeight, width: cardText, height: 18), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }

    private func drawGraph(_ n: RunNotice) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        context.saveGState()
        context.translateBy(x: graphOrigin.x, y: graphOrigin.y)
        context.scaleBy(x: graphScale, y: graphScale)
        for placed in graph.edges {
            let color: NSColor = placed.edge.fail ? RunTheme.warn : placed.edge.taken ? RunTheme.text : RunTheme.muted
            let path = NSBezierPath()
            path.lineWidth = placed.edge.taken && !placed.edge.fail ? 1.6 : 1.3
            path.lineJoinStyle = .miter
            for (i, point) in placed.points.enumerated() {
                if i == 0 { path.move(to: point) } else { path.line(to: point) }
            }
            color.setStroke()
            path.stroke()
            if placed.points.count >= 2 { arrowhead(from: placed.points[placed.points.count - 2], to: placed.points[placed.points.count - 1], color: color) }
            if let label = placed.label {
                let text = attributed(label.text, RunTheme.mono(10), placed.edge.fail ? RunTheme.warn : RunTheme.muted)
                let size = text.size()
                text.draw(at: NSPoint(x: label.trailing ? label.at.x - size.width : label.at.x, y: label.at.y - size.height + 3))
            }
        }
        for placed in graph.nodes { drawNode(placed, live: n.live) }
        context.restoreGState()
    }

    private func arrowhead(from a: CGPoint, to b: CGPoint, color: NSColor) {
        let angle = atan2(b.y - a.y, b.x - a.x)
        let size: CGFloat = 6.5
        let tip = CGPoint(x: b.x + cos(angle) * 3, y: b.y + sin(angle) * 3)
        let path = NSBezierPath()
        path.move(to: tip)
        path.line(to: CGPoint(x: tip.x - size * cos(angle - .pi / 7), y: tip.y - size * sin(angle - .pi / 7)))
        path.line(to: CGPoint(x: tip.x - size * cos(angle + .pi / 7), y: tip.y - size * sin(angle + .pi / 7)))
        path.close()
        color.setFill()
        path.fill()
    }

    private func drawNode(_ placed: PlacedNode, live: Bool) {
        let node = placed.node, frame = placed.frame
        let done = node.kind == "done"
        let dim = node.state == nil && node.visits == 0 && (live || done)
        NSGraphicsContext.current?.cgContext.setAlpha(dim ? 0.55 : 1)
        defer { NSGraphicsContext.current?.cgContext.setAlpha(1) }
        let radius: CGFloat = done || node.kind == "ask" ? 14 : 7
        let path = NSBezierPath(roundedRect: frame, xRadius: radius, yRadius: radius)
        let fill: NSColor = node.state == "now" ? RunTheme.tint : done ? .clear : node.kind == "run" ? RunTheme.term : node.kind == "merge" ? RunTheme.panel2 : RunTheme.panel
        fill.setFill()
        path.fill()
        path.lineWidth = node.state == "now" || node.state == "wait" ? 2 : 1.2
        if done { path.setLineDash([3, 3], count: 2, phase: 0) }
        (done ? RunTheme.muted : RunTheme.state(node.state) ?? RunTheme.line).setStroke()
        path.stroke()
        let mono = node.kind == "run"
        attributed(node.id, mono ? RunTheme.mono(11.5, .medium) : RunTheme.sans(11.5, .semibold), RunTheme.text).draw(at: NSPoint(x: frame.minX + 9, y: frame.minY + 3))
        if !node.who.isEmpty { attributed(node.who, mono ? RunTheme.mono(10) : RunTheme.sans(10), RunTheme.muted).draw(at: NSPoint(x: frame.minX + 9, y: frame.minY + 18)) }
        let mark: String? = switch node.state { case "pass"?: "✓"; case "fail"?: "✕"; case "now"?: "●"; case "wait"?: "?"; default: nil }
        if let mark, let color = RunTheme.state(node.state) {
            let text = attributed(mark, RunTheme.sans(11, .semibold), color)
            text.draw(at: NSPoint(x: frame.maxX - 8 - text.size().width, y: frame.minY + 3))
        }
        if node.visits > 1 {
            let pill = CGRect(x: frame.maxX - 21, y: frame.minY - 16, width: 22, height: 16)
            let shape = NSBezierPath(roundedRect: pill, xRadius: 8, yRadius: 8)
            RunTheme.panel2.setFill()
            shape.fill()
            (RunTheme.state(node.state) ?? RunTheme.line).setStroke()
            shape.lineWidth = 1
            shape.stroke()
            let text = attributed("×\(node.visits)", RunTheme.mono(10, .medium), RunTheme.text)
            let size = text.size()
            text.draw(at: NSPoint(x: pill.midX - size.width / 2, y: pill.midY - size.height / 2))
        }
    }

    private func legendWidth(_ item: String) -> CGFloat { (item == "×N visits" ? 0 : 15) + measure(item, RunTheme.sans(11.5)) + 16 }

    private func legendRows() -> [[String]] {
        var rows: [[String]] = [[]]
        var x = side
        for item in legendItems {
            if x + legendWidth(item) > width - side, !(rows.last?.isEmpty ?? true) {
                rows.append([])
                x = side
            }
            rows[rows.count - 1].append(item)
            x += legendWidth(item)
        }
        return rows
    }

    private func drawLegend() {
        for (line, items) in legendRows().enumerated() { drawLegend(items, y: (spots["legend"] ?? 0) + CGFloat(line) * 20) }
    }

    private func drawLegend(_ items: [String], y: CGFloat) {
        var x = side
        for item in items {
            if item != "×N visits" {
                let swatch = CGRect(x: x, y: y + 3, width: 10, height: 10)
                let path = NSBezierPath(roundedRect: swatch, xRadius: item == "your answer" ? 5 : 3, yRadius: item == "your answer" ? 5 : 3)
                let fill: NSColor = item == "running" ? RunTheme.tint : item == "command" ? RunTheme.term : item == "merge" ? RunTheme.panel2 : RunTheme.panel
                fill.setFill()
                path.fill()
                let stroke: NSColor = switch item { case "passed": RunTheme.ok; case "failed": RunTheme.bad; case "running": RunTheme.accent; case "waiting on you": RunTheme.warn; default: RunTheme.line }
                stroke.setStroke()
                path.lineWidth = 1.5
                path.stroke()
                x += 15
            }
            let text = attributed(item, RunTheme.sans(11.5), RunTheme.muted)
            text.draw(at: NSPoint(x: x, y: y))
            x += text.size().width + 16
        }
    }

    private func drawTiles() {
        for (frame, tile) in zip(tileFrames, tiles) {
            let path = NSBezierPath(roundedRect: frame, xRadius: 8, yRadius: 8)
            RunTheme.panel.setFill()
            path.fill()
            RunTheme.line.setStroke()
            path.lineWidth = 1
            path.stroke()
            attributed(tile.value, RunTheme.sans(16, .semibold), RunTheme.text).draw(at: NSPoint(x: frame.minX + 11, y: frame.minY + 6))
            let label = NSMutableAttributedString(attributedString: attributed(tile.label, RunTheme.sans(11), RunTheme.muted))
            if tile.label.hasPrefix("wall · "), let end = tile.label.range(of: " waiting on you") {
                let length = tile.label.distance(from: tile.label.index(tile.label.startIndex, offsetBy: 7), to: end.lowerBound)
                label.addAttribute(.foregroundColor, value: RunTheme.warn, range: NSRange(location: 7, length: length))
            }
            label.draw(at: NSPoint(x: frame.minX + 11, y: frame.minY + 28))
        }
    }

    private func drawRevision(_ n: RunNotice) {
        var y = spots["revision"] ?? 0
        toggles = []
        let revisions = shownRevisions(n)
        if revisions.isEmpty {
            attributed("the run's branch \(n.branch)", RunTheme.sans(12), RunTheme.muted).draw(at: NSPoint(x: side, y: y))
            return
        }
        let room = width - 2 * side
        let clipped = NSMutableParagraphStyle()
        clipped.lineBreakMode = .byTruncatingTail
        for (index, revision) in revisions.enumerated() {
            let refused = revision.n == nil
            let text = NSMutableAttributedString()
            text.append(attributed(revision.title, RunTheme.sans(12, .semibold), refused ? RunTheme.warn : RunTheme.text))
            text.append(attributed(" " + revision.detail + (index == 0 ? " · the run's branch \(n.branch)" : ""), RunTheme.sans(12), RunTheme.muted))
            let toggle = revision.changed ? attributed(open(revision) ? "hide diff" : "show diff", RunTheme.sans(12), RunTheme.accent) : nil
            let toggleWidth = toggle.map { ceil($0.size().width) + 14 } ?? 0
            text.addAttribute(.paragraphStyle, value: clipped, range: NSRange(location: 0, length: text.length))
            let textWidth = min(ceil(text.size().width), room - toggleWidth)
            text.draw(in: CGRect(x: side, y: y, width: textWidth, height: revisionLine))
            if let toggle, let number = revision.n {
                attributed("·", RunTheme.sans(12), RunTheme.muted).draw(at: NSPoint(x: side + textWidth + 4, y: y))
                let frame = CGRect(x: side + textWidth + 14, y: y, width: ceil(toggle.size().width), height: 16)
                toggle.draw(at: frame.origin)
                toggles.append((frame, number))
            }
            y += revisionLine
            for error in revision.errors {
                let line = NSMutableAttributedString(attributedString: attributed(error, RunTheme.mono(11.5), RunTheme.bad))
                line.addAttribute(.paragraphStyle, value: clipped, range: NSRange(location: 0, length: line.length))
                line.draw(in: CGRect(x: side + 14, y: y, width: room - 14, height: diffLine))
                y += diffLine
            }
            guard open(revision) else { continue }
            let box = CGRect(x: side, y: y + 2, width: room, height: CGFloat(revision.diff.count) * diffLine + 8)
            let path = NSBezierPath(roundedRect: box, xRadius: 6, yRadius: 6)
            RunTheme.panel.setFill()
            path.fill()
            RunTheme.line.setStroke()
            path.lineWidth = 1
            path.stroke()
            var ly = box.minY + 4
            for line in revision.diff {
                let color = line.sign == "+" ? RunTheme.ok : line.sign == "-" ? RunTheme.bad : RunTheme.muted
                let shown = NSMutableAttributedString(attributedString: attributed((line.sign == "-" ? "−" : line.sign) + " " + line.text, RunTheme.mono(11.5), color))
                shown.addAttribute(.paragraphStyle, value: clipped, range: NSRange(location: 0, length: shown.length))
                shown.draw(in: CGRect(x: box.minX + 10, y: ly, width: box.width - 20, height: diffLine))
                ly += diffLine
            }
            y += box.height + 6
        }
    }

    private func drawTable(_ n: RunNotice) {
        let header = ["#", "Stage", "Who", "Took", "Verdict", "Turns", "Tokens", "Notes"]
        let right: Set<Int> = [3, 5, 6]
        attributed("STAGES, IN ORDER", RunTheme.sans(11), RunTheme.muted).draw(at: NSPoint(x: side, y: spots["stages"] ?? 0))
        let all = [header] + rows
        var widths: [CGFloat] = header.indices.map { i in all.map { measure($0[i], i == 1 || i == 0 ? RunTheme.mono(11.5) : RunTheme.sans(12)) }.max() ?? 0 }
        widths[7] = 0
        var xs: [CGFloat] = []
        var x = side + 8
        for w in widths {
            xs.append(x)
            x += w + 18
        }
        let top = spots["table"] ?? 0
        for (r, row) in all.enumerated() {
            let y = top + CGFloat(r) * rowHeight
            let running = r > 0 && n.visits[r - 1].endedAt == nil
            if running {
                RunTheme.tint.setFill()
                CGRect(x: side, y: y, width: width - 2 * side, height: rowHeight).fill()
            }
            (r == 0 ? RunTheme.line : RunTheme.rowLine).setFill()
            CGRect(x: side, y: y + rowHeight - 1, width: width - 2 * side, height: 1).fill()
            for (i, cell) in row.enumerated() {
                let font = r > 0 && (i == 0 || i == 1) ? RunTheme.mono(11.5) : RunTheme.sans(12)
                let color: NSColor = r == 0 || i == 7 ? RunTheme.muted : i == 4 ? RunTheme.verdict(cell) : RunTheme.text
                let text = attributed(cell, r == 0 ? RunTheme.sans(12, .medium) : font, color)
                let size = text.size()
                let cellX = right.contains(i) ? xs[i] + widths[i] - size.width : xs[i]
                let limit = i == 7 ? width - side - 8 - cellX : widths[i] + 2
                text.draw(with: CGRect(x: cellX, y: y + (rowHeight - size.height) / 2, width: max(limit, 10), height: size.height), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            }
        }
    }

    private func drawWorkers(_ n: RunNotice) {
        let y = spots["workers"] ?? 0
        attributed("WORKERS NOW RUNNING", RunTheme.sans(11), RunTheme.muted).draw(at: NSPoint(x: side, y: y))
        let lines = workerLines
        if lines.isEmpty {
            let none = n.live ? "none: the run is between stages" + (n.state == "paused" ? ", paused" : "") : "none: the run has ended"
            attributed(none, RunTheme.sans(12), RunTheme.muted).draw(at: NSPoint(x: side + 8, y: y + 22))
            return
        }
        for (i, line) in lines.enumerated() {
            let dot = NSBezierPath(ovalIn: CGRect(x: side + 8, y: y + 27 + CGFloat(i) * 20, width: 7, height: 7))
            RunTheme.accent.setFill()
            dot.fill()
            attributed(line, RunTheme.sans(12), RunTheme.text).draw(at: NSPoint(x: side + 22, y: y + 22 + CGFloat(i) * 20))
        }
    }
}
