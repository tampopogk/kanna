import AppKit
import GhosttyKit
import KannaMarkdown
import KannaStatus

@MainActor
final class AppController: NSObject, NSApplicationDelegate, NSMenuDelegate {
    var runtime: GhosttyRuntime!
    private var terminals: ExternalIOBackend!
    let commands = Commands()
    var daemon: DaemonLink?
    private var server: SocketServer?
    private var signalSources: [DispatchSourceSignal] = []
    let background = CommandLine.arguments.contains("--bg") || ProcessInfo.processInfo.environment["KANNA_BG"] == "1"
    private let echo = ProcessInfo.processInfo.environment["KANNA_ECHO"] == "1"
    static let debugging = ProcessInfo.processInfo.environment["KANNA_DEBUG"] == "1"
    let debugCommands = AppController.debugging
    private var wantsFirstTerminal = ProcessInfo.processInfo.environment["KANNA_NO_INITIAL_WINDOW"] == nil
    let contents = Contents()
    var panes: PaneViews!
    var controllers: [String: WindowController] = [:]
    private(set) var state: JSON = ["key": nil, "windows": []]
    private(set) var published: JSON = ["key": nil, "windows": []]
    private var plain: JSON = ["key": nil, "windows": []]
    var localKey: String?
    var closingPlanners: Set<String> = []
    var givenPlanners: Set<String> = []
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
    var backlog: [JSON] = []
    var starting: Set<String> = []

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
            daemon.onAttach = { [weak self] in self?.attached($0) }
            daemon.onView = { [weak self] view in
                guard let self else { return }
                if view["key"] != published["key"], view["key"]?.string != nil { localKey = nil }
                published = view
                restate()
                layoutChanged()
            }
            daemon.onStart = { [weak self] pane in self?.startContent(pane) }
            daemon.onStop = { [weak self] pane in self?.contents.stop(pane) }
            daemon.onDiff = { [weak self] content, message in self?.contents.received(diff: message, content: content) }
            daemon.onPane = { [weak self] pane in self?.learn(pane) }
            daemon.onExited = { [weak self] terminal, code in
                if let pane = self?.panes.pane(showing: terminal) { self?.panes.exited(pane, code: code) }
            }
            daemon.onSnapshot = { [weak self] terminal in
                if let pane = self?.panes.pane(showing: terminal) { self?.panes.restarted(pane) }
            }
            contents.onUpdate = { [weak self, weak daemon] pane, fields in
                if fields["state"] != nil { self?.panes.report(pane, state: fields["state"]?.string, error: fields["error"]?.string) }
                daemon?.update(pane, fields)
            }
            contents.onRequest = { [weak daemon] cmd, args in daemon?.request(cmd, args) }
            contents.onNotice = { [weak daemon] cmd, args in daemon?.notice(cmd, args) }
            contents.onCall = { [weak daemon] cmd, args, reply in
                guard let daemon else { return reply(["ok": false, "error": ["message": "not connected to the daemon"]]) }
                daemon.request(cmd, args) { answer in DispatchQueue.main.async { reply(answer) } }
            }
            terminals = ExternalIOBackend(runtime: runtime, link: daemon)
            self.daemon = daemon
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
        for signal in [SIGTERM, SIGINT, SIGHUP] {
            Darwin.signal(signal, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: signal, queue: .main)
            source.setEventHandler { NSApp.terminate(nil) }
            source.resume()
            signalSources.append(source)
        }
        installMenu()
        if !background { NSApp.activate() }
        if echo {
            render(["key": "win-1", "windows": [["id": "win-1", "tabs": [["id": "tab-1", "title": "Tab 1", "panes": ["term-1"]]], "tab": "tab-1", "title": "Tab 1",
                                               "focused": "term-1", "root": ["pane": "term-1", "type": "term"]]]])
        }
        daemon?.start()
    }

    func applicationWillTerminate(_ notification: Notification) {
        for id in controllers.keys { recordFrame(id) }
        physical.save()
        panes?.stopAll()
        daemon?.stop()
        server?.stop()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        openTerminalIfEmpty()
        return true
    }

    private func attached(_ hello: JSON) {
        let panes = hello["panes"] ?? []
        if case .array(let list) = panes { learn(records: list) }
        if let view = hello["layout"] {
            published = view
            restate()
        }
        if case .array(let list) = panes { physical.keep(panes: Set(list.compactMap { $0["id"]?.string }), in: plain) }
        if case .array(let list) = panes {
            for pane in list where pane["type"]?.string != "term" {
                guard let id = pane["id"]?.string else { continue }
                if contents.shows(id) { contents.resend(id) } else { contents.start(pane, restoring: true) }
            }
        }
        contents.subscribeDiffs()
        layoutChanged()
        if wantsFirstTerminal {
            wantsFirstTerminal = false
            openTerminalIfEmpty()
        }
    }

    var windows: [JSON] {
        guard case .array(let list)? = state["windows"] else { return [] }
        return list
    }

    func window(_ id: String?) -> JSON? { windows.first { $0["id"]?.string == id } }
    var keyWindow: JSON? { window(state["key"]?.string) ?? windows.last }
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

    var shelfEntries: [JSON] {
        guard case .array(let list)? = state["shelf"] else { return [] }
        return list
    }

    var shelfTasks: [ShelfTask] {
        let tasks = shelfEntries.compactMap { entry -> ShelfTask? in
            guard let id = entry["task"]?.string else { return nil }
            return ShelfTask(id: id, title: taskTitle(id, name: entry["name"]?.string), parkedAt: entry["parkedAt"]?.number, status: agentStatuses.statuses[id], detail: taskDetail(id))
        }
        let waiting: [ShelfTask] = tasks.filter { $0.status == "needs-you" }
        let rest: [ShelfTask] = tasks.filter { $0.status != "needs-you" }
        return waiting + rest
    }

    func taskTitle(_ id: String, name: String?) -> String {
        if let name { return name }
        if let title = panes[id]?.terminal?.title, !title.isEmpty { return title }
        return "New task"
    }

    private var placed: Set<String> {
        var ids = Set(runningBacklog(in: nil).compactMap { $0["task"]?.string })
        if case .array(let held)? = state["held"] { ids.formUnion(held.compactMap(\.string)) }
        for entry in shelfEntries {
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

    func request(_ cmd: String, _ args: [String: JSON], reply: ((JSON) -> Void)? = nil) {
        guard let daemon else { return }
        daemon.request(cmd, args) { answer in
            if answer["ok"]?.bool != true {
                FileHandle.standardError.write(Data("kanna: \(cmd): \(answer["error"]?["message"]?.string ?? "failed")\n".utf8))
            }
            reply?(answer)
        }
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
            if focusCalls == 0 { focusing = nil }
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
        let known = placed
        for id in contents.ids where !known.contains(id) { contents.forget(id) }
        agentStatuses.keep(known)
    }

    func refresh() {
        guard daemon != nil else { return }
        rendering = true
        render(state)
        rendering = false
    }

    private func render(_ view: JSON) {
        state = view
        defer { statusSeen() }
        let ids = Set(windows.compactMap { $0["id"]?.string })
        for (id, controller) in controllers where !ids.contains(id) {
            controller.window.orderOut(nil)
            controllers[id] = nil
        }
        panes.keep(placed)
        let tasks = shelfTasks
        var held = Set<String>()
        for window in windows {
            guard let id = window["id"]?.string else { continue }
            let controller = controllers[id] ?? makeController(id, window)
            let docked = window["dock"]?["pane"]?.string
            let lead = window["lead"]?["pane"]?.string
            if let root = docked.flatMap(projectRoot) ?? lead.flatMap(projectRoot) {
                controller.projectRoot = root
            } else if controller.projectRoot == nil, let folder = window["project"]?.string {
                controller.projectRoot = canonical(GitPlace.of(folder)?.main ?? folder)
            }
            let mine = controller.projectRoot.map { root in tasks.filter { projectRoot($0.id) == root } } ?? tasks
            controller.shelf = mine
            let ours = Set(mine.map(\.id))
            let docks = window["desk"]?.bool == true
                ? (shelfEntries.filter { $0["task"]?.string.map(ours.contains) == true } + runningBacklog(in: controller.projectRoot)).filter { $0["task"]?.string.map(held.contains) == false }
                : []
            held.formUnion(docks.compactMap { $0["task"]?.string })
            controller.shelfDocks = docks
            controller.backlog = backlogTasks(in: controller.projectRoot)
            controller.running = runningTasks(in: controller.projectRoot)
            controller.project = controller.projectRoot.map(tilde) ?? projectName(mine.map(\.id))
            controller.taskTitle = docked.map { taskTitle($0, name: window["dock"]?["name"]?.string) }
            controller.headerText = docked.map(headerText) ?? leadHeader(window, controller.projectRoot)
            controller.render(window, panes)
            if wantsPlanner(window) { openPlanner(in: id) }
        }
        if let key = state["key"]?.string, let controller = controllers[key], !controller.window.isKeyWindow {
            present(controller.window)
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
        controller.tabBar.onSelect = { [weak self] tab in self?.layoutCommand("focus", ["tab": .string(tab)]) }
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
        controller.shelfStrip.onPull = { [weak self] task in self?.pullTask(task, into: id) }
        controller.agentHeader.onPark = { [weak self] in self?.park(desk: id) }
        controller.shelfPage.onPull = { [weak self] task in self?.pullTask(task, into: id) }
        controller.shelfPage.onStart = { [weak self] task in self?.startTask(task, from: id) }
        controller.shelfPage.onLeaveSearch = { [weak self, weak controller] in
            guard let self, let controller, let lead = controller.leadShown, let view = panes.keyView(lead) else { return }
            controller.window.makeFirstResponder(view)
        }
        controller.onBecomeKey = { [weak self, weak controller] in
            guard let self, let controller else { return }
            if controller.window.isKeyWindow, state["key"]?.string != id {
                if isDesk(id) {
                    localKey = id
                    restate()
                } else {
                    localKey = nil
                    layoutCommand("focus", ["window": .string(id)])
                }
            }
            if let window = window(id) { controller.render(window, panes) }
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
