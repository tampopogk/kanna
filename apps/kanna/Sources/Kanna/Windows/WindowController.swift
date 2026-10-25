import AppKit
import KannaMarkdown
import KannaPerf

@MainActor
final class WindowController: NSObject, NSWindowDelegate {
    static let divider: CGFloat = 1

    let id: String
    let window: NSWindow
    private let stage = StageView()
    private let content = StageView()
    private let dockStage = NSView()
    private let shelfStage = NSView()
    private(set) var dockDivider: DockDividerView?
    var relayout: (() -> Void)? { didSet { content.onLayout = relayout } }
    let tabBar = TabBar()
    let shelfStrip = ShelfStrip()
    let shelfPage = ShelfPage()
    var shelf: [ShelfTask] = []
    var shelfDocks: [JSON] = []
    var backlog: [BacklogTask] = []
    var running = 0
    var note = ""
    var project = ""
    var projectRoot: String?
    private var cheerAt = Int.random(in: 0..<ShelfPage.cheers.count)
    var cheer: String { ShelfPage.cheers[cheerAt] }
    private var deskTask = false
    let agentHeader = AgentHeader()
    var headerText = ""
    var taskTitle: String?
    private(set) var desk = false
    private(set) var emptyDesk = false
    private(set) var leadShown: String?
    var onCloseRequest: (() -> Void)?
    var onBecomeKey: (() -> Void)?
    var onFrameChange: (() -> Void)?
    var onDivider: ((DividerView, Double) -> Void)?
    var onDockWidth: ((String, Double) -> Void)?
    private(set) var shown: [String] = []
    private(set) var tab: String?

