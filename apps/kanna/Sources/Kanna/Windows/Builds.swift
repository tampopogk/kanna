import AppKit

extension AppController {
    func loadBuilds() {
        request("build.pool", [:]) { [weak self] reply in
            guard let self else { return }
            let macs = BuildMac.rows(from: reply, now: Date().timeIntervalSince1970)
            for controller in controllers.values {
                controller.builds = macs
                if controller.shownPage == WindowController.pages[2] { redraw(controller) }
            }
        }
        guard buildsTimer == nil else { return }
        buildsTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshBuilds() }
        }
    }

    func cancelBuild(_ job: String) {
        request("build.pool", ["job": .string(job)]) { [weak self] _ in self?.loadBuilds() }
    }

    private func refreshBuilds() {
        guard controllers.values.contains(where: { $0.shownPage == WindowController.pages[2] }) else {
            buildsTimer?.invalidate()
            buildsTimer = nil
            return
        }
        loadBuilds()
    }
}
