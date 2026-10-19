import AppKit

extension AppController {
    static let app = "mac"

    static func claimed(_ workspace: JSON, as place: String) -> Bool {
        workspace["where"]?[place]?.string == app
    }

    func composed(_ view: JSON) -> JSON {
        guard case .object(var fields) = view, let workspaces = view["workspaces"]?.array else { return view }
        let mine = workspaces.filter { Self.claimed($0, as: "shelf") }
        let planners = workspaces.filter { Self.claimed($0, as: "planner") }
        let listed = Set(planners.compactMap { $0["id"]?.string })
        givenPlanners.subtract(listed)
        physical.keep(shelf: mine.compactMap { ws in ws["id"]?.string.map { ($0, ws["parkedAt"]?.number) } }, planners: listed.union(givenPlanners))
        let known = Set(physical.desks.compactMap(\.planner)).union(closingPlanners)
        for planner in planners where starting.isEmpty {
            guard let id = planner["id"]?.string, !known.contains(id) else { continue }
            var desk = physical.newDesk(project: planner["dock"]?["pane"]?.string.flatMap { folders[$0] })
            desk.planner = id
            physical.setDesk(desk)
        }
        let byID = Dictionary(workspaces.compactMap { ws in ws["id"]?.string.map { ($0, ws) } }, uniquingKeysWith: { first, _ in first })
        let shown = Set(physical.desks.compactMap(\.shows))
        var windows: [JSON] = physical.desks.map { desk in deskWindow(desk, task: desk.shows.flatMap { byID[$0] }, planner: desk.planner.flatMap { byID[$0] }) }
        windows += (view["windows"]?.array ?? []).filter { window in window["workspace"]?.string.map(shown.contains) ?? true }
        fields["windows"] = .array(windows)
        fields["shelf"] = .array(physical.shelf.filter { !shown.contains($0.workspace) }.compactMap { entry in byID[entry.workspace].map { shelfEntry($0, parkedAt: entry.parkedAt) } })
        fields["held"] = .array(workspaces.filter { $0["where"]?.string == "backlog" }.flatMap { $0["panes"]?.array ?? [] })
        let ids = Set(windows.compactMap { $0["id"]?.string })
        if let key = localKey, ids.contains(key) {
            fields["key"] = .string(key)
        } else if !(view["key"]?.string.map(ids.contains) ?? false), let desk = physical.desks.last?.id {
            fields["key"] = .string(desk)
        }
        return .object(fields)
    }

    private func deskWindow(_ desk: DeskRecord, task: JSON?, planner: JSON?) -> JSON {
        var out: [String: JSON] = ["tabs": [], "tab": nil, "title": nil, "focused": nil, "zoomed": nil, "root": nil]
        for key in ["tabs", "tab", "title", "focused", "zoomed", "root"] {
            if let value = (task ?? planner)?[key] { out[key] = value }
        }
        out["id"] = .string(desk.id)
        out["desk"] = true
        out["dock"] = task?["dock"] ?? .null
        out["shows"] = task?["id"] ?? .null
        out["lead"] = planner?["dock"] ?? .null
        out["planner"] = planner?["id"] ?? .null
        out["project"] = desk.project.map { .string($0) } ?? .null
        out["workspace"] = .null
        return .object(out)
    }

    private func shelfEntry(_ ws: JSON, parkedAt: Double) -> JSON {
        let dock = ws["dock"] ?? .null
        return [
            "task": dock["pane"] ?? .null, "workspace": ws["id"] ?? .null, "type": dock["type"] ?? "term", "parkedAt": .number(parkedAt),
            "edge": dock["edge"] ?? "left", "width": dock["width"] ?? .null, "name": dock["name"] ?? .null,
            "tabs": .array((ws["tabs"]?.array ?? []).map { $0["title"] ?? .null }), "panes": ws["panes"] ?? [],
        ]
    }

    func isDesk(_ window: String) -> Bool { physical.desk(window) != nil }

    func address(_ window: String) -> [String: JSON] {
        guard let desk = physical.desk(window) else { return ["window": .string(window)] }
        return (desk.shows ?? desk.planner).map { ["workspace": .string($0)] } ?? ["window": "new"]
    }

    func workspace(ofTask pane: String) -> String? {
        published["workspaces"]?.array?.first { Self.claimed($0, as: "shelf") && $0["dock"]?["pane"]?.string == pane }?["id"]?.string
    }

