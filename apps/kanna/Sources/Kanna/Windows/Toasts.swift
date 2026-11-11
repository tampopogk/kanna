import AppKit

struct ToastAction {
    let title: String
    var primary = false
    let run: @MainActor () -> Void

    init(title: String, primary: Bool = false, run: @escaping @MainActor () -> Void) {
        self.title = title
        self.primary = primary
        self.run = run
    }
}

struct Toast {
    let id: String
    let title: String
    let detail: String?
    let pane: String?
    let task: String?
    let about: String?
    let sticky: Bool
    let recordPath: String?
    var symbol: String?
    var tint: NSColor?
    var warning: String?
    var link: ToastAction?
    var progress: Double?
    var actions: [ToastAction] = []

    init(id: String, title: String, detail: String?, symbol: String, tint: NSColor, sticky: Bool = false, warning: String? = nil, link: ToastAction? = nil, progress: Double? = nil, actions: [ToastAction] = []) {
        self.id = id
        self.title = title
        self.detail = detail
        pane = nil
        task = nil
        about = nil
        self.sticky = sticky
        recordPath = nil
        self.symbol = symbol
        self.tint = tint
        self.warning = warning
        self.link = link
        self.progress = progress
        self.actions = actions
    }

    init?(_ record: JSON) {
        guard let id = record["id"]?.string, let title = record["title"]?.string else { return nil }
        self.id = id
        self.title = title
        detail = record["detail"]?.string
        pane = record["pane"]?.string
        task = record["task"]?.string
        about = record["about"]?.string
        sticky = record["sticky"]?.bool == true
        recordPath = record["record"]?.string
    }

    var opens: Bool { pane != nil || task != nil || recordPath != nil }

    var record: JSON {
        let text = { (value: String?) in value.map { JSON.string($0) } ?? .null }
        return ["id": .string(id), "title": .string(title), "detail": text(detail), "pane": text(pane), "task": text(task), "about": text(about),
                "sticky": .bool(sticky), "record": text(recordPath), "warning": text(warning), "link": text(link?.title), "actions": .array(actions.map { .string($0.title) })]
    }
}

@MainActor
final class ToastStack: NSView {
    static let width: CGFloat = 340
    static let inset: CGFloat = 12
    static let gap: CGFloat = 8
    static let most = 4
    static let life: TimeInterval = 6
    static let aboutLife: TimeInterval = 8
    static let afterHover: TimeInterval = 2
    static let fade: TimeInterval = 0.4

    var onOpen: ((Toast) -> Void)?
    var onClosed: ((Toast) -> Void)?
    private(set) var views: [ToastView] = []

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let hit = super.hitTest(point)
        return hit === self ? nil : hit
    }

    func show(_ toast: Toast) {
        if let old = self.toast(toast.id) { remove(old) }
        let view = ToastView(toast)
        view.onClose = { [weak self, weak view] in
            guard let self, let view else { return }
            remove(view)
            onClosed?(view.toast)
        }
        view.onOpen = { [weak self, weak view] in
            guard let self, let view else { return }
            remove(view)
            onOpen?(view.toast)
        }
        view.onExpire = { [weak self, weak view] in if let view { self?.fadeOut(view) } }
        views.insert(view, at: 0)
        addSubview(view)
        for old in views.dropFirst(Self.most) { fadeOut(old) }
        arrange(animated: false)
        view.alphaValue = 0
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.2
            view.animator().alphaValue = 1
        }
        if !toast.sticky { view.start(after: toast.opens ? Self.aboutLife : Self.life) }
    }

    func toast(_ id: String) -> ToastView? { views.first { $0.toast.id == id } }

    func image() throws -> CGImage {
        let scale = window?.backingScaleFactor ?? 2
        let margin: CGFloat = 24
        let size = NSSize(width: bounds.width + margin * 2, height: bounds.height + margin * 2)
        guard let layer, size.height > margin * 2,
              let context = CGContext(data: nil, width: Int(size.width * scale), height: Int(size.height * scale), bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { throw KannaError("unavailable", "no toasts to draw") }
        context.scaleBy(x: scale, y: scale)
        context.setFillColor(resolved(.windowBackgroundColor))
        context.fill(CGRect(origin: .zero, size: size))
        context.translateBy(x: margin, y: margin + bounds.height)
        context.scaleBy(x: 1, y: -1)
        layer.render(in: context)
        guard let image = context.makeImage() else { throw KannaError("unavailable", "could not draw the toasts") }
        return image
    }

    func remove(_ view: ToastView) {
        view.stop()
        views.removeAll { $0 === view }
        view.removeFromSuperview()
        arrange(animated: true)
    }

    private func fadeOut(_ view: ToastView) {
        guard !view.fading else { return }
        view.fading = true
        view.stop()
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = Self.fade
            view.animator().alphaValue = 0
        }, completionHandler: { [weak self, weak view] in
            MainActor.assumeIsolated {
                guard let self, let view, view.superview === self else { return }
                self.remove(view)
            }
        })
    }

    private var top: CGFloat = 0

    func fit(below top: CGFloat) {
        self.top = top
        arrange(animated: false)
    }

    private func arrange(animated: Bool) {
        var y: CGFloat = 0
        for view in views {
            let target = NSRect(x: 0, y: y, width: Self.width, height: view.height)
            if animated { view.animator().frame = target } else { view.frame = target }
            y += view.height + Self.gap
        }
        guard let content = superview else { return }
        let height = max(0, y - Self.gap)
        frame = NSRect(x: content.bounds.width - Self.width - Self.inset, y: content.bounds.height - top - height, width: Self.width, height: height)
    }
}

