import AppKit

struct GitPlace: Equatable {
    let root: String
    let branch: String?
    let main: String

    static func of(_ folder: String) -> GitPlace? {
        var url = URL(fileURLWithPath: folder).standardizedFileURL
        while true {
            let git = url.appendingPathComponent(".git")
            var isFolder: ObjCBool = false
            if FileManager.default.fileExists(atPath: git.path, isDirectory: &isFolder) {
                let dir = isFolder.boolValue ? git : gitdir(from: git, base: url)
                let main = isFolder.boolValue ? nil : dir.flatMap(mainRepo)
                return GitPlace(root: url.path, branch: dir.flatMap(branch), main: main ?? url.path)
            }
            let parent = url.deletingLastPathComponent()
            if parent.path == url.path { return nil }
            url = parent
        }
    }

    private static func mainRepo(_ gitdir: URL) -> String? {
        let text = gitdir.path
        return text.range(of: "/.git/worktrees/").map { String(text[..<$0.lowerBound]) }
    }

    private static func gitdir(from file: URL, base: URL) -> URL? {
        guard let text = small(file), let line = text.split(separator: "\n").first, line.hasPrefix("gitdir: ") else { return nil }
        let path = String(line.dropFirst("gitdir: ".count)).trimmingCharacters(in: .whitespaces)
        return path.hasPrefix("/") ? URL(fileURLWithPath: path) : base.appendingPathComponent(path).standardizedFileURL
    }

    private static func branch(_ dir: URL) -> String? {
        guard let text = small(dir.appendingPathComponent("HEAD"))?.trimmingCharacters(in: .whitespacesAndNewlines) else { return nil }
        if text.hasPrefix("ref: refs/heads/") {
            let name = text.dropFirst("ref: refs/heads/".count)
            return !name.isEmpty && name.unicodeScalars.allSatisfy({ $0.value > 0x20 && $0.value != 0x7f && $0 != "\\" }) ? String(name) : nil
        }
        return text.count >= 40 && text.allSatisfy(\.isHexDigit) ? String(text.prefix(7)) : nil
    }

    private static func small(_ file: URL) -> String? {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: file.path),
              attributes[.type] as? FileAttributeType == .typeRegular,
              let size = attributes[.size] as? Int, size <= 4096,
              let data = FileManager.default.contents(atPath: file.path) else { return nil }
        return String(data: data, encoding: .utf8)
    }
}

func canonical(_ path: String) -> String {
    URL(fileURLWithPath: path).resolvingSymlinksInPath().path
}

func tilde(_ path: String) -> String {
    let home = NSHomeDirectory()
    return path == home ? "~" : path.hasPrefix(home + "/") ? "~" + path.dropFirst(home.count) : path
}

@MainActor
final class AgentHeader: NSView {
    static let height = TabBar.height
    var onDeskView: (() -> Void)?
    var onNew: (() -> Void)?
    private let label = NSTextField(labelWithString: "")
    private let deskViewButton = HeaderButton(.deskView)
    private let plus = HeaderButton(.plus)
    var canShowDeskView = true {
        didSet { deskViewButton.isHidden = !canShowDeskView; needsLayout = true }
    }
    var newable = false {
        didSet { plus.isHidden = !newable; needsLayout = true }
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        label.font = .systemFont(ofSize: 12)
        label.textColor = .secondaryLabelColor
        label.lineBreakMode = .byTruncatingMiddle
        addSubview(label)
        deskViewButton.toolTip = "Desk view (⌘.)"
        deskViewButton.onPress = { [weak self] in self?.onDeskView?() }
        addSubview(deskViewButton)
        plus.toolTip = "New Tab"
        plus.onPress = { [weak self] in self?.onNew?() }
        plus.isHidden = true
        addSubview(plus)
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    private func paint() { layer?.backgroundColor = resolved(.windowBackgroundColor) }

    var plusCenter: NSPoint { plus.convert(NSPoint(x: plus.bounds.midX, y: plus.bounds.midY), to: nil) }
    var deskViewCenter: NSPoint { deskViewButton.convert(NSPoint(x: deskViewButton.bounds.midX, y: deskViewButton.bounds.midY), to: nil) }
    var text: String { label.stringValue }

    func show(_ text: String, stale: String? = nil) {
        let shown = stale.map { "\($0) · \(text)" } ?? text
        if label.stringValue != shown { label.stringValue = shown }
        label.textColor = stale == nil ? .secondaryLabelColor : .systemRed
        needsLayout = true
    }

    override func layout() {
        super.layout()
        deskViewButton.frame = NSRect(x: bounds.width - 30, y: (bounds.height - 22) / 2, width: 22, height: 22)
        let right = canShowDeskView ? deskViewButton.frame.minX : bounds.width - 8
        plus.frame = NSRect(x: right - 26, y: (bounds.height - 22) / 2, width: 22, height: 22)
        let end = newable ? plus.frame.minX : right
        label.sizeToFit()
        label.frame = NSRect(x: 14, y: (bounds.height - label.frame.height) / 2, width: max(0, end - 8 - 14), height: label.frame.height)
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
    }
}

@MainActor
private final class HeaderButton: NSView {
    enum Kind { case deskView, plus }
    var onPress: (() -> Void)?
    private let kind: Kind
    private var hover = false

