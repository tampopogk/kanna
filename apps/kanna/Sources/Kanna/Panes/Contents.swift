import AppKit
import KannaCode
import KannaDiff
import KannaMarkdown

@MainActor
final class PaneSlot: NSView {
    var content: NSView? {
        didSet {
            oldValue?.removeFromSuperview()
            if let content {
                content.frame = bounds
                content.autoresizingMask = [.width, .height]
                addSubview(content)
            }
        }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        paint()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    private func paint() { layer?.backgroundColor = resolved(.windowBackgroundColor) }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
}

@MainActor
final class Contents {
    private var slots: [String: PaneSlot] = [:]
    private(set) var states: [String: String] = [:]
    private var errors: [String: String] = [:]
    var onUpdate: ((String, [String: JSON]) -> Void)?
    var onRequest: ((String, [String: JSON]) -> Void)?
    var onNotice: ((String, [String: JSON]) -> Void)?
    var onCall: ((String, [String: JSON], @escaping (JSON) -> Void) -> Void)?
    var onFocus: ((String) -> Void)?
    var contextMenu: (() -> NSMenu?)?
    var agentStatus: ((String) -> String?)?
    private var markdownSpots: [String: [String: (y: CGFloat, zoom: CGFloat)]] = [:]
    private var markdownZoom: [String: CGFloat] = [:]
    private var codeZoom: [String: CGFloat] = [:]
    private var diffZoom: [String: CGFloat] = [:]
    private var diffSources: [String: (path: String, args: [String])] = [:]
    private var diffContents: [String: String] = [:]
    private var diffLatest: [String: JSON] = [:]

    func slot(_ id: String) -> PaneSlot {
        if let slot = slots[id] { return slot }
        let slot = PaneSlot(frame: .zero)
        slots[id] = slot
        return slot
    }

    func agentStatusChanged(_ pane: String, _ status: String?) {
        for id in ids {
            if let markdown = markdown(id), markdown.agent == pane { markdown.agentStatus = status }
            if let web = web(id), web.agent == pane { web.agentStatus = status }
        }
    }

    func shows(_ id: String) -> Bool { slots[id]?.content != nil }
    var ids: [String] { slots.keys.filter { slots[$0]?.content != nil }.sorted() }

    func web(_ id: String) -> WebPaneView? { slots[id]?.content as? WebPaneView }
    func ios(_ id: String) -> IosPaneView? { slots[id]?.content as? IosPaneView }
    func markdown(_ id: String) -> MarkdownPaneView? { slots[id]?.content as? MarkdownPaneView }
    func code(_ id: String) -> CodePaneView? { slots[id]?.content as? CodePaneView }
    func diff(_ id: String) -> DiffPaneView? { slots[id]?.content as? DiffPaneView }
    func keyView(_ id: String) -> NSView? { web(id)?.webView ?? ios(id) ?? markdown(id)?.keyView ?? code(id)?.keyView ?? diff(id)?.keyView }

    private func send(_ text: String, to agent: String, done: @escaping (String?) -> Void) {
        guard let onCall else { return done("not connected to the daemon") }
        onCall("send", ["pane": .string(agent), "text": .string(text)]) { answer in
            guard answer["ok"]?.bool == true else { return done(answer["error"]?["message"]?.string ?? "send failed") }
            done(nil)
        }
    }

    func resend(_ id: String) {
        guard let state = states[id] else { return }
        var fields: [String: JSON] = ["state": .string(state)]
        if let error = errors[id] { fields["error"] = .string(error) }
        onUpdate?(id, fields)
    }

    private func report(_ id: String, _ fields: [String: JSON]) {
        if let state = fields["state"]?.string {
            states[id] = state
            errors[id] = fields["error"]?.string
        }
        onUpdate?(id, fields)
    }

