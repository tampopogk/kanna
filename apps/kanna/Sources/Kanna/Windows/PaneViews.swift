import AppKit

@MainActor
final class PaneViews {
    private var views: [String: PaneView] = [:]
    private var running: Set<String> = []
    private let contents: Contents
    private let terminals: TerminalBackend
    var onFocus: ((String) -> Void)?
    var onDrag: ((String) -> Void)?
    var onClose: ((String) -> Void)?
    var contextMenu: (() -> NSMenu?)?

    init(contents: Contents, terminals: TerminalBackend) {
        self.contents = contents
        self.terminals = terminals
    }

    subscript(id: String) -> PaneView? { views[id] }
    var all: [PaneView] { Array(views.values) }

    func view(_ id: String, type: String) -> PaneView? {
        if let view = views[id] { return view }
        let content = type == "term" ? terminal(for: id) : contents.slot(id)
        let view = PaneView(id: id, type: type, content: content)
        view.grip.onDrag = { [weak self] _ in self?.onDrag?(id) }
        views[id] = view
        return view
    }

    private func terminal(for id: String) -> TerminalSurfaceView {
        let terminal = TerminalSurfaceView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        terminal.focused = { [weak self] in self?.onFocus?(id) }
        terminal.contextMenu = { [weak self] in self?.contextMenu?() }
        return terminal
    }

    func started(_ id: String) {
        guard !running.contains(id), let view = views[id] else { return }
        running.insert(id)
        guard let terminal = view.terminal else { return }
        start(terminal, in: view)
    }

    func renew(_ id: String) -> Bool {
        guard running.contains(id), let view = views[id], let old = view.terminal else { return false }
        let keyboard = old.window?.firstResponder === old
        terminals.stop(old)
        let terminal = terminal(for: id)
        view.replace(terminal)
        view.message = nil
        start(terminal, in: view)
        if keyboard { view.window?.makeFirstResponder(terminal) }
        return true
    }

    private func start(_ terminal: TerminalSurfaceView, in view: PaneView) {
        if !terminals.start(terminal, pane: Pane(id: view.id)) {
            view.message = "failed: the terminal surface could not be created"
            FileHandle.standardError.write(Data("kanna: the terminal surface for \(view.id) could not be created\n".utf8))
        }
    }

    func keep(_ ids: Set<String>) {
        for (id, view) in views where !ids.contains(id) {
            if let terminal = view.terminal, running.contains(id) { terminals.stop(terminal) }
            if view.terminal == nil { contents.forget(id) }
            view.removeFromSuperview()
            views.removeValue(forKey: id)
            running.remove(id)
        }
    }

    func stopAll() {
        for (id, view) in views where running.contains(id) {
            if let terminal = view.terminal { terminals.stop(terminal) }
        }
        running.removeAll()
    }

    func terminal(_ id: String) -> TerminalSurfaceView? { views[id]?.terminal }

    func id(of terminal: TerminalSurfaceView) -> String? { views.first { $0.value.content === terminal }?.key }

    func keyView(_ id: String) -> NSView? {
        guard let view = views[id] else { return nil }
        return view.terminal ?? contents.keyView(id)
    }

    func exited(_ id: String, code: Int32) { views[id]?.message = "exited (\(code))" }
    func restarted(_ id: String) { views[id]?.message = nil }
    func report(_ id: String, state: String?, error: String?) {
        switch state {
        case "failed": views[id]?.message = "failed: \(error ?? "unknown error")"
        case "running", "starting": views[id]?.message = nil
        default: break
        }
    }
}
