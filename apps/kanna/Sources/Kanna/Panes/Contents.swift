import AppKit
import CryptoKit
import KannaCode
import KannaDiff
import KannaLog
import KannaDocs
import KannaLinks
import KannaMarkdown
import KannaSerial
import WebKit

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
    var portOpen: ((Int, String) -> Result<Int, KannaError>)?
    var contextMenu: (() -> NSMenu?)?
    var agentStatus: ((String) -> String?)?
    var agentName: ((String) -> String)?
    var onComments: ((String, NSView) -> Void)?
    private var markdownSpots: [String: [String: (y: CGFloat, textSize: CGFloat, pageZoom: CGFloat)]] = [:]
    var textSize: ((String) -> CGFloat?)?
    var onTextSize: ((String, CGFloat) -> Void)?
    var pageZoom: ((String) -> CGFloat?)?
    var onPageZoom: ((String, CGFloat) -> Void)?
    private var diffSources: [String: (path: String, args: [String])] = [:]
    private var diffContents: [String: String] = [:]
    private var diffLatest: [String: JSON] = [:]
    private var logSources: [String: (path: String, args: [String])] = [:]
    private var logContents: [String: String] = [:]
    private var logLatest: [String: JSON] = [:]
    private var fileSources: [String: (path: String, kind: String, diff: String?)] = [:]
    private var fileContents: [String: String] = [:]
    private var fileLatest: [String: JSON] = [:]
    private var threadSources: [String: String] = [:]
    private var threadContents: [String: String] = [:]
    private var threadLatest: [String: JSON] = [:]
    private var serialContents: [String: String] = [:]

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

    func update(_ pane: JSON) {
        guard let id = pane["id"]?.string else { return }
        let agent = pane["agent"]?.string
        if let markdown = markdown(id) {
            markdown.set(markup: pane["markup"]?.bool == true)
            if markdown.markup, threadSources[id] == nil {
                threadSources[id] = markdown.path
                subscribe(threads: id)
            }
            if markdown.agent != agent {
                markdown.agent = agent
                markdown.agentStatus = agent.flatMap { agentStatus?($0) }
            }
        }
        if let web = web(id) {
            web.markable = pane["markup"]?.bool == true
            if web.agent != agent {
                web.agent = agent
                web.agentStatus = agent.flatMap { agentStatus?($0) }
            }
        }
    }

    func shows(_ id: String) -> Bool { slots[id]?.content != nil }
    var ids: [String] { slots.keys.filter { slots[$0]?.content != nil }.sorted() }

    func web(_ id: String) -> WebPaneView? { slots[id]?.content as? WebPaneView }
    func ios(_ id: String) -> IosPaneView? { slots[id]?.content as? IosPaneView }
    func markdown(_ id: String) -> MarkdownPaneView? { slots[id]?.content as? MarkdownPaneView }
    func code(_ id: String) -> CodePaneView? { slots[id]?.content as? CodePaneView }
    func diff(_ id: String) -> DiffPaneView? { slots[id]?.content as? DiffPaneView }
    func log(_ id: String) -> LogPaneView? { slots[id]?.content as? LogPaneView }
    func serial(_ id: String) -> SerialPaneView? { slots[id]?.content as? SerialPaneView }
    func keyView(_ id: String) -> NSView? { web(id)?.webView ?? ios(id) ?? markdown(id)?.keyView ?? code(id)?.keyView ?? diff(id)?.keyView ?? log(id)?.keyView ?? serial(id)?.keyView }
    func keyViews(_ id: String) -> [NSView] { diff(id)?.keyViews ?? keyView(id).map { [$0] } ?? [] }

    private func tell(_ text: String, to agent: String, doc: String, thread: String, state: String) {
        onCall?("tasks", ["action": .string("thread_event"), "pane": .string(agent), "text": .string(text), "doc": .string(doc), "thread": .string(thread), "state": .string(state)]) { _ in }
    }

    private func send(_ text: String, to agent: String, doc: String, thread: String, done: @escaping (DocDelivery) -> Void) {
        guard let onCall else { return done(.failed("not connected to the daemon")) }
        let args: [String: JSON] = ["action": .string("comment"), "pane": .string(agent), "text": .string(text), "doc": .string(doc), "thread": .string(thread)]
        onCall("tasks", args) { answer in
            guard answer["ok"]?.bool == true else { return done(.failed(answer["error"]?["message"]?.string ?? "send failed")) }
            done(answer["state"]?.string == "waiting" ? .waiting : .sent)
        }
    }

    func resend(_ id: String) {
        guard let state = states[id] else { return }
        var fields: [String: JSON] = ["state": .string(state)]
        if let error = errors[id] { fields["error"] = .string(error) }
        onUpdate?(id, fields)
    }

    static func refusal(_ type: String, _ pane: JSON, from machine: String) -> String? {
        switch type {
        case "ios": return "an iOS pane runs only for this Mac's own tasks"
        case "web":
            guard let url = pane["url"]?.string.flatMap(URL.init(string:)), ["http", "https", "file"].contains(url.scheme?.lowercased() ?? "") else {
                return "a page from \(machine) opens only over http or https, or from a file"
            }
            if url.isFileURL { return MachineFileURL.page(url.absoluteString) == nil ? "\(machine)'s page has no path" : nil }
            return url.host?.isEmpty == false ? nil : "\(machine)'s page has no host"
        default: return nil
        }
    }

    static func store(of machine: String) -> WKWebsiteDataStore {
        var bytes = Array(SHA256.hash(data: Data("kanna.machine.\(machine)".utf8)).prefix(16))
        bytes[6] = bytes[6] & 0x0f | 0x50
        bytes[8] = bytes[8] & 0x3f | 0x80
        let uuid = UUID(uuid: (bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]))
        return WKWebsiteDataStore(forIdentifier: uuid)
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
        if let machine = MachineIDs.machine(of: id), let refusal = Self.refusal(type, pane, from: machine) {
            stop(id)
            states[id] = "failed"
            return report(id, ["state": "failed", "error": .string(refusal)])
        }
        stop(id)
        states[id] = "starting"
        let content: NSView
        switch type {
        case "md":
            let machine = MachineIDs.machine(of: id)
            let path = machine == nil ? Self.absolute(pane["path"]?.string ?? "") : pane["path"]?.string ?? ""
            let spot = markdownSpots[id]?[path]
            let markdown = MarkdownPaneView(path: path, textSize: spot?.textSize ?? textSize?(id) ?? 1, pageZoom: spot?.pageZoom ?? pageZoom?(id) ?? 1, markup: pane["markup"]?.bool == true, diff: pane["diff"]?.string)
            markdown.canReveal = machine == nil
            markdown.readFile = { [weak self] path, data, done in self?.read(path, data: data, for: id, done: done) }
            markdown.changeThreads = changer(for: id, path: path)
            markdown.startScrollY = spot?.y
            markdown.onHistory = { [weak self] back in self?.onRequest?("navigate", ["pane": .string(id), back ? "back" : "forward": true]) }
            markdown.onZoom = { [weak self, weak markdown] in
                guard let markdown else { return }
                self?.onTextSize?(id, markdown.textSize)
                self?.onPageZoom?(id, markdown.pageZoom)
            }
            markdown.onFocus = { [weak self] in self?.onFocus?(id) }
            markdown.agent = pane["agent"]?.string
            markdown.agentStatus = markdown.agent.flatMap { self.agentStatus?($0) }
            markdown.onSend = { [weak self] agent, text, doc, thread, done in self?.send(text, to: agent, doc: doc, thread: thread, done: done) }
            markdown.onThreadEvent = { [weak self] agent, text, doc, thread, state in self?.tell(text, to: agent, doc: doc, thread: thread, state: state) }
            markdown.agentName = { [weak self] agent in self?.agentName?(agent) ?? agent }
            markdown.onComments = { [weak self] button in self?.onComments?(id, button) }
            if let mark = pane["tour"], let thread = mark["thread"]?.string, let at = mark["at"]?.number, let owner = mark["owner"]?.string {
                markdown.onTourStep = { [weak self] to in self?.onRequest?("tour", ["action": "go", "owner": .string(owner), "to": .string(to)]) }
                markdown.tour = DocTourStep(thread: thread, at: Int(at), of: Int(mark["of"]?.number ?? at))
            }
            markdown.contextMenu = { [weak self] in self?.contextMenu?() }
            markdown.onOpenMarkdown = { [weak self] target in self?.onRequest?("navigate", ["pane": .string(id), "path": .string(target)]) }
            markdown.onOpenCode = { [weak self] path, line in
                self?.onCall?("open", ["type": "code", "path": .string(path), "line": .number(Double(line)), "from": .string(id)]) { _ in }
            }
            fileSources[id] = (pane["path"]?.string ?? "", "md", pane["diff"]?.string)
            slot(id).content = markdown
            subscribe(file: id)
            if markdown.markup {
                threadSources[id] = path
                subscribe(threads: id)
            }
            return
        case "code":
            let path = pane["path"]?.string ?? ""
            let note = pane["comment"]
            let mark = pane["tour"]
            let step = mark.flatMap { mark in mark["at"]?.number.map { CodeTourStep(at: Int($0), of: Int(mark["of"]?.number ?? $0)) } }
            let comment = note?["path"]?.string == path ? note.flatMap { note in
                note["first"]?.number.map { CodeComment(first: Int($0), last: Int(note["last"]?.number ?? $0), text: note["text"]?.string ?? "", tour: step) }
            } : nil
            let code = CodePaneView(path: Self.absolute(path), line: pane["line"]?.number.map { Int($0) }, zoom: textSize?(id) ?? 1, comment: comment)
            code.onZoom = { [weak self, weak code] in
                guard let code else { return }
                self?.onTextSize?(id, code.zoom)
            }
            code.onHistory = { [weak self] back in self?.onRequest?("navigate", ["pane": .string(id), back ? "back" : "forward": true]) }
            code.onFocus = { [weak self] in self?.onFocus?(id) }
            if let owner = mark?["owner"]?.string {
                code.onTourStep = { [weak self] to in self?.onRequest?("tour", ["action": "go", "owner": .string(owner), "to": .string(to)]) }
            }
            code.contextMenu = { [weak self] in self?.contextMenu?() }
            fileSources[id] = (path, "code", nil)
            slot(id).content = code
            subscribe(file: id)
            return
        case "diff":
            let diff = DiffPaneView(view: pane["view"]?.string, zoom: textSize?(id) ?? 1)
            diff.onFocus = { [weak self] in self?.onFocus?(id) }
            diff.contextMenu = { [weak self] in self?.contextMenu?() }
            diff.onZoom = { [weak self, weak diff] in
                guard let diff else { return }
                self?.onTextSize?(id, diff.zoom)
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
        case "log":
            let log = LogPaneView(zoom: textSize?(id) ?? 1)
            let path = pane["path"]?.string ?? ""
            log.onFocus = { [weak self] in self?.onFocus?(id) }
            log.contextMenu = { [weak self] in self?.contextMenu?() }
            log.onZoom = { [weak self, weak log] in
                guard let log else { return }
                self?.onTextSize?(id, log.zoom)
            }
            log.onOpenDiff = { [weak self] _, args in
                self?.onCall?("open", ["type": "diff", "path": .string(path), "args": .array(args.map { .string($0) }), "from": .string(id), "tab": true]) { _ in }
            }
            log.onCommit = { [weak self] hash, done in
                guard let onCall = self?.onCall else { return done(nil, "Kanna.app isn't connected to its daemon") }
                onCall("log.commit", ["path": .string(path), "hash": .string(hash)]) { answer in
                    guard answer["ok"]?.bool == true, let commit = answer["commit"] else { return done(nil, answer["error"]?["message"]?.string ?? "git couldn't show \(hash)") }
                    done(commit.encoded(), nil)
                }
            }
            var logArgs: [String] = []
            if case .array(let list)? = pane["args"] { logArgs = list.compactMap(\.string) }
            logSources[id] = (path, logArgs)
            content = log
            subscribe(log: log, id)
        case "serial":
            let serial = SerialPaneView()
            serial.onFocus = { [weak self] in self?.onFocus?(id) }
            serial.contextMenu = { [weak self] in self?.contextMenu?() }
            serial.onSend = { [weak self, weak serial] text, ending in
                self?.onCall?("serial", ["action": "send", "pane": .string(id), "text": .string(text), "ending": .bool(ending)]) { answer in
                    if answer["ok"]?.bool != true { serial?.flash(answer["error"]?["message"]?.string ?? "couldn't send") }
                }
            }
            serial.onReset = { [weak self, weak serial] in
                self?.onCall?("serial", ["action": "reset", "pane": .string(id)]) { answer in
                    if answer["ok"]?.bool != true { serial?.flash(answer["error"]?["message"]?.string ?? "couldn't reset") }
                }
            }
            serial.onSet = { [weak self] baud, ending in
                var args: [String: JSON] = ["action": "set", "pane": .string(id)]
                if let baud { args["baud"] = .number(Double(baud)) }
                if let ending { args["lineEnding"] = .string(ending) }
                self?.onCall?("serial", args) { _ in }
            }
            slot(id).content = serial
            subscribe(serial: id)
            return
        case "web":
            var url = pane["url"]?.string ?? "about:blank"
            var unmap: (URL) -> URL = { $0 }
            var waitingFor: String?
            var files: MachineFiles?
            if let machine = MachineIDs.machine(of: id), let page = MachineFileURL.page(url) {
                url = page.absoluteString
                unmap = MachineFileURL.file
                files = MachineFiles(page: MachineFileURL.path(of: page) ?? "/") { [weak self] (path: String, done: @escaping @MainActor (Result<(real: String, data: Data), MachineFiles.Failure>) -> Void) in
                    guard let self else { return done(.failure(.init(message: "Kanna.app isn't showing this page"))) }
                    read(path, data: true, for: id) { result in
                        switch result {
                        case .success(let file) where file.type == "file": done(.success((real: file.path, data: file.data ?? Data())))
                        case .success(let file): done(.failure(.init(message: "\(file.path) isn't a file")))
                        case .failure(let failure): done(.failure(.init(message: failure.message)))
                        }
                    }
                }
            } else if let machine = MachineIDs.machine(of: id), let tunnel = PortTunnelURL(url) {
                switch portOpen?(tunnel.port, machine) ?? .failure(KannaError("unavailable", "this Mac has no machines endpoint")) {
                case .success(let local):
                    url = tunnel.local(local).absoluteString
                    unmap = { tunnel.remote($0, local: local) }
                    waitingFor = "\(tunnel.address) on \(machine)"
                case .failure(let failure):
                    stop(id)
                    states[id] = "failed"
                    return report(id, ["state": "failed", "error": .string(failure.message)])
                }
            }
            let store: WKWebsiteDataStore = if waitingFor != nil || files != nil, let machine = MachineIDs.machine(of: id) { Self.store(of: machine) } else { .default() }
            let web = WebPaneView(url: url, markup: pane["markup"]?.bool == true, store: store, files: files)
            if files != nil { web.pagePath = { url in MachineFileURL.path(of: url).flatMap { ["html", "htm"].contains(($0 as NSString).pathExtension.lowercased()) ? $0 : nil } } }
            web.waitingFor = waitingFor
            web.unmap = unmap
            web.onFocus = { [weak self] in self?.onFocus?(id) }
            web.webView.contextMenu = { [weak self] in self?.contextMenu?() }
            web.onNavigate = { [weak self] url in self?.onRequest?("navigate", ["pane": .string(id), "url": .string(URL(string: url).map(unmap)?.absoluteString ?? url)]) }
            web.onURLChange = { [weak self] url in self?.onUpdate?(id, ["url": .string(URL(string: url).map(unmap)?.absoluteString ?? url)]) }
            web.onTitleChange = { [weak self] title in self?.onUpdate?(id, ["title": .string(title)]) }
            web.agent = pane["agent"]?.string
            web.agentStatus = web.agent.flatMap { self.agentStatus?($0) }
            web.onSend = { [weak self] agent, text, doc, thread, done in self?.send(text, to: agent, doc: doc, thread: thread, done: done) }
            web.onThreadEvent = { [weak self] agent, text, doc, thread, state in self?.tell(text, to: agent, doc: doc, thread: thread, state: state) }
            web.agentName = { [weak self] agent in self?.agentName?(agent) ?? agent }
            web.onComments = { [weak self] button in self?.onComments?(id, button) }
            web.onMarkup = { [weak self] markup in self?.marked(markup, in: id) }
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
        if let web = content as? WebPaneView { marked(web.markup, in: id) }
        DispatchQueue.main.async { [weak self] in
            guard self?.slots[id]?.content === content else { return }
            self?.report(id, ["state": "running"])
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
        if let log = content as? LogPaneView {
            log.stop()
            logSources.removeValue(forKey: id)
            logContents.removeValue(forKey: id)
            logLatest = logLatest.filter { logContents.values.contains($0.key) }
            onNotice?("log.unsubscribe", ["view": .string(id)])
        }
        if let serial = content as? SerialPaneView {
            serial.stop()
            serialContents.removeValue(forKey: id)
            onNotice?("serial.unsubscribe", ["view": .string(id)])
        }
        if fileSources.removeValue(forKey: id) != nil {
            fileContents.removeValue(forKey: id)
            fileLatest = fileLatest.filter { fileContents.values.contains($0.key) }
            onNotice?("file.unsubscribe", ["view": .string(id)])
        }
        if threadSources.removeValue(forKey: id) != nil {
            threadContents.removeValue(forKey: id)
            threadLatest = threadLatest.filter { threadContents.values.contains($0.key) }
            onNotice?("markup.unsubscribe", ["view": .string(id)])
        }
        if let markdown = content as? MarkdownPaneView {
            markdown.stop()
            markdownSpots[id, default: [:]][markdown.path] = (markdown.scrollTop, markdown.textSize, markdown.pageZoom)
        }
        slot.content = nil
        states.removeValue(forKey: id)
        errors.removeValue(forKey: id)
    }

    func forget(_ id: String) {
        stop(id)
        slots.removeValue(forKey: id)?.removeFromSuperview()
        markdownSpots.removeValue(forKey: id)
    }

    func subscribeDiffs(on machine: String?) {
        diffLatest = diffLatest.filter { MachineIDs.machine(of: $0.key) != machine }
        for id in ids where MachineIDs.machine(of: id) == machine {
            if let view = diff(id) { subscribe(diff: view, id) }
        }
    }

    private func subscribe(diff view: DiffPaneView, _ id: String) {
        guard let source = diffSources[id] else { return }
        let args: [String: JSON] = ["view": .string(id), "path": .string(source.path), "args": .array(source.args.map { .string($0) })]
        guard let onCall else { return view.receive(DiffResult.failed(folder: source.path, args: source.args, error: "Kanna.app isn't connected to its daemon"), serial: 0) }
        onCall("diff.subscribe", args) { [weak self, weak view] answer in
            guard let self, let view, slots[id]?.content === view else { return }
            guard answer["ok"]?.bool == true, let content = answer["content"]?.string else {
                let message = answer["error"]?["message"]?.string ?? "The daemon couldn't show this diff"
                return view.receive(DiffResult.failed(folder: source.path, args: source.args, error: message), serial: 0)
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
        view.receive(found.encoded(), serial: Int(message["serial"]?.number ?? 0))
        if message["updating"]?.bool == true { view.setUpdating(true) }
    }

    func subscribeLogs(on machine: String?) {
        for id in ids where MachineIDs.machine(of: id) == machine {
            if let view = log(id) { subscribe(log: view, id) }
        }
    }

    private func subscribe(log view: LogPaneView, _ id: String) {
        guard let source = logSources[id] else { return }
        let args: [String: JSON] = ["view": .string(id), "path": .string(source.path), "args": .array(source.args.map { .string($0) })]
        guard let onCall else { return view.receive(LogResult.failed(folder: source.path, args: source.args, error: "Kanna.app isn't connected to its daemon"), serial: 0) }
        onCall("log.subscribe", args) { [weak self, weak view] answer in
            guard let self, let view, slots[id]?.content === view else { return }
            guard answer["ok"]?.bool == true, let content = answer["content"]?.string else {
                let message = answer["error"]?["message"]?.string ?? "The daemon couldn't show this log"
                return view.receive(LogResult.failed(folder: source.path, args: source.args, error: message), serial: 0)
            }
            logContents[id] = content
            let latest = logLatest[content]
            show(log: (latest?["serial"]?.number ?? -1) > (answer["serial"]?.number ?? 0) ? latest ?? answer : answer, in: view)
        }
    }

    func received(log message: JSON, content: String) {
        if let found = message["log"], found != .null { logLatest[content] = message }
        for (id, shown) in logContents where shown == content {
            if let view = log(id) { show(log: message, in: view) }
        }
    }

    private func show(log message: JSON, in view: LogPaneView) {
        guard let found = message["log"], found != .null else { return }
        view.receive(found.encoded(), serial: Int(message["serial"]?.number ?? 0))
    }

    func subscribeFiles(on machine: String?) {
        fileLatest = fileLatest.filter { MachineIDs.machine(of: $0.key) != machine }
        threadLatest = threadLatest.filter { MachineIDs.machine(of: $0.key) != machine }
        for id in ids where fileSources[id] != nil && MachineIDs.machine(of: id) == machine { subscribe(file: id) }
        for id in ids where threadSources[id] != nil && MachineIDs.machine(of: id) == machine { subscribe(threads: id) }
    }

    private func marked(_ markup: PageMarkupView?, in id: String) {
        guard let markup else {
            if fileSources.removeValue(forKey: id) != nil {
                fileContents.removeValue(forKey: id)
                onNotice?("file.unsubscribe", ["view": .string(id)])
            }
            if threadSources.removeValue(forKey: id) != nil {
                threadContents.removeValue(forKey: id)
                onNotice?("markup.unsubscribe", ["view": .string(id)])
            }
            return
        }
        markup.changeThreads = changer(for: id, path: markup.path)
        fileSources[id] = (markup.path, "md", nil)
        threadSources[id] = markup.path
        subscribe(file: id)
        subscribe(threads: id)
    }

    private func read(_ path: String, data: Bool, for id: String, done: @escaping @MainActor (Result<MarkdownFile, MarkdownFile.Failure>) -> Void) {
        guard let onCall else { return done(.failure(.init("Kanna.app isn't connected to its daemon"))) }
        var args: [String: JSON] = ["view": .string(id), "path": .string(path)]
        if data { args["data"] = true }
        onCall("file.read", args) { answer in
            guard answer["ok"]?.bool == true, let real = answer["path"]?.string else {
                return done(.failure(.init(answer["error"]?["message"]?.string ?? "can't read \(path)")))
            }
            done(.success(MarkdownFile(path: real, type: answer["type"]?.string ?? "other", data: answer["data"]?.string.flatMap { Data(base64Encoded: $0) }, stamp: answer["stamp"]?.string)))
        }
    }

    private func changer(for id: String, path: String) -> DocChanger {
        { [weak self] change, done in
            guard let onCall = self?.onCall else { return done(DocAnswer(error: "Kanna.app isn't connected to its daemon")) }
            var args: [String: JSON] = ["view": .string(id), "path": .string(path)]
            let cmd: String
            switch change {
            case .post(let thread, let mark):
                cmd = "markup.post"
                args["thread"] = Self.json(thread)
                if let mark { args["mark"] = .string(mark.base64EncodedString()) }
            case .edit(let thread, let edit):
                cmd = "markup.edit"
                args["id"] = .string(thread)
                if let state = edit.state { args["state"] = .string(state) }
                if let status = edit.status { args["status"] = .string(status) }
                if let error = edit.error { args["error"] = error.map { .string($0) } ?? .null }
                if let line = edit.line { args["line"] = .number(Double(line)) }
                if let message = edit.message { args["message"] = Self.json(message) }
            case .agent(let agent):
                cmd = "markup.agent"
                args["agent"] = .string(agent)
            }
            onCall(cmd, args) { answer in
                guard answer["ok"]?.bool == true else { return done(DocAnswer(error: answer["error"]?["message"]?.string ?? "the daemon didn't save the comment")) }
                done(DocAnswer(id: answer["thread"]?.string, record: answer["record"].flatMap(Self.record)))
            }
        }
    }

    static func json(_ value: some Encodable) -> JSON {
        (try? JSONEncoder().encode(value)).flatMap { try? JSON.parse($0) } ?? .null
    }

    static func record(_ json: JSON) -> DocRecord? {
        try? JSONDecoder().decode(DocRecord.self, from: json.encoded())
    }

    private func subscribe(threads id: String) {
        guard let path = threadSources[id], let view = slots[id]?.content, let onCall else { return }
        onCall("markup.subscribe", ["view": .string(id), "path": .string(path)]) { [weak self, weak view] answer in
            guard let self, let view, slots[id]?.content === view, answer["ok"]?.bool == true, let content = answer["content"]?.string else { return }
            threadContents[id] = content
            let latest = threadLatest[content]
            let newest = (latest?["serial"]?.number ?? -1) > (answer["serial"]?.number ?? 0) ? latest ?? answer : answer
            if (newest["serial"]?.number ?? 0) > 0 { show(threads: newest, in: id) }
        }
    }

    func received(markup message: JSON, content: String) {
        if (threadLatest[content]?["serial"]?.number ?? -1) < (message["serial"]?.number ?? 0) { threadLatest[content] = message }
        for (id, shown) in threadContents where shown == content { show(threads: message, in: id) }
    }

    private func show(threads message: JSON, in id: String) {
        guard let record = message["record"].flatMap(Self.record) else { return }
        markdown(id)?.show(threads: record)
        web(id)?.markup?.show(threads: record)
        if let name = message["name"]?.string { web(id)?.markup?.show(name: name) }
    }

    private func subscribe(file id: String) {
        guard let source = fileSources[id], let view = slots[id]?.content else { return }
        guard let onCall else { return show(file: ["error": ["message": "Kanna.app isn't connected to its daemon"]], in: id) }
        var args: [String: JSON] = ["view": .string(id), "path": .string(source.path), "kind": .string(source.kind)]
        if let diff = source.diff { args["diff"] = .string(diff) }
        onCall("file.subscribe", args) { [weak self, weak view] answer in
            guard let self, let view, slots[id]?.content === view else { return }
            guard answer["ok"]?.bool == true, let content = answer["content"]?.string else {
                return show(file: ["error": ["message": answer["error"]?["message"] ?? "The daemon couldn't read this file"]], in: id)
            }
            fileContents[id] = content
            let latest = fileLatest[content]
            let newest = (latest?["serial"]?.number ?? -1) > (answer["serial"]?.number ?? 0) ? latest ?? answer : answer
            if (newest["serial"]?.number ?? 0) > 0 { show(file: newest, in: id) }
        }
    }

    func received(file message: JSON, content: String) {
        if (fileLatest[content]?["serial"]?.number ?? -1) < (message["serial"]?.number ?? 0) { fileLatest[content] = message }
        for (id, shown) in fileContents where shown == content { show(file: message, in: id) }
    }

    private func show(file message: JSON, in id: String) {
        if let markup = web(id)?.markup {
            if let text = message["text"]?.string { markup.show(source: text) }
            return
        }
        var failure = message["error"]?["message"]?.string
        if let text = message["text"]?.string {
            if let name = message["name"]?.string { markdown(id)?.show(name: name) }
            markdown(id)?.show(text: text, compared: Self.compared(message))
            code(id)?.show(text: text)
            failure = markdown(id)?.diffError
        } else if let failure {
            code(id)?.show(failure: failure)
        }
        let state = failure == nil ? "running" : "failed"
        guard states[id] != state else { return }
        report(id, failure.map { ["state": .string(state), "error": .string($0)] } ?? ["state": .string(state)])
    }

    static func compared(_ message: JSON) -> MarkdownComparison? {
        if let error = message["diff_error"]?.string { return .failed(error) }
        if let base = message["base"] { return .base(base["text"]?.string) }
        if let old = message["old"] { return .range(old.string) }
        return nil
    }

    func subscribeSerials(on machine: String?) {
        for id in ids where serial(id) != nil && MachineIDs.machine(of: id) == machine { subscribe(serial: id) }
    }

    private func subscribe(serial id: String) {
        guard let view = serial(id) else { return }
        guard let onCall else { return view.show(failure: "Kanna.app isn't connected to its daemon") }
        onCall("serial.subscribe", ["view": .string(id), "pane": .string(id)]) { [weak self, weak view] answer in
            guard let self, let view, serial(id) === view else { return }
            guard answer["ok"]?.bool == true, let content = answer["content"]?.string else {
                let message = answer["error"]?["message"]?.string ?? "The daemon couldn't open this port"
                view.show(failure: message)
                return report(id, ["state": "failed", "error": .string(message)])
            }
            serialContents[id] = content
            view.load(rows: Self.serialRows(answer["rows"]), partial: Self.serialRow(answer["partial"]), state: Self.serialState(answer["state"]) ?? SerialState(),
                      history: answer["history"]?.array?.compactMap(\.string) ?? [])
            if states[id] != "running" { report(id, ["state": "running"]) }
        }
    }

    func received(serial message: JSON, content: String) {
        for (id, shown) in serialContents where shown == content {
            serial(id)?.receive(rows: Self.serialRows(message["rows"]), partial: Self.serialRow(message["partial"]), state: Self.serialState(message["state"]), rx: message["rx"]?.number.map { Int($0) })
        }
    }

    static func serialRow(_ json: JSON?) -> SerialRow? {
        guard let json, let text = json["text"]?.string, let t = json["t"]?.number else { return nil }
        return SerialRow(n: Int(json["n"]?.number ?? 0), t: t, dir: json["d"]?.string ?? "<", text: text, level: json["l"]?.string)
    }

    static func serialRows(_ json: JSON?) -> [SerialRow] {
        json?.array?.compactMap { serialRow($0) } ?? []
    }

    static func serialState(_ json: JSON?) -> SerialState? {
        guard let json, json != .null, let name = json["state"]?.string else { return nil }
        var state = SerialState()
        state.state = name
        state.cmd = json["cmd"]?.string
        state.why = json["why"]?.string
        state.path = json["path"]?.string ?? ""
        state.name = json["name"]?.string ?? ""
        state.title = json["title"]?.string ?? ""
        state.product = json["product"]?.string
        state.device = json["device"]?.string
        state.baud = Int(json["baud"]?.number ?? 115_200)
        state.lineEnding = json["lineEnding"]?.string ?? "lf"
        state.rx = Int(json["rx"]?.number ?? 0)
        state.format = json["format"]?.string
        state.log = json["log"]?.string ?? ""
        return state
    }

    static func absolute(_ path: String) -> String {
        let expanded = (path as NSString).expandingTildeInPath
        return expanded.hasPrefix("/") ? expanded : (NSHomeDirectory() as NSString).appendingPathComponent(expanded)
    }
}
