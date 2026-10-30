import AppKit
import GhosttyKit
import KannaMarkdown
import KannaPerf
import KannaStatus
import KannaWindows

@MainActor
final class AppController: NSObject, NSApplicationDelegate, NSMenuDelegate {
    var runtime: GhosttyRuntime!
    private var terminals: ExternalIOBackend!
    let commands = Commands()
    var daemon: DaemonLink?
    var links: Links?
    var views: [String: JSON] = [:]
    var shelves: [String: [JSON]] = [:]
    var deskMade: [String: [String: Int]] = [:]
    var paneSets: [String: Set<String>] = [:]
    var places: [String: JSON] = [:]
    var identities: [String: String] = [:]
    var lastSeen: [String: Date] = [:]
    private var server: SocketServer?
    private var signalSources: [DispatchSourceSignal] = []
    private var ownerSource: DispatchSourceProcess?
    let background = CommandLine.arguments.contains("--bg") || ProcessInfo.processInfo.environment["KANNA_BG"] == "1"
    private let echo = ProcessInfo.processInfo.environment["KANNA_ECHO"] == "1"
    static let debugging = ProcessInfo.processInfo.environment["KANNA_DEBUG"] == "1"
    let debugCommands = AppController.debugging
    private var wantsFirstTerminal = ProcessInfo.processInfo.environment["KANNA_NO_INITIAL_WINDOW"] == nil
    let contents = Contents()
    var panes: PaneViews!
    var controllers: [String: WindowController] = [:]
    private(set) var state: JSON = ["windows": []]
    private(set) var published: JSON = ["windows": []]
    private var plain: JSON = ["windows": []]
    private var restoring: [String]?
    var closingPlanners: Set<String> = []
    var givenPlanners: Set<String> = []
    var recentProjects: [RecentProject] = []
    var recentBy: [String: [RecentProject]] = [:]
    var recentMenu: NSMenu?
    let physical = Physical(path: AppController.physicalPath, fresh: AppController.fresh)
    private var focusing: String?
    private var focusCalls = 0
    private var rendering = false
    var paneMenu: NSMenu?
    var testingDrag = false
    var agentStatuses = AgentStatuses()
    var folders: [String: String] = [:]
    private var terminalIDs: [String: String] = [:]
    private var names: [String: String] = [:]
    var shelf: [JSON] = []
    var starting: Set<String> = []
    let toasts = ToastStack()
    var toastShowing: (pane: String, desk: String?)?

    static var daemonSocketPath: String {
        if let path = ProcessInfo.processInfo.environment["KANNA_SOCKET"], !path.isEmpty { return path }
        return instanceSocket(KannaBuild.defaultInstance)
    }

    static var physicalPath: String? {
        let environment = ProcessInfo.processInfo.environment
        if environment["KANNA_NO_STATE"] == "1" { return nil }
        let folder = environment["KANNA_STATE_DIR"].flatMap { $0.isEmpty ? nil : $0 } ?? (daemonSocketPath as NSString).deletingLastPathComponent
        return (folder as NSString).appendingPathComponent("app-state.json")
    }

    static var appKeyPath: String? {
        physicalPath.map { (($0 as NSString).deletingLastPathComponent as NSString).appendingPathComponent("app.key") }
    }

    static var fresh: Bool {
        CommandLine.arguments.contains("--fresh") || ProcessInfo.processInfo.environment["KANNA_FRESH"] == "1"
    }

    static var startFolder: String? {
        guard let folder = ProcessInfo.processInfo.environment["KANNA_START_DIR"], folder.hasPrefix("/") else { return nil }
        var isFolder: ObjCBool = false
        return FileManager.default.fileExists(atPath: folder, isDirectory: &isFolder) && isFolder.boolValue ? folder : nil
    }

    static var instanceName: String {
        KannaBuild.instance(ofFolder: ((daemonSocketPath as NSString).deletingLastPathComponent as NSString).lastPathComponent)
    }

    static func instanceSocket(_ name: String) -> String {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appendingPathComponent(KannaBuild.instanceFolder(name)).appendingPathComponent("kanna.sock").path
    }

    static var appSocketPath: String {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appendingPathComponent("kanna/kanna.sock").path
    }

