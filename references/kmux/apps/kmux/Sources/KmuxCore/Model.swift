import Foundation

public enum PaneType: String, Sendable, CaseIterable {
    case term, web, ios, md

    /// The kind of content a pane of this type shows, which its content ID
    /// starts with: term-1, page-1 (web), file-1 (md), ios-1.
    public var kind: String {
        switch self {
        case .term: "term"
        case .web: "page"
        case .md: "file"
        case .ios: "ios"
        }
    }
}
public enum PaneState: String, Sendable { case starting, running, exited, failed }
public enum Axis: String, Sendable { case row, column }
public enum Edge: String, Sendable { case left, right }

/// A pane's content, by its content ID. A terminal is one program run, so
/// restarting it gives it a new ID; a page, a file or an iOS app keeps its ID.
public final class Pane {
    public internal(set) var id: String
    public var name: String?
    public let type: PaneType
    public var state: PaneState = .starting
    public var exitCode: Int?
    public var error: String?
    public var command: String?
    public var cwd: String?
    public var url: String?
    /// iOS panes: the app (a .app path or a bundle ID) and the simulator
    /// device it runs on (a name or UDID; the host fills in the one it chose).
    public var app: String?
    public var device: String?
    /// The markdown file a `md` pane shows.
    public var path: String?
    /// Back/forward history: of URLs for web panes opened with `history: true`;
    /// of files, always, for markdown panes.
    public var history: History?
    /// Markdown panes: the magnification, kept across restarts (the host
    /// fills it in before the layout is saved).
    public var zoom: Double?
    /// Terminals: reserved for re-attaching a hosted terminal session. Saved
    /// and restored, unused for now.
    public var session: String?

    init(id: String, name: String?, type: PaneType) {
        self.id = id
        self.name = name
        self.type = type
    }

    public var label: String { name.map { "\($0) (\(id))" } ?? id }
}

public final class Split {
    public var axis: Axis
    public var kids: [Child]

    init(axis: Axis, kids: [Child]) {
        self.axis = axis
        self.kids = kids
    }
}

public struct Child {
    public var node: Node
    public var size: Double
}

public enum Node {
    case pane(String)
    case split(Split)
}

public final class Tab {
    public let id: String
    public var title: String
    public var root: Node?
    public var lastFocus: String?

    init(id: String, title: String) {
        self.id = id
        self.title = title
    }
}

public final class Window {
    public let id: String
    public var tabs: [Tab] = []
    public var active: String?
    public var focused: String?
    public var zoom = false
    /// Where the window is on screen, in the host's coordinates; saved with
    /// the layout so a restored window comes back in place.
    public var frame: Frame?
    public var dock: Dock?
    /// The ID of the workspace the window is in, or nil: a free window.
    public var workspace: String?
    /// Tabs are named "Tab 1", "Tab 2", … in the order the window made them.
    var tabCount = 0

    init(id: String) { self.id = id }

    public var zoomed: String? { zoom ? focused : nil }

    public var activeTab: Tab? { tabs.first { $0.id == active } ?? tabs.first }

    public var shownPanes: [String] {
        let ids = Model.paneIDs(activeTab?.root)
        guard let dock else { return ids }
        return dock.edge == .left ? [dock.pane] + ids : ids + [dock.pane]
    }
}

/// A named group of windows, shown or hidden together. A hidden workspace's
/// windows are off screen, their panes still running.
public final class Workspace {
    public let id: String
    public var name: String?
    public var hidden = false

    init(id: String, name: String?) {
        self.id = id
        self.name = name
    }

    public var label: String { name ?? id }
}

public struct Dock: Sendable, Equatable {
    public static let defaultWidth = 2.0 / 5
    public var pane: String
    public var edge: Edge
    public var width: Double
    /// A frozen docked pane doesn't move, and nothing drops on it.
    public var frozen: Bool

    public init(pane: String, edge: Edge, width: Double, frozen: Bool = false) {
        self.pane = pane
        self.edge = edge
        self.width = width
        self.frozen = frozen
    }
}

public struct Frame: Sendable, Equatable {
    public var x, y, w, h: Double
    public init(x: Double, y: Double, w: Double, h: Double) {
        self.x = x
        self.y = y
        self.w = w
        self.h = h
    }
}

struct Location {
    let window: Window
    let tab: Tab
    let parent: Split?
    let index: Int
}

