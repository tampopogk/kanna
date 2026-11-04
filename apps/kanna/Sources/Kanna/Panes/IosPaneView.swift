import AppKit
import SimBridge

enum Simulator {
    struct Device {
        let name: String
        let udid: String
        let booted: Bool
        let version: [Int]
    }

    struct Failure: Error { let message: String }

    static let developerDir: String = {
        if let dir = ProcessInfo.processInfo.environment["DEVELOPER_DIR"], !dir.isEmpty { return dir }
        let out = (try? run("/usr/bin/xcode-select", ["-p"]))?.out.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return out.isEmpty ? "/Applications/Xcode.app/Contents/Developer" : out
    }()

    static func run(_ tool: String, _ arguments: [String]) throws -> (status: Int32, out: String, err: String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        let out = Pipe(), err = Pipe()
        process.standardOutput = out
        process.standardError = err
        try process.run()
        let output = out.fileHandleForReading.readDataToEndOfFile()
        let errors = err.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return (process.terminationStatus, String(decoding: output, as: UTF8.self), String(decoding: errors, as: UTF8.self))
    }

    @discardableResult
    static func simctl(_ arguments: String...) async throws -> String {
        let result = try await Task.detached { try run("/usr/bin/xcrun", ["simctl"] + arguments) }.value
        guard result.status == 0 else {
            let message = result.err.trimmingCharacters(in: .whitespacesAndNewlines)
            throw Failure(message: message.isEmpty ? "simctl \(arguments.first ?? "") failed (\(result.status))" : message)
        }
        return result.out
    }

    static func devices() async throws -> [Device] {
        let json = try await simctl("list", "devices", "available", "--json")
        guard let root = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any],
              let runtimes = root["devices"] as? [String: [[String: Any]]] else { throw Failure(message: "can't read the simulator device list") }
        var out: [Device] = []
        for (runtime, list) in runtimes {
            guard let last = runtime.split(separator: ".").last, last.hasPrefix("iOS-") else { continue }
            let version = last.dropFirst(4).split(separator: "-").compactMap { Int($0) }
            for d in list {
                guard let name = d["name"] as? String, let udid = d["udid"] as? String else { continue }
                out.append(Device(name: name, udid: udid, booted: d["state"] as? String == "Booted", version: version))
            }
        }
        return out.sorted { $1.version.lexicographicallyPrecedes($0.version) }
    }

    static func resolve(_ wanted: String?) async throws -> Device {
        let all = try await devices()
        if let wanted {
            if let device = all.first(where: { $0.udid.caseInsensitiveCompare(wanted) == .orderedSame }) { return device }
            let named = all.filter { $0.name == wanted }
            if let device = named.first(where: \.booted) ?? named.first { return device }
            var names: [String] = []
            for device in all where !names.contains(device.name) { names.append(device.name) }
            throw Failure(message: "Unknown device \"\(wanted)\".\nAvailable: \(names.joined(separator: ", "))")
        }
        let phones = all.filter { $0.name.hasPrefix("iPhone") }
        if let booted = phones.first(where: \.booted) { return booted }
        let newest = phones.filter { $0.version == phones.first?.version }
        guard let device = newest.min(by: { $0.name.count < $1.name.count }) else { throw Failure(message: "No iPhone simulators are installed (Xcode → Settings → Components).") }
        return device
    }

    static func boot(_ device: Device) async throws {
        try await simctl("bootstatus", device.udid, "-b")
    }

    static func checkFits(_ app: String, on device: Device) throws {
        guard let info = Bundle(path: app)?.infoDictionary else { throw Failure(message: "no app at \(app)") }
        let name = (app as NSString).lastPathComponent
        if let platform = info["DTPlatformName"] as? String, platform != "iphonesimulator" {
            throw Failure(message: "\(name) was built for \(platform), not the simulator (build with -sdk iphonesimulator)")
        }
        guard let minimum = info["MinimumOSVersion"] as? String else { return }
        let wanted = minimum.split(separator: ".").compactMap { Int($0) }
        if device.version.lexicographicallyPrecedes(wanted) {
            let have = device.version.map(String.init).joined(separator: ".")
            throw Failure(message: "\(name) needs iOS \(minimum) and \(device.name) runs iOS \(have). Build it with IPHONEOS_DEPLOYMENT_TARGET=\(have), or add that runtime to this Mac.")
        }
    }

    static func launch(_ app: String, on device: Device) async throws -> String {
        var bundleID = app
        var name = app
        if app.hasSuffix(".app") || app.contains("/") {
            guard let bundle = Bundle(path: app), let id = bundle.bundleIdentifier else { throw Failure(message: "no app at \(app)") }
            try await simctl("install", device.udid, app)
            bundleID = id
            name = (app as NSString).lastPathComponent.replacingOccurrences(of: ".app", with: "")
        } else if let installed = try? await simctl("get_app_container", device.udid, app, "app"),
                  let bundle = Bundle(path: installed.trimmingCharacters(in: .whitespacesAndNewlines)) {
            name = bundle.infoDictionary?["CFBundleDisplayName"] as? String ?? bundle.infoDictionary?["CFBundleName"] as? String ?? app
        }
        try await simctl("launch", "--terminate-running-process", device.udid, bundleID)
        return name
    }

    static func reattach(_ app: String, on device: Device) async throws -> String {
        let bundled = app.hasSuffix(".app") || app.contains("/")
        let bundleID = bundled ? Bundle(path: app)?.bundleIdentifier ?? app : app
        guard let running = try? await simctl("spawn", device.udid, "launchctl", "list"), running.contains("UIKitApplication:\(bundleID)[") else {
            return try await launch(app, on: device)
        }
        if bundled { return (app as NSString).lastPathComponent.replacingOccurrences(of: ".app", with: "") }
        guard let installed = try? await simctl("get_app_container", device.udid, app, "app"),
              let bundle = Bundle(path: installed.trimmingCharacters(in: .whitespacesAndNewlines)) else { return app }
        return bundle.infoDictionary?["CFBundleDisplayName"] as? String ?? bundle.infoDictionary?["CFBundleName"] as? String ?? app
    }
}