    static func isAppSocket(_ path: String) -> Bool {
        (path as NSString).standardizingPath.lowercased() == (appSocketPath as NSString).standardizingPath.lowercased()
    }

    static var echoSocketPath: String {
        if let path = ProcessInfo.processInfo.environment["KANNA_APP_SOCKET"], !path.isEmpty { return path }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appendingPathComponent(KannaBuild.instanceFolder(KannaBuild.defaultInstance)).appendingPathComponent("kanna-app.sock").path
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        KannaBuild.badgeIcon()
        restoring = physical.order
        HangWatch.start { hang in FileHandle.standardError.write(Data("kanna: \(HangWatch.line(hang))\n".utf8)) }
        do {
            runtime = try GhosttyRuntime()
        } catch {
            fatal("kanna: \(error.localizedDescription)")
        }
        if echo {
            terminals = ExternalIOBackend(runtime: runtime, link: EchoLink())
            let server = SocketServer(path: Self.echoSocketPath) { [weak self] request in await self?.commands.handle(request) ?? nil }
            do {
                try server.start()
                self.server = server
            } catch let error as KannaError {
                fatal("kanna: \(error.message)")
            } catch {
                fatal("kanna: \(error)")
            }
        } else {
            if Self.isAppSocket(Self.daemonSocketPath) { fatal("kanna: \(Self.daemonSocketPath) belongs to Kanna.app; use another KANNA_SOCKET") }
            let daemon = DaemonLink(socketPath: Self.daemonSocketPath, commands: commands, starter: DaemonStarter(socketPath: Self.daemonSocketPath))
            let links = Links(local: daemon, commands: commands, keyPath: Self.appKeyPath)
            links.configure = { [weak self] link in self?.wire(link) }
            links.onGone = { [weak self] machine in self?.machineGone(machine) }
            links.onPeers = { [weak self] in self?.peersChanged() }
            wire(daemon)
            daemon.onQuit = {
                FileHandle.standardError.write(Data("kanna: the daemon is shutting down; quitting\n".utf8))
                NSApp.terminate(nil)
            }
            contents.onUpdate = { [weak self, weak links] pane, fields in
                if fields["state"] != nil { self?.panes.report(pane, state: fields["state"]?.string, error: fields["error"]?.string) }
                links?.link(for: pane)?.update(pane, fields)
            }
            contents.onRequest = { [weak self] cmd, args in self?.request(cmd, args) }
            contents.onNotice = { [weak links] cmd, args in links?.link(MachineIDs.machine(in: .object(args)))?.notice(cmd, args) }
            contents.onCall = { [weak self] cmd, args, reply in
                guard let self else { return reply(["ok": false, "error": ["message": "not connected to the daemon"]]) }
                route(cmd, args) { answer in DispatchQueue.main.async { reply(answer) } }
            }
            terminals = ExternalIOBackend(runtime: runtime, link: links)
            self.daemon = daemon
            self.links = links
        }
        panes = PaneViews(contents: contents, terminals: terminals)
        panes.terminalOf = { [weak self] pane in self?.terminalIDs[pane] }
        panes.onFocus = { [weak self] pane in self?.userFocused(pane) }
        panes.onDrag = { [weak self] pane in self?.dragPane(pane) }
        panes.contextMenu = { [weak self] in self?.paneMenu?.copy() as? NSMenu }
        contents.onFocus = { [weak self] pane in self?.userFocused(pane) }
        contents.textSize = { [weak self] pane in self?.physical.textSize(pane) }
        contents.onTextSize = { [weak self] pane, size in self?.physical.setTextSize(pane, size) }
        contents.agentStatus = { [weak self] pane in self?.agentStatuses.statuses[pane] }
        contents.contextMenu = { [weak self] in self?.paneMenu?.copy() as? NSMenu }
        runtime.onAction = { [weak self] view, action in
            guard let self, let id = panes.id(of: view) else { return false }
            return ghosttyAction(id, action)
        }
        runtime.onClose = { [weak self] view in
            guard let self, let id = panes.id(of: view) else { return }
            closePane(id)
        }
        installAppCommands()
        installCommands()
        installScreenshot()
        installAgentStatus()
        installDesk()
        installToasts()
        Darwin.signal(SIGALRM, SIG_DFL)
        for signal in [SIGTERM, SIGINT, SIGHUP] {
            Darwin.signal(signal) { _ in alarm(5) }
            let source = DispatchSource.makeSignalSource(signal: signal, queue: .main)
            source.setEventHandler { NSApp.terminate(nil) }
            source.resume()
            signalSources.append(source)
        }
        watchOwner()
        installMenu()
        if !background { NSApp.activate() }
        if echo {
            render(["windows": [["id": "win-1", "tabs": [["id": "tab-1", "title": "Tab 1", "panes": ["term-1"]]], "tab": "tab-1", "title": "Tab 1",
                                               "focused": "term-1", "root": ["pane": "term-1", "type": "term"]]]])
        }
        daemon?.start()
    }

