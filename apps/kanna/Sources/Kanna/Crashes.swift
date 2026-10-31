import AppKit
import Darwin
import KannaCrashC

typealias ExceptionPreprocessor = @convention(c) (UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer?

@_silgen_name("objc_setExceptionPreprocessor")
func objc_setExceptionPreprocessor(_ preprocessor: ExceptionPreprocessor?) -> ExceptionPreprocessor?

final class KannaApplication: NSApplication {
    override func reportException(_ exception: NSException) {
        CrashHandlers.record(exception)
        super.reportException(exception)
    }
}

enum CrashHandlers {
    nonisolated(unsafe) static var stateDir = ""
    nonisolated(unsafe) static var panes: [JSON] = []
    nonisolated(unsafe) private static var altStack: UnsafeMutableRawPointer?
    nonisolated(unsafe) private static var preprocessor: ExceptionPreprocessor?
    nonisolated(unsafe) private static var thrown: (exception: NSException, at: Date)?
    static let thrownWithin: TimeInterval = 5
    static let signals: [Int32: String] = [SIGSEGV: "SIGSEGV", SIGBUS: "SIGBUS", SIGILL: "SIGILL", SIGTRAP: "SIGTRAP", SIGFPE: "SIGFPE", SIGABRT: "SIGABRT"]

    @MainActor static func install() {
        UserDefaults.standard.register(defaults: ["NSApplicationCrashOnExceptions": true])
        installSignals()
        let environment = ProcessInfo.processInfo.environment
        stateDir = environment["KANNA_STATE_DIR"].flatMap { $0.isEmpty ? nil : $0 } ?? (AppController.daemonSocketPath as NSString).deletingLastPathComponent
        kanna_crash_install(stateDir)
        NSSetUncaughtExceptionHandler { exception in CrashHandlers.record(exception) }
        preprocessor = objc_setExceptionPreprocessor { object in
            let result = CrashHandlers.preprocessor.map { $0(object) } ?? object
            if let object, let exception = Unmanaged<AnyObject>.fromOpaque(object).takeUnretainedValue() as? NSException {
                CrashHandlers.thrown = (exception, Date())
            }
            return result
        }
    }

    static func installSignals() {
        let size = 1 << 16
        altStack = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: 16)
        var stack = stack_t(ss_sp: altStack, ss_size: size, ss_flags: 0)
        sigaltstack(&stack, nil)
        for signal in signals.keys {
            var action = sigaction()
            action.__sigaction_u.__sa_handler = { signal in CrashHandlers.caught(signal) }
            action.sa_flags = SA_ONSTACK
            sigemptyset(&action.sa_mask)
            sigaction(signal, &action, nil)
        }
    }

    static var build: JSON {
        let info = Bundle.main.infoDictionary ?? [:]
        return ["id": KannaBuild.daemonBuild.map { .string($0) } ?? .null, "channel": .string(KannaBuild.channel), "commit": .string(KannaBuild.commit),
                "version": .string(info["CFBundleShortVersionString"] as? String ?? "")]
    }

    static func record(_ exception: NSException) {
        guard !kanna_crash_written() else { return }
        let reason = "\(exception.name.rawValue): \(exception.reason ?? "")"
        let stack = exception.callStackSymbols.isEmpty ? Thread.callStackSymbols : exception.callStackSymbols
        write(kind: "exception", reason: reason, stack: stack)
    }

    static func caught(_ signal: Int32) {
        if signal == SIGTRAP || signal == SIGABRT, let thrown, Date().timeIntervalSince(thrown.at) < thrownWithin {
            record(thrown.exception)
        } else if !kanna_crash_written() {
            write(kind: "signal", reason: signals[signal] ?? "signal \(signal)", stack: Thread.callStackSymbols)
        }
        Darwin.signal(signal, SIG_DFL)
        raise(signal)
    }

    static func write(kind: String, reason: String, stack: [String]) {
        let request: JSON = ["stateDir": .string(stateDir), "process": "app", "kind": .string(kind), "reason": .string(reason),
                             "stack": .array(stack.map { .string($0) }), "build": build, "panes": .array(panes)]
        guard let text = String(data: request.encoded(), encoding: .utf8), let answer = kanna_crash_write(text) else { return }
        defer { kanna_crash_free(answer) }
        let reply = (try? JSON.parse(Data(String(cString: answer).utf8))) ?? .null
        let line = reply["path"]?.string.map { "kanna: wrote the crash record \($0)\n" } ?? "kanna: can't write a crash record: \(reply["error"]?.string ?? "")\n"
        FileHandle.standardError.write(Data(line.utf8))
    }
}

final class CrashView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSException(name: .internalInconsistencyException, reason: "debug.crash in a draw", userInfo: nil).raise()
    }
}

extension AppController {
    func notePanes() {
        let named = Dictionary(names.map { ($0.value, $0.key) }, uniquingKeysWith: { first, _ in first })
        CrashHandlers.panes = panes.all.sorted { $0.id < $1.id }.map { view in
            ["id": .string(view.id), "type": .string(view.type), "name": named[view.id].map { .string($0) } ?? .null]
        }
    }

    func installCrashCommands() {
        guard debugCommands else { return }
        commands["debug.crash"] = { [weak self] _ in
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in self?.crashInADraw() }
            return ["crashing": true, "pid": .number(Double(ProcessInfo.processInfo.processIdentifier))]
        }
    }

    private func crashInADraw() {
        notePanes()
        let window = controllers.values.first { $0.window.isVisible }?.window ?? {
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 40, height: 40), styleMask: .borderless, backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.orderFrontRegardless()
            return window
        }()
        let view = CrashView(frame: NSRect(x: 0, y: 0, width: 20, height: 20))
        window.contentView?.addSubview(view)
        view.needsDisplay = true
    }

    func dismissCrashes(_ toast: Toast) {
        guard let record = toast.recordPath else { return }
        request("crashes", ["action": "dismiss", "file": .string(record)])
    }

    func openCrashRecord(_ toast: Toast) {
        guard let record = toast.recordPath else { return }
        dismissCrashes(toast)
        layoutCommand("open", ["type": "code", "path": .string(record), "window": "new"])
    }
}
