import AppKit

extension AppController {
    func loadStats(_ controller: WindowController) {
        guard let root = controller.projectRoot else { return }
        let (machine, path) = MachineIDs.split(location: root)
        controller.statsPage.onOpen = { [weak self, weak controller] open in
            guard let self, let controller else { return }
            self.openFromStats(open, repo: path, machine: machine, window: controller.id)
        }
        controller.statsPage.onDays = { [weak self, weak controller] in
            guard let self, let controller else { return }
            loadStats(controller)
        }
        if controller.stats?.project != path { controller.stats = nil }
        request("stats", ["repo": .string(path), "days": .number(Double(controller.statsPage.runDays))], on: .some(machine)) { [weak self, weak controller] reply in
            guard let self, let controller, controller.projectRoot == root, let report = StatsReport(reply) else { return }
            controller.stats = report
            if controller.shownPage == WindowController.pages[3] { redraw(controller) }
        }
        guard statsTimer == nil else { return }
        statsTimer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshStats() }
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
        let showing = controllers.values.filter { $0.shownPage == WindowController.pages[3] }
        guard !showing.isEmpty else {
            statsTimer?.invalidate()
            statsTimer = nil
            return
        }
        showing.forEach(loadStats)
    }
}