@MainActor
final class IosPaneView: NSView {
    private let screenView = NSView()
    private let label = NSTextField(labelWithString: "")
    private let status = NSTextField(labelWithString: "")
    private let spinner = NSProgressIndicator()
    private(set) var screen: KSimScreen?
    var onFocus: (() -> Void)?
    var contextMenu: (() -> NSMenu?)?

    static let background = NSColor(red: 0x12 / 255, green: 0x14 / 255, blue: 0x1a / 255, alpha: 1)
    private static let labelHeight: CGFloat = 22

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: 400, height: 800))
        wantsLayer = true
        layer?.backgroundColor = Self.background.cgColor

        screenView.wantsLayer = true
        screenView.layer?.backgroundColor = NSColor.black.cgColor
        screenView.layer?.contentsGravity = .resize
        screenView.layer?.masksToBounds = true
        screenView.layer?.borderColor = NSColor(white: 0.25, alpha: 1).cgColor
        screenView.layer?.borderWidth = 1
        addSubview(screenView)

        for field in [label, status] {
            field.font = .systemFont(ofSize: 12)
            field.textColor = .secondaryLabelColor
            field.alignment = .center
            field.lineBreakMode = .byTruncatingMiddle
            addSubview(field)
        }
        status.textColor = .white
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        addSubview(spinner)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private var question: IosApproval?
    private var answered: ((Bool) -> Void)?

    func ask(_ text: String, done: @escaping (Bool) -> Void) {
        question?.removeFromSuperview()
        let approval = IosApproval(text: text) { [weak self] run in _ = self?.answer(run) }
        addSubview(approval)
        question = approval
        answered = done
        show(status: nil)
        needsLayout = true
    }

    @discardableResult
    func answer(_ run: Bool) -> Bool {
        guard let done = answered else { return false }
        answered = nil
        question?.removeFromSuperview()
        question = nil
        done(run)
        return true
    }

    func show(status text: String?) {
        status.stringValue = text ?? ""
        status.isHidden = text == nil
        text == nil ? spinner.stopAnimation(nil) : spinner.startAnimation(nil)
        needsLayout = true
    }

    func show(device: String, app: String) {
        label.stringValue = "\(device) · \(app)"
        needsLayout = true
    }

    func attach(udid: String) throws {
        guard let layer = screenView.layer else { return }
        let screen = try KSimScreen(udid: udid, developerDir: Simulator.developerDir, layer: layer)
        screen.onResize = { [weak self] in self?.needsLayout = true }
        self.screen = screen
        show(status: nil)
        needsLayout = true
    }

    func stop() {
        screen?.stop()
        screen = nil
    }

    private var aspect: CGFloat {
        guard let size = screen?.pixelSize, size.width > 0, size.height > 0 else { return 1179 / 2556 }
        return size.width / size.height
    }

    override func layout() {
        super.layout()
        let area = bounds.insetBy(dx: 8, dy: 8)
        let room = NSRect(x: area.minX, y: area.minY + Self.labelHeight, width: area.width, height: max(area.height - Self.labelHeight, 1))
        var size = NSSize(width: room.width, height: room.width / aspect)
        if size.height > room.height { size = NSSize(width: room.height * aspect, height: room.height) }
        screenView.frame = NSRect(x: (room.midX - size.width / 2).rounded(), y: (room.midY - size.height / 2).rounded(), width: size.width.rounded(), height: size.height.rounded())
        screenView.layer?.cornerRadius = size.width * 0.13
        label.frame = NSRect(x: area.minX, y: screenView.frame.minY - Self.labelHeight, width: area.width, height: 16)
        status.sizeToFit()
        let width = status.frame.width + 22
        status.frame.origin = NSPoint(x: screenView.frame.midX - width / 2 + 22, y: screenView.frame.midY - status.frame.height / 2)
        spinner.frame = NSRect(x: screenView.frame.midX - width / 2, y: screenView.frame.midY - 8, width: 16, height: 16)
        if let question {
            let inset = min(16, screenView.frame.width / 12)
            let area = screenView.frame.insetBy(dx: inset, dy: 0)
            let height = question.height(for: area.width)
            question.frame = NSRect(x: area.minX, y: (screenView.frame.midY - height / 2).rounded(), width: area.width, height: height)
        }
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var acceptsFirstResponder: Bool { true }

    override func becomeFirstResponder() -> Bool {
        onFocus?()
        return true
    }

    private func fraction(_ event: NSEvent, clamped: Bool) -> CGPoint? {
        let point = screenView.convert(event.locationInWindow, from: nil)
        guard clamped || screenView.bounds.contains(point), screenView.bounds.width > 0, screenView.bounds.height > 0 else { return nil }
        return CGPoint(x: point.x / screenView.bounds.width, y: 1 - point.y / screenView.bounds.height)
    }

    private var touching = false

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        onFocus?()
        guard let screen, let at = fraction(event, clamped: false) else { return }
        touching = true
        screen.touch(.leftMouseDown, at: at)
    }

    override func mouseDragged(with event: NSEvent) {
        guard touching, let at = fraction(event, clamped: true) else { return }
        screen?.touch(.leftMouseDragged, at: at)
    }

    override func mouseUp(with event: NSEvent) {
        guard touching, let at = fraction(event, clamped: true) else { return }
        touching = false
        screen?.touch(.leftMouseUp, at: at)
    }

    override func menu(for event: NSEvent) -> NSMenu? { contextMenu?() }
}

