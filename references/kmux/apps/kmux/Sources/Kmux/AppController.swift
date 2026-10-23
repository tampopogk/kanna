import AppKit
import GhosttyKit
import KmuxCore
import KmuxDiagram
import KmuxMarkdown
import UniformTypeIdentifiers

/// Owns the model, the control socket and one WindowController per window,
/// and keeps the windows in step with the model.
@MainActor
final class AppController: NSObject, NSApplicationDelegate {
    private let core = Core()
    private var instance = Instance(name: Instance.defaultName)
    private var host: ContentHost!
    private var server: SocketServer?
    private var signalSources: [DispatchSourceSignal] = []
    private var paneMenu: NSMenu?
    private var workspaceMenu: NSMenu?
    private var testingDrag = false
    /// Started with `--bg` (or `KMUX_BG=1`, `KANNA_BG=1` in Kanna), e.g. by tests: don't take over the screen.
    private let background = CommandLine.arguments.contains("--bg") || ProcessInfo.processInfo.environment[Brand.current.variable("BG")] == "1"
    private var controllers: [String: WindowController] = [:]
    /// Where the layout is saved (docs/kmux-spec.md §3.6); nil when this kmux
    /// doesn't keep one (`KMUX_NO_STATE=1`, or `KMUX_NO_INITIAL_WINDOW` for benchmarks).
    private var stateFile: StateFile?
    /// Where the app keeps what is only its own: where each window is (also
    /// while its workspace is hidden) and markdown panes' zoom.
    private var appFile: StateFile?
    private var frames: [String: NSRect] = [:]
    private var saveTimer: Timer?
    private var lastSaved: Data?
    private var lastSavedApp: Data?

