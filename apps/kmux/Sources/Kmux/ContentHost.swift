import AppKit
import KmuxCore
import KmuxMarkdown

/// Runs pane contents: Ghostty terminals for `term` panes, web views for
/// `web` panes, native markdown views for `md` panes and the simulator for
/// `ios` panes, and reports their lifecycle to the core.
@MainActor
final class ContentHost: PaneHost {
    let runtime: GhosttyRuntime
    weak var core: Core?
    /// Terminals get `KMUX_INSTANCE`, `KMUX_SOCKET` and `KMUX_TERMINAL`, their
    /// own content ID (`KANNA_…` in Kanna), so `kmux` run inside a pane talks
    /// to the kmux that owns it.
    var instance = Instance(name: Instance.defaultName)
    /// Panes started now come back from the saved layout: their terminals
    /// get `KMUX_RESTORED=1`, so a terminal wrapper can reattach a session.
    var restoring = false
    private(set) var views: [String: PaneView] = [:]
    var onFocus: ((String) -> Void)?
    var onCloseRequest: ((String) -> Void)?
    var onNavigate: ((String, String) -> Void)?
    /// A markdown pane followed a link to another markdown file.
    var onOpenMarkdown: ((String, String) -> Void)?
    /// A markdown pane's mouse back (true) or forward (false) button.
    var onMarkdownHistory: ((String, Bool) -> Void)?
    /// Something worth saving changed inside a pane (a markdown pane's zoom).
    var onPaneChange: (() -> Void)?
    /// Where each file was left in each markdown pane (scroll, magnification),
    /// so going back returns to it.
    private var markdownSpots: [String: [String: (y: CGFloat, zoom: CGFloat)]] = [:]
    /// Markdown panes' zoom, kept when a pane moves to another file or
    /// restarts, and saved with the app's own state.
    var markdownZoom: [String: CGFloat] = [:]
    /// The pane's ⋯ grip was pressed: a drag to move the pane begins.
    var onPaneDrag: ((String, NSEvent) -> Void)?
    /// The Pane menu, for right-clicks (acting on the pane, which takes focus first).
    var contextMenu: (() -> NSMenu?)?

    init(runtime: GhosttyRuntime) {
        self.runtime = runtime
        runtime.onChildExited = { [weak self] view, code in
            guard let id = self?.paneID(of: view) else { return }
            self?.core?.update(id, state: .exited, exitCode: code)
        }
        runtime.onClose = { [weak self] view in
            guard let id = self?.paneID(of: view) else { return }
            self?.onCloseRequest?(id)
        }
    }

    func terminal(_ id: String) -> TerminalSurfaceView? { views[id]?.content as? TerminalSurfaceView }
    func web(_ id: String) -> WebPaneView? { views[id]?.content as? WebPaneView }
    func ios(_ id: String) -> IosPaneView? { views[id]?.content as? IosPaneView }
    func markdown(_ id: String) -> MarkdownPaneView? { views[id]?.content as? MarkdownPaneView }

    /// The view that should have the keyboard when the pane is focused.
    func keyView(_ id: String) -> NSView? { terminal(id) ?? web(id)?.webView ?? ios(id) ?? markdown(id)?.keyView }

