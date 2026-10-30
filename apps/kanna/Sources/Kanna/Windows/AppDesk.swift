import AppKit

extension AppController {
    static let app = "mac"

    func composed(_ view: JSON) -> JSON {
        guard case .object(var fields) = view, let workspaces = view["workspaces"]?.array else { return view }
        let mine = workspaces.filter { claimed($0, as: "shelf") }
        let planners = workspaces.filter { claimed($0, as: "planner") }
        let listed = Set(planners.compactMap { $0["id"]?.string })
        givenPlanners.subtract(listed)
        physical.keep(shelf: mine.compactMap { ws in ws["id"]?.string.map { ($0, ws["parkedAt"]?.number) } }, planners: listed.union(givenPlanners), present: Set(views.keys))
        let known = Set(physical.desks.compactMap(\.planner)).union(closingPlanners)
        for planner in planners where starting.isEmpty {
            guard let id = planner["id"]?.string, !known.contains(id) else { continue }
            var desk = physical.newDesk(project: planner["dock"]?["pane"]?.string.flatMap { pane in folders[pane].map { MachineIDs.location($0, on: MachineIDs.machine(of: pane)) } })
            desk.planner = id
            physical.setDesk(desk)
        }
        let byID = Dictionary(workspaces.compactMap { ws in ws["id"]?.string.map { ($0, ws) } }, uniquingKeysWith: { first, _ in first })
        let shown = Set(physical.desks.compactMap(\.shows))
        var windows: [JSON] = physical.desks.map { desk in deskWindow(desk, task: desk.shows.flatMap { byID[$0] }, planner: desk.planner.flatMap { byID[$0] }, active: desk.active.flatMap { byID[$0] }) }
        windows += (view["windows"]?.array ?? []).filter { window in window["workspace"]?.string.map(shown.contains) ?? true }
        fields["windows"] = .array(windows)
        fields["shelf"] = .array(physical.shelf.filter { !shown.contains($0.workspace) }.compactMap { entry in byID[entry.workspace].map { shelfEntry($0, parkedAt: entry.parkedAt) } })
        fields["held"] = .array(workspaces.filter { $0["where"]?.string == "backlog" }.flatMap { $0["panes"]?.array ?? [] })
        return .object(fields)
    }

    private func deskWindow(_ desk: DeskRecord, task: JSON?, planner: JSON?, active: JSON?) -> JSON {
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
        out["active"] = active?["dock"]?["pane"] ?? .null
        out["project"] = desk.project.map { .string($0) } ?? .null
        out["workspace"] = .null
        return .object(out)
    }

    private func shelfEntry(_ ws: JSON, parkedAt: Double) -> JSON {
        let dock = ws["dock"] ?? .null
        return [
            "task": dock["pane"] ?? .null, "workspace": ws["id"] ?? .null, "type": dock["type"] ?? "term", "parkedAt": .number(parkedAt),
            "edge": dock["edge"] ?? "left", "width": dock["width"] ?? .null, "name": dock["name"] ?? .null, "agent": ws["agent"] ?? .null,
            "tabs": .array((ws["tabs"]?.array ?? []).map { $0["title"] ?? .null }), "panes": ws["panes"] ?? [],
        ]
    }

    func isDesk(_ window: String) -> Bool { physical.desk(window) != nil }

    func address(_ window: String) -> [String: JSON] {
        guard let desk = physical.desk(window) else { return ["window": .string(window)] }
        return (desk.shows ?? desk.planner).map { ["workspace": .string($0)] } ?? ["window": "new"]
    }

    func workspace(ofTask pane: String) -> String? {
        published["workspaces"]?.array?.first { claimed($0, as: "shelf") && $0["dock"]?["pane"]?.string == pane }?["id"]?.string
    }

    func deskShowing(_ workspace: String) -> DeskRecord? { physical.desks.first { $0.shows == workspace } }

    @discardableResult
    func pullTask(_ task: String, into window: String?, front: Bool = true) -> (moved: Bool, desk: String)? {
        guard let workspace = workspace(ofTask: task) else { return nil }
        if let there = deskShowing(workspace) {
            show(window: there.id)
            return (false, there.id)
        }
        var desk = physical.desks.first { $0.active == workspace } ?? window.flatMap { physical.desk($0) } ?? (front ? frontDesk : nil) ?? physical.newDesk(project: nil)
        if let old = desk.shows ?? desk.active, old != workspace { physical.park(old) }
        desk.shows = workspace
        desk.active = workspace
        physical.setDesk(desk)
        show(window: desk.id)
        return (true, desk.id)
    }