    init(_ kind: Kind) {
        self.kind = kind
        super.init(frame: .zero)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        if hover {
            NSColor.quaternaryLabelColor.setFill()
            NSBezierPath(roundedRect: bounds, xRadius: 5, yRadius: 5).fill()
        }
        let scale = bounds.width / 22
        let transform = AffineTransform(translationByX: 3 * scale, byY: 3 * scale)
        (hover ? NSColor.labelColor : NSColor.secondaryLabelColor).setStroke()
        if kind == .plus {
            let plus = NSBezierPath()
            plus.move(to: NSPoint(x: 8, y: 3.5))
            plus.line(to: NSPoint(x: 8, y: 12.5))
            plus.move(to: NSPoint(x: 3.5, y: 8))
            plus.line(to: NSPoint(x: 12.5, y: 8))
            plus.transform(using: transform)
            plus.lineWidth = 1.4
            plus.lineCapStyle = .round
            plus.stroke()
            return
        }
        let frame = NSBezierPath(roundedRect: NSRect(x: 2, y: 2, width: 12, height: 12), xRadius: 2, yRadius: 2)
        let chevron = NSBezierPath()
        chevron.move(to: NSPoint(x: 5.5, y: 5.5))
        chevron.line(to: NSPoint(x: 8, y: 8))
        chevron.line(to: NSPoint(x: 10.5, y: 5.5))
        let base = NSBezierPath()
        base.move(to: NSPoint(x: 5, y: 11))
        base.line(to: NSPoint(x: 11, y: 11))
        for (path, width) in [(frame, 1.4), (chevron, 1.4), (base, 2.0)] {
            path.transform(using: transform)
            path.lineWidth = width
            path.lineCapStyle = .round
            path.lineJoinStyle = .round
            path.stroke()
        }
    }

    override func mouseEntered(with event: NSEvent) { hover = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hover = false; needsDisplay = true }
    override func mouseDown(with event: NSEvent) { onPress?() }
}

extension AppController {
    func isTask(_ pane: String) -> Bool {
        windows.contains { $0["dock"]?["pane"]?.string == pane } || deskEntries.contains { $0["task"]?.string == pane }
    }

    func place(of pane: String) -> (folder: String, git: GitPlace?)? {
        guard let folder = folders[pane] else { return nil }
        guard MachineIDs.machine(of: pane) != nil else { return (folder, GitPlace.of(folder)) }
        guard let place = places[pane], let root = place["root"]?.string, let main = place["main"]?.string else { return (folder, nil) }
        return (folder, GitPlace(root: root, branch: place["branch"]?.string, main: main))
    }

    func headerText(_ pane: String) -> String {
        guard let place = place(of: pane) else { return "" }
        return [MachineIDs.machine(of: pane) == nil ? tilde(place.folder) : place.folder, place.git?.branch].compactMap { $0 }.joined(separator: " · ")
    }

    func projectRoot(_ pane: String) -> String? {
        let machine = MachineIDs.machine(of: pane)
        return place(of: pane).map { place in
            let path = place.git?.main ?? place.folder
            return machine == nil ? canonical(path) : MachineIDs.location(path, on: machine)
        }
    }

    func rootLocation(_ location: String) -> String {
        let (machine, path) = MachineIDs.split(location: location)
        return machine == nil ? canonical(GitPlace.of(path)?.main ?? path) : location
    }

    func sameProject(_ a: String?, _ b: String?) -> Bool {
        guard let a, let b else { return false }
        if a == b { return true }
        guard MachineIDs.split(location: a).machine != MachineIDs.split(location: b).machine, let one = identities[a] else { return false }
        return one == identities[b]
    }

    func displayName(_ location: String) -> String {
        let (machine, path) = MachineIDs.split(location: location)
        guard machine != nil else { return tilde(path) }
        return recentProjects.first { $0.path == location }?.label ?? (path as NSString).lastPathComponent
    }

    func projectName(_ tasks: [String]) -> String {
        tasks.lazy.compactMap(projectRoot).first.map(displayName) ?? "no project"
    }

    func taskDetail(_ pane: String) -> String {
        place(of: pane)?.git?.branch ?? ""
    }
}

extension AppController {
    func made(_ pane: String) -> Int {
        deskMade[MachineIDs.machine(of: pane) ?? ""]?[pane] ?? .max
    }

    func backlogChanged(_ tasks: [JSON], desk: [JSON], on machine: String?) {
        deskMade[machine ?? ""] = Dictionary(desk.enumerated().compactMap { at, entry in entry["pane"]?.string.map { ($0, at) } }, uniquingKeysWith: { first, _ in first })
        let tag: JSON = machine.map { .string($0) } ?? .null
        backlogs[machine ?? ""] = tasks.map { entry in
            guard case .object(var fields) = entry else { return entry }
            fields["machine"] = tag
            return .object(fields)
        }
        for entry in backlogs[machine ?? ""] ?? [] {
            if let location = location(ofEntry: entry), let project = entry["project"]?.string { identities[location] = project }
        }
        backlog = backlogs.keys.sorted().flatMap { backlogs[$0] ?? [] }
        refresh()
    }

