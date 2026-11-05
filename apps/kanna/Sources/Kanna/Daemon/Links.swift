import Darwin
import Foundation
import KannaMachinesC

final class Dial: @unchecked Sendable {
    private let lock = NSLock()
    private var request: String?
    private var failure: JSON?

    func set(_ json: JSON) {
        let text = String(decoding: json.encoded(), as: UTF8.self)
        lock.withLock { request = text }
    }

    func portOpen(_ port: Int) -> JSON {
        guard let request = lock.withLock({ request }), case .object(var fields) = try? JSON.parse(Data(request.utf8)) else {
            return ["error": ["code": "unavailable", "message": "the machine isn't linked yet"]]
        }
        fields["port"] = .number(Double(port))
        let text = String(decoding: JSON.object(fields).encoded(), as: UTF8.self)
        return Links.call { kanna_machines_port_open(text) }
    }

    var lastFailure: JSON? { lock.withLock { failure } }

    var key: String? {
        guard let request = lock.withLock({ request }) else { return nil }
        return (try? JSON.parse(Data(request.utf8)))?["key"]?.string
    }

    func callAsFunction() -> Int32? {
        guard let request = lock.withLock({ request }) else { return nil }
        let reply = Links.call { kanna_machines_link(request) }
        lock.withLock { failure = reply["error"] }
        guard let fd = reply["fd"]?.number.flatMap({ Int32(exactly: $0) }) else { return nil }
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
        return fd
    }
}

@MainActor
final class Links: TerminalLink {
    let local: DaemonLink
    private(set) var remotes: [String: DaemonLink] = [:]
    private var dials: [String: Dial] = [:]
    private(set) var ports: [String: [Int: Int]] = [:]
    private(set) var peers: [JSON] = []
    private(set) var phones: [JSON] = []
    private(set) var name: String?
    var configure: ((DaemonLink) -> Void)?
    var onPeers: (() -> Void)?
    var onGone: ((String) -> Void)?
    private var appKey: String?
    private var cert: JSON?
    private var relayToken: JSON = .null
    private var certifying = false
    private var timer: Timer?
    private let keyPath: String?
    private let commands: Commands

    static let refreshInterval: TimeInterval = 10

    init(local: DaemonLink, commands: Commands, keyPath: String?) {
        self.local = local
        self.commands = commands
        self.keyPath = keyPath
    }

    nonisolated static func call(_ body: () -> UnsafeMutablePointer<CChar>?) -> JSON {
        guard let raw = body() else { return ["error": ["code": "failed", "message": "the app's endpoint didn't answer"]] }
        defer { kanna_machines_free(raw) }
        return (try? JSON.parse(Data(String(cString: raw).utf8))) ?? .null
    }

    var all: [DaemonLink] { [local] + remotes.keys.sorted().compactMap { remotes[$0] } }

    func link(_ machine: String?) -> DaemonLink? { machine.map { remotes[$0] } ?? local }

    func link(for id: String) -> DaemonLink? { link(MachineIDs.machine(of: id)) }

