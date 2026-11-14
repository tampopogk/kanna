import AppKit
import KannaWindows

@MainActor
final class RowAnimator {
    static var stillForTests: Bool?
    static var still: Bool { stillForTests ?? NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }
    static let timing = CAMediaTimingFunction(controlPoints: RowMotion.curve.0, RowMotion.curve.1, RowMotion.curve.2, RowMotion.curve.3)

    let list: String
    private var order: [String]?
    private var views: [String: NSView] = [:]
    private var frames: [String: NSRect] = [:]
    private var fresh = true
    private var catchingUp = false
    private(set) var last: RowMotion?
    private(set) var updates = 0
    private(set) var slid = 0

    init(_ list: String) { self.list = list }

    var debug: JSON {
        guard let last else { return ["list": .string(list), "updates": .number(0)] }
        return ["list": .string(list), "why": .string(last.why.rawValue), "animated": .bool(last.animated), "updates": .number(Double(updates)),
                "arrived": .array(last.arrived.map { .string($0) }), "left": .array(last.left.map { .string($0) }), "moved": .array(last.moved.map { .string($0) }), "slid": .number(Double(slid))]
    }

    func appeared(catchingUp: Bool = false) {
        fresh = true
        self.catchingUp = catchingUp
    }

    func caughtUp() {
        guard catchingUp else { return }
        catchingUp = false
        fresh = false
    }

    func update(_ container: NSView, rows: [(key: String, view: NSView)], fixtures: [(key: String, view: NSView)] = [], quiet: Bool = false) {
        let shown = Self.onScreen(container)
        let plan = RowMotion.plan(from: order, to: rows.map(\.key), shown: shown && !fresh, still: Self.still, quiet: quiet)
        if plan.why != .unchanged {
            last = plan
            updates += 1
        }
        if plan.animated { animate(plan, in: container, rows: rows, fixtures: fixtures) }
        order = rows.map(\.key)
        if shown, !catchingUp || plan.why != .unchanged {
            fresh = false
            catchingUp = false
        }
        let all = rows + fixtures
        views = Dictionary(all.map { ($0.key, $0.view) }, uniquingKeysWith: { _, new in new })
        frames = views.mapValues(\.frame)
    }

    static func onScreen(_ view: NSView) -> Bool {
        guard let window = view.window, window.isVisible, !view.isHiddenOrHasHiddenAncestor else { return false }
        return TerminalSurfaceView.ignoresOcclusion || window.occlusionState.contains(.visible)
    }

    private func animate(_ plan: RowMotion, in container: NSView, rows: [(key: String, view: NSView)], fixtures: [(key: String, view: NSView)]) {
        let seen = container.visibleRect
        let arrived = Set(plan.arrived)
        slid = 0
        for key in plan.left {
            guard let view = views[key], let was = shown(key), seen.intersects(was) else { continue }
            fade(view, at: was, in: container)
        }
        for (key, view) in rows + fixtures {
            guard let layer = view.layer else { continue }
            if arrived.contains(key) {
                guard seen.intersects(view.frame) else { continue }
                let fade = CABasicAnimation(keyPath: "opacity")
                fade.fromValue = 0
                fade.toValue = 1
                Self.time(fade)
                layer.add(fade, forKey: "row.arrive")
                continue
            }
            guard let was = shown(key) else { continue }
            let dx = was.minX - view.frame.minX, dy = was.minY - view.frame.minY
            guard dx != 0 || dy != 0, seen.intersects(was) || seen.intersects(view.frame) else { continue }
            let slide = CABasicAnimation(keyPath: "position")
            slide.isAdditive = true
            slide.fromValue = NSValue(point: NSPoint(x: dx, y: dy))
            slide.toValue = NSValue(point: .zero)
            Self.time(slide)
            layer.add(slide, forKey: "row.slide")
            slid += 1
        }
    }

    private func shown(_ key: String) -> NSRect? {
        guard let frame = frames[key] else { return nil }
        guard let layer = views[key]?.layer, let seen = layer.presentation(), layer.animation(forKey: "row.slide") != nil else { return frame }
        return NSRect(origin: seen.frame.origin, size: frame.size)
    }

    private func fade(_ view: NSView, at frame: NSRect, in container: NSView) {
        guard view.bounds.width > 0, view.bounds.height > 0, let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        let image = NSImage(size: view.bounds.size)
        image.addRepresentation(rep)
        let ghost = RowGhost(frame: frame)
        ghost.image = image
        container.addSubview(ghost)
        guard let layer = ghost.layer else { return ghost.removeFromSuperview() }
        layer.opacity = 0
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 1
        fade.toValue = 0
        Self.time(fade)
        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak ghost] in
            MainActor.assumeIsolated { ghost?.removeFromSuperview() }
        }
        layer.add(fade, forKey: "row.leave")
        CATransaction.commit()
    }

    private static func time(_ animation: CABasicAnimation) {
        animation.duration = RowMotion.duration
        animation.timingFunction = timing
    }
}

final class RowGhost: NSImageView {
    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        imageScaling = .scaleNone
        imageAlignment = .alignTopLeft
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
