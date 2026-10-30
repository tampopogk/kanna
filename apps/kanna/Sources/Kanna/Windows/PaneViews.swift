import AppKit

@MainActor
final class PaneViews {
    private var views: [String: PaneView] = [:]
    private var running: [String: String] = [:]
    private let contents: Contents
    private let terminals: TerminalBackend
    var onFocus: ((String) -> Void)?
    var onDrag: ((String) -> Void)?
    var onClose: ((String) -> Void)?
    var contextMenu: (() -> NSMenu?)?
    var terminalOf: ((String) -> String?)?

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
        view.waiting = type == "term"
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
        guard let view = views[id], let surface = view.terminal else { return }
        guard let wanted = terminalOf?(id) else { return }
        switch running[id] {
        case wanted: return
        case nil: start(surface, wanted, in: view)
        default: renew(id, wanted, in: view)
        }
    }

    private func renew(_ id: String, _ wanted: String, in view: PaneView) {
        guard let old = view.terminal else { return }
        let keyboard = old.window?.firstResponder === old
        terminals.stop(old)
        let terminal = terminal(for: id)
        view.replace(terminal)
        view.message = nil
        start(terminal, wanted, in: view)
        if keyboard { view.window?.makeFirstResponder(terminal) }
    }

    func pane(showing terminal: String) -> String? { running.first { $0.value == terminal }?.key }

    private func start(_ terminal: TerminalSurfaceView, _ id: String, in view: PaneView) {
        running[view.id] = id
        view.waiting = false
        if !terminals.start(terminal, terminal: id) {
            view.message = "failed: the terminal surface could not be created"
            FileHandle.standardError.write(Data("kanna: the terminal surface for \(view.id) could not be created\n".utf8))
        }
    }

    func keep(_ ids: Set<String>) {
        for (id, view) in views where !ids.contains(id) {
            if let terminal = view.terminal, running[id] != nil { terminals.stop(terminal) }
            if view.terminal == nil { contents.forget(id) }
            view.removeFromSuperview()
            views.removeValue(forKey: id)
            running.removeValue(forKey: id)
        }
    }

    func stopAll() {
        for (id, view) in views where running[id] != nil {
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
