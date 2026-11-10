import AppKit
import GhosttyKit
import WebKit

@MainActor
final class Offstage {
    let window: NSWindow
    let stage: NSView

    init(size: NSSize, appearance: NSAppearance?) {
        window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.isExcludedFromWindowsMenu = true
        window.ignoresMouseEvents = true
        window.hasShadow = false
        window.animationBehavior = .none
        window.collectionBehavior = [.transient, .ignoresCycle, .stationary]
        window.appearance = appearance
        stage = NSView(frame: NSRect(origin: .zero, size: size))
        stage.wantsLayer = true
        window.contentView = stage
        let screens = NSScreen.screens.map(\.frame).reduce(NSRect.null) { $0.union($1) }
        let corner = screens.isNull ? NSPoint.zero : NSPoint(x: screens.minX, y: screens.minY)
        window.setFrameOrigin(NSPoint(x: corner.x - size.width - 2000, y: corner.y - size.height - 2000))
        window.orderBack(nil)
    }

    func close() {
        window.orderOut(nil)
        window.close()
    }
}

extension AppController {
    static let newPaneSize = NSSize(width: 960, height: 600 - TabBar.height)

    func shownHere(_ pane: String) -> (window: String, view: PaneView)? {
        for window in windows {
            guard let id = window["id"]?.string, controllers[id]?.window.isMiniaturized == false,
                  let view = shownPaneViews(window).first(where: { $0.id == pane })?.view else { continue }
            return (id, view)
        }
        return nil
    }

    func hiddenShot(_ pane: String, _ args: JSON) async throws -> [String: JSON] {
        let workspace = (published["workspaces"]?.array ?? []).first { ws in
            ws["dock"]?["pane"]?.string == pane || (ws["panes"]?.array ?? []).contains(.string(pane))
        }
        guard workspace != nil || windowOf(pane: pane) != nil else { throw KannaError("not_found", "pane \(pane) isn't in a window or a task") }
        if let id = workspace?["id"]?.string, let entry = args["stopped"]?.array?.first(where: { $0["workspace"]?.string == id }) {
            let task = entry["task"]?.string ?? id
            let name = entry["name"]?.string.map { " \"\($0)\"" } ?? ""
            throw KannaError("stopped", "pane \(pane) is in \(task)\(name), which is stopped on the Shelf: nothing is running to draw (kanna task start \(task) runs it)")
        }
        let window = windowOf(pane: pane)?["id"]?.string
        let appearance = window.flatMap { controllers[$0]?.window.effectiveAppearance } ?? NSApp.effectiveAppearance
        let given = args["size"].flatMap { size in size["width"]?.number.flatMap { w in size["height"]?.number.map { NSSize(width: w, height: $0) } } }
        let image: CGImage
        if let terminal = panes.terminalOf?(pane) {
            image = try await shootTerminal(pane, terminal, size: given, appearance: appearance)
        } else if contents.shows(pane) {
            image = try await shootContent(pane, size: given ?? lastSize(pane) ?? Self.newPaneSize, appearance: appearance)
        } else {
            throw KannaError("unavailable", "pane \(pane) hasn't started: nothing is drawn yet")
        }
        let png = try Snapshot.png(image)
        guard png.count <= Self.largestShot else { throw KannaError("too_big", "the picture is over \(Self.largestShot >> 20) MiB") }
        var reply: [String: JSON] = ["png": .string(png.base64EncodedString()), "pane": .string(pane), "hidden": true,
                                     "width": .number(Double(image.width)), "height": .number(Double(image.height))]
        if let window { reply["window"] = .string(window) }
        return reply
    }

    func drawnOffstage(_ pane: String, size: NSSize, appearance: NSAppearance) async -> CGImage? {
        if let terminal = panes.terminalOf?(pane) { return try? await shootTerminal(pane, terminal, size: size, appearance: appearance) }
        guard contents.shows(pane) else { return nil }
        return try? await shootContent(pane, size: size, appearance: appearance)
    }

    private func lastSize(_ pane: String) -> NSSize? {
        guard let size = panes[pane]?.frame.size, size.width >= 1, size.height >= 1 else { return nil }
        return size
    }