    private func watchOwner() {
        guard let owner = ProcessInfo.processInfo.environment["KANNA_E2E_OWNER"].flatMap({ pid_t($0) }), owner > 0 else { return }
        if kill(owner, 0) != 0 && errno == ESRCH {
            FileHandle.standardError.write(Data("kanna: its owner (pid \(owner)) is gone; quitting\n".utf8))
            return ownerGone()
        }
        let source = DispatchSource.makeProcessSource(identifier: owner, eventMask: .exit, queue: .main)
        source.setEventHandler { [weak self] in
            FileHandle.standardError.write(Data("kanna: its owner (pid \(owner)) exited; quitting\n".utf8))
            self?.ownerGone()
        }
        source.resume()
        ownerSource = source
    }

    private func ownerGone() {
        guard let daemon else { return NSApp.terminate(nil) }
        daemon.quit()
    }

    func applicationWillTerminate(_ notification: Notification) {
        for id in controllers.keys { recordFrame(id) }
        physical.save()
        panes?.stopAll()
        links?.stop()
        daemon?.stop()
        server?.stop()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        openTerminalIfEmpty()
        return true
    }

    private func wire(_ link: DaemonLink) {
        let machine = link.machine
        link.onAttach = { [weak self] in self?.attached($0, on: machine) }
        link.onView = { [weak self] view in self?.viewArrived(view, on: machine) }
        link.onStart = { [weak self] pane in self?.startContent(pane) }
        link.onStop = { [weak self] pane in self?.contents.stop(pane) }
        link.onDiff = { [weak self] content, message in self?.contents.received(diff: message, content: content) }
        link.onFile = { [weak self] content, message in self?.contents.received(file: message, content: content) }
        link.onPane = { [weak self] pane in self?.learn(pane) }
        link.onExited = { [weak self] terminal, code in
            if let pane = self?.panes.pane(showing: terminal) { self?.panes.exited(pane, code: code) }
        }
        link.onSnapshot = { [weak self] terminal in
            if let pane = self?.panes.pane(showing: terminal) { self?.panes.restarted(pane) }
        }
        link.onStatus = { [weak self] pane, status in self?.statusChanged(pane, status) }
        link.onFolder = { [weak self] pane, cwd, place in self?.folderChanged(pane, cwd, place) }
        link.onShelf = { [weak self] tasks, desk in self?.shelfChanged(tasks, desk: desk, on: machine) }
        link.onToast = { [weak self] record in self?.toastArrived(record) }
        link.onLost = { [weak self] in self?.linkLost(machine) }
    }

    private func viewArrived(_ view: JSON, on machine: String?) {
        views[machine ?? ""] = view
        published = merged()
        restate()
        layoutChanged()
    }

    func merged() -> JSON {
        var windows: [JSON] = []
        var workspaces: [JSON] = []
        for machine in views.keys.sorted() {
            guard let view = views[machine] else { continue }
            let tag: JSON = machine.isEmpty ? .null : .string(machine)
            windows += (view["windows"]?.array ?? []).map { Self.tagged($0, tag) }
            workspaces += (view["workspaces"]?.array ?? []).map { Self.tagged($0, tag) }
        }
        return ["windows": .array(windows), "workspaces": .array(workspaces)]
    }

