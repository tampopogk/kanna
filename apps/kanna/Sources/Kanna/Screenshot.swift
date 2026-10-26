import AppKit

extension AppController {
    static let largestShot = 11 << 20

    func installScreenshot() {
        commands["screenshot"] = { [weak self] args in try await self?.screenshot(args) ?? [:] }
    }

    func shownPaneViews(_ window: JSON) -> [(id: String, view: PaneView)] {
        WindowController.shownPanes(window).compactMap { pane in
            guard let view = panes[pane], view.window != nil, !view.isHiddenOrHasHiddenAncestor, view.superview != nil else { return nil }
            return (pane, view)
        }
    }

    private func screenshot(_ args: JSON) async throws -> [String: JSON] {
        let pane = args["pane"]?.string
        let (id, shot) = try screenshotTarget(pane: pane, window: args["window"]?.string, from: args["from"]?.string)
        guard let controller = controllers[id], let window = window(id) else { throw KannaError("not_found", "no window \"\(id)\"") }
        let shown = shownPaneViews(window)
        var image = try await Snapshot.capture(controller.window, terminals: shown.compactMap(\.view.terminal))
        if let shot {
            guard let view = shown.first(where: { $0.id == shot })?.view else { throw KannaError("not_shown", "pane \(shot) isn't showing") }
            guard let cropped = Snapshot.crop(image, to: view, in: controller.window) else { throw KannaError("unavailable", "pane \(shot) has no area to capture") }
            image = cropped
        }
        let png = try Snapshot.png(image)
        guard png.count <= Self.largestShot else { throw KannaError("too_big", "the picture is over \(Self.largestShot >> 20) MiB") }
        var reply: [String: JSON] = ["png": .string(png.base64EncodedString()), "window": .string(id), "width": .number(Double(image.width)), "height": .number(Double(image.height))]
        if let shot { reply["pane"] = .string(shot) }
        return reply
    }

    private func screenshotTarget(pane: String?, window: String?, from: String?) throws -> (window: String, pane: String?) {
        if let pane {
            if let shown = windows.first(where: { shownPaneViews($0).contains { $0.id == pane } }), let id = shown["id"]?.string { return (id, pane) }
            if case .array(let tabs)? = windowOf(pane: pane)?["tabs"],
               let tab = tabs.first(where: { if case .array(let ids)? = $0["panes"] { ids.contains(.string(pane)) } else { false } })?["id"]?.string {
                throw KannaError("not_shown", "pane \(pane) is in tab \(tab), which isn't showing")
            }
            throw KannaError("not_shown", "pane \(pane) isn't in a window")
        }
        if let window {
            guard self.window(window) != nil else { throw KannaError("not_found", "no window \"\(window)\"") }
            return (window, nil)
        }
        if let from, let id = (windows.first { shownPaneViews($0).contains { $0.id == from } } ?? windowOf(pane: from))?["id"]?.string {
            return (id, nil)
        }
        guard let id = keyWindow?["id"]?.string else { throw KannaError("not_found", "no window to capture") }
        return (id, nil)
    }
}