    @discardableResult
    func park(desk id: String) -> String? {
        guard var desk = physical.desk(id), let workspace = desk.shows else { return nil }
        let task = published["workspaces"]?.array?.first { $0["id"]?.string == workspace }?["dock"]?["pane"]?.string
        physical.park(workspace)
        desk.shows = nil
        physical.setDesk(desk)
        show(window: id)
        return task
    }

    var frontDesk: DeskRecord? { frontmost(physical.desks) }

    func frontmost(_ desks: [DeskRecord]) -> DeskRecord? {
        if let desk = physical.order.lazy.compactMap({ id in desks.first { $0.id == id } }).first { return desk }
        let ordered = NSApp.orderedWindows.compactMap { window in controllers.first { $0.value.window === window }?.key }
        return ordered.lazy.compactMap { id in desks.first { $0.id == id } }.first ?? desks.last
    }

    func desk(of pane: String) -> DeskRecord? {
        guard let workspace = published["workspaces"]?.array?.first(where: { $0["panes"]?.array?.contains(.string(pane)) == true })?["id"]?.string else { return nil }
        return physical.desks.first { $0.shows == workspace || $0.planner == workspace }
    }

    func projectDesk(_ root: String?) -> DeskRecord? {
        guard let root else { return nil }
        return frontmost(physical.desks.filter { sameProject(controllers[$0.id]?.projectRoot, root) || sameProject($0.project.map(rootLocation), root) })
    }

    func show(window id: String) {
        let inFront = keyWindow?["id"]?.string == id
        physical.setFront(id)
        restate()
        refresh()
        if !inFront, let controller = controllers[id], !controller.window.isKeyWindow { present(controller.window) }
    }

    func front(_ args: JSON) -> String? {
        let workspace = args["workspace"]?["id"]?.string
        let shown = workspace.flatMap { ws in physical.desks.first { $0.shows == ws || $0.planner == ws }?.id }
        guard let id = shown ?? args["window"]?.string, window(id) != nil else { return nil }
        show(window: id)
        return id
    }

    func openDesk(project: String?, fresh: Bool) -> (desk: String, project: String?, new: Bool) {
        let root = project.map(rootLocation)
        if !fresh, let root { touchProject(root) }
        if !fresh, let found = projectDesk(root) {
            show(window: found.id)
            return (found.id, found.project.flatMap { sameProject(rootLocation($0), root) ? $0 : nil } ?? project ?? root, false)
        }
        let desk = physical.newDesk(project: project)
        show(window: desk.id)
        return (desk.id, project, true)
    }

    func closeDesk(_ id: String) {
        guard let desk = physical.desk(id) else { return }
        if let workspace = desk.shows { physical.park(workspace) }
        physical.closeDesk(id)
        physical.forget(window: id)
        if let planner = desk.planner {
            closingPlanners.insert(planner)
            let pane = published["workspaces"]?.array?.first { $0["id"]?.string == planner }?["dock"]?["pane"]?.string
            if let pane { request("close", ["task": .string(pane)]) { [weak self] _ in self?.closingPlanners.remove(planner) } }
        }
        restate()
        refresh()
    }

    func installDesk() {
        commands["app.front"] = { [weak self] args in ["front": self?.front(args).map { .string($0) } ?? .null] }
        commands["desk"] = { [weak self] args in
            guard let self else { throw KannaError("gone", "Kanna.app is quitting") }
            switch args["action"]?.string {
            case "pull":
                guard let task = args["task"]?.string else { throw KannaError("bad_request", "pull needs a task") }
                let desk = args["by"]?.string.flatMap { self.desk(of: $0) } ?? projectDesk(projectRoot(task))
                guard let pulled = pullTask(task, into: desk?.id, front: false) else { throw KannaError("not_found", "\(task) isn't on this app's Desk") }
                return ["task": .string(task), "moved": .bool(pulled.moved), "desk": .string(pulled.desk)]
            case "park":
                guard let task = args["task"]?.string else { throw KannaError("bad_request", "park needs a task") }
                guard let workspace = workspace(ofTask: task) else { throw KannaError("not_found", "\(task) isn't on this app's Desk") }
                guard let desk = deskShowing(workspace), park(desk: desk.id) != nil else { return ["task": .string(task), "parked": false] }
                return ["task": .string(task), "parked": true, "desk": .string(desk.id)]
            case "desk":
                let place = args["place"]
                let project = (place?["main"]?.string ?? args["project"]?.string).map { MachineIDs.location($0, on: args["machine"]?.string) }
                if let project, let identity = place?["project"]?.string { identities[rootLocation(project)] = identity }
                let opened = openDesk(project: project, fresh: args["new"]?.bool == true)
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