    private static func tagged(_ item: JSON, _ machine: JSON) -> JSON {
        guard case .object(var fields) = item else { return item }
        fields["machine"] = machine
        return .object(fields)
    }

    private func attached(_ hello: JSON, on machine: String?) {
        let panes = hello["panes"] ?? []
        if case .array(let list) = panes { learn(records: list) }
        if let view = hello["layout"] {
            views[machine ?? ""] = view
            published = merged()
            restate()
        }
        if case .array(let list) = panes {
            let ids = Set(list.compactMap { $0["id"]?.string })
            paneSets[machine ?? ""] = ids
            physical.keep(panes: ids, in: plain, on: machine)
        }
        if case .array(let list) = panes {
            for pane in list where pane["type"]?.string != "term" {
                guard let id = pane["id"]?.string else { continue }
                if contents.shows(id) { contents.resend(id) } else { contents.start(pane, restoring: true) }
            }
        }
        contents.subscribeDiffs(on: machine)
        contents.subscribeFiles(on: machine)
        loadRecent()
        layoutChanged()
        if machine == nil {
            links?.start()
            if wantsFirstTerminal {
                wantsFirstTerminal = false
                openTerminalIfEmpty()
            }
        } else {
            seen(machine)
            refresh()
        }
    }

    private func linkLost(_ machine: String?) {
        guard machine != nil else { return }
        seen(machine)
        refresh()
    }

    private func machineGone(_ machine: String) {
        views.removeValue(forKey: machine)
        shelves.removeValue(forKey: machine)
        deskMade.removeValue(forKey: machine)
        shelf = shelves.keys.sorted().flatMap { shelves[$0] ?? [] }
        published = merged()
        restate()
        layoutChanged()
    }

    func peersChanged() {
        refresh()
    }

    func claimName(_ machine: String?) -> String {
        machine == nil ? Self.app : links?.name ?? Self.app
    }

    func claimed(_ workspace: JSON, as place: String) -> Bool {
        workspace["where"]?[place]?.string == claimName(workspace["machine"]?.string)
    }

    var windows: [JSON] {
        guard case .array(let list)? = state["windows"] else { return [] }
        return list
    }

    func window(_ id: String?) -> JSON? { windows.first { $0["id"]?.string == id } }
    var keyWindow: JSON? { window(KeyWindow.resolve(appKit: appKitKey, front: physical.order, windows: windows.compactMap { $0["id"]?.string })) }
    var appKitKey: String? { NSApp.keyWindow.flatMap { key in controllers.first { $0.value.window === key }?.key } }
    var focusedPane: String? { keyWindow?["focused"]?.string }

    func windowOf(tab: String) -> JSON? {
        windows.first { window in
            guard case .array(let tabs)? = window["tabs"] else { return false }
            return tabs.contains { $0["id"]?.string == tab }
        }
    }

    func docked(_ pane: String) -> Bool {
        windows.contains { $0["dock"]?["pane"]?.string == pane }
    }

    func frozen(_ pane: String) -> Bool {
        windows.contains { $0["dock"]?["pane"]?.string == pane && $0["dock"]?["frozen"]?.bool == true }
    }

    func windowOf(pane: String) -> JSON? {
        windows.first { window in
            if window["dock"]?["pane"]?.string == pane { return true }
            guard case .array(let tabs)? = window["tabs"] else { return false }
            return tabs.contains { tab in
                guard case .array(let ids)? = tab["panes"] else { return false }
                return ids.contains(.string(pane))
            }
        }
    }

    func tabOf(pane: String) -> String? {
        for window in windows {
            guard case .array(let tabs)? = window["tabs"] else { continue }
            if let tab = tabs.first(where: { tab in
                guard case .array(let ids)? = tab["panes"] else { return false }
                return ids.contains(.string(pane))
            }) { return tab["id"]?.string }
        }
        return nil
    }

    var deskEntries: [JSON] {
        guard case .array(let list)? = state["deskTasks"] else { return [] }
        return list
    }