    func start(_ pane: JSON, restoring: Bool = false) {
        guard let id = pane["id"]?.string, let type = pane["type"]?.string, type != "term" else { return }
        stop(id)
        states[id] = "starting"
        let content: NSView
        var failure: String?
        switch type {
        case "md":
            let path = Self.absolute(pane["path"]?.string ?? "")
            var directory: ObjCBool = false
            if !FileManager.default.fileExists(atPath: path, isDirectory: &directory) || directory.boolValue {
                content = NSView()
                failure = directory.boolValue ? "\(path) is a folder, not a markdown file" : "No such file: \(path)"
            } else {
                let spot = markdownSpots[id]?[path]
                let markdown = MarkdownPaneView(path: path, zoom: spot?.zoom ?? markdownZoom[id] ?? pane["zoom"]?.number.map { CGFloat($0) } ?? 1, markup: pane["markup"]?.bool == true)
                markdown.startScrollY = spot?.y
                markdown.onHistory = { [weak self] back in self?.onRequest?("navigate", ["pane": .string(id), back ? "back" : "forward": true]) }
                markdown.onZoom = { [weak self, weak markdown] in
                    guard let markdown else { return }
                    self?.onUpdate?(id, ["zoom": .number(Double(markdown.zoom))])
                }
                markdown.onFocus = { [weak self] in self?.onFocus?(id) }
                markdown.agent = pane["agent"]?.string
                markdown.agentStatus = markdown.agent.flatMap { self.agentStatus?($0) }
                markdown.onSend = { [weak self] agent, text, done in self?.send(text, to: agent, done: done) }
                markdown.contextMenu = { [weak self] in self?.contextMenu?() }
                markdown.onOpenMarkdown = { [weak self] target in self?.onRequest?("navigate", ["pane": .string(id), "path": .string(target)]) }
                content = markdown
            }
        case "code":
            let path = pane["path"]?.string ?? ""
            let note = pane["comment"]
            let comment = note?["path"]?.string == path ? note.flatMap { note in
                note["first"]?.number.map { CodeComment(first: Int($0), last: Int(note["last"]?.number ?? $0), text: note["text"]?.string ?? "") }
            } : nil
            let code = CodePaneView(path: Self.absolute(path), line: pane["line"]?.number.map { Int($0) }, zoom: codeZoom[id] ?? pane["zoom"]?.number.map { CGFloat($0) } ?? 1, comment: comment)
            code.onZoom = { [weak self, weak code] in
                guard let code else { return }
                self?.codeZoom[id] = code.zoom
                self?.onUpdate?(id, ["zoom": .number(Double(code.zoom))])
            }
            code.onHistory = { [weak self] back in self?.onRequest?("navigate", ["pane": .string(id), back ? "back" : "forward": true]) }
            code.onFocus = { [weak self] in self?.onFocus?(id) }
            code.contextMenu = { [weak self] in self?.contextMenu?() }
            failure = code.failure?.message
            content = code
        case "diff":
            let diff = DiffPaneView(view: pane["view"]?.string, zoom: diffZoom[id] ?? pane["zoom"]?.number.map { CGFloat($0) } ?? 1)
            diff.onFocus = { [weak self] in self?.onFocus?(id) }
            diff.contextMenu = { [weak self] in self?.contextMenu?() }
            diff.onZoom = { [weak self, weak diff] in
                guard let diff else { return }
                self?.diffZoom[id] = diff.zoom
                self?.onUpdate?(id, ["zoom": .number(Double(diff.zoom))])
            }
            diff.onView = { [weak self] view in self?.onUpdate?(id, ["view": .string(view)]) }
            diff.onOpen = { [weak self] path, line in
                self?.onCall?("open", ["type": "code", "path": .string(path), "line": .number(Double(line)), "from": .string(id)]) { _ in }
            }
            diff.onShowFiles = { [weak self] paths in
                self?.onNotice?("diff.show", ["view": .string(id), "show": .array(paths.map { .string($0) })])
            }
            var diffArgs: [String] = []
            if case .array(let list)? = pane["args"] { diffArgs = list.compactMap(\.string) }
            diffSources[id] = (pane["path"]?.string ?? "", diffArgs)
            content = diff
            subscribe(diff: diff, id)
        case "web":
            let web = WebPaneView(url: pane["url"]?.string ?? "about:blank", markup: pane["markup"]?.bool == true)
            web.onFocus = { [weak self] in self?.onFocus?(id) }
            web.webView.contextMenu = { [weak self] in self?.contextMenu?() }
            web.onNavigate = { [weak self] url in self?.onRequest?("navigate", ["pane": .string(id), "url": .string(url)]) }
            web.onURLChange = { [weak self] url in self?.onUpdate?(id, ["url": .string(url)]) }
            web.onTitleChange = { [weak self] title in self?.onUpdate?(id, ["title": .string(title)]) }
            web.agent = pane["agent"]?.string
            web.agentStatus = web.agent.flatMap { self.agentStatus?($0) }
            web.onSend = { [weak self] agent, text, done in self?.send(text, to: agent, done: done) }
            content = web
        case "ios":
            let ios = IosPaneView()
            ios.onFocus = { [weak self] in self?.onFocus?(id) }
            ios.contextMenu = { [weak self] in self?.contextMenu?() }
            slot(id).content = ios
            startSimulator(id, app: pane["app"]?.string ?? "", device: pane["device"]?.string, restoring: restoring, in: ios)
            return
        default:
            return
        }
        slot(id).content = content
        DispatchQueue.main.async { [weak self] in
            guard self?.slots[id]?.content === content else { return }
            if let failure { self?.report(id, ["state": "failed", "error": .string(failure)]) } else { self?.report(id, ["state": "running"]) }
        }
    }

