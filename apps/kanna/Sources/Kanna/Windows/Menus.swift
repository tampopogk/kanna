import AppKit
import GhosttyKit
import KannaOverview
import KannaWindows
import UniformTypeIdentifiers

extension AppController {
    func installMenu() {
        let runtime = runtime!
        let main = NSMenu()
        func fill(_ submenu: NSMenu, _ items: [(String, Selector, String?, Shortcut?)?]) {
            for entry in items {
                guard let (label, action, ghostty, fallback) = entry else {
                    submenu.addItem(.separator())
                    continue
                }
                let shortcut = ghostty.flatMap { runtime.shortcut(for: $0) } ?? fallback
                let menuItem = submenu.addItem(withTitle: label, action: action, keyEquivalent: shortcut?.key ?? "")
                menuItem.keyEquivalentModifierMask = shortcut?.modifiers ?? []
                menuItem.target = self
            }
        }
        func menu(_ title: String, _ items: [(String, Selector, String?, Shortcut?)?]) -> NSMenu {
            let item = NSMenuItem()
            let submenu = NSMenu(title: title)
            fill(submenu, items)
            item.submenu = submenu
            main.addItem(item)
            return submenu
        }
        let app = menu("Kanna", [])
        app.addItem(withTitle: "About \(KannaBuild.name)", action: #selector(showAbout), keyEquivalent: "").target = self
        app.addItem(.separator())
        if Self.instanceName != "default" {
            app.addItem(withTitle: "Instance: \(Self.instanceName)", action: nil, keyEquivalent: "")
            app.addItem(.separator())
        }
        if let updates {
            app.addItem(updates.menuItem)
            app.addItem(.separator())
        }
        let accounts = app.addItem(withTitle: "Agent Accounts", action: nil, keyEquivalent: "")
        accounts.submenu = NSMenu(title: accounts.title)
        for (label, action) in [("Show Accounts", #selector(showAccounts)), ("Add Claude Subscription…", #selector(addClaudeSubscription)), ("Add Anthropic API Key…", #selector(addAnthropicKey)), ("Add Codex (ChatGPT) Account…", #selector(addCodexAccount)), ("Add OpenAI API Key…", #selector(addOpenAIKey)), ("Remove Claude Account", #selector(removeClaudeAccount)), ("Remove Codex Account", #selector(removeCodexAccount))] {
            if label.hasPrefix("Add Claude") || label.hasPrefix("Remove Claude") { accounts.submenu?.addItem(.separator()) }
            accounts.submenu?.addItem(withTitle: label, action: action, keyEquivalent: "").target = self
        }
        app.addItem(.separator())
        app.addItem(withTitle: "Quit \(KannaBuild.name)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        let file = menu("File", [("New Desk", #selector(newDeskWindow), nil, .cmd("n"))])
        file.addItem(recentMenuItem())
        let edit = NSMenu(title: "Edit")
        let editItems: [(String, String, String?, Shortcut)?] = [
            ("Undo", "undo:", nil, .cmd("z")), ("Redo", "redo:", nil, .shiftCmd("z")), nil,
            ("Cut", "cut:", nil, .cmd("x")), ("Copy", "copy:", "copy_to_clipboard", .cmd("c")),
            ("Paste", "paste:", "paste_from_clipboard", .cmd("v")), ("Select All", "selectAll:", "select_all", .cmd("a")),
        ]
        for entry in editItems {
            guard let (label, action, ghostty, fallback) = entry else {
                edit.addItem(.separator())
                continue
            }
            let shortcut = ghostty.flatMap { runtime.shortcut(for: $0) } ?? fallback
            let item = edit.addItem(withTitle: label, action: Selector(action), keyEquivalent: shortcut.key)
            item.keyEquivalentModifierMask = shortcut.modifiers
        }
        edit.addItem(.separator())
        for (label, finder, shortcut) in [("Find…", NSTextFinder.Action.showFindInterface, Shortcut.cmd("f")), ("Find Next", .nextMatch, .cmd("g")), ("Find Previous", .previousMatch, .shiftCmd("g"))] {
            let item = edit.addItem(withTitle: label, action: #selector(NSResponder.performTextFinderAction(_:)), keyEquivalent: shortcut.key)
            item.keyEquivalentModifierMask = shortcut.modifiers
            item.tag = finder.rawValue
        }
        let editItem = NSMenuItem()
        editItem.submenu = edit
        main.addItem(editItem)
        let newPane = NSMenu(title: "New Pane")
        fill(newPane, [
            ("New Web Pane Right", #selector(webRight), nil, nil),
            ("New Web Pane Below", #selector(webDown), nil, nil),
            ("New iOS Pane Right", #selector(iosRight), nil, nil),
            ("New iOS Pane Below", #selector(iosDown), nil, nil),
            ("New Markdown Pane Right…", #selector(markdownRight), nil, nil),
            ("New Markdown Pane Below…", #selector(markdownDown), nil, nil),
        ])
        let pane = menu("Pane", [
            nil,
            ("Split Right", #selector(splitRight), "new_split:right", .cmd("d")),
            ("Split Down", #selector(splitDown), "new_split:down", .shiftCmd("d")),
            ("Next Pane", #selector(nextPane), "goto_split:next", .cmd("]")),
            ("Previous Pane", #selector(previousPane), "goto_split:previous", .cmd("[")),
            ("Zoom", #selector(toggleZoom), "toggle_split_zoom", .shiftCmd("\r")),
            ("Move Pane to New Window", #selector(movePaneToNewWindow), nil, nil),
            ("Close Pane", #selector(closeFocusedPane), "close_surface", .cmd("w")),
            nil,
            ("Open URL…", #selector(openURL), nil, .cmd("l")),
            ("Back", #selector(goBack), nil, nil),
            ("Forward", #selector(goForward), nil, nil),
            ("Reload", #selector(reloadPane), nil, .cmd("r")),
            ("Comments", #selector(toggleComments), nil, nil),
            ("Home", #selector(pressHome), nil, .shiftCmd("h")),
        ])
        pane.insertItem(withTitle: "New Pane", action: nil, keyEquivalent: "", at: 0).submenu = newPane
        paneMenu = pane
        _ = menu("View", [
            ("New Tab", #selector(newTabInKeyWindow), "new_tab", .cmd("t")),
            ("Next Tab", #selector(nextTab), "next_tab", .shiftCmd("]")),
            ("Previous Tab", #selector(previousTab), "previous_tab", .shiftCmd("[")),
            nil,
            ("Zoom In", #selector(zoomIn), "increase_font_size:1", .cmd("=")),
            ("Zoom Out", #selector(zoomOut), "decrease_font_size:1", .cmd("-")),
            ("Actual Size", #selector(actualSize), "reset_font_size", .cmd("0")),
            nil,
            ("Overview", #selector(toggleOverview), nil, nil),
        ])
        _ = menu("Task", [
            ("Show Desk", #selector(toggleDeskView), nil, .cmd(".")),
            ("Move to Shelf", #selector(deferTask), nil, nil),
            nil,
            ("Select Previous Task", #selector(deskPrevious), nil, .shiftCmd("p")),
            ("Select Next Task", #selector(deskNext), nil, .shiftCmd("n")),
            nil,
            ("Move to Desk", #selector(deskEnter), nil, nil),
            ("Start", #selector(deskRun), nil, nil),
            nil,
            ("Desk Task 1", #selector(activateDeskTask1), "goto_tab:1", .cmd("1")),
            ("Desk Task 2", #selector(activateDeskTask2), "goto_tab:2", .cmd("2")),
        ])
        let windowMenu = menu("Window", [
            ("New Window", #selector(newWindow), nil, nil),
            ("Close Window", #selector(closeWindow), "close_window", .shiftCmd("w")),
            ("Move Tab to New Window", #selector(moveTabToNewWindow), nil, nil),
            ("Free Window", #selector(freeWindow), nil, nil),
            nil,
            ("Next Window", #selector(nextWindow), nil, .cmd("`")),
            ("Previous Window", #selector(previousWindow), nil, .cmd("~")),
            nil,
        ])
        if let at = windowMenu.items.firstIndex(where: { $0.title == "Free Window" }) {
            let assign = windowMenu.insertItem(withTitle: "Assign Window to Task", action: nil, keyEquivalent: "", at: at)
            assign.submenu = NSMenu(title: assign.title)
            assign.submenu?.delegate = self
        }
        NSApp.mainMenu = main
        NSApp.windowsMenu = windowMenu
    }

    var startedTasks: [(pane: String, title: String)] {
        let root = keyWindow?["id"]?.string.flatMap { controllers[$0]?.projectRoot }
        let desks = windows.compactMap { window in window["dock"]?["pane"]?.string.map { ($0, window["dock"]?["name"]?.string) } }
        let onDesk = deskEntries.compactMap { entry in entry["task"]?.string.map { ($0, entry["name"]?.string) } }
        let held = shelf.filter { $0["running"]?.bool == true }.compactMap { entry in entry["held"]?.string.map { ($0, entry["name"]?.string) } }
        return (desks + onDesk + held).filter { root == nil || sameProject(projectRoot($0.0), root) }.map { (pane: $0.0, title: taskTitle($0.0, name: $0.1)) }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        if menu === recentMenu {
            fillRecent(menu)
            return loadRecent()
        }
        menu.removeAllItems()
        let window = keyWindow
        let reason: String? = window == nil ? "No window" : window?["desk"]?.bool == true ? "A Desk isn't a task's" : window?["lead"]?["pane"]?.string != nil ? "This window has its own agent" : nil
        let tasks = reason == nil ? startedTasks : []
        if tasks.isEmpty {
            menu.addItem(withTitle: reason ?? "No started tasks", action: nil, keyEquivalent: "").isEnabled = false
            return
        }
        let current = window?["workspace"]?.string
        for task in tasks {
            let item = menu.addItem(withTitle: task.title, action: #selector(assignWindow(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = task.pane
            item.state = workspaceOf(task.pane) == current && current != nil ? .on : .off
        }
    }

    private func workspaceOf(_ pane: String) -> String? {
        deskEntries.first { $0["task"]?.string == pane }?["workspace"]?.string ?? windows.first { $0["dock"]?["pane"]?.string == pane }?["shows"]?.string
    }

    @objc func assignWindow(_ item: NSMenuItem) {
        guard let window = keyWindow?["id"]?.string, let task = item.representedObject as? String else { return }
        layoutCommand("window", ["action": "assign", "window": .string(window), "workspace": .string(task)])
    }

    @objc func freeWindow() {
        guard let window = keyWindow?["id"]?.string else { return }
        layoutCommand("window", ["action": "free", "window": .string(window)])
    }

    func choose(menuItem title: String) throws {
        for menu in allMenus {
            menu.update()
            guard let at = menu.items.firstIndex(where: { $0.title == title }) else { continue }
            guard menu.items[at].isEnabled else { throw KannaError("disabled", "\(title) is disabled") }
            menu.performActionForItem(at: at)
            return
        }
        throw KannaError("not_found", "no menu item \"\(title)\"")
    }

    private var allMenus: [NSMenu] {
        func within(_ menu: NSMenu) -> [NSMenu] {
            menu.delegate?.menuNeedsUpdate?(menu)
            menu.update()
            return [menu] + menu.items.compactMap(\.submenu).flatMap(within)
        }
        return (NSApp.mainMenu?.items ?? []).compactMap(\.submenu).flatMap(within)
    }

    var menuItems: [JSON] {
        allMenus.flatMap { menu in
            menu.items.filter { !$0.isSeparatorItem }.map { item -> JSON in
                let shortcut = Shortcut(key: item.keyEquivalent, modifiers: item.keyEquivalentModifierMask)
                return ["menu": .string(menu.title), "item": .string(item.title), "shortcut": .string(item.keyEquivalent.isEmpty ? "" : shortcut.label)]
            }
        }
    }

    @objc func newDeskWindow() { newDesk() }

    @objc func splitRight() { split("right") }
    @objc func splitDown() { split("down") }
    @objc func webRight() { split("right", type: "web") }
    @objc func webDown() { split("down", type: "web") }
    @objc func iosRight() { split("right", type: "ios") }
    @objc func iosDown() { split("down", type: "ios") }
    @objc func markdownRight() { openMarkdown("right") }
    @objc func markdownDown() { openMarkdown("down") }

    private func split(_ direction: String, type: String = "term") {
        guard !showingDeskView else { return NSSound.beep() }
        var args: [String: JSON] = switch type {
        case "web": ["type": "web", "url": "http://localhost:3000"]
        case "ios": ["type": "ios", "app": "com.apple.Preferences", "wait": false]
        default: ["type": "term"]
        }
        if let window = keyWindow?["id"]?.string {
            args.merge(address(window)) { _, new in new }
            args["split"] = .string(direction)
            if type == "term" { args["cwd"] = repo(of: window) }
        }
        request("open", args) { [weak self] reply in
            guard type == "web", let id = reply["pane"]?["id"]?.string else { return }
            self?.contents.web(id)?.editURL()
        }
    }

    private func openMarkdown(_ direction: String) {
        guard !showingDeskView else { return NSSound.beep() }
        let panel = NSOpenPanel()
        panel.allowedContentTypes = ["md", "markdown", "mdown"].compactMap { UTType(filenameExtension: $0) }
        panel.allowsOtherFileTypes = true
        panel.message = "Choose a markdown file to show"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        var args: [String: JSON] = ["type": "md", "path": .string(url.path)]
        if let window = keyWindow?["id"]?.string {
            args.merge(address(window)) { _, new in new }
            args["split"] = .string(direction)
        }
        request("open", args)
    }

    @objc func pressHome() { focusedPane.flatMap { contents.ios($0)?.screen }?.pressHome() }

    @objc func openURL() {
        guard let pane = focusedPane else { return }
        contents.web(pane)?.editURL()
    }

    @objc func zoomIn() { zoomFocused(1, ghostty: "increase_font_size:1") }
    @objc func zoomOut() { zoomFocused(-1, ghostty: "decrease_font_size:1") }
    @objc func actualSize() { zoomFocused(0, ghostty: "reset_font_size") }

    private func zoomFocused(_ step: Int, ghostty action: String) {
        guard let pane = focusedPane else { return }
        if let markdown = contents.markdown(pane) {
            markdown.zoom(by: step)
        } else if let code = contents.code(pane) {
            code.zoom(by: step)
        } else if let diff = contents.diff(pane) {
            diff.zoom(by: step)
        } else if let log = contents.log(pane) {
            log.zoom(by: step)
        } else if let web = contents.web(pane) {
            web.zoom(by: step)
        } else if let surface = panes.terminal(pane)?.surface {
            _ = ghostty_surface_binding_action(surface, action, UInt(action.utf8.count))
        }
    }

    @objc func validateMenuItem(_ item: NSMenuItem) -> Bool {
        let pane = focusedPane ?? ""
        switch item.action {
        case #selector(zoomIn), #selector(zoomOut), #selector(actualSize): return panes.terminal(pane) != nil || contents.markdown(pane) != nil || contents.web(pane) != nil || contents.code(pane) != nil
        case #selector(pressHome): return contents.ios(pane)?.screen != nil
        case #selector(toggleOverview):
            item.state = OverviewSetting.enabled ? .on : .off
            return true
        case #selector(openURL): return contents.web(pane) != nil
        case #selector(reloadPane): return contents.web(pane) != nil || contents.diff(pane) != nil || contents.log(pane) != nil
        case #selector(toggleComments):
            item.state = commentsOn(pane) ? .on : .off
            return contents.web(pane) != nil || contents.markdown(pane) != nil
        case #selector(goBack), #selector(goForward): return contents.web(pane) != nil || contents.markdown(pane) != nil || contents.code(pane) != nil
        case #selector(nextPane), #selector(previousPane), #selector(toggleZoom):
            return focusedPane != nil
        case #selector(movePaneToNewWindow): return focusedPane.map { !frozen($0) } ?? false
        case #selector(closeFocusedPane): return focusedPane.map { !docked($0) } ?? false
        case #selector(nextTab), #selector(previousTab), #selector(moveTabToNewWindow), #selector(closeWindow): return keyWindow != nil
        case #selector(nextWindow), #selector(previousWindow): return cyclable.count > 1
        case #selector(freeWindow): return keyWindow?["workspace"]?.string != nil
        case #selector(newTabInKeyWindow), #selector(splitRight), #selector(splitDown), #selector(webRight), #selector(webDown),
             #selector(iosRight), #selector(iosDown), #selector(markdownRight), #selector(markdownDown): return !showingDeskView
        case #selector(toggleDeskView):
            item.title = keyWindow?["dock"]?["pane"] != nil || deskBack == nil ? "Show Desk" : "Show Active Task"
            return keyWindow?["dock"]?["pane"] != nil || deskBack != nil
        case #selector(deferTask): return keyWindow?["dock"]?["pane"] != nil || deskSelection?.onDesk == true
        case #selector(deskPrevious), #selector(deskNext): return deskView?.canSelect == true
        case #selector(deskEnter):
            let task = deskSelection
            item.title = task.map { $0.onDesk ? ($0.active ? "Show Active Task" : "Make Active") : "Move to Desk" } ?? "Move to Desk"
            return task != nil
        case #selector(deskRun):
            let task = deskSelection
            item.title = task?.running == true ? "Stop" : "Start"
            return task.map { !$0.onDesk } ?? false
        case #selector(activateDeskTask1): return keyedDeskTask(1) != nil
        case #selector(activateDeskTask2): return keyedDeskTask(2) != nil
        default: return true
        }
    }

    @objc func nextPane() { cyclePane(1) }
    @objc func previousPane() { cyclePane(-1) }

    private func cyclePane(_ step: Int) {
        guard let window = keyWindow else { return }
        let ids = WindowController.shownPanes(window)
        guard !ids.isEmpty else { return }
        let at = window["focused"]?.string.flatMap { ids.firstIndex(of: $0) } ?? 0
        let pane = ids[((at + step) % ids.count + ids.count) % ids.count]
        userFocused(pane)
        giveKeyboard(pane)
    }

    @objc func toggleOverview() { setOverview(!OverviewSetting.enabled) }

    func setOverview(_ on: Bool) {
        OverviewSetting.enabled = on
        physical.setOverview(on)
    }

    @objc func toggleZoom() {
        guard let pane = focusedPane else { return }
        layoutCommand("zoom", ["pane": .string(pane)])
    }

    @objc func goBack() { history(back: true) }
    @objc func goForward() { history(back: false) }

    private func history(back: Bool) {
        guard let pane = focusedPane else { return }
        request("navigate", ["pane": .string(pane), back ? "back" : "forward": true])
    }

    @objc func toggleComments() {
        guard let pane = focusedPane else { return }
        switchComments(pane)
    }

    func switchComments(_ pane: String) {
        guard contents.web(pane) != nil || contents.markdown(pane) != nil else { return }
        if commentsOn(pane) { return request("markup", ["pane": .string(pane), "off": true]) }
        turnOnComments(pane, under: nil)
    }

    private func commentsOn(_ pane: String) -> Bool {
        contents.markdown(pane)?.markup ?? contents.web(pane)?.markable ?? false
    }

    @objc func movePaneToNewWindow() {
        guard let pane = focusedPane else { return }
        layoutCommand("move", ["pane": .string(pane), "window": "new"])
    }

    @objc func moveTabToNewWindow() {
        guard let tab = keyWindow?["tab"]?.string else { return }
        layoutCommand("move-tab", ["tab": .string(tab), "window": "new"])
    }

    @objc func reloadPane() {
        guard let pane = focusedPane else { return }
        contents.web(pane)?.webView.reload()
        contents.diff(pane)?.refresh()
        contents.log(pane)?.refresh()
    }

    @objc func closeFocusedPane() {
        guard let pane = focusedPane else { return }
        closePane(pane)
    }

    func closePane(_ pane: String) {
        guard !docked(pane) else { return }
        request("close", ["pane": .string(pane)])
    }
    func closeTab(_ tab: String) { request("close", ["tab": .string(tab)]) }

    @objc func newTabInKeyWindow() {
        guard !showingDeskView else { return NSSound.beep() }
        newTab(in: keyWindow?["id"]?.string)
    }

    var showingDeskView: Bool { keyWindow?["desk"]?.bool == true && keyWindow?["dock"]?["pane"]?.string == nil }

    func newTab(in window: String?) {
        var args: [String: JSON] = ["type": "term", "tab": true]
        if let window { args.merge(address(window)) { _, new in new } }
        args["cwd"] = repo(of: window)
        request("open", args)
    }

    private func repo(of window: String?) -> JSON? {
        guard let window, self.window(window)?["dock"]?["pane"] == nil, let root = controllers[window]?.projectRoot else { return nil }
        return .string(MachineIDs.split(location: root).path)
    }

    @objc func nextTab() { cycleTab(1) }
    @objc func previousTab() { cycleTab(-1) }

    func cycleTab(_ step: Int) {
        guard let window = keyWindow, let id = window["id"]?.string else { return }
        let controller = controllers[id]
        let pages = (controller?.pages ?? []).map { "page:\($0)" }
        let stops = pages + (window["tabs"]?.array ?? []).compactMap { $0["id"]?.string }
        guard !stops.isEmpty else { return }
        let current = controller?.shownPage.map { "page:\($0)" } ?? window["tab"]?.string
        let at = current.flatMap { stops.firstIndex(of: $0) } ?? 0
        let next = stops[((at + step) % stops.count + stops.count) % stops.count]
        if next.hasPrefix("page:") {
            controller?.tabBar.onPage?(String(next.dropFirst(5)))
            return
        }
        if controller?.shownPage != nil {
            controller?.page = nil
            refresh()
        }
        layoutCommand("focus", ["tab": .string(next), "keep": "dock"])
    }

    @objc func showAbout() {
        NSApp.orderFrontStandardAboutPanel(options: [.applicationVersion: KannaBuild.version, .version: KannaBuild.buildLabel])
    }

    @objc func showAccounts() { accountsTerminal("kanna accounts; exec \"${SHELL:-/bin/sh}\" -l") }
    @objc func addClaudeSubscription() { accountsTerminal("kanna accounts add claude") }
    @objc func addAnthropicKey() { accountsTerminal("kanna accounts add claude --api-key") }
    @objc func addCodexAccount() { accountsTerminal("kanna accounts add codex") }
    @objc func addOpenAIKey() { accountsTerminal("kanna accounts add codex --api-key") }
    @objc func removeClaudeAccount() { accountsTerminal("kanna accounts remove claude") }
    @objc func removeCodexAccount() { accountsTerminal("kanna accounts remove codex") }

    private func accountsTerminal(_ command: String) {
        request("open", ["type": "term", "window": "new", "cmd": .string(command)], on: .some(nil))
    }

    @objc func newWindow() {
        var args: [String: JSON] = ["type": "term", "window": "new"]
        let root = keyWindow?["id"]?.string.flatMap { controllers[$0]?.projectRoot }.map(MachineIDs.split(location:))
        if let root { args["cwd"] = .string(root.path) }
        request("open", args, on: .some(root?.machine))
    }

    @objc func closeWindow() {
        guard let window = keyWindow?["id"]?.string else { return }
        close(window: window)
    }

    func close(window id: String) {
        if isDesk(id) {
            closeDesk(id)
        } else {
            request("close", ["window": .string(id)])
        }
    }

    @objc func toggleDeskView() {
        if let (task, window) = deskBack {
            activateTask(task, into: window)
            return
        }
        guard let window = keyWindow?["id"]?.string else { return }
        showDeskView(desk: window)
    }

    private var deskBack: (task: String, window: String)? {
        guard let window = keyWindow, window["desk"]?.bool == true, window["dock"]?["pane"] == nil, let id = window["id"]?.string else { return nil }
        if let active = controllers[id]?.active { return (active, id) }
        return keyedDeskTask(1)
    }

    private var deskView: DeskView? {
        keyWindow?["id"]?.string.flatMap { controllers[$0] }.map(\.deskView)
    }

    private var deskSelection: (id: String, onDesk: Bool, blocked: Bool, running: Bool, active: Bool)? { deskView?.selectedTask }

    @objc func deskPrevious() { deskView?.select(-1) }

    @objc func deskNext() { deskView?.select(1) }

    @objc func deskEnter() { deskView?.enter() }

    @objc func deskRun() { deskView?.run() }

    func deskKeysChanged() {
        let keyed = deskView?.keyed == true
        for item in NSApp.mainMenu?.items.compactMap(\.submenu).flatMap(\.items) ?? [] {
            switch item.action {
            case #selector(deferTask)?:
                item.keyEquivalent = keyed ? "\u{7f}" : ""
                item.keyEquivalentModifierMask = keyed ? [.command, .shift] : []
            case #selector(deskEnter)?:
                item.keyEquivalent = keyed ? "\r" : ""
                item.keyEquivalentModifierMask = []
            case #selector(deskRun)?:
                item.keyEquivalent = keyed ? "\r" : ""
                item.keyEquivalentModifierMask = keyed ? .command : []
            default: break
            }
        }
    }

    @objc func deferTask() {
        if deskView?.shelveSelected() == true { return }
        guard let window = keyWindow?["id"]?.string, let pane = keyWindow?["dock"]?["pane"]?.string else { return }
        request("tasks", ["action": "shelve", "task": .string(pane)]) { [weak self] reply in
            guard let self, reply["ok"]?.bool != true, let controller = controllers[window] else { return }
            controller.note = refusal(reply, otherwise: "it didn't move")
            refresh()
        }
    }

    @objc func activateDeskTask1() { activateDeskTask(1) }
    @objc func activateDeskTask2() { activateDeskTask(2) }

    private func keyedDeskTask(_ place: Int) -> (task: String, window: String)? {
        guard let window = keyWindow, window["desk"]?.bool == true, let id = window["id"]?.string, let tasks = controllers[id]?.others else { return nil }
        return DeskOrder.task(tasks, key: place).map { ($0.id, id) }
    }

    private func activateDeskTask(_ place: Int) {
        guard let (task, window) = keyedDeskTask(place) else { return }
        activateTask(task, into: window)
    }

    @objc func nextWindow() { cycleWindow(1) }
    @objc func previousWindow() { cycleWindow(-1) }

    private var cyclable: [String] {
        windows.compactMap { $0["id"]?.string }.filter { controllers[$0].map { !$0.window.isMiniaturized } ?? false }
    }

    private func cycleWindow(_ step: Int) {
        let ids = cyclable
        guard !ids.isEmpty else { return }
        let at = ids.firstIndex { $0 == keyWindow?["id"]?.string } ?? (step > 0 ? -1 : 0)
        let next = ids[((at + step) % ids.count + ids.count) % ids.count]
        show(window: next)
        if let pane = window(next)?["focused"]?.string { giveKeyboard(pane) }
    }

    func ghosttyAction(_ pane: String, _ action: ghostty_action_s) -> Bool {
        let perform: (() -> Void)?
        switch action.tag {
        case GHOSTTY_ACTION_NEW_SPLIT:
            let direction = action.action.new_split
            perform = direction == GHOSTTY_SPLIT_DIRECTION_DOWN || direction == GHOSTTY_SPLIT_DIRECTION_UP ? splitDown : splitRight
        case GHOSTTY_ACTION_GOTO_SPLIT:
            let target = action.action.goto_split
            perform = target == GHOSTTY_GOTO_SPLIT_PREVIOUS || target == GHOSTTY_GOTO_SPLIT_LEFT || target == GHOSTTY_GOTO_SPLIT_UP ? previousPane : nextPane
        case GHOSTTY_ACTION_TOGGLE_SPLIT_ZOOM: perform = toggleZoom
        case GHOSTTY_ACTION_NEW_TAB: perform = newTabInKeyWindow
        case GHOSTTY_ACTION_GOTO_TAB:
            let target = action.action.goto_tab
            let place = Int(target.rawValue)
            perform = target == GHOSTTY_GOTO_TAB_PREVIOUS ? previousTab : target == GHOSTTY_GOTO_TAB_NEXT ? nextTab
                : (1...2).contains(place) && keyedDeskTask(place) != nil ? { [weak self] in self?.activateDeskTask(place) } : nil
        case GHOSTTY_ACTION_NEW_WINDOW: perform = newWindow
        case GHOSTTY_ACTION_CLOSE_WINDOW: perform = closeWindow
        case GHOSTTY_ACTION_GOTO_WINDOW:
            perform = action.action.goto_window == GHOSTTY_GOTO_WINDOW_PREVIOUS ? previousWindow : nextWindow
        default: perform = nil
        }
        guard let perform else { return false }
        userFocused(pane)
        perform()
        return true
    }
}
