import Foundation

/// Persistence (docs/kmux-spec.md §3.6): the layout as a versioned state,
/// ported from the reference model (reference/kmux/core.js: exportState,
/// importState). Panes keep their IDs. Restoring starts every pane again:
/// terminals as new terminals in their cwd with their command, web panes at
/// their URL, md panes on their file, iOS panes relaunching their app.
/// Version 1 named panes p1, p2, … tabs t1, windows w1 and workspaces s1; it
/// loads with each renamed (`upgrade`): a pane takes its content's kind (p3
/// showing a web page is page-3), tabs tab-N, windows win-N and workspaces ws-N.
///
///     { kmux: "state", version: 2, key, counters: { term, page, file, ios, tab, window, workspace },
///       workspaces: [{ id, name, hidden }],
///       windows: [{ id, frame?, active, focused, zoomed, tabCount, dock?: { pane, edge, width, frozen? }, workspace?,
///                   tabs: [{ id, title, lastFocus, layout }] }],
///       panes: [{ id, name, type, …what it shows }] }
///
/// A layout is `{ pane: ID }` or `{ split: row|column, children: [{ …layout, size }] }`.
extension Core {
    public static let stateVersion = 2

    public func exportState() -> JSON {
        let model = self.model
        let counters = model.savedCounters
        let windows: [JSON] = model.windows.map { window in
            var out: [String: JSON] = [
                "id": .string(window.id), "active": Self.optional(window.active), "focused": Self.optional(window.focused),
                "zoomed": Self.optional(window.zoomed), "tabCount": .number(Double(window.tabCount)),
                "tabs": .array(window.tabs.map { tab in
                    ["id": .string(tab.id), "title": .string(tab.title), "lastFocus": Self.optional(tab.lastFocus), "layout": tab.root.map(Self.exportNode) ?? .null]
                }),
            ]
            if let frame = window.frame { out["frame"] = ["x": .number(frame.x), "y": .number(frame.y), "w": .number(frame.w), "h": .number(frame.h)] }
            if let dock = window.dock {
                var saved: [String: JSON] = ["pane": .string(dock.pane), "edge": .string(dock.edge.rawValue), "width": .number(dock.width)]
                if dock.frozen { saved["frozen"] = true }
                out["dock"] = .object(saved)
            }
            if let workspace = window.workspace { out["workspace"] = .string(workspace) }
            return .object(out)
        }
        let panes: [JSON] = model.paneOrder.compactMap { model.panes[$0] }.map { pane in
            var out: [String: JSON] = ["id": .string(pane.id), "name": Self.optional(pane.name), "type": .string(pane.type.rawValue)]
            switch pane.type {
            case .term:
                out["cmd"] = Self.optional(pane.command)
                out["cwd"] = Self.optional(pane.cwd)
                out["session"] = Self.optional(pane.session)
            case .web:
                out["url"] = Self.optional(pane.url)
                out["history"] = .bool(pane.history != nil)
            case .md:
                out["path"] = Self.optional(pane.path)
                out["zoom"] = .number(pane.zoom ?? 1)
            case .ios:
                out["app"] = Self.optional(pane.app)
                out["device"] = Self.optional(pane.device)
            }
            return .object(out)
        }
        return [
            "kmux": "state", "version": .number(Double(Self.stateVersion)), "key": Self.optional(model.key),
            "counters": .object(Dictionary(uniqueKeysWithValues: PaneType.allCases.map { ($0.kind, JSON.number(Double(counters.content[$0.kind] ?? 0))) })
                .merging(["tab": .number(Double(counters.tab)), "window": .number(Double(counters.window)), "workspace": .number(Double(counters.workspace))]) { $1 }),
            "workspaces": .array(model.workspaces.map(model.workspaceView)), "windows": .array(windows), "panes": .array(panes),
        ]
    }

    private static func exportNode(_ node: Node) -> JSON {
        switch node {
        case .pane(let id): return ["pane": .string(id)]
        case .split(let split):
            return ["split": .string(split.axis.rawValue), "children": .array(split.kids.map { kid in
                guard case .object(var fields) = exportNode(kid.node) else { return .null }
                fields["size"] = .number(kid.size)
                return .object(fields)
            })]
        }
    }