    var deskTasks: [DeskTask] {
        let tasks = deskEntries.compactMap { entry -> DeskTask? in
            guard let id = entry["task"]?.string else { return nil }
            return DeskTask(id: id, title: taskTitle(id, name: entry["name"]?.string), leftAt: entry["leftAt"]?.number, status: agentStatuses.statuses[id], detail: taskDetail(id), agent: entry["agent"]?.string,
                             place: placeText(id), offline: offline(MachineIDs.machine(of: id)), made: made(id))
        }
        return DeskOrder.byRecency(tasks) { $0.status == "needs-you" }
    }

    func taskTitle(_ id: String, name: String?) -> String {
        if let name { return name }
        if let title = panes[id]?.terminal?.title, !title.isEmpty { return title }
        return "New task"
    }

    private var placed: Set<String> {
        var ids = Set(runningOnShelf(in: nil).compactMap { $0["task"]?.string })
        if case .array(let held)? = state["held"] { ids.formUnion(held.compactMap(\.string)) }
        for entry in deskEntries {
            guard case .array(let list)? = entry["panes"] else { continue }
            ids.formUnion(list.compactMap(\.string))
        }
        for window in windows {
            if let pane = window["dock"]?["pane"]?.string { ids.insert(pane) }
            if let pane = window["lead"]?["pane"]?.string { ids.insert(pane) }
            guard case .array(let tabs)? = window["tabs"] else { continue }
            for tab in tabs {
                guard case .array(let list)? = tab["panes"] else { continue }
                ids.formUnion(list.compactMap(\.string))
            }
        }
        return ids
    }

    func openTerminalIfEmpty() {
        guard windows.isEmpty, daemon?.attached == true else { return }
        _ = openDesk(project: Self.startFolder, fresh: Self.startFolder == nil)
    }

    private func startContent(_ pane: JSON) {
        learn(pane)
        if let id = pane["id"]?.string { panes.restarted(id) }
        contents.start(pane)
        refresh()
    }

    private func learn(_ pane: JSON) {
        if learn(records: [pane]) { refresh() }
    }

    @discardableResult
    private func learn(records: [JSON]) -> Bool {
        var changed = false
        for pane in records {
            guard let id = pane["id"]?.string else { continue }
            names = names.filter { $0.value != id }
            if let name = pane["name"]?.string { names[name] = id }
            guard pane["type"]?.string == "term", let terminal = pane["terminal"]?.string, terminalIDs[id] != terminal else { continue }
            terminalIDs[id] = terminal
            changed = true
        }
        return changed
    }

    func layoutCommand(_ cmd: String, _ args: [String: JSON], reply: ((JSON) -> Void)? = nil) {
        request(cmd, args, reply: reply)
    }

    func paneID(named name: String) -> String? { names[name] }

    func request(_ cmd: String, _ args: [String: JSON], on machine: String?? = nil, reply: ((JSON) -> Void)? = nil, sent: (@Sendable () -> Void)? = nil) {
        route(cmd, args, on: machine, reply: { answer in
            if answer["ok"]?.bool != true {
                FileHandle.standardError.write(Data("kanna: \(cmd): \(answer["error"]?["message"]?.string ?? "failed")\n".utf8))
            }
            reply?(answer)
        }, sent: sent)
    }

    func route(_ cmd: String, _ args: [String: JSON], on given: String?? = nil, reply: ((JSON) -> Void)? = nil, sent: (@Sendable () -> Void)? = nil) {
        guard let links else { return }
        let machine = given ?? MachineIDs.machine(in: .object(args))
        guard let link = links.link(machine) else {
            reply?(["ok": false, "error": ["code": "offline", "message": .string("\(machine ?? "this machine") isn't connected")]])
            return
        }
        var args = args
        if machine != nil, cmd == "tasks", ["pick", "plan"].contains(args["action"]?.string), args["app"] == nil {
            args["app"] = .string(claimName(machine))
        }
        link.request(cmd, args, reply: reply, sent: sent)
    }

    func userFocused(_ pane: String) {
        guard !rendering else { return }
        focusing = pane
        focusCalls += 1
        restate()
        refresh()
        request("focus", ["pane": .string(pane)]) { [weak self] _ in
            guard let self else { return }
            focusCalls -= 1
            guard focusCalls == 0 else { return }
            focusing = nil
            restate()
            refresh()
        }
    }

