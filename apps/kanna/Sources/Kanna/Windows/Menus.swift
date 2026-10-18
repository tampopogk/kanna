import AppKit
import GhosttyKit
import UniformTypeIdentifiers

extension AppController {
    func installMenu() {
        let runtime = runtime!
        let main = NSMenu()
        func menu(_ title: String, _ items: [(String, Selector, String?, Shortcut?)?]) -> NSMenu {
            let item = NSMenuItem()
            let submenu = NSMenu(title: title)
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
            item.submenu = submenu
            main.addItem(item)
            return submenu
        }
        let app = menu("Kanna", [])
        if Self.instanceName != "default" {
            app.addItem(withTitle: "Instance: \(Self.instanceName)", action: nil, keyEquivalent: "")
            app.addItem(.separator())
        }
        app.addItem(withTitle: "Quit \(KannaBuild.name)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        _ = menu("File", [("New Desk", #selector(newDeskWindow), nil, .cmd("n"))])
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
        paneMenu = menu("Pane", [
            ("Split Right", #selector(splitRight), "new_split:right", .cmd("d")),
            ("Split Down", #selector(splitDown), "new_split:down", .shiftCmd("d")),
            ("New Web Pane Right", #selector(webRight), nil, nil),
            ("New Web Pane Below", #selector(webDown), nil, nil),
            ("New iOS Pane Right", #selector(iosRight), nil, nil),
            ("New iOS Pane Below", #selector(iosDown), nil, nil),
            ("New Markdown Pane Right…", #selector(markdownRight), nil, nil),
            ("New Markdown Pane Below…", #selector(markdownDown), nil, nil),
            ("Open URL…", #selector(openURL), nil, .cmd("l")),
            ("Back", #selector(goBack), nil, nil),
            ("Forward", #selector(goForward), nil, nil),
            ("Home", #selector(pressHome), nil, .shiftCmd("h")),
            ("Comments", #selector(toggleComments), nil, nil),
            nil,
            ("Next Pane", #selector(nextPane), "goto_split:next", .cmd("]")),
            ("Previous Pane", #selector(previousPane), "goto_split:previous", .cmd("[")),
            ("Zoom", #selector(toggleZoom), "toggle_split_zoom", .shiftCmd("\r")),
            ("Move Pane to New Window", #selector(movePaneToNewWindow), nil, nil),
            nil,
            ("Reload", #selector(reloadPane), nil, .cmd("r")),
            ("Close Pane", #selector(closeFocusedPane), "close_surface", .cmd("w")),
        ])
        _ = menu("View", [
            ("New Tab", #selector(newTabInKeyWindow), "new_tab", .cmd("t")),
            ("Next Tab", #selector(nextTab), "next_tab", .shiftCmd("]")),
            ("Previous Tab", #selector(previousTab), "previous_tab", .shiftCmd("[")),
            nil,
            ("Zoom In", #selector(zoomIn), "increase_font_size:1", .cmd("=")),
            ("Zoom Out", #selector(zoomOut), "decrease_font_size:1", .cmd("-")),
            ("Actual Size", #selector(actualSize), "reset_font_size", .cmd("0")),
        ])
        _ = menu("Task", [
            ("Park", #selector(parkTask), nil, .cmd(".")),
            ("Move to Backlog", #selector(deferTask), nil, nil),
            nil,
            ("Task Waiting Longest", #selector(pullWaitingTask), nil, .shiftCmd("A")),
            nil,
            ("Pull Shelf Task 1", #selector(pullShelf1), "goto_tab:1", .cmd("1")),
            ("Pull Shelf Task 2", #selector(pullShelf2), "goto_tab:2", .cmd("2")),
            ("Pull Shelf Task 3", #selector(pullShelf3), "goto_tab:3", .cmd("3")),
        ])
        let windowMenu = menu("Window", [
            ("New Window", #selector(newWindow), nil, nil),
            ("Close Window", #selector(closeWindow), "close_window", .shiftCmd("w")),
            ("Move Tab to New Window", #selector(moveTabToNewWindow), nil, nil),
            ("Free Window", #selector(freeWindow), nil, nil),
            nil,
            ("Next Window", #selector(nextWindow), nil, .cmd("`")),
            ("Previous Window", #selector(previousWindow), nil, .shiftCmd("`")),
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
        let shelf = shelfEntries.compactMap { entry in entry["task"]?.string.map { ($0, entry["name"]?.string) } }
        let held = backlog.filter { $0["running"]?.bool == true }.compactMap { entry in entry["held"]?.string.map { ($0, entry["name"]?.string) } }
        return (desks + shelf + held).filter { root == nil || projectRoot($0.0) == root }.map { (pane: $0.0, title: taskTitle($0.0, name: $0.1)) }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
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
        shelfEntries.first { $0["task"]?.string == pane }?["workspace"]?.string ?? windows.first { $0["dock"]?["pane"]?.string == pane }?["shows"]?.string
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
        for top in NSApp.mainMenu?.items ?? [] {
            guard let menu = top.submenu, let at = menu.items.firstIndex(where: { $0.title == title }) else { continue }
            menu.update()
            guard menu.items[at].isEnabled else { throw KannaError("disabled", "\(title) is disabled") }
            menu.performActionForItem(at: at)
            return
        }
        throw KannaError("not_found", "no menu item \"\(title)\"")
    }

    var menuItems: [JSON] {
        (NSApp.mainMenu?.items ?? []).flatMap { top in
            (top.submenu?.items ?? []).filter { !$0.isSeparatorItem }.map { item -> JSON in
                let shortcut = Shortcut(key: item.keyEquivalent, modifiers: item.keyEquivalentModifierMask)
                return ["menu": .string(top.submenu?.title ?? ""), "item": .string(item.title), "shortcut": .string(item.keyEquivalent.isEmpty ? "" : shortcut.label)]
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
        guard !onShelf else { return NSSound.beep() }
        var args: [String: JSON] = switch type {
        case "web": ["type": "web", "url": "http://localhost:3000"]
        case "ios": ["type": "ios", "app": "com.apple.Preferences", "wait": false]
        default: ["type": "term"]
        }
        if let window = keyWindow?["id"] {
            args["window"] = window
            args["split"] = .string(direction)
            if type == "term" { args["cwd"] = repo(of: window.string) }
        }
        request("open", args) { [weak self] reply in
            guard type == "web", let id = reply["pane"]?["id"]?.string else { return }
            self?.contents.web(id)?.editURL()
        }
    }

    private func openMarkdown(_ direction: String) {
        guard !onShelf else { return NSSound.beep() }
        let panel = NSOpenPanel()
        panel.allowedContentTypes = ["md", "markdown", "mdown"].compactMap { UTType(filenameExtension: $0) }
        panel.allowsOtherFileTypes = true
        panel.message = "Choose a markdown file to show"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        var args: [String: JSON] = ["type": "md", "path": .string(url.path)]
        if let window = keyWindow?["id"] {
            args["window"] = window
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
        case #selector(openURL), #selector(reloadPane): return contents.web(pane) != nil
        case #selector(toggleComments):
            item.state = commentsOn(pane) ? .on : .off
            return contents.web(pane) != nil || contents.markdown(pane) != nil
        case #selector(goBack), #selector(goForward): return contents.web(pane) != nil || contents.markdown(pane) != nil || contents.code(pane) != nil
        case #selector(nextPane), #selector(previousPane), #selector(toggleZoom):
            return focusedPane != nil
        case #selector(movePaneToNewWindow): return focusedPane.map { !frozen($0) } ?? false
        case #selector(closeFocusedPane): return focusedPane.map { !docked($0) } ?? false
        case #selector(nextTab), #selector(previousTab), #selector(moveTabToNewWindow), #selector(closeWindow): return keyWindow != nil
        case #selector(nextWindow), #selector(previousWindow): return windows.count > 1
        case #selector(freeWindow): return keyWindow?["workspace"]?.string != nil
        case #selector(newTabInKeyWindow), #selector(splitRight), #selector(splitDown), #selector(webRight), #selector(webDown),
             #selector(iosRight), #selector(iosDown), #selector(markdownRight), #selector(markdownDown): return !onShelf
        case #selector(parkTask):
            item.title = lastParked == nil ? "Park" : "Unpark"
            return keyWindow?["dock"]?["pane"] != nil || lastParked != nil
        case #selector(deferTask): return keyWindow?["dock"]?["pane"] != nil
        case #selector(pullWaitingTask): return waitingTask != nil
        case #selector(pullShelf1): return shelfPull(1) != nil
        case #selector(pullShelf2): return shelfPull(2) != nil
        case #selector(pullShelf3): return shelfPull(3) != nil
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
        layoutCommand("focus", ["pane": .string(ids[((at + step) % ids.count + ids.count) % ids.count])])
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
        guard let pane = focusedPane, contents.web(pane) != nil || contents.markdown(pane) != nil else { return }
        request("markup", commentsOn(pane) ? ["pane": .string(pane), "off": true] : ["pane": .string(pane)])
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
        guard !onShelf else { return NSSound.beep() }
        newTab(in: keyWindow?["id"]?.string)
    }

    var onShelf: Bool { keyWindow?["desk"]?.bool == true && keyWindow?["dock"]?["pane"]?.string == nil }

    func newTab(in window: String?) {
        var args: [String: JSON] = ["type": "term", "tab": true]
        if let window { args["window"] = .string(window) }
        args["cwd"] = repo(of: window)
        request("open", args)
    }

    private func repo(of window: String?) -> JSON? {
        guard let window, self.window(window)?["dock"]?["pane"] == nil, let root = controllers[window]?.projectRoot else { return nil }
        return .string(root)
    }

    @objc func nextTab() { cycleTab(1) }
    @objc func previousTab() { cycleTab(-1) }

    private func cycleTab(_ step: Int) {
        guard let window = keyWindow, case .array(let tabs)? = window["tabs"], !tabs.isEmpty else { return }
        let at = tabs.firstIndex { $0["id"] == window["tab"] } ?? 0
        guard let tab = tabs[((at + step) % tabs.count + tabs.count) % tabs.count]["id"]?.string else { return }
        layoutCommand("focus", ["tab": .string(tab)])
    }

    @objc func newWindow() {
        var args: [String: JSON] = ["type": "term", "window": "new"]
        if let root = keyWindow?["id"]?.string.flatMap({ controllers[$0]?.projectRoot }) { args["cwd"] = .string(root) }
        request("open", args)
    }

    @objc func closeWindow() {
        guard let window = keyWindow?["id"]?.string else { return }
        close(window: window)
    }

    func close(window id: String) {
        if window(id)?["desk"]?.bool == true {
            if let lead = window(id)?["lead"]?["pane"]?.string { request("close", ["pane": .string(lead)]) }
            layoutCommand("task", ["action": "park", "window": .string(id), "close": true])
        } else {
            request("close", ["window": .string(id)])
        }
    }

    @objc func parkTask() {
        if let (task, window) = lastParked { return pullTask(task, into: window) }
        guard let window = keyWindow?["id"]?.string else { return }
        layoutCommand("task", ["action": "park", "window": .string(window)])
    }

    private var lastParked: (task: String, window: String)? {
        guard let window = keyWindow, window["desk"]?.bool == true, window["dock"]?["pane"] == nil,
              let id = window["id"]?.string, let tasks = controllers[id]?.shelf,
              let task = tasks.max(by: { ($0.parkedAt ?? 0) < ($1.parkedAt ?? 0) }) else { return nil }
        return (task.id, id)
    }

    /// Puts the Desk's task back on its repo's backlog, its agent and tabs still running until it's started again.
    @objc func deferTask() {
        guard let window = keyWindow?["id"]?.string, let pane = keyWindow?["dock"]?["pane"]?.string else { return }
        request("backlog", ["action": "defer", "task": .string(pane)]) { [weak self] reply in
            guard let self, reply["ok"]?.bool != true, let controller = controllers[window] else { return }
            controller.note = reply["error"]?["message"]?.string ?? "it didn't move"
            refresh()
        }
    }

    @objc func pullShelf1() { pullShelf(1) }
    @objc func pullShelf2() { pullShelf(2) }
    @objc func pullShelf3() { pullShelf(3) }

    private var waitingTask: (task: String, window: String)? {
        guard let (_, window) = shelfPull(1), let tasks = controllers[window]?.shelf,
              let task = agentStatuses.waitingLongest(among: tasks.map(\.id)) else { return nil }
        return (task, window)
    }

    @objc func pullWaitingTask() {
        guard let (task, window) = waitingTask else { return }
        pullTask(task, into: window)
    }

    private func shelfPull(_ place: Int) -> (task: String, window: String)? {
        guard let window = keyWindow, window["desk"]?.bool == true, let id = window["id"]?.string, let tasks = controllers[id]?.shelf else { return nil }
        return place <= tasks.count ? (tasks[place - 1].id, id) : nil
    }

    private func pullShelf(_ place: Int) {
        guard let (task, window) = shelfPull(place) else { return }
        pullTask(task, into: window)
    }

    @objc func nextWindow() { cycleWindow(1) }
    @objc func previousWindow() { cycleWindow(-1) }

    private func cycleWindow(_ step: Int) {
        let ids = windows.compactMap { $0["id"]?.string }
        guard !ids.isEmpty else { return }
        let at = ids.firstIndex { $0 == state["key"]?.string } ?? 0
        layoutCommand("focus", ["window": .string(ids[((at + step) % ids.count + ids.count) % ids.count])])
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
                : (1...3).contains(place) && shelfPull(place) != nil ? { [weak self] in self?.pullShelf(place) } : nil
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