    private static func optional(_ value: String?) -> JSON { value.map(JSON.string) ?? .null }

    /// Replaces everything with a saved state, as kmux does when it launches:
    /// the old panes stop, the restored ones start (without waiting for them).
    /// Throws `bad_request`, changing nothing, if the state can't be read.
    public func restoreState(_ state: JSON) throws {
        let (panes, windows, workspaces, key, counters) = try Self.importState(state)
        for pane in model.paneOrder.compactMap({ model.panes[$0] }) { closePane(pane) }
        model.install(panes: panes, windows: windows, workspaces: workspaces, key: key, counters: counters)
        model.dropEmpty()
        onChange()
        for pane in panes where model.panes[pane.id] === pane { host?.start(pane) }
    }

    /// Checks a saved state and builds what it describes. Lenient where it can
    /// repair (dangling focus, empty tabs, sizes), strict about anything that
    /// would make the layout ambiguous.
    /// A version-1 state as version 2: the same layout, its IDs renamed. What
    /// isn't an ID of version 1 is left as it is, for `importState` to judge.
    static func upgrade(_ state: JSON) -> JSON {
        guard case .object(var out) = state else { return state }
        var kinds: [String: String] = [:]
        if case .array(let panes)? = state["panes"] {
            for pane in panes {
                if let id = pane["id"]?.string, let type = pane["type"]?.string.flatMap(PaneType.init(rawValue:)) { kinds[id] = type.kind }
            }
        }
        func id(_ value: JSON?) -> JSON? {
            guard let text = value?.string, let match = text.wholeMatch(of: /([ptws])([1-9][0-9]*)/) else { return value }
            switch match.1 {
            case "p": return kinds[text].map { .string("\($0)-\(match.2)") } ?? value
            case "t": return .string("tab-\(match.2)")
            case "w": return .string("win-\(match.2)")
            default: return .string("ws-\(match.2)")
            }
        }
        func renaming(_ value: JSON?, _ keys: [String]) -> JSON? {
            guard case .object(var fields)? = value else { return value }
            for key in keys where fields[key] != nil { fields[key] = id(fields[key]) }
            return .object(fields)
        }
        func node(_ value: JSON) -> JSON {
            guard case .object(var fields)? = renaming(value, ["pane"]) else { return value }
            if case .array(let children)? = fields["children"] { fields["children"] = .array(children.map(node)) }
            return .object(fields)
        }
        func each(_ value: JSON?, _ transform: (JSON) -> JSON) -> JSON? {
            guard case .array(let items)? = value else { return value }
            return .array(items.map(transform))
        }
        let counters = state["counters"] ?? [:]
        let pane = counters["pane"] ?? .null
        out["version"] = .number(Double(stateVersion))
        out["key"] = id(state["key"])
        out["counters"] = ["term": pane, "page": pane, "file": pane, "ios": pane, "tab": counters["tab"] ?? .null,
                           "window": counters["window"] ?? .null, "workspace": counters["workspace"] ?? .null]
        out["workspaces"] = each(state["workspaces"]) { renaming($0, ["id"]) ?? $0 }
        out["panes"] = each(state["panes"]) { renaming($0, ["id"]) ?? $0 }
        out["windows"] = each(state["windows"]) { window in
            guard case .object(var fields)? = renaming(window, ["id", "active", "focused", "zoomed", "workspace"]) else { return window }
            if let dock = fields["dock"] { fields["dock"] = renaming(dock, ["pane"]) }
            fields["tabs"] = each(fields["tabs"]) { tab in
                guard case .object(var fields)? = renaming(tab, ["id", "lastFocus"]) else { return tab }
                if let layout = fields["layout"] { fields["layout"] = node(layout) }
                return .object(fields)
            }
            return .object(fields)
        }
        return .object(out)
    }

