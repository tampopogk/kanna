import AppKit

extension AppController {
    func loadStats(_ controller: WindowController) {
        guard let root = controller.projectRoot else { return }
        let (machine, path) = MachineIDs.split(location: root)
        controller.statsPage.onOpen = { [weak self, weak controller] open in
            guard let self, let controller else { return }
            self.openFromStats(open, repo: path, machine: machine, window: controller.id)
        }
        if controller.statsRoot != root {
            controller.statsRoot = root
            controller.stats = nil
        }
        request("stats", ["repo": .string(path)], on: .some(machine)) { [weak self, weak controller] reply in
            guard let self, let controller, controller.projectRoot == root, let report = StatsReport(reply) else { return }
            controller.stats = report.keeping(controller.stats)
            if controller.shownPage == WindowController.pages[2] { redraw(controller) }
        }
        guard statsTimer == nil else { return }
        statsTimer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshStats() }
        }
    }

    func statsChanged(_ repo: String, on machine: String?) {
        for controller in controllers.values where controller.shownPage == WindowController.pages[2] {
            guard let root = controller.projectRoot else { continue }
            let (owner, path) = MachineIDs.split(location: root)
            if owner == machine, (controller.stats?.project ?? path) == repo || path == repo { loadStats(controller) }
        }
    }

    func openFromStats(_ open: JSON, repo: String, machine: String?, window: String) {
        var args: [String: JSON] = ["window": .string(window), "tab": true, "user": true]
        if let file = open["path"]?.string {
            args["type"] = "code"
            args["path"] = .string(file.hasPrefix("/") ? file : "\(repo)/\(file)")
            if let line = open["line"]?.number { args["line"] = .number(line) }
        } else if let commit = open["commit"]?.string {
            args["type"] = "diff"
            args["path"] = .string(repo)
            args["args"] = .array([.string("\(commit)^1"), .string(commit)])
        } else {
            return
        }
        request("open", args, on: .some(machine))
    }

    private func refreshStats() {
        let showing = controllers.values.filter { $0.shownPage == WindowController.pages[2] }
        guard !showing.isEmpty else {
            statsTimer?.invalidate()
            statsTimer = nil
            return
        }
        showing.forEach(loadStats)
    }
}