    func deskShowing(_ workspace: String) -> DeskRecord? { physical.desks.first { $0.shows == workspace } }

    @discardableResult
    func pullTask(_ task: String, into window: String?) -> (moved: Bool, desk: String)? {
        guard let workspace = workspace(ofTask: task) else { return nil }
        if let there = deskShowing(workspace) {
            show(desk: there.id)
            return (false, there.id)
        }
        var desk = window.flatMap { physical.desk($0) } ?? frontDesk ?? physical.newDesk(project: nil)
        if let old = desk.shows { physical.park(old) }
        desk.shows = workspace
        physical.setDesk(desk)
        show(desk: desk.id)
        return (true, desk.id)
    }

    @discardableResult
    func park(desk id: String) -> String? {
        guard var desk = physical.desk(id), let workspace = desk.shows else { return nil }
        let task = published["workspaces"]?.array?.first { $0["id"]?.string == workspace }?["dock"]?["pane"]?.string
        physical.park(workspace)
        desk.shows = nil
        physical.setDesk(desk)
        show(desk: id)
        return task
    }

    var frontDesk: DeskRecord? {
        if let key = localKey, let desk = physical.desk(key) { return desk }
        let ordered = NSApp.orderedWindows.compactMap { window in controllers.first { $0.value.window === window }?.key }
        return ordered.lazy.compactMap { self.physical.desk($0) }.first ?? physical.desks.last
    }

    func show(desk id: String) {
        localKey = id
        restate()
        refresh()
    }

    func openDesk(project: String?, fresh: Bool) -> (desk: String, project: String?, new: Bool) {
        let root = project.map { canonical(GitPlace.of($0)?.main ?? $0) }
        if !fresh, let root { touchProject(root) }
        if !fresh, let root, let found = physical.desks.first(where: { controllers[$0.id]?.projectRoot == root || $0.project.map { canonical(GitPlace.of($0)?.main ?? $0) } == root }) {
            show(desk: found.id)
            return (found.id, found.project ?? root, false)
        }
        let desk = physical.newDesk(project: project)
        show(desk: desk.id)
        return (desk.id, project, true)
    }

    func closeDesk(_ id: String) {
        guard let desk = physical.desk(id) else { return }
        if let workspace = desk.shows { physical.park(workspace) }
        physical.closeDesk(id)
        if localKey == id { localKey = nil }
        if let planner = desk.planner {
            closingPlanners.insert(planner)
            let pane = published["workspaces"]?.array?.first { $0["id"]?.string == planner }?["dock"]?["pane"]?.string
            if let pane { request("close", ["task": .string(pane)]) { [weak self] _ in self?.closingPlanners.remove(planner) } }
        }
        restate()
        refresh()
    }

    func installDesk() {
        commands["desk"] = { [weak self] args in
            guard let self else { throw KannaError("gone", "Kanna.app is quitting") }
            switch args["action"]?.string {
            case "pull":
                guard let task = args["task"]?.string else { throw KannaError("bad_request", "pull needs a task") }
                guard let pulled = pullTask(task, into: nil) else { throw KannaError("not_found", "\(task) isn't on this app's Shelf") }
                return ["task": .string(task), "moved": .bool(pulled.moved), "desk": .string(pulled.desk)]
            case "park":
                let desk = args["task"]?.string.flatMap { task in workspace(ofTask: task).flatMap(deskShowing) } ?? (args["task"] == nil ? frontDesk : nil)
                if let task = args["task"]?.string, workspace(ofTask: task) == nil { throw KannaError("not_found", "\(task) isn't on this app's Shelf") }
                guard let desk, let task = park(desk: desk.id) else {
                    if let task = args["task"]?.string { return ["task": .string(task), "parked": false] }
                    throw KannaError("not_found", "the front Desk has no task to park")
                }
                return ["task": .string(task), "parked": true, "desk": .string(desk.id)]
            case "desk":
                let opened = openDesk(project: args["project"]?.string, fresh: args["new"]?.bool == true)
                return ["desk": .string(opened.desk), "project": opened.project.map { .string($0) } ?? .null, "new": .bool(opened.new)]
            case "close":
                guard let id = args["desk"]?.string, isDesk(id) else { throw KannaError("not_found", "no Desk \"\(args["desk"]?.string ?? "")\"") }
                closeDesk(id)
                return ["desk": .string(id)]
            default:
                throw KannaError("bad_request", "desk needs an action: pull, park, desk or close")
            }
        }
    }
}
