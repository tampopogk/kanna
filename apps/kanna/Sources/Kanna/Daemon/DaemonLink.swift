import Foundation
import KannaFleet
import KannaPerf

@MainActor
final class DaemonLink: TerminalLink {
    private var connection: DaemonConnection!
    private let commands: Commands
    private var terminals: [String: ExternalTerminal] = [:]
    private var sizes: [String: JSON] = [:]
    private(set) var attached = false
    let machine: String?
    var client: String?
    private(set) var refusal: JSON?
    private(set) var daemonInfo: JSON?
    var onAttach: ((JSON) -> Void)?
    var onView: ((JSON) -> Void)?
    var onStart: ((JSON) -> Void)?
    var onStop: ((String) -> Void)?
    var onPane: ((JSON) -> Void)?
    var onExited: ((String, Int32) -> Void)?
    var onSnapshot: ((String) -> Void)?
    var onStatus: ((String, String?) -> Void)?
    var onFolder: ((String, String, JSON?) -> Void)?
    var onBacklog: (([JSON]) -> Void)?
    var onDiff: ((String, JSON) -> Void)?
    var onToast: ((JSON) -> Void)?
    var onQuit: (() -> Void)?
    var onFile: ((String, JSON) -> Void)?
    var onLost: (() -> Void)?
    private var requests = 0
    private var replies: [Int: (JSON) -> Void] = [:]
    private let starter: DaemonStarter?

    var socketPath: String { connection.path }

    static let protocols: [JSON] = Protocols.mine.map { .number(Double($0)) }
    static let localOnly: Set<String> = ["app.fleet.confirm", "app.snap"]

    init(socketPath: String, commands: Commands, starter: DaemonStarter? = nil, machine: String? = nil, dial: (@Sendable () -> Int32?)? = nil) {
        self.commands = commands
        self.starter = starter
        self.machine = machine
        connection = DaemonConnection(
            path: socketPath,
            dial: dial,
            hello: { [weak self] in
                var args: [String: JSON] = ["pid": .number(Double(ProcessInfo.processInfo.processIdentifier)), "protocols": .array(Self.protocols), "commit": .string(KannaBuild.commit)]
                if machine == nil, let build = KannaBuild.daemonBuild { args["build"] = .string(build) }
                if let client = self?.client { args["client"] = .string(client) }
                return ["id": 0, "cmd": "app.hello", "args": .object(args)]
            },
            connected: { [weak self] in self?.connected() },
            received: { [weak self] message, delivery in self?.received(message, delivery) },
            disconnected: { [weak self] in self?.disconnected() },
            unreachable: { starter?.unreachable() })
    }

    func start() { connection.start() }
    func stop() { connection.stop() }

    func quit() {
        starter?.quitting()
        connection.stop()
        onQuit?()
    }

    private func outbound(_ message: JSON) -> JSON {
        machine.map { MachineIDs.local(message, on: $0) } ?? message
    }

    private func send(_ cmd: String, _ args: [String: JSON]) {
        connection.send(outbound(["cmd": .string(cmd), "args": .object(args)]))
    }

    private func connected() {
        starter?.connected()
        if let settings = GhosttyRuntime.shared?.settings { send("settings", settings.objectFields) }
        for terminal in terminals.keys.sorted() {
            send("attach", ["terminal": .string(terminal)])
            if case .object(let size)? = sizes[terminal] { send("resize", size) }
        }
    }

    private func disconnected() {
        let waiting = replies
        replies.removeAll()
        for reply in waiting.values { reply(["ok": false, "error": ["code": "no_daemon", "message": "lost the daemon"]]) }
        if attached { FileHandle.standardError.write(Data("kanna: lost the daemon at \(socketPath)\n".utf8)) }
        onLost?()
        attached = false
    }

