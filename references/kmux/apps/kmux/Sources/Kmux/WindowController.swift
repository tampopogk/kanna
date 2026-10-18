import AppKit
import WebKit
import KmuxCore
import KmuxMarkdown

/// One kmux window: draws the active tab's split tree.
@MainActor
final class WindowController: NSObject, NSWindowDelegate {
    static let divider: CGFloat = 1

    let id: String
    let window: NSWindow
    private let stage = StageView()
    private let content = StageView()
    private let dockStage = NSView()
    private(set) var dockDivider: DockDividerView?
    /// Redraws from the model; called when the window resizes.
    var relayout: (() -> Void)? { didSet { content.onLayout = relayout } }
    let tabBar = TabBar()
    var onCloseRequest: (() -> Void)?
    var onBecomeKey: (() -> Void)?
    /// The window moved or was resized.
    var onFrameChange: (() -> Void)?
    /// A divider was dropped: the `resize` request's arguments.
    var onResize: ((JSON) -> Void)?
    /// A divider being dragged: drawn where it is until the core has the
    /// `resize` it ends with.
    var preview: Preview?
    enum Preview {
        case divider(Split, index: Int, share: Double)
        case dock(width: Double)
    }

    /// Named instances show their name in the title, to tell them apart.
    let instance: Instance