    func applicationDidFinishLaunching(_ notification: Notification) {
        do {
            host = ContentHost(runtime: try GhosttyRuntime())
        } catch {
            fatal("kmux: \(error.localizedDescription)")
        }
        do {
            instance = try Instance.current()
        } catch let error as KmuxError {
            fatal("kmux: \(error.message)")
        } catch {
            fatal("kmux: \(error)")
        }
        core.instance = instance
        host.core = core
        host.instance = instance
        host.onFocus = { [weak self] id in self?.focused(id) }
        host.onNavigate = { [weak self] id, url in self?.request(["cmd": "navigate", "args": ["pane": .string(id), "url": .string(url)]]) }
        host.onOpenMarkdown = { [weak self] id, path in self?.request(["cmd": "navigate", "args": ["pane": .string(id), "path": .string(path)]]) }
        host.onMarkdownHistory = { [weak self] id, back in
            guard let history = self?.core.model.panes[id]?.history, !(back ? history.back : history.forward).isEmpty else { return }
            self?.request(["cmd": "navigate", "args": ["pane": .string(id), back ? "back" : "forward": true]])
        }
        host.onPaneDrag = { [weak self] id, _ in self?.dragPane(id) }
        host.contextMenu = { [weak self] in self?.paneMenu?.copy() as? NSMenu }
        host.onCloseRequest = { [weak self] id in
            guard let self, core.model.dockWindow(id) == nil else { return }
            request(["cmd": "close", "args": ["pane": .string(id)]])
        }
        core.host = host
        core.onChange = { [weak self] in
            self?.sync()
            self?.scheduleSave()
        }
        host.onPaneChange = { [weak self] in self?.scheduleSave() }
        core.paneIsWide = { [weak self] id in
            guard let bounds = self?.host.views[id]?.bounds, bounds.height > 0 else { return true }
            return bounds.width >= bounds.height
        }
        core.extraCommands["debug.snapshot"] = { [weak self] args in try self?.snapshot(args) ?? [:] }
        core.extraCommands["debug.key"] = { [weak self] args in try self?.pressKey(args) ?? [:] }
        core.extraCommands["debug.click"] = { [weak self] args in try self?.click(args) ?? [:] }
        // Lays out Mermaid `source` natively (KmuxDiagram); writes a PNG to `png` if given.
        core.extraCommands["debug.diagram"] = { args in
            let start = Date()
            let layout: DiagramLayout
            do { layout = try Diagram.layout(source: args["source"]?.string ?? "") } catch let error as DiagramError {
                throw KmuxError("bad_request", "Diagram error: \(error.message)")
            }
            let ms = Date().timeIntervalSince(start) * 1000
            guard let scene = layout.scene else { return ["type": .string(layout.type), "supported": false] }
            if let path = args["png"]?.string { try scene.png(dark: args["dark"]?.bool == true)?.write(to: URL(fileURLWithPath: path)) }
            return ["type": .string(layout.type), "supported": true, "width": .number(Double(scene.size.width)),
                    "height": .number(Double(scene.size.height)), "labels": .array(scene.texts.map { .string($0) }), "layout_ms": .number(ms)]
        }
        core.extraCommands["debug.text"] = { [weak self] args in
            guard let self, let terminal = try host.terminal(core.needPane(args["pane"]?.string).id) else { throw KmuxError("wrong_type", "not a terminal pane") }
            return ["text": .string(terminal.viewportText())]
        }
        core.extraCommands["debug.stats"] = { [weak self] _ in
            let visible = (self?.controllers ?? [:]).filter { $0.value.window.isVisible }.keys.sorted()
            return ["pid": .number(Double(ProcessInfo.processInfo.processIdentifier)), "footprint": .number(Double(memoryFootprint())),
                    "visible": .array(visible.map(JSON.string))]
        }
        // iOS panes: tap at {x, y} (fractions of the screen) or press Home; replies with the screen's pixel size.
        core.extraCommands["debug.ios"] = { [weak self] args in
            guard let self, let screen = try host.ios(core.needPane(args["pane"]?.string).id)?.screen else { throw KmuxError("wrong_type", "not a running ios pane") }
            if let x = args["x"]?.number, let y = args["y"]?.number {
                screen.touch(.leftMouseDown, at: CGPoint(x: x, y: y))
                try await Task.sleep(for: .milliseconds(80))
                screen.touch(.leftMouseUp, at: CGPoint(x: x, y: y))
            }
            if args["home"]?.bool == true { screen.pressHome() }
            return ["width": .number(screen.pixelSize.width), "height": .number(screen.pixelSize.height)]
        }
        core.extraCommands["debug.drag"] = { [weak self] args in try await self?.debugDrag(args) ?? [:] }
        core.extraCommands["debug.web"] = { [weak self] args in
            guard let self else { return [:] }
            let id = try core.needPane(args["pane"]?.string).id
            guard let web = host.web(id) else { throw KmuxError("wrong_type", "not a web pane") }
            let text = try? await web.webView.evaluateJavaScript("document.body ? document.body.innerText : ''") as? String
            return ["url": web.webView.url.map { .string($0.absoluteString) } ?? nil, "text": .string(text ?? "")]
        }
        // A markdown pane as rendered: its text (diagrams as [diagram]), zoom and drawn diagrams.
        core.extraCommands["debug.md"] = { [weak self] args in
            guard let self, let markdown = try host.markdown(core.needPane(args["pane"]?.string).id) else { throw KmuxError("wrong_type", "not a running md pane") }
            if let y = args["scroll"]?.number { markdown.scroll(toY: CGFloat(y)) }
            let diagrams: [JSON] = markdown.drawnDiagrams.map { ["type": .string($0.type), "labels": .array($0.labels.map { .string($0) })] }
            return ["text": .string(markdown.text), "zoom": .number(Double(markdown.zoom)), "diagrams": .array(diagrams),
                    "render_ms": .number(markdown.renderMs), "parse_ms": .number(markdown.parseMs), "scroll": .number(Double(markdown.scrollTop)),
                    "layout_width": .number(Double(markdown.layoutWidth))]
        }
        // A trackpad pinch over the middle of `pane`, sent through its window as
        // magnify events (`steps`: each event's magnification). Replies with the
        // zoom after each step. `outline: true` shows the pane's focus outline
        // first, as a key window would (a --bg instance is never key).
        core.extraCommands["debug.pinch"] = { [weak self] args in
            guard let self, let id = try Optional(core.needPane(args["pane"]?.string).id), let view = host.keyView(id),
                  let window = view.window else { throw KmuxError("wrong_type", "pane is not on screen") }
            if args["outline"]?.bool == true { host.views[id]?.focused = true }
            var steps = [0.1, 0.1, 0.1, 0.1, 0.1]
            if case .array(let list)? = args["steps"] { steps = list.compactMap(\.number) }
            let point = view.convert(NSPoint(x: view.visibleRect.midX, y: view.visibleRect.midY), to: nil)
            let height = NSScreen.screens.first?.frame.height ?? 0
            var zooms: [JSON] = [], renders: [JSON] = []
            let phases = [(1, 0.0)] + steps.map { (2, $0) } + [(4, 0.0)]
            for (phase, value) in phases {
                guard let cg = CGEvent(source: nil) else { continue }
                cg.type = CGEventType(rawValue: 29)! // gesture
                cg.location = CGPoint(x: point.x, y: height - point.y)
                cg.setIntegerValueField(CGEventField(rawValue: 110)!, value: 8) // zoom
                cg.setDoubleValueField(CGEventField(rawValue: 113)!, value: value)
                cg.setIntegerValueField(CGEventField(rawValue: 132)!, value: Int64(phase))
                guard let event = NSEvent(cgEvent: cg) else { continue }
                window.sendEvent(event)
                try await Task.sleep(for: .milliseconds(40))
                if phase == 2, let markdown = host.markdown(id) {
                    zooms.append(.number(Double(markdown.zoom)))
                    renders.append(.number(markdown.renderMs))
                }
            }
            return ["zooms": .array(zooms), "render_ms": .array(renders)]
        }
        core.extraCommands["debug.menu"] = { [weak self] _ in
            guard let self else { return [:] }
            let items = (NSApp.mainMenu?.items ?? []).flatMap { top in
                (top.submenu?.items ?? []).filter { !$0.isSeparatorItem }.map { item -> JSON in
                    let mods = item.keyEquivalentModifierMask
                    let shortcut = (mods.contains(.control) ? "⌃" : "") + (mods.contains(.option) ? "⌥" : "") + (mods.contains(.shift) ? "⇧" : "")
                        + (mods.contains(.command) ? "⌘" : "") + item.keyEquivalent.replacingOccurrences(of: "\r", with: "↩")
                    return ["menu": .string(top.submenu?.title ?? ""), "item": .string(item.title), "shortcut": .string(item.keyEquivalent.isEmpty ? "" : shortcut),
                            "enabled": .bool(item.target === self ? validateMenuItem(item) : true)]
                }
            }
            return ["items": .array(items)]
        }
        host.runtime.onMuxAction = { [weak self] view, action in
            guard let self, let id = host.paneID(of: view) else { return false }
            return ghosttyAction(id, action)
        }

        let server = SocketServer(path: instance.socketPath) { [weak self] request in await self?.core.handle(request) ?? nil }
        do {
            try server.start()
            self.server = server
        } catch let error as KmuxError {
            fatal("kmux: \(error.message)")
        } catch {
            fatal("kmux: \(error)")
        }
        // Quit cleanly (removing the socket) when killed, not only on ⌘Q.
        for signal in [SIGTERM, SIGINT, SIGHUP] {
            Darwin.signal(signal, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: signal, queue: .main)
            source.setEventHandler { NSApp.terminate(nil) }
            source.resume()
            signalSources.append(source)
        }
        installMenu()
        if !background { NSApp.activate() }
        let environment = ProcessInfo.processInfo.environment
        if environment["KMUX_NO_INITIAL_WINDOW"] == nil {
            // KMUX_NO_STATE=1: neither restore nor save a layout (tests on shared sockets).
            if environment["KMUX_NO_STATE"] != "1" {
                stateFile = StateFile(path: StateFile.path(for: instance))
                appFile = StateFile(path: StateFile.appPath(for: instance))
            }
            // `--fresh` (or KMUX_FRESH=1) starts with a new window; the saved layout is replaced at the next save.
            let fresh = CommandLine.arguments.contains("--fresh") || environment[Brand.current.variable("FRESH")] == "1"
            if fresh || !restoreLayout() { newWindow() }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        saveTimer?.invalidate()
        saveLayout()
        server?.stop()
    }

    // MARK: Persistence (docs/kmux-spec.md §3.6)

    /// Restores the saved layout, if there is one kmux can trust and read.
    /// Anything else is moved aside (…json.bad) and kmux starts fresh: a bad
    /// file never stops kmux from starting.
    private func restoreLayout() -> Bool {
        guard let file = stateFile else { return false }
        switch file.load() {
        case .none:
            return false
        case .refused(let why):
            NSLog("kmux: not restoring the saved layout: \(why); moved to \(file.setAside() ?? "nowhere")")
            return false
        case .state(let state):
            restoreAppState(Core.upgrade(state))
            host.restoring = true
            defer { host.restoring = false }
            do {
                try core.restoreState(state)
            } catch {
                NSLog("kmux: not restoring the saved layout: \((error as? KmuxError)?.message ?? "\(error)"); moved to \(file.setAside() ?? "nowhere")")
                return false
            }
            return !core.model.windows.isEmpty
        }
    }

    /// Reads the app's own file: `{ kmux: "app", version: 1, frames: { win-N: { x, y, w, h } },
    /// zoom: { file-N: zoom } }`. Before there was one, the layout's own
    /// state kept them (version 1), so they are taken from there.
    private func restoreAppState(_ layout: JSON) {
        var saved: [String: JSON] = [:]
        switch appFile?.load() {
        case .state(let state) where state["kmux"] == "app" && state["version"] == 1:
            if case .object(let fields) = state { saved = fields }
        case .refused(let why):
            NSLog("kmux: not restoring where windows were: \(why); moved to \(appFile?.setAside() ?? "nowhere")")
        default:
            if case .array(let windows)? = layout["windows"] {
                saved["frames"] = .object(Dictionary(windows.compactMap { w in w["id"]?.string.flatMap { id in w["frame"].map { (id, $0) } } }) { $1 })
            }
            if case .array(let panes)? = layout["panes"] {
                saved["zoom"] = .object(Dictionary(panes.compactMap { p in p["id"]?.string.flatMap { id in p["zoom"].map { (id, $0) } } }) { $1 })
            }
        }
        if case .object(let known)? = saved["frames"] {
            for (id, f) in known {
                guard let x = f["x"]?.number, let y = f["y"]?.number, let w = f["w"]?.number, let h = f["h"]?.number,
                      [x, y, w, h].allSatisfy(\.isFinite), w > 0, h > 0 else { continue }
                frames[id] = NSRect(x: x, y: y, width: w, height: h)
            }
        }
        if case .object(let known)? = saved["zoom"] {
            for (id, zoom) in known { if let zoom = zoom.number, zoom > 0, zoom.isFinite { host.markdownZoom[id] = CGFloat(zoom) } }
        }
    }

    /// Saves the layout half a second after a change (later changes ride
    /// along; a save that would change nothing writes nothing).
    private func scheduleSave() {
        guard stateFile != nil, saveTimer?.isValid != true else { return }
        saveTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.saveLayout() }
        }
    }

    private func saveLayout() {
        guard let file = stateFile else { return }
        let state = core.exportState()
        let data = state.encoded()
        if data != lastSaved {
            do {
                try file.write(state)
                lastSaved = data
            } catch {
                NSLog("kmux: could not save the layout: \((error as? KmuxError)?.message ?? "\(error)")")
            }
        }
        guard let appFile else { return }
        for (id, controller) in controllers { frames[id] = controller.window.frame }
        frames = frames.filter { core.model.window($0.key) != nil }
        var zoom: [String: JSON] = [:]
        for pane in core.model.panes.values where pane.type == .md {
            if let value = host.markdown(pane.id)?.zoom ?? host.markdownZoom[pane.id], value != 1 { zoom[pane.id] = .number(Double(value)) }
        }
        let app: JSON = [
            "kmux": "app", "version": 1, "zoom": .object(zoom),
            "frames": .object(frames.mapValues { ["x": .number($0.minX), "y": .number($0.minY), "w": .number($0.width), "h": .number($0.height)] }),
        ]
        let appData = app.encoded()
        guard appData != lastSavedApp else { return }
        do {
            try appFile.write(app)
            lastSavedApp = appData
        } catch {
            NSLog("kmux: could not save where windows are: \((error as? KmuxError)?.message ?? "\(error)")")
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if controllers.isEmpty { newWindow() }
        return true
    }

    // MARK: Model → windows

    /// A hidden workspace's windows close, their panes still running, and
    /// come back where they were when it's shown.
    private func sync() {
        let ids = Set(core.model.shownWindows.map(\.id))
        for (id, controller) in controllers where !ids.contains(id) {
            frames[id] = controller.window.frame
            controller.window.orderOut(nil)
            controllers[id] = nil
        }
        for state in core.model.shownWindows {
            let controller = controllers[state.id] ?? makeController(state.id)
            controller.render(core.model, host)
        }
        if let key = core.model.key, let controller = controllers[key], !controller.window.isKeyWindow {
            present(controller.window)
        }
    }

    private func makeController(_ id: String) -> WindowController {
        let previous = controllers.values.map(\.window).max { $0.orderedIndex > $1.orderedIndex }
        let controller = WindowController(id: id, instance: instance, cascadeFrom: previous)
        // A restored window goes back where it was, if that is still on a screen.
        if let frame = frames[id] {
            if NSScreen.screens.contains(where: { $0.visibleFrame.intersects(frame) }) { controller.window.setFrame(frame, display: false) }
        }
        controller.onFrameChange = { [weak self] in self?.scheduleSave() }
        controller.relayout = { [weak self, weak controller] in
            guard let self, let controller else { return }
            controller.render(core.model, host)
        }
        controller.onResize = { [weak self, weak controller] args in
            guard let self else { return }
            request(["cmd": "resize", "args": args]) { _ in
                controller?.preview = nil
                controller?.render(self.core.model, self.host)
            }
        }
        controller.tabBar.onSelect = { [weak self] tab in self?.request(["cmd": "focus", "args": ["tab": .string(tab)]]) }
        controller.tabBar.onClose = { [weak self] tab in self?.request(["cmd": "close", "args": ["tab": .string(tab)]]) }
        controller.tabBar.onNew = { [weak self] in self?.newTab(in: id) }
        controller.tabBar.onDrag = { [weak self] tab in self?.dragTab(tab) }
        controller.tabBar.onMoveToNewWindow = { [weak self] tab in self?.request(["cmd": "move-tab", "args": ["tab": .string(tab), "window": "new"]]) }
        controller.tabBar.onEditEnded = { [weak self, weak controller] in
            guard let self, let controller else { return }
            controller.render(core.model, host)
        }
        controller.tabBar.onRename = { [weak self] tab, title in
            self?.request(["cmd": "rename-tab", "args": ["tab": .string(tab), "title": .string(title)]])
        }
        controller.onCloseRequest = { [weak self] in self?.request(["cmd": "close", "args": ["window": .string(id)]]) }
        controller.onBecomeKey = { [weak self, weak controller] in
            guard let self, let controller else { return }
            if controller.window.isKeyWindow, core.model.key != id { request(["cmd": "focus", "args": ["window": .string(id)]]) }
            controller.render(core.model, host)
        }
        controllers[id] = controller
        present(controller.window)
        return controller
    }

    private func focused(_ paneID: String) {
        guard let window = core.model.windows.first(where: { $0.shownPanes.contains(paneID) }) else { return }
        guard window.focused != paneID else { return }
        request(["cmd": "focus", "args": ["pane": .string(paneID)]])
    }

    // MARK: Actions

    /// Every change the app makes goes to the core as a request, as a
    /// client's would (docs/presentation-layers.md).
    private func request(_ request: JSON, then done: ((JSON) -> Void)? = nil) {
        Task { @MainActor in
            let reply = await core.handle(request)
            if reply["ok"] != true { NSLog("kmux: \(String(decoding: reply.encoded(), as: UTF8.self))") }
            done?(reply)
        }
    }

    // MARK: Shortcuts (docs/kmux-spec.md §5). Each acts on the key window.

    private var keyWindow: Window? { core.model.keyWindow }
    private var focusedPane: String? { keyWindow?.focused }

    @objc func splitRight() { split("right") }
    @objc func splitDown() { split("down") }
    @objc func webRight() { split("right", web: true) }
    @objc func webDown() { split("down", web: true) }
    /// As in the model, a new iOS pane starts with an app already there: Settings.
    @objc func iosRight() { split("right", ios: true) }
    @objc func iosDown() { split("down", ios: true) }
    @objc func markdownRight() { openMarkdown("right") }
    @objc func markdownDown() { openMarkdown("down") }

    /// Asks for a markdown file, then opens it beside the focused pane.
    private func openMarkdown(_ direction: String) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = ["md", "markdown", "mdown"].compactMap { UTType(filenameExtension: $0) }
        panel.allowsOtherFileTypes = true
        panel.message = "Choose a markdown file to show"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        var args: [String: JSON] = ["type": "md", "path": .string(url.path), "split": .string(direction)]
        if let window = keyWindow?.id { args["window"] = .string(window) }
        request(["cmd": "open", "args": .object(args)])
    }

    /// The Home button of the focused iOS pane's simulator.
    @objc func pressHome() {
        focusedPane.flatMap { host.ios($0)?.screen }?.pressHome()
    }

    /// ⌘L: edit the focused web pane's URL.
    @objc func openURL() {
        guard let pane = focusedPane else { return }
        host.web(pane)?.editURL()
    }

    /// Zoom In / Zoom Out / Actual Size: a markdown pane's zoom, or a
    /// terminal's font size (Ghostty's own actions, on the same keys).
    @objc func zoomIn() { zoomFocused(1, ghostty: "increase_font_size:1") }
    @objc func zoomOut() { zoomFocused(-1, ghostty: "decrease_font_size:1") }
    @objc func actualSize() { zoomFocused(0, ghostty: "reset_font_size") }
    private func zoomFocused(_ step: Int, ghostty action: String) {
        guard let pane = focusedPane else { return }
        if let markdown = host.markdown(pane) {
            markdown.zoom(by: step)
        } else if let surface = host.terminal(pane)?.surface {
            _ = ghostty_surface_binding_action(surface, action, UInt(action.utf8.count))
        }
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        if [#selector(zoomIn), #selector(zoomOut), #selector(actualSize)].contains(item.action) {
            return focusedPane.map { host.terminal($0) != nil || host.markdown($0) != nil } ?? false
        }
        if item.action == #selector(pressHome) { return focusedPane.flatMap { host.ios($0)?.screen } != nil }
        if item.action == #selector(openURL) || item.action == #selector(reloadPane) { return focusedPane.flatMap { host.web($0) } != nil }
        if item.action == #selector(goBack) { return !(focusedPane.flatMap { core.model.panes[$0]?.history?.back.isEmpty } ?? true) }
        if item.action == #selector(goForward) { return !(focusedPane.flatMap { core.model.panes[$0]?.history?.forward.isEmpty } ?? true) }
        if item.action == #selector(closePane) { return focusedPane.map { core.model.dockWindow($0) == nil } ?? false }
        if item.action == #selector(movePaneToNewWindow) { return focusedPane.map { core.model.dockWindow($0)?.dock?.frozen != true } ?? false }
        if item.action == #selector(showTab(_:) as (NSMenuItem) -> Void) { return (keyWindow?.tabs.count ?? 0) >= item.tag }
        if item.action == #selector(freeWindow) { return keyWindow?.workspace != nil }
        if item.action == #selector(assignWindow(_:)) || item.submenu === workspaceMenu { return keyWindow != nil }
        return true
    }

    private func split(_ direction: String, web: Bool = false, ios: Bool = false) {
        var args: [String: JSON] = web ? ["type": "web", "url": "http://localhost:3000", "split": .string(direction)]
            : ios ? ["type": "ios", "app": "com.apple.Preferences", "split": .string(direction), "wait": false]
            : ["type": "term", "split": .string(direction)]
        if let window = keyWindow?.id { args["window"] = .string(window) }
        guard web else {
            request(["cmd": "open", "args": .object(args)])
            return
        }
        // As in the model: a new web pane starts with its URL selected for editing.
        Task { @MainActor in
            let reply = await core.handle(["cmd": "open", "args": .object(args)])
            if let id = reply["pane"]?["id"]?.string { host.web(id)?.editURL() }
        }
    }

    @objc func nextPane() { cyclePane(1) }
    @objc func previousPane() { cyclePane(-1) }
    private func cyclePane(_ step: Int) {
        guard let window = keyWindow, let pane = core.cyclePane(in: window, by: step) else { return }
        request(["cmd": "focus", "args": ["pane": .string(pane)]])
    }

    @objc func toggleZoom() {
        guard let pane = focusedPane else { return }
        request(["cmd": "zoom", "args": ["pane": .string(pane)]])
    }

    /// Back and Forward, for web panes opened with history.
    @objc func goBack() {
        guard let pane = focusedPane else { return }
        request(["cmd": "navigate", "args": ["pane": .string(pane), "back": true]])
    }

    @objc func goForward() {
        guard let pane = focusedPane else { return }
        request(["cmd": "navigate", "args": ["pane": .string(pane), "forward": true]])
    }

    @objc func movePaneToNewWindow() {
        guard let pane = focusedPane else { return }
        request(["cmd": "move", "args": ["pane": .string(pane), "window": "new"]])
    }

    @objc func moveTabToNewWindow() {
        guard let tab = keyWindow?.active else { return }
        request(["cmd": "move-tab", "args": ["tab": .string(tab), "window": "new"]])
    }

    /// ⌘R: the focused web pane's page reloads in place.
    @objc func reloadPane() {
        guard let pane = focusedPane else { return }
        host.web(pane)?.webView.reload()
    }

    @objc func closePane() {
        guard let pane = focusedPane, core.model.dockWindow(pane) == nil else { return }
        request(["cmd": "close", "args": ["pane": .string(pane)]])
    }

    @objc func newTab() { newTab(in: keyWindow?.id) }
    private func newTab(in window: String?) {
        var args: [String: JSON] = ["type": "term", "tab": true]
        if let window { args["window"] = .string(window) }
        request(["cmd": "open", "args": .object(args)])
    }

    @objc func showTab(_ sender: NSMenuItem) { showTab(sender.tag) }
    private func showTab(_ number: Int) {
        guard let window = keyWindow, number >= 1, number <= window.tabs.count else { return }
        request(["cmd": "focus", "args": ["tab": .string(window.tabs[number - 1].id)]])
    }

    @objc func nextTab() { cycleTab(1) }
    @objc func previousTab() { cycleTab(-1) }
    private func cycleTab(_ step: Int) {
        guard let window = keyWindow, let tab = core.cycleTab(in: window, by: step) else { return }
        request(["cmd": "focus", "args": ["tab": .string(tab), "keep": "dock"]])
    }

    /// Starts another kmux, with its own windows and socket, named 2, 3, ….
    @objc func newInstance() {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.createsNewApplicationInstance = true
        configuration.arguments = ["--instance", Instance.unusedName()]
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration) { _, error in
            if let error { NSLog("kmux: could not start a new instance: \(error)") }
        }
    }

    @objc func newWindow() { request(["cmd": "open", "args": ["type": "term", "window": "new"]]) }

    @objc func closeWindow() {
        guard let window = keyWindow?.id else { return }
        request(["cmd": "close", "args": ["window": .string(window)]])
    }

    @objc func assignWindow(_ sender: NSMenuItem) {
        guard let window = keyWindow?.id else { return }
        if let workspace = sender.representedObject as? String {
            return request(["cmd": "window", "args": ["action": "assign", "window": .string(window), "workspace": .string(workspace)]])
        }
        Task { @MainActor in
            let made = await core.handle(["cmd": "workspace", "args": ["action": "new"]])
            guard let workspace = made["workspace"]?["id"]?.string else { return }
            request(["cmd": "window", "args": ["action": "assign", "window": .string(window), "workspace": .string(workspace)]])
        }
    }

    @objc func freeWindow() {
        guard let window = keyWindow?.id else { return }
        request(["cmd": "window", "args": ["action": "free", "window": .string(window)]])
    }

    @objc func nextWindow() { cycleWindow(1) }
    @objc func previousWindow() { cycleWindow(-1) }
    private func cycleWindow(_ step: Int) {
        let windows = core.model.shownWindows
        guard !windows.isEmpty else { return }
        let at = windows.firstIndex { $0.id == core.model.key } ?? 0
        let next = windows[((at + step) % windows.count + windows.count) % windows.count]
        request(["cmd": "focus", "args": ["window": .string(next.id)]])
    }

    /// Mux actions from Ghostty keybindings that no menu item took.
    private func ghosttyAction(_ paneID: String, _ action: ghostty_action_s) -> Bool {
        // The shell reported its working directory (OSC 7): a restored
        // terminal starts there.
        if action.tag == GHOSTTY_ACTION_PWD {
            if let pwd = action.action.pwd.pwd { core.report(paneID, cwd: String(cString: pwd)) }
            return true
        }
        let perform: (() -> Void)?
        switch action.tag {
        case GHOSTTY_ACTION_NEW_SPLIT:
            let direction = action.action.new_split
            perform = direction == GHOSTTY_SPLIT_DIRECTION_DOWN || direction == GHOSTTY_SPLIT_DIRECTION_UP ? splitDown : splitRight
        case GHOSTTY_ACTION_GOTO_SPLIT:
            let target = action.action.goto_split
            perform = target == GHOSTTY_GOTO_SPLIT_PREVIOUS || target == GHOSTTY_GOTO_SPLIT_LEFT || target == GHOSTTY_GOTO_SPLIT_UP ? previousPane : nextPane
        case GHOSTTY_ACTION_TOGGLE_SPLIT_ZOOM: perform = toggleZoom
        case GHOSTTY_ACTION_NEW_TAB: perform = newTab
        case GHOSTTY_ACTION_GOTO_TAB:
            let target = action.action.goto_tab
            let number = Int(target.rawValue)
            perform = target == GHOSTTY_GOTO_TAB_PREVIOUS ? previousTab : target == GHOSTTY_GOTO_TAB_NEXT ? nextTab
                : target == GHOSTTY_GOTO_TAB_LAST ? { [weak self] in self?.showTab(self?.keyWindow?.tabs.count ?? 0) }
                : number >= 1 ? { [weak self] in self?.showTab(number) } : nil
        case GHOSTTY_ACTION_NEW_WINDOW: perform = newWindow
        case GHOSTTY_ACTION_CLOSE_WINDOW: perform = closeWindow
        case GHOSTTY_ACTION_GOTO_WINDOW:
            perform = action.action.goto_window == GHOSTTY_GOTO_WINDOW_PREVIOUS ? previousWindow : nextWindow
        default: perform = nil
        }
        guard let perform else { return false }
        focused(paneID)
        perform()
        return true
    }

    /// Brings a window forward, except in a `--bg` kmux that isn't the app in
    /// use: there the window goes just behind the front window of the app
    /// that is, so tests driving kmux don't interrupt the user.
    private func present(_ window: NSWindow) {
        if NSApp.isActive || !background {
            return window.makeKeyAndOrderFront(nil)
        }
        window.makeKey()
        let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        let front = windows.first { ($0[kCGWindowOwnerPID as String] as? pid_t) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
        if let number = front?[kCGWindowNumber as String] as? Int, pid != ProcessInfo.processInfo.processIdentifier {
            window.order(.below, relativeTo: number)
        } else {
            window.orderFront(nil)
        }
    }

    // MARK: Dragging (reference/kmux/index.html: startPaneDrag, startTabDrag)

    private enum Drop {
        case pane(String, side: String) // dock beside a pane, or swap with it
        case tab(String) // a pane into another tab
        case newTab(window: String) // a pane into a new tab (the + button)
        case index(window: String, Int) // a tab to a place in a tab bar
        case newWindow(NSPoint) // anything, outside every kmux window
    }

    /// The kmux window at `point` (screen coordinates), beneath the drop hint.
    private func controller(at point: NSPoint, below hint: DropHint) -> WindowController? {
        if testingDrag {
            return NSApp.orderedWindows.lazy.compactMap { w in self.controllers.values.first { $0.window === w } }.first { $0.window.frame.contains(point) }
        }
        // The hint ignores the mouse, so the search normally skips it already.
        var number = NSWindow.windowNumber(at: point, belowWindowWithWindowNumber: 0)
        if number == hint.windowNumber { number = NSWindow.windowNumber(at: point, belowWindowWithWindowNumber: number) }
        return controllers.values.first { $0.window.windowNumber == number }
    }

    /// Where a window torn off at `point` goes: its title bar under the mouse.
    private func newWindowFrame(at point: NSPoint, size: NSSize) -> NSRect {
        NSRect(x: point.x - 40, y: point.y - size.height + 12, width: size.width, height: size.height)
    }

    /// What dropping pane `id` at `point` would do, shown by `hint`: onto
    /// another pane's edge docks beside it, its middle swaps; onto a tab moves
    /// it there, onto + into a new tab; outside kmux into a new window. Nothing
    /// drops on a frozen docked pane.
    private func paneDrop(_ id: String, at point: NSPoint, hint: DropHint, size: NSSize) -> Drop? {
        guard let controller = controller(at: point, below: hint) else {
            hint.show(newWindowFrame(at: point, size: size), text: "new window", rounded: true)
            return .newWindow(point)
        }
        let window = controller.window, inWindow = window.convertPoint(fromScreen: point)
        let bar = controller.tabBar, inBar = bar.convert(inWindow, from: nil)
        if bar.bounds.contains(inBar) {
            let home = core.model.windows.flatMap(\.tabs).first { Model.paneIDs($0.root).contains(id) }?.id
            guard let (tab, frame) = bar.target(at: inBar), tab != home else {
                hint.hide()
                return nil
            }
            hint.show(window.convertToScreen(bar.convert(frame, to: nil)), text: tab == nil ? "+" : "")
            return tab.map(Drop.tab) ?? .newTab(window: controller.id)
        }
        for view in host.views.values where view.window === window && view.superview != nil && view.id != id {
            let p = view.convert(inWindow, from: nil), b = view.bounds
            guard b.contains(p) else { continue }
            if core.model.dockWindow(view.id)?.dock?.frozen == true {
                hint.hide()
                return nil
            }
            let fx = p.x / b.width, fy = 1 - p.y / b.height
            var side = "swap"
            if core.model.dockWindow(view.id) == nil, !(fx > 0.3 && fx < 0.7 && fy > 0.3 && fy < 0.7) {
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

    /// What dropping tab `id` at `point` would do: into a tab bar at the gap
    /// nearest the mouse, or outside kmux into a new window.
    private func tabDrop(at point: NSPoint, hint: DropHint, size: NSSize) -> Drop? {
        guard let controller = controller(at: point, below: hint) else {
            hint.show(newWindowFrame(at: point, size: size), text: "new window", rounded: true)
            return .newWindow(point)
        }
        let bar = controller.tabBar, inBar = bar.convert(controller.window.convertPoint(fromScreen: point), from: nil)
        guard bar.bounds.contains(inBar) else {
            hint.hide()
            return nil
        }
        let (index, x) = bar.insertion(at: inBar.x)
        hint.show(controller.window.convertToScreen(bar.convert(NSRect(x: x - 1, y: 4, width: 3, height: TabBar.height - 8), to: nil)))
        return .index(window: controller.id, index)
    }

    @discardableResult
    private func dragPane(_ id: String) -> Task<Void, Never>? {
        guard let size = host.views[id]?.window?.frame.size, core.model.dockWindow(id)?.dock?.frozen != true else { return nil }
        let hint = DropHint()
        let end = trackMouse { _ = paneDrop(id, at: $0, hint: hint, size: size) }
        let drop = paneDrop(id, at: end, hint: hint, size: size)
        hint.hide()
        var args: [String: JSON] = ["pane": .string(id)]
        switch drop {
        case .pane(let to, let side): args["to"] = .string(to); args["side"] = .string(side)
        case .tab(let tab): args["tab"] = .string(tab)
        case .newTab(let window): args["tab"] = "new"; args["window"] = .string(window)
        case .newWindow: args["window"] = "new"
        case .index, nil: return nil
        }
        return perform(["cmd": "move", "args": .object(args)], tornOff: drop, size: size)
    }

    @discardableResult
    private func dragTab(_ id: String) -> Task<Void, Never>? {
        guard let window = core.model.windows.first(where: { $0.tabs.contains { $0.id == id } }), let size = controllers[window.id]?.window.frame.size else { return nil }
        let hint = DropHint()
        let end = trackMouse { _ = tabDrop(at: $0, hint: hint, size: size) }
        let drop = tabDrop(at: end, hint: hint, size: size)
        hint.hide()
        switch drop {
        case .index(let window, let index):
            return perform(["cmd": "move-tab", "args": ["tab": .string(id), "window": .string(window), "index": .number(Double(index))]], tornOff: nil, size: size)
        case .newWindow:
            return perform(["cmd": "move-tab", "args": ["tab": .string(id), "window": "new"]], tornOff: drop, size: size)
        default: return nil
        }
    }

    /// Sends a drag's request; a new window opens where the drag ended.
    private func perform(_ request: JSON, tornOff drop: Drop?, size: NSSize) -> Task<Void, Never> {
        Task { @MainActor in
            let reply = await core.handle(request)
            guard reply["ok"] == true else { return NSLog("kmux: \(String(decoding: reply.encoded(), as: UTF8.self))") }
            if case .newWindow(let point)? = drop, let id = reply["window"]?.string, let controller = controllers[id] {
                controller.window.setFrame(newWindowFrame(at: point, size: size), display: true)
            }
        }
    }

    /// Replays a drag along scripted mouse positions (for tests):
    /// `{pane: P, to: TARGET}`, `{tab: T, to: TARGET}` or `{divider: P, at: FRACTION}`
    /// (the divider after pane P, moved to that fraction of the pair). TARGET is
    /// `{pane: P, x, y}` (fractions from the top left), `{tab: T}`, `{plus: WINDOW}`
    /// or `{outside: true}` (beyond every kmux window).
    private func debugDrag(_ args: JSON) async throws -> [String: JSON] {
        func post(_ points: [NSPoint]) { scriptedDrag = points }
        func screen(_ view: NSView, _ point: NSPoint) throws -> NSPoint {
            guard let window = view.window else { throw KmuxError("not_found", "not on screen") }
            return window.convertPoint(toScreen: view.convert(point, to: nil))
        }
        // Look only at kmux's own windows, and show no hint: tests run while
        // other apps cover kmux, and must not put windows in front of them.
        testingDrag = true
        DropHint.quiet = true
        defer {
            testingDrag = false
            DropHint.quiet = false
        }
        try await Task.sleep(for: .milliseconds(50))
        if let ref = args["divider"]?.string {
            let pane = try core.needPane(ref)
            if let (controller, handle) = controllers.values.lazy.compactMap({ c in c.dockDivider.flatMap { $0.paneID == pane.id ? (c, $0) : nil } }).first,
               let content = handle.superview {
                let to = try screen(content, handle.position(CGFloat(args["at"]?.number ?? 0.5)))
                post([to, to])
                _ = await core.handle(["cmd": "resize", "args": handle.drag(in: content)])
                controller.preview = nil
                controller.render(core.model, host)
                return [:]
            }
            guard let (controller, handle) = controllers.values.lazy.compactMap({ c in
                c.dividers.first { if case .pane(pane.id) = $0.split.kids[$0.index].node { true } else { false } }.map { (c, $0) }
            }).first, let stage = handle.superview else { throw KmuxError("not_found", "no divider after \(pane.id)") }
            let to = try screen(stage, handle.position(CGFloat(args["at"]?.number ?? 0.5)))
            post([to, to])
            _ = await core.handle(["cmd": "resize", "args": handle.drag(in: stage)])
            controller.preview = nil
            controller.render(core.model, host)
            return [:]
        }
        let to: NSPoint
        let target = args["to"] ?? [:]
        if let ref = target["pane"]?.string {
            guard let view = host.views[try core.needPane(ref).id] else { throw KmuxError("not_found", "no pane \(ref)") }
            let b = view.bounds
            to = try screen(view, NSPoint(x: b.width * CGFloat(target["x"]?.number ?? 0.5), y: b.height * (1 - CGFloat(target["y"]?.number ?? 0.5))))
        } else if let tab = target["tab"]?.string ?? target["plus"]?.string {
            guard let controller = controllers.values.first(where: { target["plus"] != nil ? $0.id == tab : $0.tabBar.center(of: tab) != nil }) else { throw KmuxError("not_found", "no tab bar for \(tab)") }
            let point = target["plus"] != nil ? controller.tabBar.plusCenter : controller.tabBar.convert(controller.tabBar.center(of: tab)!, from: nil)
            to = try screen(controller.tabBar, point)
        } else {
            let screenFrame = NSScreen.main?.visibleFrame ?? .zero
            let corners = [NSPoint(x: screenFrame.maxX - 30, y: screenFrame.minY + 30), NSPoint(x: screenFrame.minX + 30, y: screenFrame.minY + 30),
                           NSPoint(x: screenFrame.maxX - 30, y: screenFrame.maxY - 30), NSPoint(x: screenFrame.minX + 30, y: screenFrame.maxY - 30)]
            guard let free = corners.first(where: { p in !controllers.values.contains { $0.window.frame.contains(p) } }) else {
                throw KmuxError("bad_request", "every corner of the screen is covered by a kmux window")
            }
            to = free
        }
        post([to, to])
        let task: Task<Void, Never>?
        if let ref = args["pane"]?.string {
            task = dragPane(try core.needPane(ref).id)
        } else if let tab = args["tab"]?.string {
            task = dragTab(tab)
        } else {
            throw KmuxError("bad_request", "debug.drag needs pane, tab or divider")
        }
        await task?.value
        let under = NSWindow.windowNumber(at: to, belowWindowWithWindowNumber: 0)
        let owner = (CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(under)) as? [[String: Any]])?.first?[kCGWindowOwnerName as String] as? String
        let window = controllers.values.first { $0.window.windowNumber == under }?.id
        return ["dropped": .bool(task != nil), "at": [.number(to.x), .number(to.y)], "under": .string(window ?? owner ?? "\(under)")]
    }

    private func snapshot(_ args: JSON) throws -> [String: JSON] {
        let window = try core.targetWindow(args["window"])
        guard let controller = controllers[window.id] else { throw KmuxError("not_found", "no window \"\(window.id)\"") }
        controller.window.displayIfNeeded()
        let panes = window.shownPanes.compactMap { host.views[$0] }.filter { $0.window != nil && !$0.isHiddenOrHasHiddenAncestor }
        return try Snapshot.run(controller, panes: panes, path: args["path"]?.string, marker: args["marker"]?.string)
    }

    private func installMenu() {
        let runtime = host.runtime
        let main = NSMenu()
        func menu(_ title: String, _ items: [(String, Selector, String?, Shortcut?)?]) -> NSMenu {
            let item = NSMenuItem()
            let submenu = NSMenu(title: title)
            for entry in items {
                guard let (label, action, ghostty, fallback) = entry else {
                    submenu.addItem(.separator())
                    continue
                }
                let shortcut = ghostty.flatMap { runtime.shortcut(for: $0) } ?? fallback
                let menuItem = submenu.addItem(withTitle: label, action: action, keyEquivalent: shortcut?.key ?? "")
                menuItem.keyEquivalentModifierMask = shortcut?.modifiers ?? []
                menuItem.target = self
            }
            item.submenu = submenu
            main.addItem(item)
            return submenu
        }
        let app = menu(Brand.current.displayName, [("New Instance", #selector(newInstance), nil, nil)])
        if !instance.isDefault { app.insertItem(withTitle: "Instance: \(instance.name)", action: nil, keyEquivalent: "", at: 0) }
        app.addItem(.separator())
        app.addItem(withTitle: "Quit \(Brand.current.displayName)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        // Sent to whatever has focus: a terminal (Ghostty's copy and paste), a
        // web page or its URL field, or a markdown pane's text.
        let edit = NSMenu(title: "Edit")
        let editItems: [(String, String, String?, Shortcut)?] = [
            ("Undo", "undo:", nil, .cmd("z")), ("Redo", "redo:", nil, .shiftCmd("z")), nil,
            ("Cut", "cut:", nil, .cmd("x")), ("Copy", "copy:", "copy_to_clipboard", .cmd("c")),
            ("Paste", "paste:", "paste_from_clipboard", .cmd("v")), ("Select All", "selectAll:", "select_all", .cmd("a")),
        ]
        for entry in editItems {
            guard let (label, action, ghostty, fallback) = entry else {
                edit.addItem(.separator())
                continue
            }
            let shortcut = ghostty.flatMap { runtime.shortcut(for: $0) } ?? fallback
            let item = edit.addItem(withTitle: label, action: Selector(action), keyEquivalent: shortcut.key)
            item.keyEquivalentModifierMask = shortcut.modifiers
        }
        let editItem = NSMenuItem()
        editItem.submenu = edit
        main.addItem(editItem)
        paneMenu = menu("Pane", [
            ("Split Right", #selector(splitRight), "new_split:right", .cmd("d")),
            ("Split Down", #selector(splitDown), "new_split:down", .shiftCmd("d")),
            ("New Web Pane Right", #selector(webRight), nil, nil),
            ("New Web Pane Below", #selector(webDown), nil, nil),
            ("New iOS Pane Right", #selector(iosRight), nil, nil),
            ("New iOS Pane Below", #selector(iosDown), nil, nil),
            ("New Markdown Pane Right…", #selector(markdownRight), nil, nil),
            ("New Markdown Pane Below…", #selector(markdownDown), nil, nil),
            ("Open URL…", #selector(openURL), nil, .cmd("l")),
            ("Back", #selector(goBack), nil, nil),
            ("Forward", #selector(goForward), nil, nil),
            ("Home", #selector(pressHome), nil, .shiftCmd("h")),
            nil,
            ("Next Pane", #selector(nextPane), "goto_split:next", .cmd("]")),
            ("Previous Pane", #selector(previousPane), "goto_split:previous", .cmd("[")),
            ("Zoom", #selector(toggleZoom), "toggle_split_zoom", .shiftCmd("\r")),
            ("Move Pane to New Window", #selector(movePaneToNewWindow), nil, nil),
            nil,
            ("Reload", #selector(reloadPane), nil, .cmd("r")),
            ("Close Pane", #selector(closePane), "close_surface", .cmd("w")),
        ])
        let view = menu("View", [
            ("New Tab", #selector(newTab as () -> Void), "new_tab", .cmd("t")),
            ("Next Tab", #selector(nextTab), "next_tab", .shiftCmd("]")),
            ("Previous Tab", #selector(previousTab), "previous_tab", .shiftCmd("[")),
            nil,
            ("Zoom In", #selector(zoomIn), "increase_font_size:1", .cmd("=")),
            ("Zoom Out", #selector(zoomOut), "decrease_font_size:1", .cmd("-")),
            ("Actual Size", #selector(actualSize), "reset_font_size", .cmd("0")),
        ])
        let tabs = NSMenu(title: "Show Tab")
        for number in 1...9 {
            let shortcut = runtime.shortcut(for: "goto_tab:\(number)") ?? .cmd("\(number)")
            let item = tabs.addItem(withTitle: "Tab \(number)", action: #selector(showTab(_:) as (NSMenuItem) -> Void), keyEquivalent: shortcut.key)
            item.keyEquivalentModifierMask = shortcut.modifiers
            item.tag = number
            item.target = self
        }
        view.insertItem(.separator(), at: 3)
        view.insertItem(withTitle: "Show Tab", action: nil, keyEquivalent: "", at: 4).submenu = tabs
        let windowMenu = menu("Window", [
            ("New Window", #selector(newWindow), "new_window", .cmd("n")),
            ("Close Window", #selector(closeWindow), "close_window", .shiftCmd("w")),
            ("Move Tab to New Window", #selector(moveTabToNewWindow), nil, nil),
            nil,
            ("Free Window", #selector(freeWindow), nil, nil),
            nil,
            ("Next Window", #selector(nextWindow), nil, .cmd("`")),
            ("Previous Window", #selector(previousWindow), nil, .shiftCmd("`")),
            nil,
        ])
        let workspaces = NSMenu(title: "Assign Window to Workspace")
        workspaces.delegate = self
        workspaceMenu = workspaces
        let free = windowMenu.indexOfItem(withTitle: "Free Window")
        windowMenu.insertItem(withTitle: "Assign Window to Workspace", action: nil, keyEquivalent: "", at: free).submenu = workspaces
        NSApp.mainMenu = main
        NSApp.windowsMenu = windowMenu
    }

    /// `debug.key`: delivers a key press through AppKit's normal event path,
    /// as if typed into the key window (e.g. "cmd+shift+d").
    private func pressKey(_ args: JSON) throws -> [String: JSON] {
        guard let name = args["key"]?.string else { throw KmuxError("bad_request", "debug.key needs a key") }
        let window = core.model.key.flatMap { controllers[$0]?.window }
        guard let event = SyntheticKey.event(name, window: window) else { throw KmuxError("bad_request", "unknown key \"\(name)\"") }
        NSApp.sendEvent(event)
        return ["characters": .string(event.characters ?? ""), "charactersIgnoringModifiers": .string(event.charactersIgnoringModifiers ?? "")]
    }

    /// `debug.click`: clicks a tab (`tab`, `clicks`), or a pane (`pane`, `at`, `button`), with mouse events sent
    /// through AppKit's normal event path.
    private func click(_ args: JSON) throws -> [String: JSON] {
        let window: NSWindow, point: NSPoint
        if args["pane"] != nil {
            // `at`: [x, y] from the pane's top left.
            let id = try core.needPane(args["pane"]?.string).id
            guard let view = host.views[id], let paneWindow = view.window else { throw KmuxError("not_found", "pane \"\(id)\" is not on screen") }
            var at = [view.bounds.midX, view.bounds.midY]
            if case .array(let list)? = args["at"] { at = list.compactMap(\.number).map { CGFloat($0) } }
            window = paneWindow
            point = view.convert(NSPoint(x: at[0], y: view.bounds.height - at[1]), to: nil)
        } else {
            let id = args["tab"]?.string ?? ""
            guard let state = core.model.windows.first(where: { $0.tabs.contains { $0.id == id } }), let controller = controllers[state.id],
                  let center = controller.tabBar.center(of: id) else { throw KmuxError("not_found", "no tab \"\(id)\" on screen") }
            window = controller.window
            point = center
        }
        // `button`: 3 or 4 is the mouse's back or forward button, sent through the window.
        if let button = args["button"]?.number {
            guard let cg = NSEvent.mouseEvent(with: .otherMouseDown, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                              windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)?.cgEvent
            else { throw KmuxError("bad_request", "can't make that mouse event") }
            cg.setIntegerValueField(.mouseEventButtonNumber, value: Int64(button))
            guard let down = NSEvent(cgEvent: cg) else { throw KmuxError("bad_request", "can't make that mouse event") }
            window.sendEvent(down)
            return [:]
        }
        for count in 1...max(1, Int(args["clicks"]?.number ?? 1)) {
            func event(_ type: NSEvent.EventType) -> NSEvent? {
                NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                   windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: count, pressure: 1)
            }
            // Queue the mouse-up first: a tab waits for it to tell a click from a drag.
            guard let down = event(.leftMouseDown), let up = event(.leftMouseUp) else { continue }
            NSApp.postEvent(up, atStart: false)
            NSApp.sendEvent(down)
        }
        return [:]
    }

    private func fatal(_ message: String) -> Never {
        FileHandle.standardError.write(Data((message + "\n").utf8))
        exit(1)
    }
}

/// The app's memory footprint in bytes (what Activity Monitor shows as Memory).
private func memoryFootprint() -> UInt64 {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    let result = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count) }
    }
    return result == KERN_SUCCESS ? info.phys_footprint : 0
}

extension AppController: NSMenuDelegate {
    /// Assign Window to Workspace: the workspaces (the key window's checked),
    /// then a new one.
    func menuNeedsUpdate(_ menu: NSMenu) {
        guard menu === workspaceMenu else { return }
        menu.removeAllItems()
        for workspace in core.model.workspaces {
            let item = menu.addItem(withTitle: workspace.label + (workspace.hidden ? " (hidden)" : ""), action: #selector(assignWindow(_:)), keyEquivalent: "")
            item.representedObject = workspace.id
            item.state = keyWindow?.workspace == workspace.id ? .on : .off
            item.target = self
        }
        if !core.model.workspaces.isEmpty { menu.addItem(.separator()) }
        menu.addItem(withTitle: "New Workspace", action: #selector(assignWindow(_:)), keyEquivalent: "").target = self
    }
}