    private func startSimulator(_ id: String, app: String, device wanted: String?, restoring: Bool, in view: IosPaneView) {
        let appName = ((app as NSString).lastPathComponent as NSString).deletingPathExtension
        view.show(device: wanted ?? "iPhone", app: appName)
        view.show(status: "Finding \(wanted ?? "a device")…")
        Task { @MainActor [weak self] in
            let current = { self?.slots[id]?.content === view }
            do {
                let device = try await Simulator.resolve(wanted)
                guard current() else { return }
                if device.name != wanted { self?.onUpdate?(id, ["device": .string(device.name)]) }
                view.show(device: device.name, app: appName)
                view.show(status: "Booting \(device.name)…")
                try await Simulator.boot(device)
                guard current() else { return }
                view.show(status: "Launching \(appName)…")
                let name = restoring ? try await Simulator.reattach(app, on: device) : try await Simulator.launch(app, on: device)
                guard current() else { return }
                view.show(device: device.name, app: name)
                try view.attach(udid: device.udid)
                self?.report(id, ["state": "running"])
            } catch {
                guard current() else { return }
                view.show(status: nil)
                self?.report(id, ["state": "failed", "error": .string((error as? Simulator.Failure)?.message ?? error.localizedDescription)])
            }
        }
    }

    func stop(_ id: String) {
        guard let slot = slots[id], let content = slot.content else { return }
        (content as? WebPaneView)?.stop()
        (content as? IosPaneView)?.stop()
        (content as? CodePaneView)?.stop()
        if let diff = content as? DiffPaneView {
            diff.stop()
            diffSources.removeValue(forKey: id)
            diffContents.removeValue(forKey: id)
            diffLatest = diffLatest.filter { diffContents.values.contains($0.key) }
            onNotice?("diff.unsubscribe", ["view": .string(id)])
        }
        if let markdown = content as? MarkdownPaneView {
            markdown.stop()
            markdownZoom[id] = markdown.zoom
            markdownSpots[id, default: [:]][markdown.path] = (markdown.scrollTop, markdown.zoom)
        }
        slot.content = nil
        states.removeValue(forKey: id)
        errors.removeValue(forKey: id)
    }

    func forget(_ id: String) {
        stop(id)
        slots.removeValue(forKey: id)?.removeFromSuperview()
        markdownSpots.removeValue(forKey: id)
        markdownZoom.removeValue(forKey: id)
        codeZoom.removeValue(forKey: id)
        diffZoom.removeValue(forKey: id)
    }

    func subscribeDiffs() {
        for id in ids {
            if let view = diff(id) { subscribe(diff: view, id) }
        }
    }

    private func subscribe(diff view: DiffPaneView, _ id: String) {
        guard let source = diffSources[id] else { return }
        let args: [String: JSON] = ["view": .string(id), "path": .string(source.path), "args": .array(source.args.map { .string($0) })]
        guard let onCall else { return view.show(DiffResult.failed(folder: source.path, args: source.args, error: "Kanna.app isn't connected to its daemon"), serial: 0) }
        onCall("diff.subscribe", args) { [weak self, weak view] answer in
            guard let self, let view, slots[id]?.content === view else { return }
            guard answer["ok"]?.bool == true, let content = answer["content"]?.string else {
                let message = answer["error"]?["message"]?.string ?? "The daemon couldn't show this diff"
                return view.show(DiffResult.failed(folder: source.path, args: source.args, error: message), serial: 0)
            }
            diffContents[id] = content
            let latest = diffLatest[content]
            show(diff: (latest?["serial"]?.number ?? -1) > (answer["serial"]?.number ?? 0) ? latest ?? answer : answer, in: view)
        }
    }

    func received(diff message: JSON, content: String) {
        if let found = message["diff"], found != .null { diffLatest[content] = message }
        for (id, shown) in diffContents where shown == content {
            if let view = diff(id) { show(diff: message, in: view) }
        }
    }

    private func show(diff message: JSON, in view: DiffPaneView) {
        guard let found = message["diff"], found != .null else {
            if let updating = message["updating"]?.bool { view.setUpdating(updating) }
            return
        }
        view.show(found.encoded(), serial: Int(message["serial"]?.number ?? 0))
        if message["updating"]?.bool == true { view.setUpdating(true) }
    }

    static func absolute(_ path: String) -> String {
        let expanded = (path as NSString).expandingTildeInPath
        return expanded.hasPrefix("/") ? expanded : (NSHomeDirectory() as NSString).appendingPathComponent(expanded)
    }
}
