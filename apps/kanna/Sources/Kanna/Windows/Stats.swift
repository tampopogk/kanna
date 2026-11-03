import AppKit

extension AppController {
    func loadStats(_ controller: WindowController) {
        guard let root = controller.projectRoot else { return }
        let (machine, path) = MachineIDs.split(location: root)
        if controller.stats?.project != path { controller.stats = nil }
        request("stats", ["repo": .string(path)], on: .some(machine)) { [weak self, weak controller] reply in
            guard let self, let controller, controller.projectRoot == root, let report = StatsReport(reply) else { return }
            controller.stats = report
            if controller.shownPage == WindowController.pages[2] { redraw(controller) }
        }
        guard statsTimer == nil else { return }
        statsTimer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshStats() }
        }
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