/// Windows, tabs and split trees. Sizes are fractions of the parent split and
/// always add up to 1. Empty containers close: a tab without panes, then a
/// window without tabs. Mirrors reference/kmux/index.html.
public final class Model {
    public private(set) var panes: [String: Pane] = [:]
    public private(set) var paneOrder: [String] = []
    public private(set) var windows: [Window] = []
    public private(set) var workspaces: [Workspace] = []
    public var key: String?
    private var counters = Counters()

    public init() {}

    /// The key window: always one on screen.
    public var keyWindow: Window? { windows.first { $0.id == key && isShown($0) } }

    public func window(_ id: String) -> Window? { windows.first { $0.id == id } }

    public func workspace(_ ref: String) -> Workspace? { workspaces.first { $0.id == ref } ?? workspaces.first { $0.name == ref } }

    public func workspace(of window: Window) -> Workspace? { window.workspace.flatMap { id in workspaces.first { $0.id == id } } }

    /// Whether a window is on screen: it's free, or its workspace isn't hidden.
    public func isShown(_ window: Window) -> Bool { workspace(of: window)?.hidden != true }

    public var shownWindows: [Window] { windows.filter(isShown) }

    /// Makes a window key, if it's on screen.
    func raise(_ window: Window) {
        if isShown(window) { key = window.id }
    }

    func makeWorkspace(name: String?) -> Workspace {
        counters.workspace += 1
        let workspace = Workspace(id: "ws-\(counters.workspace)", name: name)
        workspaces.append(workspace)
        return workspace
    }

    func forget(_ workspace: Workspace) {
        workspaces.removeAll { $0 === workspace }
        for window in windows where window.workspace == workspace.id { window.workspace = nil }
    }

    public func pane(_ ref: String) -> Pane? {
        panes[ref] ?? paneOrder.lazy.compactMap { self.panes[$0] }.first { $0.name == ref }
    }

    public func tab(_ id: String) -> Tab? { windows.lazy.flatMap(\.tabs).first { $0.id == id } }

    func makeWindow() -> Window {
        counters.window += 1
        let window = Window(id: "win-\(counters.window)")
        windows.append(window)
        key = window.id
        return window
    }

    @discardableResult
    func makeTab(in window: Window, activate: Bool = true) -> Tab {
        counters.tab += 1
        window.tabCount += 1
        let tab = Tab(id: "tab-\(counters.tab)", title: "Tab \(window.tabCount)")
        window.tabs.append(tab)
        if activate || window.active == nil { window.active = tab.id }
        return tab
    }

    func makePane(type: PaneType, name: String?) -> Pane {
        let pane = Pane(id: nextContentID(type), name: name, type: type)
        panes[pane.id] = pane
        paneOrder.append(pane.id)
        return pane
    }

    func nextContentID(_ type: PaneType) -> String {
        counters.content[type.kind, default: 0] += 1
        return "\(type.kind)-\(counters.content[type.kind]!)"
    }

    /// Gives a pane's content a new ID where it is shown: in its tab's tree or
    /// its window's dock, as the focus and a tab's last focus. It keeps its
    /// place in the list and its name.
    func rename(_ pane: Pane, to id: String) {
        let from = pane.id
        panes[from] = nil
        pane.id = id
        panes[id] = pane
        paneOrder = paneOrder.map { $0 == from ? id : $0 }
        func renamed(_ node: Node?) -> Node? {
            switch node {
            case nil: return nil
            case .pane(let leaf): return .pane(leaf == from ? id : leaf)
            case .split(let split):
                for i in split.kids.indices { split.kids[i].node = renamed(split.kids[i].node)! }
                return node
            }
        }
        for window in windows {
            if window.dock?.pane == from { window.dock?.pane = id }
            if window.focused == from { window.focused = id }
            for tab in window.tabs {
                if tab.lastFocus == from { tab.lastFocus = id }
                tab.root = renamed(tab.root)
            }
        }
    }

    /// Replaces everything with a restored layout. IDs are never reused while
    /// kmux runs, so the counters only move forward.
    func install(panes: [Pane], windows: [Window], workspaces: [Workspace], key: String?, counters: Counters) {
        self.panes = Dictionary(uniqueKeysWithValues: panes.map { ($0.id, $0) })
        paneOrder = panes.map(\.id)
        self.windows = windows
        self.workspaces = workspaces
        self.key = key
        self.counters = self.counters.max(counters)
    }

