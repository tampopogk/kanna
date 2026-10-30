import AppKit

struct DeskRecord: Equatable {
    var id: String
    var project: String?
    var shows: String?
    var planner: String?
    var active: String?
}

@MainActor
final class Physical {
    private(set) var desks: [DeskRecord] = []
    private(set) var deskTasks: [(workspace: String, leftAt: Double)] = []
    private(set) var order: [String] = []
    private var deskCount = 0
    private var frames: [String: NSRect] = [:]
    private var docks: [String: (width: Double, over: Double)] = [:]
    private var textSizes: [String: Double] = [:]
    private(set) var overview = true
    private let path: String?
    private var pending: DispatchWorkItem?

    init(path: String?, fresh: Bool) {
        self.path = path
        guard let path, !fresh, let data = FileManager.default.contents(atPath: path), let saved = try? JSON.parse(data) else { return }
        if case .object(let list)? = saved["frames"] {
            for (id, frame) in list {
                guard let x = frame["x"]?.number, let y = frame["y"]?.number, let w = frame["w"]?.number, let h = frame["h"]?.number, w > 0, h > 0 else { continue }
                let legacy = id.first == "w" && id.count > 1 && id.dropFirst().allSatisfy(\.isNumber)
                frames[legacy ? "win-\(id.dropFirst())" : id] = NSRect(x: x, y: y, width: w, height: h)
            }
        }
        if case .object(let list)? = saved["docks"] {
            docks = list.compactMapValues { dock in dock["width"]?.number.flatMap { width in dock["over"]?.number.map { (width, $0) } } }
        }
        if case .object(let list)? = saved["textSizes"] { textSizes = list.compactMapValues(\.number) }
        overview = saved["overview"]?.bool ?? true
        desks = (saved["desks"]?.array ?? []).compactMap { desk in
            desk["id"]?.string.map { DeskRecord(id: $0, project: desk["project"]?.string, shows: desk["shows"]?.string, planner: desk["planner"]?.string, active: desk["active"]?.string ?? desk["shows"]?.string) }
        }
        order = (saved["order"]?.array ?? []).compactMap(\.string)
        deskTasks = (saved["deskTasks"]?.array ?? []).compactMap { entry in entry["workspace"]?.string.map { ($0, entry["leftAt"]?.number ?? 0) } }
        if saved["deskTasks"] == nil { deskTasks = (saved["shelf"]?.array ?? []).compactMap { entry in entry["workspace"]?.string.map { ($0, entry["leftAt"]?.number ?? 0) } } }
        deskCount = max(Int(saved["deskCount"]?.number ?? 0), desks.compactMap { Int($0.id.dropFirst()) }.max() ?? 0)
    }

    func newDesk(project: String?) -> DeskRecord {
        deskCount += 1
        let desk = DeskRecord(id: "d\(deskCount)", project: project)
        desks.append(desk)
        changed()
        return desk
    }

    func desk(_ id: String) -> DeskRecord? { desks.first { $0.id == id } }

    func setDesk(_ desk: DeskRecord) {
        guard let at = desks.firstIndex(where: { $0.id == desk.id }), desks[at] != desk else { return }
        desks[at] = desk
        changed()
    }

    func closeDesk(_ id: String) {
        desks.removeAll { $0.id == id }
        changed()
    }

    func leave(_ workspace: String) {
        deskTasks.removeAll { $0.workspace == workspace }
        deskTasks.insert((workspace, Date().timeIntervalSince1970.rounded(.down)), at: 0)
        changed()
    }

    func keep(deskTasks claimed: [(workspace: String, leftAt: Double?)], planners given: Set<String>, present: Set<String>) {
        let absent = { (id: String) in !present.contains(MachineIDs.machine(of: id) ?? "") }
        let ids = Set(claimed.map(\.workspace)).union(deskTasks.map(\.workspace).filter(absent)).union(desks.compactMap(\.shows).filter(absent)).union(desks.compactMap(\.active).filter(absent))
        let planners = given.union(desks.compactMap(\.planner).filter(absent))
        var kept = deskTasks.filter { ids.contains($0.workspace) }
        for entry in claimed where !kept.contains(where: { $0.workspace == entry.workspace }) {
            kept.insert((entry.workspace, entry.leftAt ?? Date().timeIntervalSince1970.rounded(.down)), at: 0)
        }
        let desksKept = desks.map { desk in
            DeskRecord(id: desk.id, project: desk.project, shows: desk.shows.flatMap { ids.contains($0) ? $0 : nil }, planner: desk.planner.flatMap { planners.contains($0) ? $0 : nil },
                       active: desk.active.flatMap { ids.contains($0) ? $0 : nil })
        }
        guard kept.map(\.workspace) != deskTasks.map(\.workspace) || desksKept != desks else { return }
        deskTasks = kept
        desks = desksKept
        changed()
    }

