import AppKit

struct RecentProject: Equatable {
    var path: String
    var label: String
}

extension AppController {
    func recentMenuItem() -> NSMenuItem {
        let item = NSMenuItem(title: "Open Recent", action: nil, keyEquivalent: "")
        let menu = NSMenu(title: item.title)
        menu.delegate = self
        item.submenu = menu
        recentMenu = menu
        return item
    }

    func fillRecent(_ menu: NSMenu) {
        menu.removeAllItems()
        for project in recentProjects {
            let item = menu.addItem(withTitle: project.label, action: #selector(openRecent(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = project.path
            item.toolTip = MachineIDs.split(location: project.path).machine == nil ? tilde(project.path) : project.path
        }
        if !recentProjects.isEmpty { menu.addItem(.separator()) }
        let clear = menu.addItem(withTitle: "Clear Menu", action: recentProjects.isEmpty ? nil : #selector(clearRecent), keyEquivalent: "")
        clear.target = self
    }

    func loadRecent() {
        for link in links?.all ?? [] where link.attached {
            let machine = link.machine
            request("projects", ["action": "list"], on: .some(machine)) { [weak self] reply in self?.recentLoaded(reply, on: machine) }
        }
    }

    private func recentLoaded(_ reply: JSON, on machine: String?) {
        guard reply["ok"]?.bool == true else { return }
        recentBy[machine ?? ""] = (reply["projects"]?.array ?? []).compactMap { p in
            guard let path = p["path"]?.string else { return nil }
            let location = MachineIDs.location(path, on: machine)
            if let project = p["project"]?.string { identities[location] = project }
            return RecentProject(path: location, label: p["label"]?.string ?? (machine == nil ? tilde(path) : path))
        }
        var projects: [RecentProject] = []
        for key in recentBy.keys.sorted() {
            projects += (recentBy[key] ?? []).filter { project in !projects.contains { sameProject($0.path, project.path) } }
        }
        guard projects != recentProjects else { return }
        recentProjects = projects
        if let menu = recentMenu { fillRecent(menu) }
    }

    func deskMoved(_ id: String, to root: String) {
        guard var desk = physical.desk(id), desk.project.map(rootLocation) != root else { return }
        desk.project = root
        physical.setDesk(desk)
        touchProject(root)
    }

    func touchProject(_ location: String) {
        let (machine, folder) = MachineIDs.split(location: location)
        request("projects", ["action": "touch", "folder": .string(folder)], on: .some(machine)) { [weak self] _ in self?.loadRecent() }
    }

    @objc func openRecent(_ item: NSMenuItem) {
        guard let path = item.representedObject as? String else { return }
        _ = openDesk(project: path, fresh: false)
    }

    @objc func clearRecent() {
        for link in links?.all ?? [] where link.attached {
            let machine = link.machine
            request("projects", ["action": "clear"], on: .some(machine)) { [weak self] reply in self?.recentLoaded(reply, on: machine) }
        }
    }
}