@MainActor
final class ToastView: NSView {
    let toast: Toast
    var onOpen: (() -> Void)?
    var onClose: (() -> Void)?
    var onExpire: (() -> Void)?
    var fading = false
    private(set) var hovered = false
    private let icon = NSImageView()
    private let title = NSTextField(labelWithString: "")
    private let detail = NSTextField(wrappingLabelWithString: "")
    private let about = NSTextField(labelWithString: "")
    private let close = NSButton()
    private let warning = NSTextField(wrappingLabelWithString: "")
    private let link = NSButton()
    private let meter = NSView()
    private let filled = NSView()
    private var buttons: [NSButton] = []
    private var timer: Timer?
    private var dueAt: Date?
    private var left: TimeInterval = 0
    private(set) var height: CGFloat = 0

    init(_ toast: Toast) {
        self.toast = toast
        super.init(frame: NSRect(x: 0, y: 0, width: ToastStack.width, height: 60))
        wantsLayer = true
        layer?.cornerRadius = 10
        layer?.borderWidth = 1
        layer?.shadowOpacity = 0.35
        layer?.shadowRadius = 12
        layer?.shadowOffset = CGSize(width: 0, height: -4)
        icon.image = NSImage(systemSymbolName: toast.symbol ?? (toast.recordPath == nil ? "exclamationmark.triangle.fill" : "xmark.octagon.fill"), accessibilityDescription: "Warning")
        icon.contentTintColor = toast.tint ?? (toast.recordPath == nil ? .systemOrange : .systemRed)
        icon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 14, weight: .regular)
        title.stringValue = toast.title
        title.font = .systemFont(ofSize: 13, weight: .semibold)
        title.lineBreakMode = .byTruncatingTail
        detail.stringValue = toast.detail ?? ""
        detail.font = .systemFont(ofSize: 12)
        detail.textColor = .secondaryLabelColor
        detail.maximumNumberOfLines = 2
        detail.lineBreakMode = .byWordWrapping
        detail.cell?.truncatesLastVisibleLine = true
        detail.isHidden = toast.detail == nil
        about.stringValue = toast.recordPath == nil ? toast.about ?? "" : "Open the crash record"
        about.font = .systemFont(ofSize: 11)
        about.textColor = .controlAccentColor
        about.lineBreakMode = .byTruncatingTail
        about.isHidden = !toast.opens || (toast.about == nil && toast.recordPath == nil)
        close.bezelStyle = .inline
        close.isBordered = false
        close.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close")
        close.imagePosition = .imageOnly
        close.contentTintColor = .secondaryLabelColor
        close.target = self
        close.action = #selector(closePressed)
        close.isHidden = true
        close.toolTip = "Close"
        warning.stringValue = toast.warning ?? ""
        warning.font = .systemFont(ofSize: 12)
        warning.textColor = .systemOrange
        warning.maximumNumberOfLines = 2
        warning.isHidden = toast.warning == nil
        link.isBordered = false
        link.attributedTitle = NSAttributedString(string: toast.link?.title ?? "", attributes: [.foregroundColor: NSColor.controlAccentColor, .font: NSFont.systemFont(ofSize: 11)])
        link.target = self
        link.action = #selector(linkPressed)
        link.refusesFirstResponder = true
        link.isHidden = toast.link == nil
        meter.wantsLayer = true
        meter.layer?.cornerRadius = 2
        meter.layer?.masksToBounds = true
        filled.wantsLayer = true
        meter.addSubview(filled)
        meter.isHidden = toast.progress == nil
        progress = toast.progress ?? 0
        for action in toast.actions {
            let button = NSButton(title: action.title, target: self, action: #selector(actionPressed))
            button.bezelStyle = .rounded
            button.controlSize = .small
            button.font = .systemFont(ofSize: 12)
            button.refusesFirstResponder = true
            if action.primary {
                button.bezelColor = .controlAccentColor
                button.contentTintColor = .white
            }
            buttons.append(button)
        }
        for view in [icon, title, detail, about, close, warning, link, meter] + buttons { addSubview(view) }
        toolTip = toast.recordPath ?? (toast.opens ? "Click to show \(toast.about ?? "it")" : nil)
        paint()
        measure()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        paint()
    }