    func start() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: Self.refreshInterval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refresh() }
        }
        refresh()
    }

    func stop() {
        timer?.invalidate()
        for link in remotes.values { link.stop() }
    }

    func refresh() {
        guard local.attached else { return }
        local.request("machine", ["action": "status"]) { [weak self] status in self?.arrived(status) }
    }

    func path(of machine: String) -> JSON {
        guard let key = peers.first(where: { $0["name"]?.string == machine })?["key"]?.string, appKey != nil else { return .null }
        return Self.call { kanna_machines_path(key) }
    }

    func portOpen(_ port: Int, on machine: String) -> Result<Int, KannaError> {
        guard let dial = dials[machine] else { return .failure(KannaError("unavailable", "\(machine) isn't one of your linked machines")) }
        let reply = dial.portOpen(port)
        guard let local = reply["port"]?.number.flatMap({ Int(exactly: $0) }) else {
            return .failure(KannaError(reply["error"]?["code"]?.string ?? "failed", reply["error"]?["message"]?.string ?? "can't open \(machine)'s port \(port)"))
        }
        ports[machine, default: [:]][port] = local
        return .success(local)
    }

    func failure(of machine: String) -> JSON? {
        remotes[machine]?.refusal?["error"] ?? (remotes[machine]?.attached == true ? nil : dials[machine]?.lastFailure)
    }

    private func arrived(_ status: JSON) {
        guard status["ok"]?.bool == true else { return }
        name = status["name"]?.string
        peers = (status["peers"]?.array ?? []).filter { $0["me"]?.bool != true }
        phones = status["phones"]?.array ?? []
        if status["relay_token"]?.bool == true {
            local.request("machine.relay_token", [:]) { [weak self] reply in
                guard let self, reply["ok"]?.bool == true, let token = reply["token"], token != relayToken else { return }
                relayToken = token
                refresh()
            }
        } else {
            relayToken = .null
        }
        let wanted = status["paired"]?.bool == true && status["broken"] == .null ? peers.filter { peer in
            peer["trusted"]?.bool == true && peer["revoked"]?.bool != true && peer["conflict"]?.bool != true && peer["name"]?.string != nil
        } : []
        if !wanted.isEmpty, appKey == nil, let keyPath {
            appKey = Self.call { kanna_machines_client(keyPath) }["key"]?.string
        }
        if !wanted.isEmpty, cert == nil, let appKey, !certifying {
            certifying = true
            local.request("machine", ["action": "certify", "key": .string(appKey)]) { [weak self] reply in
                guard let self else { return }
                certifying = false
                if reply["ok"]?.bool == true, let cert = reply["cert"] {
                    self.cert = cert
                    refresh()
                }
            }
        }
        let nearby = Dictionary((status["nearby"]?.array ?? []).compactMap { n in n["key"]?.string.map { ($0, n["addrs"]?.array ?? []) } }, uniquingKeysWith: { a, _ in a })
        var kept = Set<String>()
        if let cert {
            for peer in wanted {
                guard let machine = peer["name"]?.string, let key = peer["key"]?.string else { continue }
                kept.insert(machine)
                let dial = dials[machine] ?? Dial()
                dials[machine] = dial
                dial.set(["key": .string(key), "addrs": .array((peer["addrs"]?.array ?? []) + (nearby[key] ?? [])), "relay": peer["relay"] ?? .null, "relay_token": relayToken, "cert": cert])
                if remotes[machine] == nil { open(machine, dial) }
            }
        }
        for machine in remotes.keys where !kept.contains(machine) {
            remotes.removeValue(forKey: machine)?.stop()
            if let key = dials.removeValue(forKey: machine)?.key { kanna_machines_port_close(key) }
            ports.removeValue(forKey: machine)
            onGone?(machine)
        }
        onPeers?()
    }

    private func open(_ machine: String, _ dial: Dial) {
        let link = DaemonLink(socketPath: "machine:\(machine)", commands: commands, machine: machine, dial: { dial() })
        link.client = name
        configure?(link)
        remotes[machine] = link
        link.start()
    }

    func opened(_ terminal: ExternalTerminal) { link(for: terminal.id)?.opened(terminal) }

    func input(_ bytes: Data, from terminal: ExternalTerminal) {
        guard let link = link(for: terminal.id), link.attached else { return }
        link.input(bytes, from: terminal)
    }

    func resizeRequested(_ terminal: ExternalTerminal, cols: UInt16, rows: UInt16, width: UInt32, height: UInt32) {
        link(for: terminal.id)?.resizeRequested(terminal, cols: cols, rows: rows, width: width, height: height)
    }

    func closed(_ terminal: ExternalTerminal) { link(for: terminal.id)?.closed(terminal) }

    func scrolledToTop(_ terminal: ExternalTerminal) -> Bool { link(for: terminal.id)?.scrolledToTop(terminal) ?? false }

    func settingsChanged(_ settings: TerminalSettings) {
        for link in all { link.settingsChanged(settings) }
    }
}