    init(id: String, instance: Instance, cascadeFrom previous: NSWindow?) {
        self.id = id
        self.instance = instance
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 960, height: 600), styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        super.init()
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.title = Brand.current.displayName
        window.delegate = self
        stage.wantsLayer = true
        stage.layer?.backgroundColor = NSColor.separatorColor.cgColor
        let size = window.contentLayoutRect.size
        content.frame = NSRect(origin: .zero, size: size)
        content.wantsLayer = true
        content.layer?.backgroundColor = NSColor.separatorColor.cgColor
        tabBar.frame = NSRect(x: 0, y: size.height - TabBar.height, width: size.width, height: TabBar.height)
        stage.frame = NSRect(x: 0, y: 0, width: size.width, height: size.height - TabBar.height)
        content.addSubview(stage)
        content.addSubview(tabBar)
        content.addSubview(dockStage)
        window.contentView = content
        if let previous { window.setFrameTopLeftPoint(window.cascadeTopLeft(from: previous.frame.origin + NSPoint(x: 0, y: previous.frame.height))) } else { window.center() }
    }

    func render(_ model: Model, _ host: ContentHost) {
        guard let state = model.window(id), state.activeTab != nil || state.dock != nil else { return }
        let tab = state.activeTab
        let name = tab?.title ?? state.dock.map { model.panes[$0.pane]?.name ?? $0.pane } ?? ""
        window.title = "\(name) — \(id)" + (instance.isDefault ? "" : " · \(instance.name)")
        tabBar.show(model, state)
        let dockZoomed = state.dock != nil && state.zoomed == state.dock?.pane
        arrange(state)
        let root = dockZoomed ? nil : state.zoomed.map { Node.pane($0) } ?? tab?.root
        let shown = Set(Model.paneIDs(root))
        for case let view as PaneView in stage.subviews where !shown.contains(view.id) { view.removeFromSuperview() }
        for case let view as PaneView in dockStage.subviews where view.id != state.dock?.pane { view.removeFromSuperview() }
        dividers.forEach { $0.removeFromSuperview() }
        dividers = []
        place(root, in: stage.bounds, host, tab: tab?.id ?? "", path: [])
        dividers.forEach { stage.addSubview($0) }
        if let dock = state.dock, let view = host.views[dock.pane] {
            if view.superview !== dockStage { dockStage.addSubview(view) }
            view.frame = dockStage.bounds
            view.autoresizingMask = []
        }
        window.invalidateCursorRects(for: content)
        let visible = shown.union(state.dock.map { [$0.pane] } ?? [])
        let many = state.zoomed == nil && state.shownPanes.count > 1
        for id in visible {
            guard let view = host.views[id], let pane = model.panes[id] else { continue }
            view.show(pane)
            view.focused = many && id == state.focused && window.isKeyWindow
            view.grip.isHidden = state.dock?.pane == id && state.dock?.frozen == true
        }
        // Keyboard focus follows the model's focused pane, but only takes over
        // from another pane's view or from nothing: a tab being renamed keeps it.
        let responder = window.firstResponder
        if let focused = state.focused, let target = host.keyView(focused), responder !== target,
           responder == nil || responder === window || responder is TerminalSurfaceView || responder is FocusReportingWebView
           || responder is MarkdownTextView {
            window.makeFirstResponder(target)
        }
    }

    private func arrange(_ state: Window) {
        let bounds = content.bounds
        var side = bounds
        dockDivider?.removeFromSuperview()
        dockDivider = nil
        if let dock = state.dock {
            if state.zoomed == dock.pane {
                dockStage.frame = bounds
                side = .zero
            } else if state.zoomed == nil {
                let total = bounds.width - Self.divider
                let low = min(DividerView.minimum, total / 2)
                let share = if case .dock(let width)? = preview { width } else { dock.width }
                let width = min(max((total * share).rounded(), low), total - low)
                let left = dock.edge == .left
                dockStage.frame = NSRect(x: left ? 0 : bounds.width - width, y: 0, width: width, height: bounds.height)
                side = NSRect(x: left ? width + Self.divider : 0, y: 0, width: total - width, height: bounds.height)
                let line = NSRect(x: left ? width : side.maxX, y: 0, width: Self.divider, height: bounds.height)
                let handle = DockDividerView(window: state, line: line, span: total, extent: bounds.width)
                handle.onPreview = { [weak self] width in
                    self?.preview = .dock(width: width)
                    self?.relayout?()
                }
                handle.onResize = { [weak self] args in self?.onResize?(args) }
                content.addSubview(handle)
                dockDivider = handle
            }
        }
        let tabZoomed = state.dock != nil && state.zoomed != nil && state.zoomed != state.dock?.pane
        dockStage.isHidden = state.dock == nil || tabZoomed
        tabBar.isHidden = side.isEmpty || tabZoomed
        stage.isHidden = side.isEmpty
        let bar = tabBar.isHidden ? 0 : TabBar.height
        tabBar.frame = NSRect(x: side.minX, y: side.maxY - TabBar.height, width: side.width, height: TabBar.height)
        stage.frame = NSRect(x: side.minX, y: side.minY, width: side.width, height: max(0, side.height - bar))
    }

    /// The handles over the dividers of the tab on screen, rebuilt by each render.
    private(set) var dividers: [DividerView] = []

    private func place(_ node: Node?, in rect: NSRect, _ host: ContentHost, tab: String, path: [Int]) {
        switch node {
        case nil: return
        case .pane(let id):
            guard let view = host.views[id] else { return }
            if view.superview !== stage { stage.addSubview(view) }
            view.frame = rect.integral
            view.autoresizingMask = []
        case .split(let split):
            let horizontal = split.axis == .row
            let total = (horizontal ? rect.width : rect.height) - Self.divider * CGFloat(split.kids.count - 1)
            var offset: CGFloat = 0
            var previous: NSRect?
            var sizes = split.kids.map(\.size)
            if case .divider(let dragged, let index, let share)? = preview, dragged === split, index + 1 < sizes.count {
                (sizes[index], sizes[index + 1]) = (share, sizes[index] + sizes[index + 1] - share)
            }
            for (index, kid) in split.kids.enumerated() {
                let length = (total * sizes[index]).rounded()
                let frame = horizontal
                    ? NSRect(x: rect.minX + offset, y: rect.minY, width: length, height: rect.height)
                    : NSRect(x: rect.minX, y: rect.maxY - offset - length, width: rect.width, height: length)
                place(kid.node, in: frame, host, tab: tab, path: path + [index])
                if let previous {
                    let line = horizontal
                        ? NSRect(x: previous.maxX, y: rect.minY, width: Self.divider, height: rect.height)
                        : NSRect(x: rect.minX, y: previous.minY - Self.divider, width: rect.width, height: Self.divider)
                    let handle = DividerView(split: split, tab: tab, path: path, index: index - 1, line: line, first: previous, second: frame)
                    handle.onPreview = { [weak self] share in
                        self?.preview = .divider(split, index: index - 1, share: share)
                        self?.relayout?()
                    }
                    handle.onResize = { [weak self] args in self?.onResize?(args) }
                    dividers.append(handle)
                }
                previous = frame
                offset += length + Self.divider
            }
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
    override func layout() {
        super.layout()
        onLayout?()
    }
}

private func + (a: NSPoint, b: NSPoint) -> NSPoint { NSPoint(x: a.x + b.x, y: a.y + b.y) }
