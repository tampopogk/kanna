import AppKit
import KannaMachines
import KannaUpdates
import Sparkle

@MainActor
final class Updates: NSObject, SPUUserDriver, SPUUpdaterDelegate, NSMenuItemValidation {
    private static let pendingKey = "KannaUpdatePending"
    private static let installingKey = "KannaUpdateInstalling"
    private static let toastID = "update"

    let menuItem = NSMenuItem(title: "Check for Updates…", action: #selector(menuPressed), keyEquivalent: "")
    private weak var app: AppController?
    private var state: UpdateState
    private var updater: SPUUpdater?
    private var answer: ((SPUUserUpdateChoice) -> Void)?
    private var cancelDownload: (() -> Void)?
    private var checkingByHand = false
    private var received: UInt64 = 0
    private var total: UInt64 = 0
    private var stale = false

    static var enabled: Bool {
        Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil && AppController.instanceName == KannaBuild.defaultInstance
    }

    init(_ app: AppController) {
        self.app = app
        let pending = Self.load(Self.pendingKey)
        stale = pending.map { Self.isCurrent($0) } ?? false
        state = UpdateState(pending: stale ? nil : pending)
        super.init()
        menuItem.target = self
        if stale { UserDefaults.standard.removeObject(forKey: Self.pendingKey) }
        refresh()
    }

    func start() {
        let updater = SPUUpdater(hostBundle: .main, applicationBundle: .main, userDriver: self, delegate: self)
        do {
            try updater.start()
        } catch {
            FileHandle.standardError.write(Data("kanna: the updater didn't start: \(error.localizedDescription)\n".utf8))
            menuItem.isHidden = true
            return
        }
        self.updater = updater
        announceInstalled()
        if state.pending != nil { updater.checkForUpdatesInBackground() }
    }

    private static func isCurrent(_ offer: UpdateOffer) -> Bool {
        let running = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        return running.compare(offer.build, options: .numeric) != .orderedAscending
    }

    private static func load(_ key: String) -> UpdateOffer? {
        UserDefaults.standard.data(forKey: key).flatMap { try? JSONDecoder().decode(UpdateOffer.self, from: $0) }
    }

    private static func save(_ offer: UpdateOffer?, _ key: String) {
        if let offer, let data = try? JSONEncoder().encode(offer) {
            UserDefaults.standard.set(data, forKey: key)
        } else {
            UserDefaults.standard.removeObject(forKey: key)
        }
    }

    private func refresh() {
        menuItem.title = state.menuTitle
        Self.save(state.pending, Self.pendingKey)
    }

    private func announceInstalled() {
        guard let offer = Self.load(Self.installingKey) else { return }
        Self.save(nil, Self.installingKey)
        guard Self.isCurrent(offer) else { return }
        show(Toast(id: Self.toastID, title: UpdateText.updated(offer.version, app: KannaBuild.name), detail: UpdateText.carriedOn, symbol: "checkmark.circle", tint: .systemGreen,
                   link: notesLink(offer)))
    }

    private func show(_ toast: Toast) {
        app?.showLocal(toast)
    }

    private func closeToast() {
        app?.closeLocal(Self.toastID)
    }

    private func notesLink(_ offer: UpdateOffer) -> ToastAction {
        ToastAction(title: "What's new") { NSWorkspace.shared.open(UpdateText.notes(offer.version)) }
    }

    private func machinesBehind(_ offer: UpdateOffer) -> String? {
        guard let app else { return nil }
        var machines: [String: Int?] = [:]
        for name in app.links?.remotes.keys.sorted() ?? [] {
            let refused = app.links?.failure(of: name)
            machines[name] = refused?["code"]?.string == "other_protocol" ? refused?["protocol"]?.number.map(Int.init) : Protocols.mine
        }
        return UpdateText.fallsBehind(UpdateText.machinesBehind(speaks: offer.speaks, mine: Protocols.mine, machines: machines))
    }

    private func ask(_ offer: UpdateOffer) {
        show(Toast(id: Self.toastID, title: UpdateText.available(offer.version, app: KannaBuild.name), detail: UpdateText.keepsRunning, symbol: "arrow.up.circle", tint: .controlAccentColor, sticky: true,
                   warning: machinesBehind(offer), link: notesLink(offer), actions: [
                       ToastAction(title: "Later") { [weak self] in self?.later() },
                       ToastAction(title: "Install and Relaunch", primary: true) { [weak self] in self?.accept() },
                   ]))
    }

    private func showDownloading(_ offer: UpdateOffer) {
        show(Toast(id: Self.toastID, title: UpdateText.downloading(offer.version, app: KannaBuild.name), detail: UpdateText.size(received: received, total: total), symbol: "arrow.up.circle", tint: .controlAccentColor, sticky: true,
                   progress: progress, actions: [ToastAction(title: "Cancel") { [weak self] in self?.cancel() }]))
    }

    private var progress: Double { total > 0 ? min(1, Double(received) / Double(total)) : 0 }

    private func later() {
        state.later()
        refresh()
        closeToast()
        answer?(.dismiss)
        answer = nil
    }

    private func accept() {
        state.accepted()
        refresh()
        received = 0
        total = 0
        if let offer = state.offer { showDownloading(offer) }
        answer?(.install)
        answer = nil
    }

    private func cancel() {
        state.stopped()
        refresh()
        closeToast()
        cancelDownload?()
        cancelDownload = nil
    }

    private func fail(_ error: Error) {
        let offer = state.offer
        state.stopped()
        refresh()
        show(Toast(id: Self.toastID, title: UpdateText.failed(offer?.version, app: KannaBuild.name), detail: error.localizedDescription, symbol: "exclamationmark.triangle.fill", tint: .systemRed, sticky: true,
                   actions: [ToastAction(title: "Close") { [weak self] in self?.closeToast() }]))
    }

    @objc private func menuPressed() {
        if case .asking = state.phase { return accept() }
        if state.wantsInstallFromMenu { state.requestInstall() }
        checkingByHand = true
        updater?.checkForUpdates()
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        guard state.menuEnabled else { return false }
        if case .asking = state.phase { return true }
        return updater?.canCheckForUpdates == true
    }

    private func offer(_ item: SUAppcastItem) -> UpdateOffer {
        let speaks = (item.propertiesDictionary["kanna:protocol"] as? String).flatMap { Int($0) }
        return UpdateOffer(version: item.displayVersionString, build: item.versionString, speaks: speaks)
    }

    nonisolated func updaterWillRelaunchApplication(_ updater: SPUUpdater) {
        MainActor.assumeIsolated { Self.save(state.offer, Self.installingKey) }
    }

    func show(_ request: SPUUpdatePermissionRequest, reply: @escaping (SUUpdatePermissionResponse) -> Void) {
        reply(SUUpdatePermissionResponse(automaticUpdateChecks: true, sendSystemProfile: false))
    }

    func showUserInitiatedUpdateCheck(cancellation: @escaping () -> Void) {}

    func showUpdateFound(with appcastItem: SUAppcastItem, state update: SPUUserUpdateState, reply: @escaping (SPUUserUpdateChoice) -> Void) {
        let found = offer(appcastItem)
        let byHand = checkingByHand || update.userInitiated
        checkingByHand = false
        switch state.found(found, byHand: byHand) {
        case .ask:
            answer = reply
            ask(found)
        case .dismiss:
            reply(.dismiss)
        case .install:
            received = 0
            total = 0
            showDownloading(found)
            reply(.install)
        }
        refresh()
    }

    func showUpdateReleaseNotes(with downloadData: SPUDownloadData) {}

    func showUpdateReleaseNotesFailedToDownloadWithError(_ error: any Error) {}

    func showUpdateNotFoundWithError(_ error: any Error, acknowledgement: @escaping () -> Void) {
        let byHand = checkingByHand
        checkingByHand = false
        if (error as NSError).code == Int(Sparkle.SUError.noUpdateError.rawValue) {
            state.notFound()
            refresh()
            if byHand {
                let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
                show(Toast(id: Self.toastID, title: "\(KannaBuild.name) is up to date", detail: "\(version) is the newest version.", symbol: "checkmark.circle", tint: .systemGreen))
            }
        } else if byHand {
            fail(error)
        }
        acknowledgement()
    }

    func showUpdaterError(_ error: any Error, acknowledgement: @escaping () -> Void) {
        let byHand = checkingByHand
        checkingByHand = false
        if byHand || state.pending != nil && !state.wantsInstallFromMenu { fail(error) }
        acknowledgement()
    }

    func showDownloadInitiated(cancellation: @escaping () -> Void) {
        cancelDownload = cancellation
    }

    func showDownloadDidReceiveExpectedContentLength(_ expectedContentLength: UInt64) {
        total = expectedContentLength
    }

    func showDownloadDidReceiveData(ofLength length: UInt64) {
        received += length
        app?.updateLocal(Self.toastID, detail: UpdateText.size(received: received, total: total), progress: progress)
    }

    func showDownloadDidStartExtractingUpdate() {
        state.installing()
        refresh()
        cancelDownload = nil
        if let offer = state.offer {
            show(Toast(id: Self.toastID, title: UpdateText.installing(offer.version, app: KannaBuild.name), detail: "Your terminals keep running.", symbol: "arrow.up.circle", tint: .controlAccentColor, sticky: true))
        }
    }

    func showExtractionReceivedProgress(_ progress: Double) {}

    func showReady(toInstallAndRelaunch reply: @escaping (SPUUserUpdateChoice) -> Void) {
        reply(.install)
    }

    func showInstallingUpdate(withApplicationTerminated applicationTerminated: Bool, retryTerminatingApplication: @escaping () -> Void) {}

    func showUpdateInstalledAndRelaunched(_ relaunched: Bool, acknowledgement: @escaping () -> Void) {
        acknowledgement()
    }

    func showUpdateInFocus() {}

    func dismissUpdateInstallation() {
        cancelDownload = nil
        answer = nil
    }
}
