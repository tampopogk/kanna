import AppKit
import KannaStatus

extension AppController {
    func installAgentStatus() {
        NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.statusSeen() }
        }
        guard debugCommands else { return }
        commands["debug.status"] = { [weak self] _ in
            guard let self else { return [:] }
            var panes: [String: JSON] = [:]
            for (pane, status) in agentStatuses.statuses {
                panes[pane] = ["status": .string(status), "unread": .bool(agentStatuses.unread.contains(pane))]
            }
            return ["panes": .object(panes), "looking": .array(lookingAt.sorted().map { .string($0) })]
        }
    }

    var lookingAt: Set<String> {
        guard NSApp.isActive || background, let window = keyWindow else { return [] }
        var shown = Set<String>()
        if let zoomed = window["zoomed"]?.string {
            shown.insert(zoomed)
        } else {
            shown.formUnion(WindowController.paneIDs(window["root"] ?? .null))
        }
        if let dock = window["dock"]?["pane"]?.string { shown.insert(dock) }
        return shown
    }

    func statusChanged(_ pane: String, _ status: String?) {
        agentStatuses.set(pane, status, looking: lookingAt.contains(pane))
        contents.agentStatusChanged(pane, status)
        if isTask(pane) { refresh() }
    }

    func folderChanged(_ pane: String, _ cwd: String, _ place: JSON?) {
        if let place, let main = place["main"]?.string, let project = place["project"]?.string {
            identities[MachineIDs.location(MachineIDs.machine(of: pane) == nil ? canonical(main) : main, on: MachineIDs.machine(of: pane))] = project
        }
        let placeChanged = places[pane] != place
        places[pane] = place
        guard folders[pane] != cwd || placeChanged else { return }
        folders[pane] = cwd
        if isTask(pane) || windows.contains(where: { $0["lead"]?["pane"]?.string == pane }) { refresh() }
    }

    func statusSeen() {
        agentStatuses.looked(at: lookingAt)
    }
}