    private func received(_ raw: JSON, _ delivery: DaemonConnection.Delivery) {
        let message = machine.map { MachineIDs.qualified(raw, on: $0) } ?? raw
        let span = Perf.begin(.daemon, "message", message["cmd"]?.string ?? (message["ok"] != nil ? "reply" : "unknown"))
        defer { Perf.end(span) }
        let connection = connection!
        var consumed = true
        defer { if consumed { connection.consumed(delivery) } }
        if let ok = message["ok"]?.bool {
            guard message["id"]?.number == 0 else {
                if let id = message["id"]?.number.map(Int.init), let reply = replies.removeValue(forKey: id) { return reply(message) }
                if !ok { FileHandle.standardError.write(Data("kanna: the daemon refused a request: \(message["error"]?["message"]?.string ?? "")\n".utf8)) }
                return
            }
            attached = ok
            if ok {
                refusal = nil
                daemonInfo = message["daemon"]
                connection.accepted()
                onAttach?(message)
            } else if message["error"]?["code"]?.string == "other_protocol" {
                refusal = message
                connection.refused()
                FileHandle.standardError.write(Data("kanna: \(machine ?? "the daemon") speaks another protocol: \(message["error"]?["message"]?.string ?? "")\n".utf8))
            } else if message["error"]?["code"]?.string == "other_build" {
                connection.accepted()
                starter?.replacing()
                FileHandle.standardError.write(Data("kanna: the daemon is from another build and is stopping; starting this build's\n".utf8))
            } else {
                refusal = message
                connection.refused()
                FileHandle.standardError.write(Data("kanna: the daemon refused this app: \(message["error"]?["message"]?.string ?? "")\n".utf8))
            }
            return
        }
        guard let cmd = message["cmd"]?.string else { return }
        if message["id"] != nil, machine != nil, Self.localOnly.contains(cmd) {
            connection.send(["id": message["id"] ?? .null, "ok": false, "error": ["code": "local_only", "message": .string("\(cmd) is answered only for this Mac's own daemon")]], generation: delivery.generation)
            return
        }
        if message["id"] != nil {
            var request = message
            if let machine, case .object(var fields) = request, case .object(var args) = fields["args"] ?? .object([:]) {
                args["machine"] = .string(machine)
                fields["args"] = .object(args)
                request = .object(fields)
            }
            Task { [commands, machine] in
                let reply = await commands.handle(request)
                connection.send(machine.map { MachineIDs.local(reply, on: $0) } ?? reply, generation: delivery.generation)
            }
            return
        }
        let args = message["args"] ?? [:]
        let terminal = args["terminal"]?.string.flatMap { terminals[$0] }
        switch cmd {
        case "start":
            if let pane = args["pane"] { onStart?(pane) }
        case "stop":
            if let pane = args["pane"]?.string { onStop?(pane) }
        case "pane":
            if let pane = args["pane"] { onPane?(pane) }
        case "output":
            if let terminal, let data = args["data"]?.string.flatMap({ Data(base64Encoded: $0) }) {
                consumed = false
                terminal.output(data) { connection.consumed(delivery) }
            }
        case "snapshot":
            if let id = args["terminal"]?.string { onSnapshot?(id) }
            if let terminal, let data = args["data"]?.string.flatMap({ Data(base64Encoded: $0) }) {
                consumed = false
                terminal.loadSnapshot(data) { connection.consumed(delivery) }
            }
        case "grid":
            if let cols = args["cols"]?.number.flatMap({ UInt16(exactly: $0) }), let rows = args["rows"]?.number.flatMap({ UInt16(exactly: $0) }) {
                terminal?.setGrid(cols: cols, rows: rows)
            }
        case "exited":
            if let code = args["code"]?.number.flatMap({ Int32(exactly: $0) }) {
                terminal?.exited(status: code)
                if let id = args["terminal"]?.string { onExited?(id, code) }
            }
        case "status":
            if let pane = args["pane"]?.string { onStatus?(pane, args["status"]?.string) }
        case "layout":
            onView?(args)
        case "diff":
            if let content = args["content"]?.string { onDiff?(content, args) }
        case "file":
            if let content = args["content"]?.string { onFile?(content, args) }
        case "backlog":
            if case .array(let tasks)? = args["tasks"] { onBacklog?(tasks) }
        case "toast":
            onToast?(args)
        case "quit":
            if machine == nil { quit() }
        case "folder":
            if let pane = args["pane"]?.string, let cwd = args["cwd"]?.string { onFolder?(pane, cwd, args["place"]) }
        default: FileHandle.standardError.write(Data("kanna: unknown message from the daemon: \(cmd)\n".utf8))
        }
    }

    func notice(_ cmd: String, _ args: [String: JSON]) {
        guard attached else { return }
        send(cmd, args)
    }

    func update(_ pane: String, _ fields: [String: JSON]) {
        guard attached else { return }
        var args = fields
        args["pane"] = .string(pane)
        send("update", args)
    }

    func request(_ cmd: String, _ args: [String: JSON], reply: ((JSON) -> Void)? = nil, sent: (@Sendable () -> Void)? = nil) {
        guard attached else {
            reply?(["ok": false, "error": ["code": "no_daemon", "message": "Kanna.app is not attached to its daemon"]])
            return
        }
        requests += 1
        if let reply { replies[requests] = reply }
        connection.send(outbound(["id": .number(Double(requests)), "cmd": .string(cmd), "args": .object(args)]), sent: sent)
    }

    func opened(_ terminal: ExternalTerminal) {
        terminals[terminal.id] = terminal
        send("attach", ["terminal": .string(terminal.id)])
    }

    func input(_ bytes: Data, from terminal: ExternalTerminal) {
        send("input", ["terminal": .string(terminal.id), "data": .string(bytes.base64EncodedString())])
    }

    func resizeRequested(_ terminal: ExternalTerminal, cols: UInt16, rows: UInt16, width: UInt32, height: UInt32) {
        let size: [String: JSON] = ["terminal": .string(terminal.id), "cols": .number(Double(cols)), "rows": .number(Double(rows)),
                                    "width": .number(Double(width)), "height": .number(Double(height))]
        sizes[terminal.id] = .object(size)
        send("resize", size)
    }

    func settingsChanged(_ settings: TerminalSettings) {
        send("settings", settings.objectFields)
    }

    func closed(_ terminal: ExternalTerminal) {
        terminals.removeValue(forKey: terminal.id)
        sizes.removeValue(forKey: terminal.id)
        send("detach", ["terminal": .string(terminal.id)])
    }
}
