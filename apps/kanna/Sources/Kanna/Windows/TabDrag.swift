import AppKit

@MainActor var scriptedCancel = false

@MainActor func trackDrag(_ moved: (NSPoint) -> Void) -> NSPoint? {
    if let points = scriptedDrag, let end = points.last {
        scriptedDrag = nil
        points.dropLast().forEach(moved)
        guard !scriptedCancel else {
            scriptedCancel = false
            moved(end)
            return nil
        }
        return end
    }
    while let event = NSApp.nextEvent(matching: [.leftMouseDragged, .leftMouseUp, .keyDown], until: .distantFuture, inMode: .eventTracking, dequeue: true) {
        if event.type == .keyDown {
            if event.keyCode == 53 { return nil }
            continue
        }
        let point = screenPoint(event)
        if event.type == .leftMouseUp { return point }
        moved(point)
    }
    return NSEvent.mouseLocation
}

@MainActor
final class Motion {
    static var still: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }
    private var timer: Timer?
    private var step: ((Double) -> Bool)?

    func run(_ step: @escaping (Double) -> Bool) {
        self.step = step
        guard timer == nil else { return }
        let start = CACurrentMediaTime()
        let timer = Timer(timeInterval: 1.0 / 120, repeats: true) { _ in
            MainActor.assumeIsolated {
                guard let step = self.step else { return self.stop() }
                if step(CACurrentMediaTime() - start) { self.stop() }
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        step = nil
    }
}

struct Spring {
    static let stiffness = 600.0
    static let damping = 0.72
    private var value: [Double]
    private var velocity: [Double]

    init(_ rect: NSRect) {
        value = Self.parts(rect)
        velocity = value.map { _ in 0 }
    }

    var rect: NSRect { NSRect(x: value[0], y: value[1], width: value[2], height: value[3]) }

    mutating func step(to target: NSRect, dt: Double) -> Bool {
        let goal = Self.parts(target), friction = 2 * Self.damping * Self.stiffness.squareRoot()
        for i in value.indices {
            velocity[i] += (-Self.stiffness * (value[i] - goal[i]) - friction * velocity[i]) * dt
            value[i] += velocity[i] * dt
        }
        return zip(value, goal).allSatisfy { abs($0 - $1) < 0.3 } && velocity.allSatisfy { abs($0) < 5 }
    }

    private static func parts(_ rect: NSRect) -> [Double] { [rect.minX, rect.minY, rect.width, rect.height].map(Double.init) }
}

func blend(_ a: NSRect, _ b: NSRect, _ t: Double) -> NSRect {
    let t = CGFloat(t)
    return NSRect(x: a.minX + (b.minX - a.minX) * t, y: a.minY + (b.minY - a.minY) * t,
                  width: a.width + (b.width - a.width) * t, height: a.height + (b.height - a.height) * t)
}

func easeOut(_ t: Double) -> Double { 1 - pow(1 - min(max(t, 0), 1), 3) }

@MainActor
final class TabGhost {
    let panel: NSPanel
    private let box = NSView()
    private let tabView = NSImageView()
    private let previewView = NSImageView()
    private let shown: Bool
    private(set) var preview = false

    init(tab: NSImage?, preview: NSImage, at rect: NSRect, appearance: NSAppearance, shown: Bool) {
        self.shown = shown
        panel = NSPanel(contentRect: rect, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.ignoresMouseEvents = true
        panel.level = DropHint.level
        panel.appearance = appearance
        box.frame = NSRect(origin: .zero, size: rect.size)
        box.wantsLayer = true
        box.layer?.cornerRadius = 6
        box.layer?.masksToBounds = true
        box.layer?.borderColor = NSColor.black.withAlphaComponent(0.5).cgColor
        box.layer?.backgroundColor = box.resolved(.windowBackgroundColor)
        for (view, image) in [(previewView, preview), (tabView, tab)] {
            view.image = image
            view.frame = box.bounds
            view.autoresizingMask = [.width, .height]
            box.addSubview(view)
        }
        previewView.imageScaling = .scaleAxesIndependently
        previewView.alphaValue = 0
        tabView.imageScaling = .scaleNone
        tabView.imageAlignment = .alignLeft
        panel.contentView = box
        place(rect)
    }

    var windowNumber: Int { panel.windowNumber }

    func place(_ rect: NSRect) {
        panel.setFrame(rect, display: true)
        if shown, !panel.isVisible { panel.orderFront(nil) }
    }

    func setPreview(_ on: Bool, still: Bool) {
        preview = on
        box.layer?.cornerRadius = on ? 8 : 6
        box.layer?.borderWidth = on ? 1 : 0
        NSAnimationContext.runAnimationGroup { context in
            context.duration = still ? 0 : 0.12
            previewView.animator().alphaValue = on ? 1 : 0
            tabView.animator().alphaValue = on ? 0 : 1
        }
    }

    func close() { panel.orderOut(nil) }
}

@MainActor
final class TabDrag {
    static let scale: CGFloat = 0.32
    static let pull: CGFloat = 14
    static let morph = 0.18
    static let grow = 0.24

    private let app: AppController
    private let tab: String
    private let source: WindowController
    private let origin: Int
    private let grab: CGFloat
    private let width: CGFloat
    private let size: NSSize
    private let ghost: TabGhost
    private let still: Bool
    private let motion = Motion()
    private var owner: ObjectIdentifier { ObjectIdentifier(self) }
    private var over: (controller: WindowController, gap: Int)?
    private var cursor: NSPoint
    private var morphing: (from: NSRect, at: Double)?

    init?(app: AppController, tab: String, grab: CGFloat?, quiet: Bool) {
        guard let id = app.windowOf(tab: tab)?["id"]?.string, let source = app.controllers[id],
              let index = source.tabBar.index(of: tab), let frame = source.tabBar.frame(of: tab) else { return nil }
        let window = source.window
        self.app = app
        self.tab = tab
        self.source = source
        origin = index
        width = frame.width
        self.grab = min(max(grab ?? frame.width / 2, 0), frame.width)
        size = window.frame.size
        still = quiet || Motion.still
        let bar = source.tabBar
        let image = bar.image(of: tab)
        let active = source.tab == tab && source.shownPage == nil
        let start = window.convertToScreen(bar.convert(frame, to: nil))
        ghost = TabGhost(tab: image, preview: Self.preview(of: window, tab: image, active: active), at: start, appearance: window.effectiveAppearance, shown: !quiet)
        cursor = NSPoint(x: start.minX + self.grab, y: start.midY)
        over = (source, index)
        bar.setGap(TabBar.Gap(owner: owner, hiding: tab, width: width, index: index), animated: false)
    }

    private static func preview(of window: NSWindow, tab: NSImage?, active: Bool) -> NSImage {
        let size = window.frame.size
        if active, let image = Snapshot.image(of: window) { return NSImage(cgImage: image, size: size) }
        let content = window.contentRect(forFrameRect: window.frame).height
        return NSImage(size: size, flipped: false) { rect in
            NSColor.windowBackgroundColor.setFill()
            rect.fill()
            NSColor.textBackgroundColor.setFill()
            NSRect(x: 0, y: 0, width: size.width, height: content - TabBar.height).fill()
            tab?.draw(at: NSPoint(x: 8, y: content - TabBar.height), from: .zero, operation: .sourceOver, fraction: 1)
            return true
        }
    }

    func move(_ point: NSPoint) {
        cursor = point
        over = target(at: point)
        showGaps()
        let preview = over == nil
        if preview != ghost.preview {
            ghost.setPreview(preview, still: still)
            if !still {
                morphing = (ghost.panel.frame, CACurrentMediaTime())
                motion.run { [self] _ in follow() }
            }
        }
        follow()
    }

    func drop(at end: NSPoint?) -> Bool {
        guard let end else {
            home()
            return false
        }
        move(end)
        motion.stop()
        morphing = nil
        if let (controller, gap) = over {
            if controller === source && gap == origin {
                home()
                return false
            }
            let index = controller === source && gap >= origin ? gap + 1 : gap
            app.layoutCommand("move-tab", app.address(controller.id).merging(["tab": .string(tab), "index": .number(Double(index))]) { _, new in new })
            settle(in: controller, gap: gap)
            return true
        }
        if app.controller(at: end, skipping: ghost.windowNumber) != nil {
            home()
            return false
        }
        tearOff(at: end)
        return true
    }

    private func target(at point: NSPoint) -> (controller: WindowController, gap: Int)? {
        guard let controller = app.controller(at: point, skipping: ghost.windowNumber), !controller.tabBar.isHiddenOrHasHiddenAncestor else { return nil }
        let bar = controller.tabBar
        let inBar = bar.convert(controller.window.convertPoint(fromScreen: point), from: nil)
        guard bar.bounds.insetBy(dx: 0, dy: -Self.pull).contains(inBar) else { return nil }
        return (controller, bar.gapIndex(center: inBar.x - grab + width / 2, width: width, hiding: tab))
    }

    private func showGaps() {
        for controller in app.controllers.values {
            let index: Int?? = controller === over?.controller ? .some(over?.gap) : controller === source ? .some(nil) : nil
            if let index {
                controller.tabBar.setGap(TabBar.Gap(owner: owner, hiding: tab, width: width, index: index), animated: !still)
            } else {
                controller.tabBar.clearGap(owner: owner, animated: !still)
            }
        }
    }

    private func goal(_ point: NSPoint) -> NSRect {
        guard let (controller, _) = over else {
            let w = size.width * Self.scale, h = size.height * Self.scale
            return NSRect(x: point.x - 40 * Self.scale, y: point.y + 12 * Self.scale - h, width: w, height: h)
        }
        let bar = controller.tabBar, span = bar.tabsSpan(hiding: tab)
        let inBar = bar.convert(controller.window.convertPoint(fromScreen: point), from: nil)
        let x = min(max(inBar.x - grab, span.lowerBound), span.upperBound)
        return slot(in: controller, NSRect(x: x, y: 0, width: width, height: TabBar.height))
    }

    private func slot(in controller: WindowController, _ rect: NSRect) -> NSRect {
        controller.window.convertToScreen(controller.tabBar.convert(rect, to: nil))
    }

    @discardableResult
    private func follow() -> Bool {
        var rect = goal(cursor)
        if let (from, at) = morphing {
            let t = (CACurrentMediaTime() - at) / Self.morph
            rect = blend(from, rect, easeOut(t))
            if t >= 1 { morphing = nil }
        }
        ghost.place(rect)
        return morphing == nil
    }

    private func home() {
        over = (source, origin)
        showGaps()
        ghost.setPreview(false, still: still)
        settle(in: source, gap: origin)
    }

    private func settle(in controller: WindowController, gap: Int) {
        let bar = controller.tabBar
        let target = slot(in: controller, bar.gapRect(gap, width: width, hiding: tab))
        guard !still else {
            ghost.place(target)
            return waitThenFinish(in: bar)
        }
        var spring = Spring(ghost.panel.frame)
        var last = 0.0
        motion.run { [self] elapsed in
            var dt = elapsed - last, done = false
            last = elapsed
            while dt > 0, !done {
                done = spring.step(to: target, dt: min(dt, 1.0 / 240))
                dt -= 1.0 / 240
            }
            ghost.place(done ? target : spring.rect)
            guard done else { return elapsed > 1 ? finished() : false }
            waitThenFinish(in: bar)
            return false
        }
    }

    private func waitThenFinish(in bar: TabBar) {
        let deadline = CACurrentMediaTime() + 1
        guard bar.index(of: tab) == nil else { return finish() }
        motion.run { [self] _ in
            bar.index(of: tab) != nil || CACurrentMediaTime() > deadline ? finished() : false
        }
    }

    private func finished() -> Bool {
        finish()
        return true
    }

    private func finish() {
        motion.stop()
        for controller in app.controllers.values { controller.tabBar.clearGap(owner: owner, animated: false) }
        ghost.close()
    }

    private func tearOff(at point: NSPoint) {
        let frame = app.newWindowFrame(at: point, size: size)
        app.layoutCommand("move-tab", ["tab": .string(tab), "window": "new"]) { [self] reply in
            guard reply["ok"]?.bool == true, let id = reply["window"]?.string else { return home() }
            app.physical.setFrame(id, frame)
            let deadline = CACurrentMediaTime() + 1
            if let window = app.controllers[id]?.window { return grow(window, to: frame) }
            motion.run { [self] _ in
                if let window = app.controllers[id]?.window {
                    grow(window, to: frame)
                    return false
                }
                return CACurrentMediaTime() > deadline ? finished() : false
            }
        }
    }

    private func grow(_ window: NSWindow, to frame: NSRect) {
        window.setFrame(frame, display: true)
        guard !still else { return finish() }
        window.alphaValue = 0
        let from = ghost.panel.frame
        motion.stop()
        motion.run { [self] elapsed in
            let t = elapsed / Self.grow
            ghost.place(blend(from, frame, easeOut(t)))
            guard t >= 1 else { return false }
            window.alphaValue = 1
            return finished()
        }
    }
}
