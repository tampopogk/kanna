import AppKit

enum Sequence {
    static let dotted: Set<Int> = [1, 4, 6, 25, 34]
    static let messageTypes: Set<Int> = [0, 1, 3, 4, 5, 6, 24, 25, 33, 34]
    static let note = 2, autonumber = 26, activeStart = 17, activeEnd = 18
    static let frameStarts: [Int: String] = [10: "loop", 12: "alt", 15: "opt", 19: "par", 22: "rect", 27: "critical", 30: "break", 32: "par"]
    static let frameSections: Set<Int> = [13, 20, 28]
    static let frameEnds: Set<Int> = [11, 14, 16, 21, 23, 29, 31]

    private struct OpenFrame {
        var keyword: String
        var condition: String
        var color: CGColor?
        var top = CGFloat.infinity, bottom = -CGFloat.infinity, left = CGFloat.infinity, right = -CGFloat.infinity
        var sections: [(condition: String, afterBottom: CGFloat, beforeTop: CGFloat)] = []
        var pendingSection: (condition: String, afterBottom: CGFloat)?
        var nested = false
        mutating func include(_ r: CGRect) {
            if let pending = pendingSection {
                sections.append((pending.condition, pending.afterBottom, r.minY))
                pendingSection = nil
            }
            top = min(top, r.minY); bottom = max(bottom, r.maxY); left = min(left, r.minX); right = max(right, r.maxX)
        }
    }

    static func scene(semantic: [String: Any], layout: [String: Any], log: MeasureLog = MeasureLog()) -> DiagramScene {
        let frame = Frame(layout: layout)
        var scene = DiagramScene(size: frame.size)
        let actors = semantic.dict("actors") ?? [:]
        let nodes = Dictionary(layout.list("nodes").compactMap { n in n.string("id").map { ($0, n) } }, uniquingKeysWith: { a, _ in a })
        let edges = Dictionary(layout.list("edges").compactMap { e in e.string("id").map { ($0, e) } }, uniquingKeysWith: { a, _ in a })
        var lifelineX: [String: CGFloat] = [:]
        var lifelineBottom: [String: CGFloat] = [:]
        for (id, edge) in edges where id.hasPrefix("lifeline-") {
            let points = edge.points.map(frame.point)
            guard let first = points.first, let last = points.last else { continue }
            let name = String(id.dropFirst("lifeline-".count))
            lifelineX[name] = first.x
            lifelineBottom[name] = last.y
            scene.shape(Paths.line(points), stroke: .role(.lifeline), lineWidth: 0.5)
        }

        var stack: [OpenFrame] = []
        var finished: [OpenFrame] = []
        var activations: [String: [CGFloat]] = [:]
        var bars: [(actor: String, top: CGFloat, bottom: CGFloat, depth: Int)] = []
        var lastLineY = (edges.values.compactMap { $0.points.first.map(frame.point)?.y }.min() ?? 0) - 10
        var lastBottom = lastLineY
        var number = 0
        var numbering = false
        var deferred: [() -> Void] = []
        func include(_ rect: CGRect) {
            for i in stack.indices { stack[i].include(rect) }
            lastBottom = rect.maxY
        }

        for message in semantic.list("messages") {
            let id = message.string("id") ?? ""
            let type = (message["type"] as? NSNumber)?.intValue ?? -1
            let text = message["message"] as? String ?? ""
            switch type {
            case _ where messageTypes.contains(type):
                guard let edge = edges["msg-\(id)"] else { continue }
                let points = edge.points.map(frame.point)
                guard let start = points.first, let end = points.last else { continue }
                lastLineY = start.y
                let labelBox = edge.dict("label")?.box.map(frame.rect)
                var extent = CGRect(x: min(start.x, end.x), y: start.y, width: abs(end.x - start.x), height: 0)
                if let labelBox { extent = extent.union(labelBox) }
                if start == end { extent = extent.union(CGRect(x: start.x, y: start.y, width: 60, height: 22)) }
                include(extent)
                if numbering { number += 1 }
                let shown = numbering ? number : nil
                deferred.append { drawMessage(type: type, start: start, end: end, label: text, labelBox: labelBox, number: shown, into: &scene) }
            case note:
                guard var box = nodes["note-\(id)"]?.box.map(frame.rect) else { continue }
                let needed = log.width(of: text, defaultSize: 14) + 20
                if message["wrap"] as? Bool != true, box.width < needed { box = box.insetBy(dx: (box.width - needed) / 2, dy: 0) }
                include(box)
                deferred.append {
                    scene.shape(Paths.rect(box), fill: .role(.noteFill), stroke: .role(.noteStroke))
                    scene.text(text, in: box.insetBy(dx: 4, dy: 2), htmlLike: false)
                }
            case autonumber:
                numbering = (message["message"] as? [String: Any])?["visible"] as? Bool ?? true
            case activeStart:
                guard let actor = message.string("from") else { continue }
                activations[actor, default: []].append(lastLineY)
            case activeEnd:
                guard let actor = message.string("from"), let top = activations[actor]?.popLast() else { continue }
                bars.append((actor, top, lastLineY, activations[actor]?.count ?? 0))
            case _ where frameStarts[type] != nil:
                let keyword = frameStarts[type]!
                stack.append(OpenFrame(keyword: keyword, condition: keyword == "rect" ? "" : text,
                                       color: keyword == "rect" ? Paths.color(text) : nil, nested: !stack.isEmpty))
            case _ where frameSections.contains(type):
                guard !stack.isEmpty else { continue }
                stack[stack.count - 1].pendingSection = (text, lastBottom)
            case _ where frameEnds.contains(type):
                guard var done = stack.popLast(), done.top.isFinite else { continue }
                done.pendingSection = nil
                let rect = frameRect(done)
                for i in stack.indices { stack[i].include(rect.insetBy(dx: -2, dy: -2)) }
                finished.append(done)
            default:
                break
            }
        }
        for (actor, tops) in activations {
            for (depth, top) in tops.enumerated() { bars.append((actor, top, lifelineBottom[actor] ?? top + 20, depth)) }
        }

        for f in finished where f.keyword == "rect" {
            scene.shape(Paths.rect(frameRect(f)), fill: .color(f.color ?? Palette(dark: false).color(.labelBackground)))
        }
        for f in finished where f.keyword != "rect" { drawFrame(f, into: &scene) }
        for bar in bars {
            guard let x = lifelineX[bar.actor] else { continue }
            let rect = CGRect(x: x - 5 + CGFloat(bar.depth) * 5, y: bar.top, width: 10, height: max(bar.bottom - bar.top, 10))
            scene.shape(Paths.rect(rect), fill: .role(.activationFill), stroke: .role(.activationStroke))
        }
        deferred.forEach { $0() }
        for (id, node) in nodes where id.hasPrefix("actor-") {
            guard let box = node.box.map(frame.rect) else { continue }
            let name = String(id.split(separator: "-", maxSplits: 2).last ?? "")
            let actor = actors[name] as? [String: Any] ?? [:]
            drawActor(name: actor.string("description") ?? name, stickFigure: actor.string("type") == "actor", box: box, into: &scene)
        }
        return scene
    }

