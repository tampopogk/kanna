import AppKit
import GhosttyKit

@MainActor
enum TerminalDebug {
    static func install(_ commands: Commands, runtime: GhosttyRuntime, terminal: @escaping @MainActor (String?) throws -> TerminalSurfaceView) {
        commands["debug.terminal"] = { args in
            let view = try terminal(args["pane"]?.string)
            guard let surface = view.surface else { throw KannaError("not_found", "the terminal is not running") }
            let effects = runtime.effects
            if let appearance = args["appearance"]?.string {
                NSApp.appearance = appearance == "dark" ? NSAppearance(named: .darkAqua) : appearance == "light" ? NSAppearance(named: .aqua) : nil
            }
            if let text = args["clipboard"]?.string { effects.write(text, to: GHOSTTY_CLIPBOARD_STANDARD) }
            if let action = args["action"]?.string {
                view.window?.makeFirstResponder(view)
                _ = ghostty_surface_binding_action(surface, action, UInt(action.utf8.count))
            }
            if let zoom = args["zoom"]?.number { view.zoom(by: Int(zoom)) }
            if case .object(let click)? = args["click"] { try Self.click(view, click) }
            let size = ghostty_surface_size(surface)
            var colors = ghostty_surface_colors_s()
            let background = ghostty_surface_colors(surface, false, &colors) && colors.has_background ? TerminalSettings.hex(colors.background) : nil
            var reply: [String: JSON] = [
                "cols": .number(Double(size.columns)), "rows": .number(Double(size.rows)),
                "cellWidth": .number(Double(size.cell_width_px)), "cellHeight": .number(Double(size.cell_height_px)),
                "title": .string(view.title), "hoveredLink": view.hoveredLink.map { .string($0) } ?? .null,
                "selection": .bool(ghostty_surface_has_selection(surface)), "background": background.map { .string($0) } ?? .null,
                "clipboard": effects.read(GHOSTTY_CLIPBOARD_STANDARD).map { .string($0) } ?? .null,
                "opened": .array(effects.opened.map { .string($0) }),
                "links": .array(effects.links.map { ["url": .string($0.url), "decision": .string($0.decision)] }), "bells": .number(Double(effects.bells)),
                "notifications": .array(effects.notifications.map { ["title": .string($0.title), "body": .string($0.body)] }),
                "settings": .object(runtime.settings.objectFields), "focused": .bool(view.window?.firstResponder === view),
                "width": .number(Double(view.bounds.width)), "inset": .number(Double(view.canvas.frame.minX)),
                "gridWidth": .number(Double(view.canvas.frame.width)), "scale": .number(Double(view.window?.backingScaleFactor ?? 0)),
                "marginColor": view.marginColor.map { .string($0) } ?? .null,
            ]
            if let progress = view.progress {
                reply["progress"] = ["state": .number(Double(progress.state.rawValue)), "value": .number(Double(progress.progress))]
            }
            return reply
        }
    }

    private static func click(_ view: TerminalSurfaceView, _ click: [String: JSON]) throws {
        guard let window = view.window, let surface = view.surface else { throw KannaError("not_found", "the terminal is not on screen") }
        let x: Double, y: Double
        if case .array(let at)? = click["at"], at.count == 2, let left = at[0].number, let top = at[1].number {
            (x, y) = (left, top)
        } else if case .array(let cell)? = click["cell"], cell.count == 2, let column = cell[0].number, let row = cell[1].number {
            let size = ghostty_surface_size(surface)
            let scale = Double(window.backingScaleFactor)
            (x, y) = (TerminalSurfaceView.margin + 2 + (column + 0.5) * Double(size.cell_width_px) / scale, 2 + (row + 0.5) * Double(size.cell_height_px) / scale)
        } else {
            throw KannaError("bad_request", "click needs at: [x, y] from the pane's top left, or cell: [column, row]")
        }
        var flags: NSEvent.ModifierFlags = []
        if case .array(let mods)? = click["mods"] {
            for mod in mods.compactMap(\.string) {
                switch mod {
                case "cmd": flags.insert(.command)
                case "shift": flags.insert(.shift)
                case "opt", "alt": flags.insert(.option)
                case "ctrl": flags.insert(.control)
                default: throw KannaError("bad_request", "unknown modifier \"\(mod)\"")
                }
            }
        }
        let point = view.convert(NSPoint(x: x, y: view.bounds.height - y), to: nil)
        let button = click["button"]?.string ?? "left"
        let types: [NSEvent.EventType] = button == "right" ? [.rightMouseDown, .rightMouseUp] : [.leftMouseDown, .leftMouseUp]
        let count = max(1, Int(click["clicks"]?.number ?? 1))
        for number in 1...count {
            for type in types {
                guard let event = NSEvent.mouseEvent(with: type, location: point, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                                                     windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: number, pressure: 1) else { continue }
                switch type {
                case .leftMouseDown: view.mouseDown(with: event)
                case .leftMouseUp: view.mouseUp(with: event)
                case .rightMouseDown: view.rightMouseDown(with: event)
                default: view.rightMouseUp(with: event)
                }
            }
        }
    }
}