    func restate() {
        let view = composed(published)
        plain = focusing.map { Self.view(view, focusing: $0) } ?? view
        state = physical.applied(plain)
    }

    func redraw(_ controller: WindowController) {
        guard let window = window(controller.id) else { return }
        rendering = true
        controller.render(window, panes)
        rendering = false
    }

    private static func view(_ view: JSON, focusing pane: String) -> JSON {
        guard case .object(var fields) = view, case .array(let windows)? = fields["windows"] else { return view }
        fields["windows"] = .array(windows.map { window in
            guard case .object(var shown) = window, WindowController.shownPanes(window).contains(pane) else { return window }
            shown["focused"] = .string(pane)
            return .object(shown)
        })
        return .object(fields)
    }

    func layoutChanged() {
        guard daemon != nil else { return }
        refresh()
        showToastPane()
        let known = placed
        for id in contents.ids where !known.contains(id) { contents.forget(id) }
        agentStatuses.keep(known)
    }

    func refresh() {
        guard daemon != nil else { return }
        Perf.measure(.render, "refresh") {
            rendering = true
            render(state)
            rendering = false
        }
    }

    private func render(_ view: JSON) {
        let span = Perf.begin(.render, "render")
        defer { Perf.end(span) }
        state = view
        defer {
            statusSeen()
            placeToasts()
        }
        let ids = Set(windows.compactMap { $0["id"]?.string })
        for (id, controller) in controllers where !ids.contains(id) {
            controller.window.orderOut(nil)
            controllers[id] = nil
        }
        panes.keep(placed)
        let tasks = deskTasks
        let rows = machineRows
        var held = Set<String>()
        for window in windows {
            guard let id = window["id"]?.string else { continue }
            let controller = controllers[id] ?? makeController(id, window)
            let docked = window["dock"]?["pane"]?.string
            let lead = window["lead"]?["pane"]?.string
            if let root = docked.flatMap(projectRoot) ?? lead.flatMap(projectRoot) {
                controller.projectRoot = root
            } else if controller.projectRoot == nil, let folder = window["project"]?.string {
                controller.projectRoot = rootLocation(folder)
            }
            if window["desk"]?.bool == true, let root = controller.projectRoot { deskMoved(id, to: root) }
            let mine = controller.projectRoot.map { root in tasks.filter { sameProject(projectRoot($0.id), root) } } ?? tasks
            controller.deskTasks = mine
            controller.active = docked == nil ? window["active"]?.string : nil
            let ours = Set(mine.map(\.id))
            let docks = window["desk"]?.bool == true
                ? (deskEntries.filter { $0["task"]?.string.map(ours.contains) == true } + runningOnShelf(in: controller.projectRoot)).filter { $0["task"]?.string.map(held.contains) == false }
                : []
            held.formUnion(docks.compactMap { $0["task"]?.string })
            controller.deskDocks = docks
            controller.shelf = shelfTasks(in: controller.projectRoot)
            controller.running = runningTasks(in: controller.projectRoot)
            controller.project = controller.projectRoot.map(displayName) ?? projectName(mine.map(\.id))
            controller.taskTitle = docked.map { taskTitle($0, name: window["dock"]?["name"]?.string) }
            controller.headerText = docked.map(headerText) ?? leadHeader(window, controller.projectRoot)
            controller.machines = rows
            controller.stale = staleText(docked ?? lead)
            controller.headerTip = (docked ?? lead).map(placeText)
            controller.render(window, panes)
            if wantsPlanner(window) { openPlanner(in: id) }
        }
        if views[""] != nil, let order = restoring {
            restoring = nil
            for id in order.reversed() {
                if let controller = controllers[id] { present(controller.window) }
            }
        }
    }