@MainActor
private final class IosApproval: NSView {
    private let text: NSTextField
    private let run = NSButton(title: "Run", target: nil, action: nil)
    private let decline = NSButton(title: "Don't Run", target: nil, action: nil)
    private let done: (Bool) -> Void

    init(text words: String, done: @escaping (Bool) -> Void) {
        text = NSTextField(wrappingLabelWithString: words)
        self.done = done
        super.init(frame: .zero)
        text.font = .systemFont(ofSize: 12)
        text.textColor = .white
        text.alignment = .center
        addSubview(text)
        for (button, action) in [(run, #selector(accept)), (decline, #selector(refuse))] {
            button.target = self
            button.action = action
            button.controlSize = .small
            button.bezelStyle = .rounded
            addSubview(button)
        }
        run.keyEquivalent = "\r"
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func height(for width: CGFloat) -> CGFloat {
        text.preferredMaxLayoutWidth = width
        return text.sizeThatFits(NSSize(width: width, height: 1000)).height + 40
    }

    override func layout() {
        super.layout()
        run.sizeToFit()
        decline.sizeToFit()
        let total = run.frame.width + decline.frame.width + 8
        decline.frame.origin = NSPoint(x: (bounds.width - total) / 2, y: 0)
        run.frame.origin = NSPoint(x: decline.frame.maxX + 8, y: 0)
        text.frame = NSRect(x: 0, y: 34, width: bounds.width, height: bounds.height - 34)
    }

    @objc private func accept() { done(true) }
    @objc private func refuse() { done(false) }
}