    init(id: String, cascadeFrom previous: NSWindow?) {
        self.id = id
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 960, height: 600), styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        super.init()
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.title = "Kanna"
        window.delegate = self
        if AppController.debugging { window.addTitlebarAccessoryViewController(Self.debugLabel()) }
        stage.wantsLayer = true
        let size = window.contentLayoutRect.size
        content.frame = NSRect(origin: .zero, size: size)
        content.wantsLayer = true
        tabBar.frame = NSRect(x: 0, y: size.height - TabBar.height, width: size.width, height: TabBar.height)
        stage.frame = NSRect(x: 0, y: 0, width: size.width, height: size.height - TabBar.height)
        content.addSubview(stage)
        content.addSubview(tabBar)
        content.addSubview(dockStage)
        dockStage.addSubview(agentHeader)
        content.addSubview(shelfStrip)
        content.addSubview(shelfPage)
        content.addSubview(shelfStage)
        shelfStage.isHidden = true
        shelfStrip.isHidden = true
        shelfPage.isHidden = true
        window.contentView = content
        if let previous {
            window.setFrameTopLeftPoint(window.cascadeTopLeft(from: NSPoint(x: previous.frame.minX, y: previous.frame.maxY)))
        } else {
            window.center()
        }
    }

    private static func debugLabel() -> NSTitlebarAccessoryViewController {
        let label = NSTextField(labelWithString: "Debug")
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .medium)
        label.textColor = .secondaryLabelColor
        label.sizeToFit()
        let view = NSView(frame: NSRect(x: 0, y: 0, width: label.frame.width + 16, height: 28))
        label.frame.origin = NSPoint(x: 8, y: (view.frame.height - label.frame.height) / 2)
        view.addSubview(label)
        let accessory = NSTitlebarAccessoryViewController()
        accessory.view = view
        accessory.layoutAttribute = .trailing
        return accessory
    }

    func render(_ state: JSON, _ panes: PaneViews) {
        let span = Perf.begin(.render, "WindowController.render")
        defer { Perf.end(span) }
        tab = state["tab"]?.string
        let instance = AppController.instanceName
        desk = state["desk"]?.bool == true
        let named = desk ? taskTitle ?? "Shelf" : state["lead"]?["name"]?.string ?? "\(state["title"]?.string ?? "Kanna") — \(id)"
        window.title = named + (instance == "default" ? "" : " · \(instance)")
        tabBar.show(state)
        let root = state["root"] ?? .null
        let task = Self.side(state["dock"])
        let lead = task == nil ? Self.side(state["lead"]) : nil
        let dock = task ?? lead
        let docked = dock?["pane"]?.string
        leadShown = lead?["pane"]?.string
        let ids = Self.paneIDs(root)
        shown = Self.shownPanes(state)
        emptyDesk = desk && task == nil && ids.isEmpty
        if deskTask && emptyDesk { cheerAt = (cheerAt + 1) % ShelfPage.cheers.count }
        deskTask = desk && task != nil
        if deskTask, let pulled = task?["pane"]?.string { Clicks.pulled(pulled) }
        let zoomed = state["zoomed"]?.string
        arrange(dock, zoomed: zoomed.map { $0 == docked ? .dock : .tab })
        let visible = Set(ids)
        for case let view as PaneView in stage.subviews where !visible.contains(view.id) { view.removeFromSuperview() }
        for case let view as PaneView in dockStage.subviews where view.id != docked { view.removeFromSuperview() }
        dividers.forEach { $0.removeFromSuperview() }
        dividers = []
        place(root, path: [], in: stage.bounds, panes)
        dividers.forEach { stage.addSubview($0) }
        if let dock, let docked, let view = panes.view(docked, type: dock["type"]?.string ?? "term") {
            if view.superview !== dockStage { dockStage.addSubview(view, positioned: .below, relativeTo: agentHeader) }
            let headed = self.desk || lead != nil
            let header = headed ? AgentHeader.height : 0
            view.frame = NSRect(x: 0, y: 0, width: dockStage.bounds.width, height: dockStage.bounds.height - header)
            view.autoresizingMask = []
            agentHeader.isHidden = !headed
            agentHeader.parkable = lead == nil
            agentHeader.frame = NSRect(x: 0, y: dockStage.bounds.height - header, width: dockStage.bounds.width, height: header)
            agentHeader.show(headerText)
        }
        hold(shelfDocks, panes)
        window.invalidateCursorRects(for: content)
        let focusedID = state["focused"]?.string
        let visibleIDs = ids + (docked.map { [$0] } ?? [])
        let many = state["zoomed"]?.string == nil && shown.count > 1
        for id in visibleIDs {
            guard let view = panes[id] else { continue }
            panes.started(id)
            view.focused = many && id == focusedID && window.isKeyWindow
            view.grip.isHidden = id == docked && dock?["frozen"]?.bool == true
        }
        let responder = window.firstResponder
        if let focusedID, let target = panes.keyView(focusedID), responder !== target,
           responder == nil || responder === window || responder is TerminalSurfaceView || responder is FocusReportingWebView
           || responder is MarkdownTextView || responder is IosPaneView {
            window.makeFirstResponder(target)
        }
    }

    private enum Zoom { case dock, tab }

    private func arrange(_ dock: JSON?, zoomed: Zoom?) {
        var bounds = content.bounds
        let taskless = desk && leadShown != nil || emptyDesk
        if shelfPage.isHidden == emptyDesk {
            shelfPage.isHidden = !emptyDesk
            if !emptyDesk { shelfPage.clearSearch() }
        }
        if shelfStrip.isHidden == (desk && !taskless) { shelfStrip.isHidden = !desk || taskless }
        if taskless {
        } else if desk {
            shelfStrip.frame = NSRect(x: 0, y: 0, width: bounds.width, height: ShelfStrip.height)
            shelfStrip.show(shelf, waiting: backlog.count)
            bounds = NSRect(x: 0, y: ShelfStrip.height, width: bounds.width, height: bounds.height - ShelfStrip.height)
        }
        var side = bounds
        dockDivider?.removeFromSuperview()
        dockDivider = nil
        if let dock, let pane = dock["pane"]?.string {
            if zoomed == .dock {
                dockStage.frame = bounds
                side = .zero
            } else if zoomed != .tab {
                let total = bounds.width - Self.divider
                let left = dock["edge"]?.string != "right"
                dockStage.frame = Self.dockFrame(dock, in: bounds)
                let width = dockStage.frame.width
                side = NSRect(x: left ? width + Self.divider : 0, y: bounds.minY, width: total - width, height: bounds.height)
                let line = NSRect(x: left ? width : side.maxX, y: bounds.minY, width: Self.divider, height: bounds.height)
                let handle = DockDividerView(pane: pane, left: left, line: line, span: total, extent: bounds.width)
                handle.onWidth = { [weak self] width in self?.onDockWidth?(pane, width) }
                content.addSubview(handle)
                dockDivider = handle
            }
        }
        dockStage.isHidden = dock == nil || zoomed == .tab
        if emptyDesk {
            shelfPage.frame = side
            shelfPage.show(DeskPage(tasks: shelf, backlog: backlog, running: running, project: project, cheer: cheer, note: note))
            side = .zero
        }
        tabBar.isHidden = side.isEmpty
        stage.isHidden = side.isEmpty
        let bar = tabBar.isHidden ? 0 : TabBar.height
        tabBar.frame = NSRect(x: side.minX, y: side.maxY - TabBar.height, width: side.width, height: TabBar.height)
        stage.frame = NSRect(x: side.minX, y: side.minY, width: side.width, height: max(0, side.height - bar))
    }

    static func dockFrame(_ dock: JSON, in bounds: NSRect) -> NSRect {
        let total = bounds.width - divider
        let low = min(DividerView.minimum, total / 2)
        let width = min(max((total * CGFloat(dock["width"]?.number ?? 0.4)).rounded(), low), total - low)
        let left = dock["edge"]?.string != "right"
        return NSRect(x: left ? 0 : bounds.width - width, y: bounds.minY, width: width, height: bounds.height)
    }

    private func hold(_ docks: [JSON], _ panes: PaneViews) {
        let held = Set(docks.compactMap { $0["task"]?.string })
        for case let view as PaneView in shelfStage.subviews where !held.contains(view.id) { view.removeFromSuperview() }
        let bounds = content.bounds
        shelfStage.frame = bounds
        let desk = NSRect(x: 0, y: ShelfStrip.height, width: bounds.width, height: max(0, bounds.height - ShelfStrip.height))
        for dock in docks {
            guard let id = dock["task"]?.string, dock["type"]?.string ?? "term" == "term", let view = panes.view(id, type: "term") else { continue }
            if view.superview !== shelfStage { shelfStage.addSubview(view) }
            let frame = Self.dockFrame(dock, in: desk)
            view.frame = NSRect(x: frame.minX, y: frame.minY, width: frame.width, height: max(0, frame.height - AgentHeader.height))
            view.autoresizingMask = []
            panes.started(id)
        }
    }

    private(set) var dividers: [DividerView] = []

    @objc func performTextFinderAction(_ sender: Any?) {
        guard emptyDesk, (sender as? NSMenuItem)?.tag == NSTextFinder.Action.showFindInterface.rawValue else { return }
        shelfPage.focusSearch()
    }

    @objc func validateMenuItem(_ item: NSMenuItem) -> Bool {
        guard item.action == #selector(performTextFinderAction(_:)) else { return true }
        return emptyDesk && shelfPage.searchShown && item.tag == NSTextFinder.Action.showFindInterface.rawValue
    }

    static func shownPanes(_ window: JSON) -> [String] {
        let tab = window["tab"]?.string
        var ids: [String] = []
        if case .array(let tabs)? = window["tabs"], case .array(let list)? = tabs.first(where: { $0["id"]?.string == tab })?["panes"] {
            ids = list.compactMap(\.string)
        }
        guard let dock = window["dock"], let pane = dock["pane"]?.string else {
            guard let lead = window["lead"]?["pane"]?.string else { return ids }
            return [lead] + ids
        }
        return dock["edge"]?.string == "right" ? ids + [pane] : [pane] + ids
    }

    static func side(_ pane: JSON?) -> JSON? {
        guard let pane, pane["pane"]?.string != nil else { return nil }
        return pane
    }

    static func paneIDs(_ node: JSON) -> [String] {
        if let id = node["pane"]?.string { return [id] }
        guard case .array(let children)? = node["children"] else { return [] }
        return children.flatMap(paneIDs)
    }

    private func place(_ node: JSON, path: [Int], in rect: NSRect, _ panes: PaneViews) {
        if let id = node["pane"]?.string {
            guard let view = panes.view(id, type: node["type"]?.string ?? "term") else { return }
            if view.superview !== stage { stage.addSubview(view, positioned: .below, relativeTo: nil) }
            view.frame = rect.integral
            view.autoresizingMask = []
            return
        }
        guard case .array(let children)? = node["children"], !children.isEmpty else { return }
        let horizontal = node["split"]?.string == "row"
        let total = (horizontal ? rect.width : rect.height) - Self.divider * CGFloat(children.count - 1)
        var offset: CGFloat = 0
        var previous: (frame: NSRect, size: Double, last: String?)?
        for (index, child) in children.enumerated() {
            let size = child["size"]?.number ?? 1 / Double(children.count)
            let length = (total * CGFloat(size)).rounded()
            let frame = horizontal
                ? NSRect(x: rect.minX + offset, y: rect.minY, width: length, height: rect.height)
                : NSRect(x: rect.minX, y: rect.maxY - offset - length, width: rect.width, height: length)
            place(child, path: path + [index], in: frame, panes)
            if let previous, let tab {
                let line = horizontal
                    ? NSRect(x: previous.frame.maxX, y: rect.minY, width: Self.divider, height: rect.height)
                    : NSRect(x: rect.minX, y: previous.frame.minY - Self.divider, width: rect.width, height: Self.divider)
                let handle = DividerView(tab: tab, path: path, index: index - 1, row: horizontal, sizes: (previous.size, size), after: previous.last,
                                         line: line, first: previous.frame, second: frame)
                handle.onShare = { [weak self, weak handle] share in
                    guard let self, let handle else { return }
                    onDivider?(handle, share)
                }
                dividers.append(handle)
            }
            previous = (frame, size, child["pane"]?.string)
            offset += length + Self.divider
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        onCloseRequest?()
        return false
    }

    func windowDidMove(_ notification: Notification) { onFrameChange?() }
    func windowDidResize(_ notification: Notification) { onFrameChange?() }
    func windowDidBecomeKey(_ notification: Notification) { onBecomeKey?() }
    func windowDidResignKey(_ notification: Notification) { onBecomeKey?() }
}

private final class StageView: NSView {
    var onLayout: (() -> Void)?
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        paint()
    }
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }
    private func paint() { layer?.backgroundColor = resolved(.separatorColor) }
    override func layout() {
        super.layout()
        onLayout?()
    }
}