    func start(_ pane: Pane) {
        let id = pane.id
        let content: NSView
        var started = true
        var failure = "Ghostty could not create a terminal"
        switch pane.type {
        case .md:
            let path = Self.absolute(pane.path ?? "")
            var directory: ObjCBool = false
            if !FileManager.default.fileExists(atPath: path, isDirectory: &directory) || directory.boolValue {
                content = NSView()
                started = false
                failure = directory.boolValue ? "\(path) is a folder, not a markdown file" : "No such file: \(path)"
            } else {
                let spot = markdownSpots[id]?[path]
                let markdown = MarkdownPaneView(path: path, zoom: spot?.zoom ?? markdownZoom[id] ?? 1)
                markdown.startScrollY = spot?.y
                markdown.onHistory = { [weak self] back in self?.onMarkdownHistory?(id, back) }
                markdown.onZoom = { [weak self] in self?.onPaneChange?() }
                markdown.onFocus = { [weak self] in self?.onFocus?(id) }
                markdown.onOpenMarkdown = { [weak self] target in self?.onOpenMarkdown?(id, target) }
                markdown.contextMenu = { [weak self] in self?.contextMenu?() }
                content = markdown
            }
        case .web:
            let web = WebPaneView(url: pane.url ?? "about:blank")
            web.onFocus = { [weak self] in self?.onFocus?(id) }
            web.onNavigate = { [weak self] url in self?.onNavigate?(id, url) }
            web.onURLChange = { [weak self] url in self?.core?.pageMoved(id, to: url) }
            web.webView.contextMenu = { [weak self] in self?.contextMenu?() }
            content = web
        case .ios:
            let ios = IosPaneView()
            ios.onFocus = { [weak self] in self?.onFocus?(id) }
            ios.contextMenu = { [weak self] in self?.contextMenu?() }
            content = ios
            startSimulator(pane, in: ios)
        default:
            let terminal = TerminalSurfaceView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
            terminal.onFocus = { [weak self] in self?.onFocus?(id) }
            terminal.contextMenu = { [weak self] in self?.contextMenu?() }
            let brand = Brand.current
            var environment = [(brand.variable("INSTANCE"), instance.name), (brand.variable("SOCKET"), instance.socketPath), (brand.variable("TERMINAL"), id)]
            if restoring { environment.append((brand.variable("RESTORED"), "1")) }
            started = runtime.attach(terminal, command: pane.command, cwd: pane.cwd, environment: environment, wrapper: brand.terminalWrapper)
            content = terminal
        }
        let view = PaneView(id: id, content: content)
        view.grip.onDrag = { [weak self] event in self?.onPaneDrag?(id, event) }
        views[id] = view
        guard pane.type != .ios else { return }
        DispatchQueue.main.async { [weak self] in
            if started { self?.core?.update(id, state: .running) } else { self?.core?.update(id, state: .failed, error: failure) }
        }
    }

    /// Boots the device, launches the app and shows the screen, reporting
    /// `running`, or `failed` with simctl's reason. The simulator stays booted
    /// when the pane closes (other panes or tools may be using it).
    private func startSimulator(_ pane: Pane, in view: IosPaneView) {
        let id = pane.id
        let app = pane.app ?? ""
        let appName = ((app as NSString).lastPathComponent as NSString).deletingPathExtension
        view.show(device: pane.device ?? "iPhone", app: appName)
        view.show(status: "Finding \(pane.device ?? "a device")…")
        Task { @MainActor [weak self] in
            // Still this pane's view? (It may have been closed or restarted meanwhile.)
            let current = { self?.views[id]?.content === view }
            do {
                let device = try await Simulator.resolve(pane.device)
                guard current() else { return }
                self?.core?.report(id, device: device.name)
                view.show(device: device.name, app: appName)
                view.show(status: "Booting \(device.name)…")
                try await Simulator.boot(device)
                guard current() else { return }
                view.show(status: "Launching \(appName)…")
                let name = try await Simulator.launch(app, on: device)
                guard current() else { return }
                view.show(device: device.name, app: name)
                try view.attach(udid: device.udid)
                self?.core?.update(id, state: .running)
            } catch {
                guard current() else { return }
                view.show(status: nil)
                self?.core?.update(id, state: .failed, error: (error as? Simulator.Failure)?.message ?? error.localizedDescription)
            }
        }
    }

    func stop(_ pane: Pane) {
        guard let view = views.removeValue(forKey: pane.id) else { return }
        if let terminal = view.content as? TerminalSurfaceView { runtime.detach(terminal) }
        (view.content as? WebPaneView)?.stop()
        (view.content as? IosPaneView)?.stop()
        if let markdown = view.content as? MarkdownPaneView {
            markdown.stop()
            markdownZoom[pane.id] = markdown.zoom
            markdownSpots[pane.id, default: [:]][markdown.path] = (markdown.scrollTop, markdown.zoom)
        }
        view.removeFromSuperview()
    }

    func send(_ pane: Pane, text: String) {
        terminal(pane.id)?.typeLine(text)
    }

    /// A path from a client: `~` expanded; relative paths are relative to home
    /// (the CLI sends absolute paths).
    static func absolute(_ path: String) -> String {
        let expanded = (path as NSString).expandingTildeInPath
        return expanded.hasPrefix("/") ? expanded : (NSHomeDirectory() as NSString).appendingPathComponent(expanded)
    }

    func paneID(of terminal: TerminalSurfaceView) -> String? {
        views.first { $0.value.content === terminal }?.key
    }
}