    private func paint() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = (NSColor.windowBackgroundColor.blended(withFraction: 0.06, of: .labelColor) ?? .windowBackgroundColor).cgColor
            layer?.borderColor = (hovered && toast.opens ? NSColor.controlAccentColor : .separatorColor).cgColor
            meter.layer?.backgroundColor = NSColor.separatorColor.cgColor
            filled.layer?.backgroundColor = NSColor.controlAccentColor.cgColor
        }
    }

    private func measure() {
        let text = ToastStack.width - 12 - 18 - 10 - 12 - 18
        var y: CGFloat = 10
        title.frame = NSRect(x: 40, y: y, width: text, height: 17)
        y += 18
        if !detail.isHidden {
            detail.preferredMaxLayoutWidth = text
            let size = detail.sizeThatFits(NSSize(width: text, height: 40))
            detail.frame = NSRect(x: 40, y: y, width: text, height: min(ceil(size.height), 32))
            y += detail.frame.height + 1
        }
        if !about.isHidden {
            about.frame = NSRect(x: 40, y: y + 2, width: text, height: 15)
            y += 18
        }
        if !meter.isHidden {
            meter.frame = NSRect(x: 40, y: y + 4, width: text, height: 4)
            filled.frame = NSRect(x: 0, y: 0, width: text * CGFloat(progress), height: 4)
            y += 10
        }
        if !warning.isHidden {
            warning.preferredMaxLayoutWidth = text
            let size = warning.sizeThatFits(NSSize(width: text, height: 40))
            warning.frame = NSRect(x: 40, y: y + 2, width: text, height: min(ceil(size.height), 32))
            y += warning.frame.height + 3
        }
        if !link.isHidden {
            link.sizeToFit()
            link.frame = NSRect(x: 40, y: y + 2, width: link.frame.width, height: 15)
            y += 18
        }
        if !buttons.isEmpty {
            var x = ToastStack.width - 12
            for button in buttons.reversed() {
                button.sizeToFit()
                x -= button.frame.width
                button.frame = NSRect(x: x, y: y + 6, width: button.frame.width, height: button.frame.height)
                x -= 6
            }
            y += 6 + (buttons.first?.frame.height ?? 0)
        }
        height = y + 10
        icon.frame = NSRect(x: 12, y: 10, width: 18, height: 18)
        close.frame = NSRect(x: ToastStack.width - 28, y: 8, width: 18, height: 18)
    }

    private var progress: Double = 0

    func update(detail text: String, progress: Double) {
        detail.stringValue = text
        self.progress = progress
        filled.frame.size.width = meter.bounds.width * CGFloat(progress)
    }

    @objc private func linkPressed() { toast.link?.run() }

    @objc private func actionPressed(_ sender: NSButton) {
        guard let index = buttons.firstIndex(of: sender) else { return }
        toast.actions[index].run()
    }

    func press(action title: String) -> Bool {
        guard let action = toast.actions.first(where: { $0.title == title }) else { return false }
        action.run()
        return true
    }

    var detailText: String { detail.stringValue }
    var aboutShown: Bool { !about.isHidden }

    func start(after seconds: TimeInterval) {
        left = seconds
        resume()
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    private func resume() {
        stop()
        guard !toast.sticky else { return }
        dueAt = Date().addingTimeInterval(left)
        timer = Timer.scheduledTimer(withTimeInterval: left, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.onExpire?() }
        }
    }

    func hover(_ on: Bool) {
        guard on != hovered, !fading else { return }
        hovered = on
        close.isHidden = !on || !toast.actions.isEmpty
        paint()
        if on {
            left = max(0, dueAt?.timeIntervalSinceNow ?? left)
            stop()
        } else {
            left = max(left, ToastStack.afterHover)
            resume()
        }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas { removeTrackingArea(area) }
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    override func mouseEntered(with event: NSEvent) { hover(true) }
    override func mouseExited(with event: NSEvent) { hover(false) }

    override func resetCursorRects() {
        if toast.opens { addCursorRect(bounds, cursor: .pointingHand) }
    }

    override func mouseDown(with event: NSEvent) {}

    override func mouseUp(with event: NSEvent) {
        guard bounds.contains(convert(event.locationInWindow, from: nil)) else { return }
        guard toast.actions.isEmpty else { return }
        if toast.opens { onOpen?() } else { onClose?() }
    }

    @objc private func closePressed() { onClose?() }

    func press(close: Bool) {
        if close || !toast.opens { onClose?() } else { onOpen?() }
    }
}