    private static func frameRect(_ f: OpenFrame) -> CGRect {
        let pad: CGFloat = f.keyword == "rect" ? 10 : 22
        let titleRoom: CGFloat = f.keyword == "rect" ? 8 : 26
        return CGRect(x: f.left - pad, y: f.top - titleRoom, width: f.right - f.left + 2 * pad, height: f.bottom - f.top + titleRoom + 10)
    }

    private static func drawFrame(_ f: OpenFrame, into scene: inout DiagramScene) {
        let rect = frameRect(f)
        scene.shape(Paths.rect(rect), stroke: .role(.frameStroke), lineWidth: 2, dash: [2, 2])
        let font = LabelText.font(size: 14, bold: true)
        let tabWidth = LabelText.width(f.keyword, font: font) + 20
        let (l, t) = (rect.minX, rect.minY)
        scene.shape(Paths.polygon([CGPoint(x: l, y: t), CGPoint(x: l + tabWidth, y: t), CGPoint(x: l + tabWidth, y: t + 13),
                                   CGPoint(x: l + tabWidth - 7, y: t + 20), CGPoint(x: l, y: t + 20)]),
                    fill: .role(.frameLabelFill), stroke: .role(.frameStroke))
        scene.text(f.keyword, in: CGRect(x: l, y: t, width: tabWidth - 4, height: 20), size: 14, bold: true, htmlLike: false, fixedSize: true)
        if !f.condition.isEmpty {
            let condition = "[\(f.condition)]"
            let width = max(rect.width - tabWidth - 8, LabelText.width(condition, font: font) + 2)
            scene.text(condition, in: CGRect(x: l + tabWidth + 4, y: t + 2, width: width, height: 20),
                       size: 14, bold: true, htmlLike: false, fixedSize: true)
        }
        for section in f.sections {
            let y = (section.afterBottom + section.beforeTop) / 2 - 4
            scene.shape(Paths.line([CGPoint(x: rect.minX, y: y), CGPoint(x: rect.maxX, y: y)]), stroke: .role(.frameStroke), lineWidth: 2, dash: [3, 3])
            if !section.condition.isEmpty {
                scene.text("[\(section.condition)]", in: CGRect(x: rect.minX + 4, y: y + 1, width: rect.width - 8, height: 18), size: 14, bold: true, htmlLike: false, fixedSize: true)
            }
        }
    }