    var savedCounters: Counters { counters }

    func forget(_ pane: Pane) {
        panes[pane.id] = nil
        paneOrder.removeAll { $0 == pane.id }
    }

    // MARK: Trees

    public static func paneIDs(_ node: Node?) -> [String] {
        switch node {
        case nil: []
        case .pane(let id): [id]
        case .split(let split): split.kids.flatMap { paneIDs($0.node) }
        }
    }

    static func locate(_ node: Node?, _ id: String, parent: Split? = nil, index: Int = -1) -> (parent: Split?, index: Int)? {
        switch node {
        case nil: return nil
        case .pane(let paneID): return paneID == id ? (parent, index) : nil
        case .split(let split):
            for (i, kid) in split.kids.enumerated() {
                if let found = locate(kid.node, id, parent: split, index: i) { return found }
            }
            return nil
        }
    }

    public func dockWindow(_ id: String) -> Window? { windows.first { $0.dock?.pane == id } }

    public func window(containing id: String) -> Window? { locate(id)?.window ?? dockWindow(id) }

    func dock(_ id: String, in window: Window, edge: Edge, frozen: Bool = false) {
        if let old = window.dock, old.pane != id { append(old.pane, to: makeTab(in: window, activate: false)) }
        window.dock = Dock(pane: id, edge: edge, width: window.dock?.width ?? Dock.defaultWidth, frozen: frozen)
    }

    func locate(_ id: String) -> Location? {
        for window in windows {
            for tab in window.tabs {
                if let found = Self.locate(tab.root, id) { return Location(window: window, tab: tab, parent: found.parent, index: found.index) }
            }
        }
        return nil
    }

    static func normalize(_ node: Node?) -> Node? {
        guard case .split(let split) = node else { return node }
        for i in split.kids.indices { split.kids[i].node = normalize(split.kids[i].node)! }
        return split.kids.count == 1 ? split.kids[0].node : node
    }

    /// Removes a pane from its tree; its siblings share the freed space.
    func detach(_ id: String) {
        if let window = dockWindow(id) {
            window.dock = nil
            return
        }
        guard let location = locate(id) else { return }
        if let parent = location.parent {
            let gone = parent.kids.remove(at: location.index).size
            let count = Double(parent.kids.count)
            for i in parent.kids.indices { parent.kids[i].size = gone < 1 ? parent.kids[i].size / (1 - gone) : 1 / count }
            location.tab.root = Self.normalize(location.tab.root)
        } else {
            location.tab.root = nil
        }
    }

    /// Puts `node` next to the target pane, taking `size` of the target's space.
    /// A parent split along the same axis takes it as a sibling.
    func insert(_ node: Node, beside targetID: String, in tab: Tab, axis: Axis, after: Bool, size: Double) {
        guard let found = Self.locate(tab.root, targetID) else { return }
        if let parent = found.parent, parent.axis == axis {
            let share = parent.kids[found.index].size
            parent.kids[found.index].size = share * (1 - size)
            parent.kids.insert(Child(node: node, size: share * size), at: found.index + (after ? 1 : 0))
            return
        }
        var kids = [Child(node: .pane(targetID), size: 1 - size), Child(node: node, size: size)]
        if !after { kids.reverse() }
        let split = Node.split(Split(axis: axis, kids: kids))
        if let parent = found.parent { parent.kids[found.index].node = split } else { tab.root = split }
    }

    /// Adds a pane to a tab: as its root, or as a new column on the right
    /// (all columns then share the width equally).
    func append(_ id: String, to tab: Tab) {
        guard let root = tab.root else {
            tab.root = .pane(id)
            return
        }
        if case .split(let split) = root, split.axis == .row {
            let count = Double(split.kids.count)
            for i in split.kids.indices { split.kids[i].size *= count / (count + 1) }
            split.kids.append(Child(node: .pane(id), size: 1 / (count + 1)))
        } else {
            tab.root = .split(Split(axis: .row, kids: [Child(node: root, size: 0.5), Child(node: .pane(id), size: 0.5)]))
        }
    }

    /// Puts pane `b` where pane `a` is, and `a` where `b` is.
    func swap(_ a: String, _ b: String) {
        let la = locate(a), lb = locate(b), da = dockWindow(a), db = dockWindow(b)
        guard la != nil || da != nil, lb != nil || db != nil else { return }
        if let la { setLeaf(la, to: b) } else { da?.dock?.pane = b }
        if let lb { setLeaf(lb, to: a) } else { db?.dock?.pane = a }
    }

