import AppKit
import KannaWindows

extension AppController {
    static let app = "mac"

    func composed(_ view: JSON) -> JSON {
        guard case .object(var fields) = view, let workspaces = view["workspaces"]?.array else { return view }
        let mine = workspaces.filter { claimed($0, as: "desk") }
        let planners = workspaces.filter { claimed($0, as: "planner") }
        let listed = Set(planners.compactMap { $0["id"]?.string })
        givenPlanners.subtract(listed)
        physical.keep(deskTasks: mine.compactMap { ws in ws["id"]?.string.map { ($0, ws["leftAt"]?.number) } }, planners: listed.union(givenPlanners), present: Set(views.keys))
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
        fields["deskTasks"] = .array(physical.deskTasks.filter { !shown.contains($0.workspace) }.compactMap { entry in byID[entry.workspace].map { deskEntry($0, leftAt: entry.leftAt) } })
        fields["held"] = .array(workspaces.filter { $0["where"]?.string == "shelf" }.flatMap { $0["panes"]?.array ?? [] })
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

    private func deskEntry(_ ws: JSON, leftAt: Double) -> JSON {
        let dock = ws["dock"] ?? .null
        return [
            "task": dock["pane"] ?? .null, "workspace": ws["id"] ?? .null, "type": dock["type"] ?? "term", "leftAt": .number(leftAt),
            "edge": dock["edge"] ?? "left", "width": dock["width"] ?? .null, "name": dock["name"] ?? .null, "agent": ws["agent"] ?? .null,
            "tabs": .array((ws["tabs"]?.array ?? []).map { $0["title"] ?? .null }), "panes": ws["panes"] ?? [],
        ]
    }

    func isDesk(_ window: String) -> Bool { physical.desk(window) != nil }

    func publishDesks() {
        let local = { (id: String?) -> JSON in id.flatMap { MachineIDs.machine(of: $0) == nil ? JSON.string($0) : nil } ?? .null }
        let desks: JSON = .array(physical.desks.map { ["id": .string($0.id), "planner": local($0.planner), "shows": local($0.shows), "project": $0.project.map { .string($0) } ?? .null] })
        guard desks != sentDesks, links?.link(nil) != nil else { return }
        sentDesks = desks
        route("desks", ["desks": desks], on: .some(nil))
    }

    func address(_ window: String) -> [String: JSON] {
        guard let desk = physical.desk(window) else { return ["window": .string(window)] }
        return (desk.shows ?? desk.planner).map { ["workspace": .string($0)] } ?? ["window": "new"]
    }

    func workspace(ofTask pane: String) -> String? {
        published["workspaces"]?.array?.first { claimed($0, as: "desk") && $0["dock"]?["pane"]?.string == pane }?["id"]?.string
    }

    func deskShowing(_ workspace: String) -> DeskRecord? { physical.desks.first { $0.shows == workspace } }

    @discardableResult
    func activateTask(_ task: String, into window: String?, front: Bool = true, origin: Origin = .app, by: String? = nil) -> (moved: Bool, desk: String)? {
        guard let workspace = workspace(ofTask: task) else { return nil }
        if let there = deskShowing(workspace) {
            show(window: there.id, origin: origin)
            return (false, there.id)
        }
        var desk = physical.desks.first { $0.active == workspace } ?? deskFor(task: task, given: window, front: front)
        if let old = desk.shows ?? desk.active, old != workspace { physical.leave(old) }
        let changed = desk.active != workspace
        desk.shows = workspace
        desk.active = workspace
        physical.setDesk(desk)
        if changed {
            var args: [String: JSON] = ["action": "activated", "task": .string(task)]
            if let by { args["by"] = .string(by) }
            request("tasks", args)
        }
        show(window: desk.id, origin: origin)
        return (true, desk.id)
    }

    @discardableResult
    func showDeskView(desk id: String, origin: Origin = .app) -> String? {
        guard var desk = physical.desk(id), let workspace = desk.shows else { return nil }
        let task = published["workspaces"]?.array?.first { $0["id"]?.string == workspace }?["dock"]?["pane"]?.string
        physical.leave(workspace)
        desk.shows = nil
        physical.setDesk(desk)
        show(window: id, origin: origin)
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

    func deskProject(_ desk: DeskRecord) -> String? { desk.project.map(rootLocation) }

    func projectDesk(_ root: String?) -> DeskRecord? {
        guard let root else { return nil }
        return frontmost(physical.desks.filter { sameProject(deskProject($0), root) })
    }

    private func deskFor(task: String, given window: String?, front: Bool) -> DeskRecord {
        let given = window.flatMap { physical.desk($0) }
        guard let root = projectRoot(task) else { return given ?? (front ? frontDesk : nil) ?? physical.newDesk(project: nil) }
        if let given, sameProject(deskProject(given), root) { return given }
        return projectDesk(root) ?? physical.newDesk(project: root)
    }

    func show(window id: String, origin: Origin = .app) {
        let inFront = keyWindow?["id"]?.string == id
        let placement = placement(origin)
        if placement.recorded { physical.setFront(id) }
        restate()
        refresh()
        if !inFront, let controller = controllers[id], !controller.window.isKeyWindow { present(controller.window, origin) }
    }

    func origin(_ args: JSON) -> Origin { args["user"]?.bool == true ? .user : .agent }

    func front(_ args: JSON) -> [String: JSON] {
        let workspace = args["workspace"]?["id"]?.string
        let shown = workspace.flatMap { ws in physical.desks.first { $0.shows == ws || $0.planner == ws }?.id }
        guard let id = shown ?? args["window"]?.string else { return ["front": nil] }
        let origin = origin(args)
        if window(id) == nil {
            guard args["new"]?.bool == true else { return ["front": nil] }
            awaiting[id] = origin
        } else {
            show(window: id, origin: origin)
        }
        return placement(origin).recorded ? ["front": .string(id)] : ["front": .string(id), "behind": true]
    }

    func openDesk(project: String?, fresh: Bool, origin: Origin = .app) -> (desk: String, project: String?, new: Bool) {
        let root = project.map(rootLocation)
        if !fresh, let root { touchProject(root) }
        if !fresh, let found = projectDesk(root) {
            show(window: found.id, origin: origin)
            return (found.id, found.project ?? root, false)
        }
        let desk = physical.newDesk(project: project)
        show(window: desk.id, origin: origin)
        return (desk.id, project, true)
    }

    func closeDesk(_ id: String) {
        guard let desk = physical.desk(id) else { return }
        if let workspace = desk.shows { physical.leave(workspace) }
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
        NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.releaseWaiting() }
        }
        commands["app.front"] = { [weak self] args in self?.front(args) ?? ["front": nil] }
        commands["desk"] = { [weak self] args in
            guard let self else { throw KannaError("gone", "Kanna.app is quitting") }
            switch args["action"]?.string {
            case "activate":
                guard let task = args["task"]?.string else { throw KannaError("bad_request", "activate needs a task") }
                guard let activated = activateTask(task, into: args["by"]?.string.flatMap { self.desk(of: $0) }?.id, front: false, origin: origin(args), by: args["by"]?.string) else { throw KannaError("not_found", "\(task) isn't on this app's Desk") }
                return ["task": .string(task), "moved": .bool(activated.moved), "desk": .string(activated.desk)]
            case "park":
                guard let task = args["task"]?.string else { throw KannaError("bad_request", "park needs a task") }
                guard let workspace = workspace(ofTask: task) else { throw KannaError("not_found", "\(task) isn't on this app's Desk") }
                guard let desk = deskShowing(workspace), showDeskView(desk: desk.id, origin: origin(args)) != nil else { return ["task": .string(task), "parked": false] }
                return ["task": .string(task), "parked": true, "desk": .string(desk.id)]
            case "desk":
                let place = args["place"]
                let project = (place?["main"]?.string ?? args["project"]?.string).map { MachineIDs.location($0, on: args["machine"]?.string) }
                if let project, let identity = place?["project"]?.string { identities[rootLocation(project)] = identity }
                let opened = openDesk(project: project, fresh: args["new"]?.bool == true, origin: origin(args))
                return ["desk": .string(opened.desk), "project": opened.project.map { .string($0) } ?? .null, "new": .bool(opened.new)]
            case "close":
                guard let id = args["desk"]?.string, isDesk(id) else { throw KannaError("not_found", "no Desk \"\(args["desk"]?.string ?? "")\"") }
                closeDesk(id)
                return ["desk": .string(id)]
            default:
                throw KannaError("bad_request", "desk needs an action: activate, park, desk or close")
            }
        }
    }
}
