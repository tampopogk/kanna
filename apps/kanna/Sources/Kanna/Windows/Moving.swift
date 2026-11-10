import AppKit

extension AppController {
    enum Drop {
        case pane(String, side: String)
        case tab(String)
        case newTab(window: String)
        case newWindow(NSPoint)
    }

    func controller(at point: NSPoint, skipping floating: Int) -> WindowController? {
        if testingDrag {
            return NSApp.orderedWindows.lazy.compactMap { w in self.controllers.values.first { $0.window === w } }.first { $0.window.frame.contains(point) }
        }
        var number = NSWindow.windowNumber(at: point, belowWindowWithWindowNumber: 0)
        if number == floating { number = NSWindow.windowNumber(at: point, belowWindowWithWindowNumber: number) }
        return controllers.values.first { $0.window.windowNumber == number }
    }

    func newWindowFrame(at point: NSPoint, shape: WindowShape) -> NSRect {
        let size = shape.size
        let wanted = NSRect(x: point.x - 40, y: point.y - size.height + 12, width: size.width, height: size.height)
        guard let area = (NSScreen.screens.first { NSMouseInRect(point, $0.frame, false) } ?? NSScreen.main)?.visibleFrame else { return wanted }
        if shape.fills { return area }
        let width = min(size.width, area.width), height = min(size.height, area.height)
        let x = min(max(point.x - 40, area.minX), area.maxX - width)
        let y = min(max(point.y - height + 12, area.minY), area.maxY - height)
        return NSRect(x: x, y: y, width: width, height: height)
    }

    private func paneDrop(_ id: String, at point: NSPoint, hint: DropHint, shape: WindowShape) -> Drop? {
        guard let controller = controller(at: point, skipping: hint.windowNumber) else {
            hint.show(newWindowFrame(at: point, shape: shape), text: "new window", rounded: true)
            return .newWindow(point)
        }
        let window = controller.window, inWindow = window.convertPoint(fromScreen: point)
        let bar = controller.tabBar, inBar = bar.convert(inWindow, from: nil)
        if bar.bounds.contains(inBar) {
            let home = tabOf(pane: id)
            guard let (tab, frame) = bar.target(at: inBar), tab != home else {
                hint.hide()
                return nil
            }
            hint.show(window.convertToScreen(bar.convert(frame, to: nil)), text: tab == nil ? "+" : "")
            return tab.map(Drop.tab) ?? .newTab(window: controller.id)
        }
        for view in panes.all where view.window === window && view.superview != nil && !view.isHiddenOrHasHiddenAncestor && view.id != id {
            let p = view.convert(inWindow, from: nil), b = view.bounds
            guard b.contains(p) else { continue }
            if frozen(view.id) {
                hint.hide()
                return nil
            }
            let fx = p.x / b.width, fy = 1 - p.y / b.height
            var side = "swap"
            if !docked(view.id), !(fx > 0.3 && fx < 0.7 && fy > 0.3 && fy < 0.7) {
                side = [("left", fx), ("right", 1 - fx), ("top", fy), ("bottom", 1 - fy)].min { $0.1 < $1.1 }!.0
            }
            let area = switch side {
            case "left": NSRect(x: 0, y: 0, width: b.width / 2, height: b.height)
            case "right": NSRect(x: b.width / 2, y: 0, width: b.width / 2, height: b.height)
            case "top": NSRect(x: 0, y: b.height / 2, width: b.width, height: b.height / 2)
            case "bottom": NSRect(x: 0, y: 0, width: b.width, height: b.height / 2)
            default: b
            }
            hint.show(window.convertToScreen(view.convert(area, to: nil)), text: side == "swap" ? "swap" : "")
            return .pane(view.id, side: side)
        }
        hint.hide()
        return nil
    }

    @discardableResult
    func dragPane(_ id: String) -> Bool {
        guard let window = panes[id]?.window, !frozen(id) else { return false }
        let shape = WindowShape(window)
        let hint = DropHint()
        let end = trackMouse { _ = paneDrop(id, at: $0, hint: hint, shape: shape) }
        let drop = paneDrop(id, at: end, hint: hint, shape: shape)
        hint.hide()
        var args: [String: JSON] = ["pane": .string(id)]
        switch drop {
        case .pane(let to, let side):
            args["to"] = .string(to)
            args["side"] = .string(side)
        case .tab(let tab): args["tab"] = .string(tab)
        case .newTab(let window):
            args["tab"] = "new"
            args.merge(address(window)) { _, new in new }
        case .newWindow: args["window"] = "new"
        case nil: return false
        }
        perform("move", args, tornOff: drop, shape: shape)
        return true
    }

    @discardableResult
    func dragTab(_ id: String, grab: CGFloat? = nil) -> Bool {
        guard let drag = TabDrag(app: self, tab: id, grab: grab, quiet: DropHint.quiet) else { return false }
        let end = trackDrag { drag.move($0) }
        return drag.drop(at: end)
    }

    private func perform(_ cmd: String, _ args: [String: JSON], tornOff drop: Drop?, shape: WindowShape) {
        layoutCommand(cmd, args) { [weak self] reply in
            guard let self, reply["ok"]?.bool == true, case .newWindow(let point)? = drop, let id = reply["window"]?.string else { return }
            let frame = newWindowFrame(at: point, shape: shape)
            physical.setFrame(id, frame)
            controllers[id]?.window.setFrame(frame, display: true)
        }
    }