    private func makeController(_ id: String, _ initial: JSON) -> WindowController {
        let previous = controllers.values.map(\.window).max { $0.orderedIndex > $1.orderedIndex }
        let controller = WindowController(id: id, cascadeFrom: previous)
        if let saved = initial["frame"], let x = saved["x"]?.number, let y = saved["y"]?.number, let w = saved["w"]?.number, let h = saved["h"]?.number {
            let frame = NSRect(x: x, y: y, width: w, height: h)
            if NSScreen.screens.contains(where: { $0.visibleFrame.intersects(frame) }) { controller.window.setFrame(frame, display: false) }
        }
        controller.onFrameChange = { [weak self] in
            self?.recordFrame(id)
        }
        controller.relayout = { [weak self, weak controller] in
            guard let self, let controller else { return }
            redraw(controller)
        }
        controller.onDivider = { [weak self] divider, share in
            self?.layoutCommand("resize", ["tab": .string(divider.tab), "path": .array(divider.path.map { .number(Double($0)) }), "index": .number(Double(divider.index)), "share": .number(share)])
        }
        controller.onDockWidth = { [weak self] pane, width in
            guard let self else { return }
            physical.setDock(pane, width: width, in: plain)
            restate()
            refresh()
        }
        controller.tabBar.onSelect = { [weak self, weak controller] tab in
            if controller?.page != nil {
                controller?.page = nil
                self?.refresh()
            }
            self?.layoutCommand("focus", ["tab": .string(tab)])
        }
        controller.tabBar.onPage = { [weak self, weak controller] page in
            guard let self, let controller else { return }
            controller.page = page
            if page == WindowController.pages[1] { links?.refresh() }
            redraw(controller)
        }
        controller.tabBar.onCycle = { [weak self] step in self?.cycleTab(step) }
        controller.tabBar.onClose = { [weak self] tab in self?.closeTab(tab) }
        controller.tabBar.onNew = { [weak self] in self?.newTab(in: id) }
        controller.tabBar.onDrag = { [weak self] tab in self?.dragTab(tab) }
        controller.tabBar.onMoveToNewWindow = { [weak self] tab in self?.layoutCommand("move-tab", ["tab": .string(tab), "window": "new"]) }
        controller.tabBar.onEditEnded = { [weak self, weak controller] in
            guard let self, let controller else { return }
            redraw(controller)
        }
        controller.tabBar.onRename = { [weak self] tab, title in self?.layoutCommand("rename-tab", ["tab": .string(tab), "title": .string(title)]) }
        controller.onCloseRequest = { [weak self] in self?.close(window: id) }
        controller.deskStrip.onActivate = { [weak self] task in
            Clicks.activate(task)
            self?.activateTask(task, into: id)
        }
        controller.agentHeader.onDeskView = { [weak self] in self?.showDeskView(desk: id) }
        controller.deskStrip.onDesk = { [weak self] in self?.showDeskView(desk: id) }
        controller.agentHeader.onNew = { [weak self] in self?.newTab(in: id) }
        controller.deskView.onActivate = { [weak self] task in
            Clicks.activate(task)
            self?.activateTask(task, into: id)
        }
        controller.deskView.onStart = { [weak self] task in self?.startTask(task, from: id) }
        controller.deskView.onLeaveSearch = { [weak self, weak controller] in
            guard let self, let controller, let lead = controller.leadShown, let view = panes.keyView(lead) else { return }
            controller.window.makeFirstResponder(view)
        }
        controller.onBecomeKey = { [weak self, weak controller] in
            guard let self, let controller else { return }
            if controller.window.isKeyWindow { physical.setFront(id) }
            if let window = window(id) { controller.render(window, panes) }
            placeToasts()
        }
        controllers[id] = controller
        recordFrame(id)
        present(controller.window)
        return controller
    }

    func recordFrame(_ id: String) {
        guard let rect = controllers[id]?.window.frame else { return }
        physical.setFrame(id, rect)
    }

    func present(_ window: NSWindow) {
        if let id = controllers.first(where: { $0.value.window === window })?.key { physical.setFront(id) }
        if NSApp.isActive || !background {
            return window.makeKeyAndOrderFront(nil)
        }
        window.makeKey()
        let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        let front = windows.first { ($0[kCGWindowOwnerPID as String] as? pid_t) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
        if let number = front?[kCGWindowNumber as String] as? Int, pid != ProcessInfo.processInfo.processIdentifier {
            window.order(.below, relativeTo: number)
        } else {
            window.orderFront(nil)
        }
    }

    func fatal(_ message: String) -> Never {
        FileHandle.standardError.write(Data((message + "\n").utf8))
        exit(1)
    }
}
