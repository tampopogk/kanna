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
            item.toolTip = tilde(project.path)
        }
        if !recentProjects.isEmpty { menu.addItem(.separator()) }
        let clear = menu.addItem(withTitle: "Clear Menu", action: recentProjects.isEmpty ? nil : #selector(clearRecent), keyEquivalent: "")
        clear.target = self
    }

    func loadRecent() {
        request("projects", ["action": "list"]) { [weak self] reply in self?.recentLoaded(reply) }
    }

    private func recentLoaded(_ reply: JSON) {
        guard reply["ok"]?.bool == true else { return }
        let projects = (reply["projects"]?.array ?? []).compactMap { p in p["path"]?.string.map { RecentProject(path: $0, label: p["label"]?.string ?? tilde($0)) } }
        guard projects != recentProjects else { return }
        recentProjects = projects
        if let menu = recentMenu { fillRecent(menu) }
    }

    func deskMoved(_ id: String, to root: String) {
        guard var desk = physical.desk(id), desk.project.map({ canonical(GitPlace.of($0)?.main ?? $0) }) != root else { return }
        desk.project = root
        physical.setDesk(desk)
        touchProject(root)
    }

    func touchProject(_ folder: String) {
        request("projects", ["action": "touch", "folder": .string(folder)]) { [weak self] _ in self?.loadRecent() }
    }

    @objc func openRecent(_ item: NSMenuItem) {
        guard let path = item.representedObject as? String else { return }
        _ = openDesk(project: path, fresh: false)
    }

    @objc func clearRecent() {
        request("projects", ["action": "clear"]) { [weak self] reply in self?.recentLoaded(reply) }
    }
}