    private static func drawMessage(type: Int, start: CGPoint, end: CGPoint, label: String, labelBox: CGRect?, number: Int?,
                                    into scene: inout DiagramScene) {
        let dash: [CGFloat] = dotted.contains(type) ? [3, 3] : []
        let head: Paths.Head = [3, 4].contains(type) ? .cross : [5, 6].contains(type) ? .none : [24, 25].contains(type) ? .open : .filled
        if start == end {
            let path = CGMutablePath()
            path.move(to: start)
            path.addCurve(to: CGPoint(x: start.x, y: start.y + 20), control1: CGPoint(x: start.x + 60, y: start.y - 10),
                          control2: CGPoint(x: start.x + 60, y: start.y + 30))
            scene.shape(path, stroke: .role(.line), lineWidth: 1.5, dash: dash)
            if let (arrow, filled) = Paths.head(head, tip: CGPoint(x: start.x, y: start.y + 20), from: CGPoint(x: start.x + 20, y: start.y + 24)) {
                scene.shape(arrow, fill: filled ? .role(.line) : nil, stroke: .role(.line), lineWidth: 1.5)
            }
        } else {
            var line = [start, end]
            if head == .filled { line = Paths.shorten(line, end: 8) }
            if [33, 34].contains(type) { line = Array(Paths.shorten(line.reversed(), end: 8).reversed()) }
            scene.shape(Paths.line(line), stroke: .role(.line), lineWidth: 1.5, dash: dash)
            for (tip, from) in [(end, start)] + ([33, 34].contains(type) ? [(start, end)] : []) {
                if let (arrow, filled) = Paths.head(head, tip: tip, from: from) {
                    scene.shape(arrow, fill: filled ? .role(.line) : nil, stroke: .role(.line), lineWidth: filled ? 1 : 1.5)
                }
            }
        }
        if let labelBox { scene.text(label, in: labelBox.insetBy(dx: -2, dy: 0), htmlLike: false) }
        if let number {
            let circle = CGRect(x: start.x - 9, y: start.y - 9, width: 18, height: 18)
            scene.shape(CGPath(ellipseIn: circle, transform: nil), fill: .role(.line))
            scene.text("\(number)", in: circle, size: 11, color: .role(.numberText), bold: true, htmlLike: false, fixedSize: true)
        }
    }

    private static func drawActor(name: String, stickFigure: Bool, box: CGRect, into scene: inout DiagramScene) {
        guard stickFigure else {
            scene.shape(Paths.rect(box, radius: 3), fill: .role(.actorFill), stroke: .role(.actorStroke), lineWidth: 1.5, shadow: true)
            scene.text(name, in: box.insetBy(dx: 4, dy: 2), htmlLike: false)
            return
        }
        let cx = box.midX, top = box.minY + 2
        let figure = CGMutablePath()
        figure.addEllipse(in: CGRect(x: cx - 7, y: top, width: 14, height: 14))
        figure.addLines(between: [CGPoint(x: cx, y: top + 14), CGPoint(x: cx, y: top + 32)])
        figure.addLines(between: [CGPoint(x: cx - 12, y: top + 20), CGPoint(x: cx + 12, y: top + 20)])
        figure.addLines(between: [CGPoint(x: cx - 10, y: top + 44), CGPoint(x: cx, y: top + 32), CGPoint(x: cx + 10, y: top + 44)])
        scene.shape(figure, fill: nil, stroke: .role(.nodeStroke), lineWidth: 1.5)
        scene.text(name, in: CGRect(x: box.minX, y: top + 46, width: box.width, height: max(box.maxY - top - 46, 18)), htmlLike: false)
    }
}