    private func setLeaf(_ location: Location, to id: String) {
        if let parent = location.parent { parent.kids[location.index].node = .pane(id) } else { location.tab.root = .pane(id) }
    }

    public func window(of tab: Tab) -> Window? { windows.first { $0.tabs.contains { $0 === tab } } }

    func moveTab(_ tab: Tab, to destination: Window, index: Int?) {
        guard let source = window(of: tab), let from = source.tabs.firstIndex(where: { $0 === tab }) else { return }
        source.tabs.remove(at: from)
        var index = index ?? destination.tabs.count
        if source === destination, index > from { index -= 1 }
        destination.tabs.insert(tab, at: min(index, destination.tabs.count))
    }

    /// Closes empty tabs and windows and repairs focus.
    func dropEmpty() {
        for window in windows {
            let at = max(0, window.tabs.firstIndex { $0.id == window.active } ?? 0)
            window.tabs.removeAll { $0.root == nil }
            if !window.tabs.contains(where: { $0.id == window.active }) {
                window.active = window.tabs.isEmpty ? nil : window.tabs[min(at, window.tabs.count - 1)].id
            }
            let ids = window.shownPanes
            if let focused = window.focused, ids.contains(focused) { continue }
            if let last = window.activeTab?.lastFocus, ids.contains(last) {
                window.focused = last
            } else {
                window.focused = Self.paneIDs(window.activeTab?.root).first ?? window.dock?.pane
            }
        }
        windows.removeAll { $0.tabs.isEmpty && $0.dock == nil }
        if keyWindow == nil { key = shownWindows.last?.id }
    }

    // MARK: Views for the protocol

    public func tree(_ node: Node?, size: Double? = nil) -> JSON {
        var out: [String: JSON]
        switch node {
        case nil: return .null
        case .pane(let id): out = ["pane": .string(panes[id]?.name ?? id)]
        case .split(let split): out = ["split": .string(split.axis.rawValue), "children": .array(split.kids.map { tree($0.node, size: $0.size) })]
        }
        if let size, size < 1 - 1e-6 { out["size"] = .string(Fraction.format(size)) }
        return .object(out)
    }

    public func dockView(_ window: Window) -> JSON {
        guard let dock = window.dock else { return .null }
        var out: [String: JSON] = ["pane": .string(panes[dock.pane]?.name ?? dock.pane), "edge": .string(dock.edge.rawValue), "width": .string(Fraction.format(dock.width))]
        if dock.frozen { out["frozen"] = true }
        return .object(out)
    }

    public func workspaceView(_ workspace: Workspace) -> JSON {
        ["id": .string(workspace.id), "name": workspace.name.map(JSON.string) ?? .null, "hidden": .bool(workspace.hidden)]
    }

    public func summary(_ pane: Pane) -> JSON {
        var out: [String: JSON] = ["id": .string(pane.id), "name": pane.name.map(JSON.string) ?? .null, "type": .string(pane.type.rawValue), "state": .string(pane.state.rawValue)]
        if pane.type == .term {
            out["cmd"] = pane.command.map(JSON.string) ?? .null
            out["cwd"] = pane.cwd.map(JSON.string) ?? .null
        }
        if pane.type == .web {
            out["url"] = pane.url.map(JSON.string) ?? .null
            out["history"] = .bool(pane.history != nil)
        }
        if pane.type == .ios {
            out["app"] = pane.app.map(JSON.string) ?? .null
            out["device"] = pane.device.map(JSON.string) ?? .null
        }
        if pane.type == .md { out["path"] = pane.path.map(JSON.string) ?? .null }
        if let code = pane.exitCode { out["exitCode"] = .number(Double(code)) }
        if let error = pane.error { out["error"] = .string(error) }
        return .object(out)
    }
}

/// The last number given to each kind of content, tab, window and workspace.
struct Counters {
    var content: [String: Int] = [:]
    var tab = 0, window = 0, workspace = 0

    func max(_ other: Counters) -> Counters {
        Counters(content: content.merging(other.content, uniquingKeysWith: Swift.max), tab: Swift.max(tab, other.tab),
                 window: Swift.max(window, other.window), workspace: Swift.max(workspace, other.workspace))
    }
}

public struct History {
    public var back: [String] = []
    public var forward: [String] = []
}