    func frame(_ window: String) -> NSRect? { frames[window] }

    func setFront(_ window: String) {
        guard order.first != window else { return }
        order.removeAll { $0 == window }
        order.insert(window, at: 0)
        changed()
    }

    func forget(window: String) {
        guard order.contains(window) else { return }
        order.removeAll { $0 == window }
        changed()
    }

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

    func setOverview(_ on: Bool) {
        guard overview != on else { return }
        overview = on
        changed()
    }

    func setDock(_ pane: String, width: Double, in view: JSON) {
        guard let (key, over) = Self.dock(of: pane, in: view) else { return }
        docks[key] = (width, over)
        changed()
    }

    func applied(_ view: JSON) -> JSON {
        guard case .object(var fields) = view else { return view }
        if case .array(let windows)? = fields["windows"] { fields["windows"] = .array(windows.map(apply)) }
        if case .array(let tasks)? = fields["deskTasks"] { fields["deskTasks"] = .array(tasks.map { widened($0, $0["workspace"]?.string) }) }
        return .object(fields)
    }

    func keep(panes: Set<String>, in view: JSON, on machine: String?) {
        let before = (docks.count, textSizes.count, order.count)
        var owners = Set((view["workspaces"]?.array ?? []).compactMap { $0["id"]?.string })
        owners.formUnion((view["windows"]?.array ?? []).compactMap { $0["id"]?.string })
        owners.formUnion(desks.map(\.id))
        let elsewhere = { (id: String) in MachineIDs.machine(of: id) != machine }
        docks = docks.filter { elsewhere($0.key) || owners.contains($0.key) }
        textSizes = textSizes.filter { elsewhere($0.key) || panes.contains($0.key) }
        order = order.filter { elsewhere($0) || owners.contains($0) }
        if (docks.count, textSizes.count, order.count) != before { changed() }
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
            "overview": .bool(overview),
            "desks": .array(desks.map { desk in
                ["id": .string(desk.id), "project": desk.project.map { .string($0) } ?? .null, "shows": desk.shows.map { .string($0) } ?? .null, "planner": desk.planner.map { .string($0) } ?? .null, "active": desk.active.map { .string($0) } ?? .null]
            }),
            "order": .array(order.map { .string($0) }),
            "deskTasks": .array(deskTasks.map { ["workspace": .string($0.workspace), "leftAt": .number($0.leftAt)] }),
            "deskCount": .number(Double(deskCount)),
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
        if let dock = fields["dock"] { fields["dock"] = widened(dock, Self.owner(ofDockIn: window)) }
        if let lead = fields["lead"] { fields["lead"] = widened(lead, id) }
        return .object(fields)
    }

    private func widened(_ dock: JSON, _ owner: String?) -> JSON {
        guard case .object(var fields) = dock, let owner, let saved = docks[owner],
              let width = fields["width"]?.number, abs(width - saved.over) < 1e-9 else { return dock }
        fields["width"] = .number(saved.width)
        return .object(fields)
    }

    private static func owner(ofDockIn window: JSON) -> String? {
        window["shows"]?.string ?? window["workspace"]?.string
    }

    private static func dock(of pane: String, in view: JSON) -> (String, Double)? {
        for window in view["windows"]?.array ?? [] {
            if window["dock"]?["pane"]?.string == pane, let owner = owner(ofDockIn: window), let width = window["dock"]?["width"]?.number { return (owner, width) }
            if window["lead"]?["pane"]?.string == pane, let id = window["id"]?.string, let width = window["lead"]?["width"]?.number { return (id, width) }
        }
        for entry in view["deskTasks"]?.array ?? [] where entry["task"]?.string == pane {
            if let owner = entry["workspace"]?.string, let width = entry["width"]?.number { return (owner, width) }
        }
        return nil
    }
}
