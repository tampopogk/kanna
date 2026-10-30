import AppKit

extension AppController {
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
        let url = try screenshotURL(args, name: shot ?? id)
        try Snapshot.write(Snapshot.png(image), to: url, replacing: args["force"]?.bool == true)
        var reply: [String: JSON] = ["path": .string(url.path), "window": .string(id), "width": .number(Double(image.width)), "height": .number(Double(image.height))]
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
        guard let from else { throw KannaError("bad_request", "screenshot needs a pane, a window, or the terminal it's run in") }
        guard let id = (windows.first { shownPaneViews($0).contains { $0.id == from } } ?? windowOf(pane: from))?["id"]?.string else {
            throw KannaError("not_shown", "\(from) isn't in a window that's showing: say PANE or --window WINDOW")
        }
        return (id, nil)
    }

    private func screenshotURL(_ args: JSON, name: String) throws -> URL {
        if let path = args["path"]?.string {
            guard path.hasPrefix("/") else { throw KannaError("bad_request", "path must be absolute") }
            return URL(fileURLWithPath: path, isDirectory: false)
        }
        let dir = URL(fileURLWithPath: args["dir"]?.string ?? NSTemporaryDirectory(), isDirectory: true)
        let stamp = Date().formatted(.verbatim("\(year: .defaultDigits)\(month: .twoDigits)\(day: .twoDigits)-\(hour: .twoDigits(clock: .twentyFourHour, hourCycle: .zeroBased))\(minute: .twoDigits)\(second: .twoDigits)", timeZone: .current, calendar: .current))
        let base = "kanna-\(name)-\(stamp)"
        let taken = { FileManager.default.fileExists(atPath: dir.appendingPathComponent($0).path) }
        let file = (["\(base).png"] + (2...99).map { "\(base)-\($0).png" }).first { !taken($0) } ?? "\(base)-\(UUID().uuidString.prefix(8)).png"
        return dir.appendingPathComponent(file, isDirectory: false)
    }
}
