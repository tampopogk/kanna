import AppKit
import GhosttyKit

@MainActor
protocol TerminalLink: AnyObject {
    func opened(_ terminal: ExternalTerminal)
    func input(_ bytes: Data, from terminal: ExternalTerminal)
    func resizeRequested(_ terminal: ExternalTerminal, cols: UInt16, rows: UInt16, width: UInt32, height: UInt32)
    func closed(_ terminal: ExternalTerminal)
    func settingsChanged(_ settings: TerminalSettings)
}

@MainActor
final class ExternalTerminal {
    let id: String
    fileprivate weak var view: TerminalSurfaceView?
    var surface: ghostty_surface_t? { view?.surface }

    private enum Pending { case output(Data, (() -> Void)?), snapshot(Data, (() -> Void)?), grid(UInt16, UInt16), exited(Int32) }
    private var pending: [Pending] = []
    private var retrying = false
    private var refusals = 0
    private static let refusalLimit = 250

    fileprivate init(id: String, view: TerminalSurfaceView) {
        self.id = id
        self.view = view
    }

    func output(_ bytes: Data, taken: (() -> Void)? = nil) {
        guard !bytes.isEmpty else {
            taken?()
            return
        }
        enqueue(.output(bytes, taken))
    }

    func setGrid(cols: UInt16, rows: UInt16) { enqueue(.grid(cols, rows)) }

    func loadSnapshot(_ bytes: Data, taken: (() -> Void)? = nil) { enqueue(.snapshot(bytes, taken)) }

    func exited(status: Int32) { enqueue(.exited(status)) }

    private func enqueue(_ item: Pending) {
        pending.append(item)
        drain()
    }

    private func drain() {
        guard let surface else { return discard() }
        while let item = pending.first {
            let taken = switch item {
            case .output(let bytes, _): bytes.withUnsafeBytes { ghostty_surface_process_output(surface, $0.bindMemory(to: UInt8.self).baseAddress, bytes.count) }
            case .snapshot(let bytes, _): bytes.withUnsafeBytes { ghostty_surface_load_snapshot(surface, $0.bindMemory(to: UInt8.self).baseAddress, bytes.count) }
            case .grid(let cols, let rows): ghostty_surface_set_grid_size(surface, cols, rows)
            case .exited(let status): { ghostty_surface_set_process_exited(surface, status); return true }()
            }
            guard taken || givesUp(item) else { return retryLater() }
            refusals = 0
            pending.removeFirst()
            switch item {
            case .output(_, let done?), .snapshot(_, let done?): done()
            default: break
            }
        }
    }

    private func givesUp(_ item: Pending) -> Bool {
        guard case .snapshot = item else { return false }
        refusals += 1
        guard refusals >= Self.refusalLimit else { return false }
        FileHandle.standardError.write(Data("kanna: \(id)'s screen from the daemon was refused \(refusals) times; going on without it\n".utf8))
        return true
    }

    fileprivate func discard() {
        let dropped = pending
        pending.removeAll()
        for item in dropped {
            switch item {
            case .output(_, let done?), .snapshot(_, let done?): done()
            default: break
            }
        }
    }

    private func retryLater() {
        guard !retrying else { return }
        retrying = true
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(2)) { [weak self] in
            MainActor.assumeIsolated {
                self?.retrying = false
                self?.drain()
            }
        }
    }
}

@MainActor
final class ExternalIOBackend: TerminalBackend {
    nonisolated(unsafe) private static weak var shared: ExternalIOBackend?
    private let runtime: GhosttyRuntime
    private let link: TerminalLink
    private var terminals: [UInt: ExternalTerminal] = [:]

    init(runtime: GhosttyRuntime, link: TerminalLink) {
        self.runtime = runtime
        self.link = link
        Self.shared = self
        runtime.onSettings = { [weak link] settings in link?.settingsChanged(settings) }
    }

    func start(_ view: TerminalSurfaceView, terminal id: String) -> Bool {
        var options = GhosttyRuntime.options(for: view)
        options.io_mode = GHOSTTY_IO_MODE_EXTERNAL
        options.io_write_cb = { userdata, bytes, count in
            guard let bytes, count > 0 else { return }
            let data = Data(bytes: bytes, count: count)
            ExternalIOBackend.onMain(userdata) { backend, terminal in backend.link.input(data, from: terminal) }
        }
        options.io_resize_request_cb = { userdata, cols, rows, width, height in
            ExternalIOBackend.onMain(userdata) { backend, terminal in
                guard let pixels = terminal.view?.pixels, pixels == (width, height) else { return }
                backend.link.resizeRequested(terminal, cols: cols, rows: rows, width: width, height: height)
            }
        }
        let terminal = ExternalTerminal(id: id, view: view)
        terminals[view.surfaceID] = terminal
        guard runtime.attach(view, options: &options) else {
            terminals.removeValue(forKey: view.surfaceID)
            return false
        }
        link.opened(terminal)
        return true
    }

    func stop(_ view: TerminalSurfaceView) {
        if let terminal = terminals.removeValue(forKey: view.surfaceID) {
            link.closed(terminal)
            terminal.discard()
        }
        runtime.detach(view)
    }

    nonisolated private static func onMain(_ userdata: UnsafeMutableRawPointer?, _ body: @escaping @MainActor (ExternalIOBackend, ExternalTerminal) -> Void) {
        let surfaceID = UInt(bitPattern: userdata)
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                guard let backend = ExternalIOBackend.shared, let terminal = backend.terminals[surfaceID] else { return }
                body(backend, terminal)
            }
        }
    }
}
