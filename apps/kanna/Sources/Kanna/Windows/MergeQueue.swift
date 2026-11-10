import AppKit

extension AppController {
    func loadQueue(_ controller: WindowController) {
        guard let root = controller.projectRoot else { return }
        let (machine, path) = MachineIDs.split(location: root)
        request("queue", ["action": "list", "repo": .string(path), "recent": 8], on: .some(machine)) { [weak self, weak controller] reply in
            guard let self, let controller, controller.projectRoot == root, let list = reply["queue"]?.array else { return }
            let entries = list.map(QueueEntry.init)
            let tester = controller.queuePage.wantedTerminal
            controller.queue = entries
            guard controller.shownPage == WindowController.pages[5] else { return }
            redraw(controller)
            if controller.queuePage.wantedTerminal != tester {
                refresh()
                redraw(controller)
            }
        }
        guard queueTimer == nil else { return }
        queueTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshQueues() }
        }
    }

    func loadQueueLog(_ controller: WindowController, _ entry: String) {
        guard let root = controller.projectRoot else { return }
        let (machine, _) = MachineIDs.split(location: root)
        request("queue", ["action": "log", "queue": .string(entry)], on: .some(machine)) { [weak controller] reply in
            controller?.queuePage.show(log: QueueLog(id: entry, note: reply["note"]?.string, text: reply["text"]?.string))
        }
    }

    func showQueue(_ controller: WindowController, at entry: String) {
        controller.page = WindowController.pages[5]
        controller.queuePage.pick(entry)
        loadQueue(controller)
        redraw(controller)
    }

    func queueChanged() {
        controllers.values.filter { $0.shownPage == WindowController.pages[5] }.forEach(loadQueue)
    }

    private func refreshQueues() {
        let showing = controllers.values.filter { $0.shownPage == WindowController.pages[5] }
        guard !showing.isEmpty else {
            queueTimer?.invalidate()
            queueTimer = nil
            return
        }
        showing.forEach(loadQueue)
    }
}