    static func importState(_ state: JSON) throws -> (panes: [Pane], windows: [Window], workspaces: [Workspace], key: String?, counters: Counters) {
        func bad(_ message: String) -> KmuxError { KmuxError("bad_request", "bad state: \(message)") }
        func quoted(_ value: JSON?) -> String { String(decoding: (value ?? .null).encoded(), as: UTF8.self) }
        func number(_ id: String, _ prefix: String) -> Int? {
            guard id.hasPrefix(prefix), let n = Int(id.dropFirst(prefix.count)), n > 0, "\(prefix)\(n)" == id else { return nil }
            return n
        }
        func whole(_ value: JSON?) -> Int? {
            guard let n = value?.number, n.rounded() == n, n >= 0, n < 1e9 else { return nil }
            return Int(n)
        }
        guard case .object = state else { throw bad("not an object") }
        let state = state["version"] == 1 ? upgrade(state) : state
        guard state["version"] == .number(Double(stateVersion)) else {
            throw KmuxError("bad_request", "unsupported state version \(quoted(state["version"])) (this kmux reads 1 and \(stateVersion))")
        }
        guard case .array(let savedPanes)? = state["panes"], case .array(let savedWindows)? = state["windows"] else { throw bad("needs panes and windows") }
        let saved = state["counters"] ?? [:]
        var counters = Counters(content: Dictionary(uniqueKeysWithValues: PaneType.allCases.map { ($0.kind, whole(saved[$0.kind]) ?? 0) }),
                                tab: whole(saved["tab"]) ?? 0, window: whole(saved["window"]) ?? 0, workspace: whole(saved["workspace"]) ?? 0)

        var workspaces: [Workspace] = []
        if let saved = state["workspaces"], saved != .null {
            guard case .array(let items) = saved else { throw bad("workspaces is not a list") }
            for item in items {
                guard case .object = item, let id = item["id"]?.string, let n = number(id, "ws-"), !workspaces.contains(where: { $0.id == id }) else {
                    throw bad("workspace \(quoted(item["id"]))")
                }
                let name = item["name"]?.string?.trimmingCharacters(in: .whitespaces)
                let named = name?.isEmpty == false ? name : nil
                if let named, workspaces.contains(where: { $0.name == named }) { throw bad("workspace name \(quoted(.string(named))) is used twice") }
                let workspace = Workspace(id: id, name: named)
                workspace.hidden = item["hidden"]?.bool == true
                workspaces.append(workspace)
                counters.workspace = max(counters.workspace, n)
            }
        }

        var panes: [Pane] = [], byID: [String: Pane] = [:], names: Set<String> = []
        for item in savedPanes {
            guard case .object = item, let id = item["id"]?.string, id.wholeMatch(of: /[a-z]+-[1-9][0-9]*/) != nil, byID[id] == nil else {
                throw bad("pane \(quoted(item["id"]))")
            }
            guard let type = item["type"]?.string.flatMap(PaneType.init(rawValue:)) else { throw bad("pane \(id) has unknown type \(quoted(item["type"]))") }
            guard let n = number(id, "\(type.kind)-") else { throw bad("pane \(id) is a \(type.rawValue) pane, so its ID is \(type.kind)-N") }
            let name = item["name"]?.string
            if let name {
                guard !name.isEmpty, !names.contains(name) else { throw bad("pane name \(quoted(.string(name))) is used twice") }
                names.insert(name)
            }
            let needed: String? = switch type {
            case .web: "url"
            case .md: "path"
            case .ios: "app"
            case .term: nil
            }
            if let needed, item[needed]?.string?.isEmpty ?? true { throw bad("\(type.rawValue) pane \(id) has no \(needed)") }
            let pane = Pane(id: id, name: name, type: type)
            pane.command = item["cmd"]?.string
            pane.cwd = item["cwd"]?.string
            pane.session = item["session"]?.string
            pane.url = item["url"]?.string
            pane.path = item["path"]?.string
            pane.app = item["app"]?.string
            pane.device = item["device"]?.string
            if type == .md, let zoom = item["zoom"]?.number, zoom > 0 { pane.zoom = zoom }
            if (type == .web && item["history"] == true) || type == .md { pane.history = History() }
            if type != .term { pane.command = nil; pane.cwd = nil; pane.session = nil }
            panes.append(pane)
            byID[id] = pane
            counters.content[type.kind] = max(counters.content[type.kind] ?? 0, n)
        }

        var placed: Set<String> = [], tabIDs: Set<String> = [], windowIDs: Set<String> = []
        func build(_ node: JSON) throws -> Node {
            guard case .object = node else { throw bad("malformed layout") }
            if let ref = node["pane"], ref != .null {
                guard let id = ref.string, byID[id] != nil else { throw bad("layout names unknown pane \(quoted(ref))") }
                guard !placed.contains(id) else { throw bad("pane \(id) is placed twice") }
                placed.insert(id)
                return .pane(id)
            }
            guard let axis = node["split"]?.string.flatMap(Axis.init(rawValue:)), case .array(let children)? = node["children"], !children.isEmpty else {
                throw bad("malformed layout")
            }
            let sizes = children.map { child -> Double in
                guard let size = child["size"]?.number, size > 0, size <= 1 else { return 1 / Double(children.count) }
                return size
            }
            let total = sizes.reduce(0, +)
            return .split(Split(axis: axis, kids: try zip(children, sizes).map { Child(node: try build($0), size: $1 / total) }))
        }

        var windows: [Window] = []
        for item in savedWindows {
            guard case .object = item, let id = item["id"]?.string, let n = number(id, "win-"), !windowIDs.contains(id), case .array(let savedTabs)? = item["tabs"] else {
                throw bad("window \(quoted(item["id"]))")
            }
            windowIDs.insert(id)
            let window = Window(id: id)
            window.workspace = item["workspace"]?.string.flatMap { ref in workspaces.contains { $0.id == ref } ? ref : nil }
            if let f = item["frame"], let x = f["x"]?.number, let y = f["y"]?.number, let w = f["w"]?.number, let h = f["h"]?.number,
               [x, y, w, h].allSatisfy(\.isFinite), w > 0, h > 0 {
                window.frame = Frame(x: x, y: y, w: w, h: h)
            }
            for entry in savedTabs {
                guard case .object = entry, let tabID = entry["id"]?.string, let tn = number(tabID, "tab-"), !tabIDs.contains(tabID) else {
                    throw bad("tab \(quoted(entry["id"]))")
                }
                tabIDs.insert(tabID)
                let root = try entry["layout"].flatMap { $0 == .null ? nil : Model.normalize(try build($0)) }
                let title = (entry["title"]?.string ?? "").trimmingCharacters(in: .whitespaces)
                let tab = Tab(id: tabID, title: title.isEmpty ? "Tab \(window.tabs.count + 1)" : title)
                tab.root = root
                let ids = Model.paneIDs(root)
                tab.lastFocus = entry["lastFocus"]?.string.flatMap { ids.contains($0) ? $0 : nil }
                window.tabs.append(tab)
                counters.tab = max(counters.tab, tn)
            }
            if let saved = item["dock"], saved != .null {
                guard let pane = saved["pane"]?.string, byID[pane] != nil else { throw bad("window \(id) docks unknown pane \(quoted(saved["pane"]))") }
                guard !placed.contains(pane) else { throw bad("pane \(pane) is placed twice") }
                placed.insert(pane)
                let width = saved["width"]?.number.flatMap { $0 > 0 && $0 < 1 ? $0 : nil } ?? Dock.defaultWidth
                window.dock = Dock(pane: pane, edge: saved["edge"]?.string.flatMap(Edge.init(rawValue:)) ?? .left, width: width, frozen: saved["frozen"]?.bool == true)
            }
            window.tabs.removeAll { $0.root == nil }
            let active = item["active"]?.string
            window.active = window.tabs.contains { $0.id == active } ? active : window.tabs.first?.id
            let shown = window.shownPanes
            let focused = item["focused"]?.string
            window.focused = focused.flatMap { shown.contains($0) ? $0 : nil } ?? window.activeTab?.lastFocus ?? Model.paneIDs(window.activeTab?.root).first ?? window.dock?.pane
            if let zoomed = item["zoomed"]?.string, shown.contains(zoomed) {
                window.zoom = true
                window.focused = zoomed
            }
            window.tabCount = max(whole(item["tabCount"]) ?? 0, window.tabs.count)
            windows.append(window)
            counters.window = max(counters.window, n)
        }
        // Panes no tab shows are not restored.
        panes.removeAll { !placed.contains($0.id) }
        windows.removeAll { $0.tabs.isEmpty && $0.dock == nil }
        let key = state["key"]?.string.flatMap { key in windows.contains { $0.id == key } ? key : nil } ?? windows.last?.id
        return (panes, windows, workspaces, key, counters)
    }
}