    func location(ofEntry entry: JSON) -> String? {
        guard let repo = entry["repo"]?.string else { return nil }
        let machine = entry["machine"]?.string
        return machine == nil ? canonical(repo) : MachineIDs.location(repo, on: machine)
    }

    private func inProject(_ entry: JSON, _ root: String?) -> Bool {
        root == nil || sameProject(location(ofEntry: entry), root)
    }

    private func taskName(_ pane: String) -> String? {
        if let window = windows.first(where: { $0["dock"]?["pane"]?.string == pane }) { return taskTitle(pane, name: window["dock"]?["name"]?.string) }
        if let entry = deskEntries.first(where: { $0["task"]?.string == pane }) { return taskTitle(pane, name: entry["name"]?.string) }
        return nil
    }

    func backlogTasks(in root: String?) -> [BacklogTask] {
        backlog.filter { inProject($0, root) }.compactMap { entry in
            guard let id = entry["id"]?.string, let name = entry["name"]?.string else { return nil }
            var waits: [String] = []
            if case .array(let blockers)? = entry["waits"] { waits = blockers.compactMap { $0["name"]?.string ?? $0["id"]?.string } }
            return BacklogTask(id: id, name: name, message: entry["message"]?.string, from: entry["by"]?.string.flatMap(taskName), madeAt: entry["madeAt"]?.number, waits: waits, agent: BacklogTask.program(entry["agent"]?.string))
        }
    }

    func runningBacklog(in root: String?) -> [JSON] {
        backlog.filter { $0["running"]?.bool == true && inProject($0, root) }.compactMap { entry in
            entry["held"]?.string.map { pane in
                let ws = published["workspaces"]?.array?.first { $0["dock"]?["pane"]?.string == pane }
                return ["task": .string(pane), "type": "term", "tabs": ws?["tabs"] ?? []]
            }
        }
    }

    func runningTasks(in root: String?) -> Int {
        let docked = windows.compactMap { $0["dock"]?["pane"]?.string } + deskEntries.compactMap { $0["task"]?.string }
        return docked.filter { root == nil || sameProject(projectRoot($0), root) }.count
    }

    private func noted(_ window: String, _ reply: JSON) {
        guard reply["ok"]?.bool != true, let controller = controllers[window] else { return }
        controller.note = reply["error"]?["message"]?.string ?? "it didn't start"
        refresh()
    }

    func startTask(_ task: String, from window: String) {
        controllers[window]?.note = ""
        let click = Clicks.pick(task, desk: controllers[window]?.deskTasks.map(\.id) ?? [])
        request("tasks", ["action": "pick", "task": .string(task)], reply: { [weak self] reply in
            if reply["ok"]?.bool != true { Clicks.failed(task) }
            self?.noted(window, reply)
        }, sent: Clicks.sent(click))
    }

    func newDesk() {
        _ = openDesk(project: NSHomeDirectory(), fresh: true)
    }

    func openPlanner(in window: String) {
        guard !starting.contains(window), let controller = controllers[window] else { return }
        let location = self.window(window)?["project"]?.string ?? controller.projectRoot
        let (machine, folder) = location.map(MachineIDs.split(location:)) ?? (nil, "")
        guard links?.link(machine)?.attached == true else { return }
        starting.insert(window)
        var args: [String: JSON] = ["action": "plan", "app": .string(claimName(machine))]
        if location != nil { args["folder"] = .string(folder) }
        if let script = Bundle.main.url(forResource: "planner", withExtension: "sh") { args["script"] = .string(script.path) }
        request("tasks", args, on: .some(machine)) { [weak self] reply in
            guard let self else { return }
            if reply["ok"]?.bool == true, let planner = reply["workspace"]?.string, var desk = physical.desk(window) {
                if published["workspaces"]?.array?.contains(where: { $0["id"]?.string == planner }) != true { givenPlanners.insert(planner) }
                desk.planner = planner
                physical.setDesk(desk)
                restate()
            }
            starting.remove(window)
            noted(window, reply)
        }
    }

    func wantsPlanner(_ window: JSON) -> Bool {
        guard let id = window["id"]?.string, window["desk"]?.bool == true, window["dock"]?["pane"]?.string == nil else { return false }
        return window["lead"]?["pane"]?.string == nil && !starting.contains(id)
    }

    func leadHeader(_ window: JSON, _ root: String?) -> String {
        let lead = window["lead"]?["pane"]?.string
        let place = lead.map(headerText).flatMap { $0.isEmpty ? nil : $0 } ?? root.map(tilde)
        let role = window["desk"]?.bool == true ? "Planning" : window["lead"]?["name"]?.string
        return [role, place].compactMap { $0 }.joined(separator: " · ")
    }
}