    func debugDrag(_ args: JSON) async throws -> [String: JSON] {
        func screen(_ view: NSView, _ point: NSPoint) throws -> NSPoint {
            guard let window = view.window else { throw KannaError("not_found", "not on screen") }
            return window.convertPoint(toScreen: view.convert(point, to: nil))
        }
        testingDrag = true
        DropHint.quiet = true
        defer {
            testingDrag = false
            DropHint.quiet = false
        }
        if args["fill"]?.bool == true, let tab = args["tab"]?.string, let window = windowOf(tab: tab)?["id"]?.string.flatMap({ controllers[$0]?.window }),
           let area = window.screen?.visibleFrame {
            window.setFrame(area, display: true)
        }
        try await Task.sleep(for: .milliseconds(50))
        if let reference = args["divider"]?.string {
            let pane = try resolve(reference)
            if let (controller, handle) = controllers.values.lazy.compactMap({ c in c.dockDivider.flatMap { $0.pane == pane ? (c, $0) : nil } }).first,
               let content = handle.superview {
                let to = try screen(content, handle.position(CGFloat(args["at"]?.number ?? 0.5)))
                scriptedDrag = [to, to]
                handle.drag(in: content)
                redraw(controller)
                return [:]
            }
            guard let (controller, handle) = controllers.values.lazy.compactMap({ c in c.dividers.first { $0.after == pane }.map { (c, $0) } }).first,
                  let stage = handle.superview else { throw KannaError("not_found", "no divider after \(pane)") }
            let to = try screen(stage, handle.position(CGFloat(args["at"]?.number ?? 0.5)))
            scriptedDrag = [to, to]
            handle.drag(in: stage)
            redraw(controller)
            return [:]
        }
        let to: NSPoint
        let target = args["to"] ?? [:]
        if let reference = target["pane"]?.string {
            guard let view = panes[try resolve(reference)], view.window != nil else { throw KannaError("not_found", "no pane \(reference) on screen") }
            let b = view.bounds
            to = try screen(view, NSPoint(x: b.width * CGFloat(target["x"]?.number ?? 0.5), y: b.height * (1 - CGFloat(target["y"]?.number ?? 0.5))))
        } else if let tab = target["tab"]?.string ?? target["plus"]?.string {
            let plus = target["plus"] != nil
            guard let controller = controllers.values.first(where: { plus ? $0.id == tab : $0.tabBar.center(of: tab) != nil }) else {
                throw KannaError("not_found", "no tab bar for \(tab)")
            }
            controller.window.orderFront(nil)
            let point = plus ? controller.tabBar.plusCenter : controller.tabBar.convert(controller.tabBar.center(of: tab)!, from: nil)
            to = try screen(controller.tabBar, point)
        } else {
            let screenFrame = NSScreen.main?.visibleFrame ?? .zero
            let corners = [NSPoint(x: screenFrame.maxX - 30, y: screenFrame.minY + 30), NSPoint(x: screenFrame.minX + 30, y: screenFrame.minY + 30),
                           NSPoint(x: screenFrame.maxX - 30, y: screenFrame.maxY - 30), NSPoint(x: screenFrame.minX + 30, y: screenFrame.maxY - 30)]
            guard let free = corners.first(where: { p in !controllers.values.contains { $0.window.frame.contains(p) } }) else {
                throw KannaError("bad_request", "every corner of the screen is covered by a Kanna window")
            }
            to = free
        }
        scriptedDrag = [to, to]
        let dropped: Bool
        if let reference = args["pane"]?.string {
            dropped = dragPane(try resolve(reference))
        } else if let tab = args["tab"]?.string {
            scriptedCancel = args["cancel"]?.bool == true
            dropped = dragTab(tab)
            scriptedCancel = false
        } else {
            scriptedDrag = nil
            throw KannaError("bad_request", "debug.drag needs pane, tab or divider")
        }
        scriptedDrag = nil
        let under = NSWindow.windowNumber(at: to, belowWindowWithWindowNumber: 0)
        let owner = (CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(under)) as? [[String: Any]])?.first?[kCGWindowOwnerName as String] as? String
        let window = controllers.values.first { $0.window.windowNumber == under }?.id
        return ["dropped": .bool(dropped), "at": [.number(to.x), .number(to.y)], "under": .string(window ?? owner ?? "\(under)")]
    }

    func resolve(_ reference: String) throws -> String {
        if panes[reference] != nil { return reference }
        guard let id = paneID(named: reference) else { throw KannaError("not_found", "no pane \"\(reference)\"") }
        return id
    }
}

struct WindowShape {
    let size: NSSize
    let fills: Bool

    @MainActor init(_ window: NSWindow) {
        size = window.frame.size
        let area = window.screen?.visibleFrame ?? .null
        let edges = [window.frame.minX - area.minX, window.frame.minY - area.minY, area.maxX - window.frame.maxX, area.maxY - window.frame.maxY]
        fills = window.styleMask.contains(.fullScreen) || window.isZoomed || (!area.isNull && edges.allSatisfy { abs($0) <= 4 })
    }
}
