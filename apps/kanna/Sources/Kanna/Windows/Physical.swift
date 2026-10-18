import AppKit

@MainActor
final class Physical {
    private var frames: [String: NSRect] = [:]
    private var docks: [String: (width: Double, over: Double)] = [:]
    private var textSizes: [String: Double] = [:]
    private let path: String?
    private var pending: DispatchWorkItem?

    init(path: String?, fresh: Bool) {
        self.path = path
        guard let path, !fresh, let data = FileManager.default.contents(atPath: path), let saved = try? JSON.parse(data) else { return }
        if case .object(let list)? = saved["frames"] {
            for (id, frame) in list {
                guard let x = frame["x"]?.number, let y = frame["y"]?.number, let w = frame["w"]?.number, let h = frame["h"]?.number, w > 0, h > 0 else { continue }
                frames[id] = NSRect(x: x, y: y, width: w, height: h)
            }
        }
        if case .object(let list)? = saved["docks"] {
            docks = list.compactMapValues { dock in dock["width"]?.number.flatMap { width in dock["over"]?.number.map { (width, $0) } } }
        }
        if case .object(let list)? = saved["textSizes"] { textSizes = list.compactMapValues(\.number) }
    }

    func frame(_ window: String) -> NSRect? { frames[window] }

    func setFrame(_ window: String, _ frame: NSRect) {
        guard frames[window] != frame else { return }
        frames[window] = frame
        changed()
    }

    func textSize(_ pane: String) -> CGFloat? { textSizes[pane].map { CGFloat($0) } }

    func setTextSize(_ pane: String, _ size: CGFloat) {
        textSizes[pane] = size == 1 ? nil : Double(size)
        changed()
    }

    func setDock(_ pane: String, width: Double, in view: JSON) {
        guard let over = Self.width(of: pane, in: view) else { return }
        docks[pane] = (width, over)
        changed()
    }

    func applied(_ view: JSON) -> JSON {
        guard case .object(var fields) = view else { return view }
        if case .array(let windows)? = fields["windows"] { fields["windows"] = .array(windows.map(apply)) }
        if case .array(let shelf)? = fields["shelf"] { fields["shelf"] = .array(shelf.map(widened)) }
        return .object(fields)
    }

    func keep(panes: Set<String>) {
        let before = (docks.count, textSizes.count)
        docks = docks.filter { panes.contains($0.key) }
        textSizes = textSizes.filter { panes.contains($0.key) }
        if (docks.count, textSizes.count) != before { changed() }
    }

    func save() {
        pending?.cancel()
        pending = nil
        guard let path else { return }
        let saved: JSON = [
            "version": 1,
            "frames": .object(frames.mapValues { ["x": .number($0.minX), "y": .number($0.minY), "w": .number($0.width), "h": .number($0.height)] }),
            "docks": .object(docks.mapValues { ["width": .number($0.width), "over": .number($0.over)] }),
            "textSizes": .object(textSizes.mapValues { .number($0) }),
        ]
        let temporary = path + ".new"
        guard FileManager.default.createFile(atPath: temporary, contents: saved.encoded(), attributes: [.posixPermissions: 0o600]) else { return }
        _ = try? FileManager.default.replaceItemAt(URL(fileURLWithPath: path), withItemAt: URL(fileURLWithPath: temporary))
    }

    private func changed() {
        pending?.cancel()
        let save = DispatchWorkItem { [weak self] in self?.save() }
        pending = save
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: save)
    }

    private func apply(_ window: JSON) -> JSON {
        guard case .object(var fields) = window, let id = fields["id"]?.string else { return window }
        if let frame = frames[id] { fields["frame"] = ["x": .number(frame.minX), "y": .number(frame.minY), "w": .number(frame.width), "h": .number(frame.height)] }
        if let dock = fields["dock"] { fields["dock"] = widened(dock) }
        if let lead = fields["lead"] { fields["lead"] = widened(lead) }
        return .object(fields)
    }

    private func widened(_ dock: JSON) -> JSON {
        guard case .object(var fields) = dock, let pane = fields["pane"]?.string ?? fields["task"]?.string, let saved = docks[pane],
              let width = fields["width"]?.number, abs(width - saved.over) < 1e-9 else { return dock }
        fields["width"] = .number(saved.width)
        return .object(fields)
    }

    private static func width(of pane: String, in view: JSON) -> Double? {
        switch view {
        case .object(let fields):
            if fields["pane"]?.string ?? fields["task"]?.string == pane, let width = fields["width"]?.number { return width }
            return fields.values.lazy.compactMap { width(of: pane, in: $0) }.first
        case .array(let list):
            return list.lazy.compactMap { width(of: pane, in: $0) }.first
        default:
            return nil
        }
    }
}