    private func picture(_ pane: String, _ terminal: String) async throws -> JSON {
        guard let link = links?.link(for: pane) else { throw KannaError("no_daemon", "Kanna.app is not attached to its daemon") }
        let reply: JSON = await withCheckedContinuation { done in
            link.request("terminal.picture", ["terminal": .string(terminal)]) { done.resume(returning: $0) }
        }
        guard reply["ok"]?.bool == true else {
            let code = reply["error"]?["code"]?.string ?? "failed"
            if code == "stopped" { throw KannaError("stopped", "pane \(pane) isn't running: nothing is drawn to shoot") }
            throw KannaError(code, reply["error"]?["message"]?.string ?? "can't read \(pane)'s screen")
        }
        return reply
    }

    private func shootTerminal(_ pane: String, _ terminal: String, size given: NSSize?, appearance: NSAppearance) async throws -> CGImage {
        let screen = try await picture(pane, terminal)
        guard let data = screen["data"]?.string.flatMap({ Data(base64Encoded: $0) }) else { throw KannaError("unavailable", "\(pane)'s screen came back empty") }
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        let pixels = (screen["width"]?.number ?? 0, screen["height"]?.number ?? 0)
        let onScreen = pixels.0 >= 1 && pixels.1 >= 1 ? NSSize(width: pixels.0 / scale, height: pixels.1 / scale) : nil
        let size = given ?? lastSize(pane) ?? onScreen ?? Self.newPaneSize
        guard let runtime = GhosttyRuntime.shared else { throw KannaError("unavailable", "the terminal renderer isn't running") }
        let offstage = Offstage(size: size, appearance: appearance)
        defer { offstage.close() }
        let view = TerminalSurfaceView(frame: offstage.stage.bounds)
        offstage.stage.addSubview(view)
        var options = GhosttyRuntime.options(for: view)
        options.io_mode = GHOSTTY_IO_MODE_EXTERNAL
        options.io_write_cb = { _, _, _ in }
        options.io_resize_request_cb = { _, _, _, _, _ in }
        guard runtime.attach(view, options: &options), let surface = view.surface else { throw KannaError("unavailable", "the terminal surface could not be created") }
        defer { runtime.detach(view) }
        let deadline = ContinuousClock.now + .seconds(1)
        while !data.withUnsafeBytes({ ghostty_surface_load_snapshot(surface, $0.bindMemory(to: UInt8.self).baseAddress, data.count) }) {
            guard ContinuousClock.now < deadline else { throw KannaError("unavailable", "\(pane)'s screen couldn't be drawn") }
            try? await Task.sleep(for: .milliseconds(2))
        }
        if given == nil, let cols = screen["cols"]?.number.flatMap({ UInt16(exactly: $0) }), let rows = screen["rows"]?.number.flatMap({ UInt16(exactly: $0) }) {
            _ = ghostty_surface_set_grid_size(surface, cols, rows)
        }
        view.showToGhostty()
        let drawn = ContinuousClock.now + .seconds(2)
        var frames = 0
        var last = view.presented
        while frames < 3, ContinuousClock.now < drawn {
            ghostty_surface_refresh(surface)
            try? await Task.sleep(for: .milliseconds(5))
            if view.presented != last {
                last = view.presented
                frames += 1
            }
        }
        return try await Snapshot.capture(offstage.window, terminals: [])
    }

    private func shootContent(_ pane: String, size: NSSize, appearance: NSAppearance) async throws -> CGImage {
        let slot = contents.slot(pane)
        let parent = slot.superview
        let frame = slot.frame
        let owner = slot.window
        let responder = owner?.firstResponder
        let held = (responder as? NSView)?.isDescendant(of: slot) == true
        let offstage = Offstage(size: size, appearance: appearance)
        defer {
            if slot.window === offstage.window {
                slot.removeFromSuperview()
                if let parent = parent as? PaneView, parent.content === slot {
                    parent.addSubview(slot, positioned: .below, relativeTo: parent.subviews.first)
                    slot.frame = parent.bounds
                } else if let parent {
                    parent.addSubview(slot)
                    slot.frame = frame
                }
                if held, let owner, let responder, slot.window === owner { owner.makeFirstResponder(responder) }
            }
            offstage.close()
        }
        offstage.stage.addSubview(slot)
        slot.frame = offstage.stage.bounds
        offstage.stage.layoutSubtreeIfNeeded()
        offstage.window.displayIfNeeded()
        if let web = contents.web(pane)?.webView {
            _ = try? await web.takeSnapshot(configuration: nil)
        }
        try? await Task.sleep(for: .milliseconds(50))
        slot.frame = offstage.stage.bounds
        offstage.stage.layoutSubtreeIfNeeded()
        offstage.window.displayIfNeeded()
        return try await Snapshot.capture(offstage.window, terminals: [])
    }
}
