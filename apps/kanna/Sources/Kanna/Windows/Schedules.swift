import AppKit

extension AppController {
    func loadSchedules(_ controller: WindowController) {
        guard let root = controller.projectRoot else { return }
        let (machine, path) = MachineIDs.split(location: root)
        controller.schedulesPage.onAction = { [weak self, weak controller] action, schedule in
            self?.request("schedule", ["action": .string(action), "schedule": .string(schedule)], on: .some(machine)) { [weak self, weak controller] _ in
                guard let self, let controller else { return }
                loadSchedules(controller)
            }
        }
        request("schedule", ["action": "list", "repo": .string(path)], on: .some(machine)) { [weak self, weak controller] reply in
            guard let self, let controller, controller.projectRoot == root, let list = reply["schedules"]?.array else { return }
            controller.schedules = list.map(ScheduleRow.init)
            controller.schedulesRepo = reply["repo"]?.string
            if controller.shownPage == WindowController.pages[4] { redraw(controller) }
        }
    }

    func schedulesChanged(on machine: String?) {
        for controller in controllers.values where controller.shownPage == WindowController.pages[4] {
            guard let root = controller.projectRoot, MachineIDs.split(location: root).0 == machine else { continue }
            loadSchedules(controller)
        }
    }
}