extension AppController {
    func showLocal(_ toast: Toast) {
        toasts.show(toast)
        placeToasts()
    }

    func closeLocal(_ id: String) {
        if let view = toasts.toast(id) { toasts.remove(view) }
    }

    func updateLocal(_ id: String, detail: String, progress: Double) {
        toasts.toast(id)?.update(detail: detail, progress: progress)
    }

    func toastArrived(_ record: JSON) {
        guard let toast = Toast(record) else { return }
        toasts.show(toast)
        placeToasts()
    }

    func installToasts() {
        toasts.onOpen = { [weak self] toast in self?.open(toast) }
        toasts.onClosed = { [weak self] toast in self?.dismissCrashes(toast) }
        guard debugCommands else { return }
        commands["debug.toasts"] = { [weak self] args in
            guard let self else { return [:] }
            if let id = args["hover"]?.string {
                guard let view = toasts.toast(id) else { throw KannaError("not_found", "no toast \"\(id)\"") }
                view.hover(args["on"]?.bool ?? true)
            }
            if let id = args["click"]?.string {
                guard let view = toasts.toast(id) else { throw KannaError("not_found", "no toast \"\(id)\"") }
                if let action = args["action"]?.string {
                    guard view.press(action: action) else { throw KannaError("not_found", "no action \"\(action)\"") }
                } else {
                    view.press(close: args["close"]?.bool == true)
                }
            }
            let window = controllers.first { $0.value.window.contentView === toasts.superview }?.key
            if let name = args["png"]?.string {
                let url = try Snapshot.outputURL(name)
                try Snapshot.write(Snapshot.png(try toasts.image()), to: url, replacing: false)
            }
            return ["window": window.map { .string($0) } ?? .null, "toasts": .array(toasts.views.map { view in
                guard case .object(var fields) = view.toast.record else { return .null }
                let frame = view.convert(view.bounds, to: nil)
                fields["frame"] = .array([frame.minX, frame.minY, frame.width, frame.height].map { .number(Double($0)) })
                fields["fading"] = .bool(view.fading)
                fields["hovered"] = .bool(view.hovered)
                fields["aboutShown"] = .bool(view.aboutShown)
                return .object(fields)
            })]
        }
    }

    func placeToasts() {
        let key = keyWindow?["id"]?.string.flatMap { controllers[$0] }
        guard let controller = key ?? controllers.values.first(where: { $0.window.isVisible }), let content = controller.window.contentView else { return }
        if toasts.superview !== content || content.subviews.last !== toasts {
            content.addSubview(toasts, positioned: .above, relativeTo: nil)
            toasts.autoresizingMask = [.minXMargin, .minYMargin]
        }
        toasts.fit(below: TabBar.height + ToastStack.gap)
    }

    private func workspace(holding pane: String) -> JSON? {
        published["workspaces"]?.array?.first { ws in
            ws["dock"]?["pane"]?.string == pane || ws["panes"]?.array?.contains { $0.string == pane } == true
        }
    }

    func open(_ toast: Toast) {
        if toast.recordPath != nil { return openCrashRecord(toast) }
        let ws = toast.pane.flatMap(workspace(holding:))
        if let ws, claimed(ws, as: "desk"), let task = ws["dock"]?["pane"]?.string {
            activateTask(task, into: frontDesk?.id)
            if let pane = toast.pane { layoutCommand("focus", ["pane": .string(pane)]) }
        } else if let ws, claimed(ws, as: "planner"), let desk = physical.desks.first(where: { $0.planner == ws["id"]?.string }) {
            show(window: desk.id)
            if let pane = toast.pane { layoutCommand("focus", ["pane": .string(pane)]) }
        } else if let task = toast.task, ws == nil || ws?["where"]?.string == "shelf" {
            let desk = frontDesk?.id
            request("tasks", ["action": "pick", "task": .string(task)]) { [weak self] reply in
                guard let self else { return }
                guard reply["ok"]?.bool == true, let pane = toast.pane ?? reply["pane"]?.string else {
                    if let desk, let controller = controllers[desk] {
                        controller.note = refusal(reply, otherwise: "it didn't start")
                        refresh()
                    }
                    return
                }
                toastShowing = (pane, desk)
                showToastPane()
            }
        } else if let pane = toast.pane {
            layoutCommand("focus", ["pane": .string(pane)])
        }
    }

    func showToastPane() {
        guard let (pane, desk) = toastShowing, let ws = workspace(holding: pane), claimed(ws, as: "desk"), let task = ws["dock"]?["pane"]?.string else { return }
        toastShowing = nil
        activateTask(task, into: desk)
        layoutCommand("focus", ["pane": .string(pane)])
    }
}
