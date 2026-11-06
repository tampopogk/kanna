import Foundation
import KannaIOS
import Network

extension Result where Failure == Error {
    init(catching body: () async throws -> Success) async {
        do { self = .success(try await body()) } catch { self = .failure(error) }
    }
}

final class Lookup: @unchecked Sendable {
    private var browser: NWBrowser?
    private var connections: [NWConnection] = []
    private let queue = DispatchQueue(label: "kanna.lookup")

    func find(_ instance: String, seconds: Double) async -> String? {
        await withCheckedContinuation { continuation in
            let browser = NWBrowser(for: .bonjour(type: "_kanna-phone._udp", domain: "local."), using: NWParameters())
            self.browser = browser
            self.connections = []
            var finished = false
            let finish: (String?) -> Void = { [weak self] address in
                guard !finished else { return }
                finished = true
                self?.browser?.cancel()
                self?.connections.forEach { $0.cancel() }
                continuation.resume(returning: address)
            }
            browser.browseResultsChangedHandler = { [weak self] results, _ in
                guard let self, self.connections.isEmpty, let match = results.first(where: {
                    if case let .service(name, _, _, _) = $0.endpoint { return name == instance }
                    return false
                }) else { return }
                let parameters = NWParameters.udp
                (parameters.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options)?.version = .v4
                let connection = NWConnection(to: match.endpoint, using: parameters)
                connection.stateUpdateHandler = { state in
                    guard case .ready = state, case let .hostPort(host, port)? = connection.currentPath?.remoteEndpoint else { return }
                    var text = "\(host)"
                    if let percent = text.firstIndex(of: "%") { text = String(text[..<percent]) }
                    finish("\(text):\(port)")
                }
                self.connections.append(connection)
                connection.start(queue: self.queue)
            }
            browser.stateUpdateHandler = { state in
                if case .failed = state { finish(nil) }
            }
            browser.start(queue: queue)
            queue.asyncAfter(deadline: .now() + seconds) { finish(nil) }
        }
    }
}

@MainActor
final class LiveBackend: Backend {
    var onNotice: ((Notice) -> Void)?
    var onLost: (() -> Void)?
    private(set) var paired: Paired?
    private var secret: String?
    private var stream: UInt64?
    private var next = 1
    private var waiting: [Int: CheckedContinuation<Data, Error>] = [:]
    private let lookup = Lookup()
    private var stopping: Task<Void, Never>?
    private var pairing: String?

    private static var file: URL {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("machine.json")
    }

    init() {
        secret = Keychain.read()
        if let data = try? Data(contentsOf: Self.file) { paired = try? JSONDecoder().decode(Paired.self, from: data) }
    }

    var phoneName: String {
        guard let key = pairing ?? secret.flatMap(Rust.key) else { return "" }
        return "iphone-\(key.prefix(4))"
    }

    private func save() {
        if let data = try? JSONEncoder().encode(paired) { try? data.write(to: Self.file, options: .completeFileProtection) }
    }

    private func started() async throws {
        if secret == nil { secret = Keychain.make() }
        guard let secret else { throw Refused(code: "no_key", message: "This phone can't keep a key: set a passcode") }
        await stopping?.value
        _ = try await Task.detached { try Rust.start(secret: secret) }.value
    }

    func pair(scanned: String) async throws -> Paired {
        forget()
        secret = Keychain.make()
        pairing = secret.flatMap(Rust.key)
        try await started()
        let joined = try await Task.detached { try Rust.join(scanned) }.value
        let result = Paired(name: joined["name"] as? String ?? "", machine: joined["machine"] as? String ?? "", key: joined["key"] as? String ?? "", addrs: joined["addrs"] as? [String] ?? [], good: nil)
        paired = result
        save()
        return result
    }

    private func open(_ key: String, _ addrs: [String]) async throws -> UInt64 {
        let text = Rust.text(["key": key, "addrs": addrs, "header": ["kind": "phone", "phone": 1]])
        return try await Task.detached { () -> UInt64 in
            guard let stream = (try Rust.open(text)["stream"] as? NSNumber)?.uint64Value else { throw Refused(code: "failed", message: "no stream") }
            return stream
        }.value
    }

    func connect() async throws -> Hello {
        guard var machine = paired else { throw Refused(code: "unpaired", message: "not paired") }
        try await started()
        let known = [machine.good].compactMap { $0 } + machine.addrs
        let instance = Rust.instance(machine.key)
        let (key, lookup) = (machine.key, self.lookup)
        var refused: Refused?
        let opened: UInt64? = await withTaskGroup(of: Result<UInt64, Error>.self) { group in
            group.addTask { @MainActor in await Result(catching: { try await self.open(key, known) }) }
            group.addTask { @MainActor in
                guard let instance, let found = await lookup.find(instance, seconds: 8) else { return .failure(Refused(code: "unreachable", message: "not found")) }
                return await Result(catching: { try await self.open(key, [found]) })
            }
            for await result in group {
                switch result {
                case .success(let stream):
                    group.cancelAll()
                    return stream
                case .failure(let error):
                    if let error = error as? Refused, error.code == "revoked" { refused = error }
                }
            }
            return nil
        }
        if let refused { throw refused }
        guard let opened else { throw Refused(code: "unreachable", message: "Can't reach \(machine.machine)") }
        stream = opened
        listen(opened)
        let hello: Hello = try await call("hello", [:])
        machine.addrs = hello.addrs
        if let remote = (await Task.detached { Rust.path(key) }.value)["remote"] as? String { machine.good = remote }
        paired = machine
        save()
        return hello
    }

    private func listen(_ stream: UInt64) {
        let thread = Thread { [weak self] in
            while true {
                do {
                    let line = try Rust.read(stream)
                    let weak = self
                    DispatchQueue.main.async { MainActor.assumeIsolated { weak?.received(line) } }
                } catch {
                    let weak = self
                    DispatchQueue.main.async { MainActor.assumeIsolated { weak?.lost(stream) } }
                    return
                }
            }
        }
        thread.start()
    }

    private func received(_ data: Data) {
        let line = Rust.object(data)
        if let id = (line["id"] as? NSNumber)?.intValue, let waiter = waiting.removeValue(forKey: id) {
            if line["ok"] as? Bool == true {
                waiter.resume(returning: data)
            } else {
                let error = line["error"] as? [String: Any]
                waiter.resume(throwing: Refused(code: error?["code"] as? String ?? "failed", message: error?["message"] as? String ?? "failed"))
            }
            return
        }
        guard let repo = line["repo"] as? String else { return }
        switch line["notice"] as? String {
        case "chat":
            if let raw = line["item"], let body = try? JSONSerialization.data(withJSONObject: raw), let item = try? JSONDecoder().decode(Item.self, from: body) {
                onNotice?(.chat(repo: repo, item: item))
            }
        case "planner":
            onNotice?(.planner(repo: repo, status: line["status"] as? String ?? "", since: line["since"] as? Double ?? 0))
        default: break
        }
    }

    private func lost(_ dead: UInt64) {
        guard stream == dead else { return }
        stream = nil
        let failed = waiting
        waiting = [:]
        failed.values.forEach { $0.resume(throwing: Refused(code: "unreachable", message: "lost the link")) }
        onLost?()
    }

    private func call<T: Decodable>(_ cmd: String, _ args: [String: Any]) async throws -> T {
        guard let stream else { throw Refused(code: "unreachable", message: "not connected") }
        let id = next
        next += 1
        let line = Rust.text(["id": id, "cmd": cmd, "args": args])
        let data: Data = try await withCheckedThrowingContinuation { continuation in
            waiting[id] = continuation
            Task.detached {
                do {
                    try Rust.write(stream, line)
                } catch {
                    await MainActor.run { self.waiting.removeValue(forKey: id)?.resume(throwing: error) }
                }
            }
        }
        return try JSONDecoder().decode(T.self, from: data)
    }

    private struct Sent: Decodable { var item: Item }

    func read(repo: String, after: Int) async throws -> Page {
        try await call("chat.read", ["repo": repo, "after": after])
    }

    func send(repo: String, id: String, text: String) async throws -> Item {
        let sent: Sent = try await call("chat.send", ["repo": repo, "id": id, "text": text])
        return sent.item
    }

    func choose(repo: String, id: String, seq: Int, choice: String) async throws -> Item {
        let sent: Sent = try await call("chat.choose", ["repo": repo, "id": id, "seq": seq, "choice": choice])
        return sent.item
    }

    func close() {
        stream = nil
        let failed = waiting
        waiting = [:]
        failed.values.forEach { $0.resume(throwing: Refused(code: "unreachable", message: "closed")) }
        let before = stopping
        stopping = Task.detached {
            await before?.value
            Rust.stop()
        }
    }

    func forget() {
        close()
        paired = nil
        pairing = nil
        try? FileManager.default.removeItem(at: Self.file)
        Keychain.delete()
        secret = nil
    }

    func link() async -> Link {
        guard let key = paired?.key, stream != nil else { return Link(path: .off, milliseconds: 0) }
        let path = await Task.detached { Rust.path(key) }.value
        let milliseconds = Int((path["rtt_ms"] as? Double ?? 0).rounded())
        switch (path["path"] as? String, path["lan"] as? Bool) {
        case ("direct", true): return Link(path: .lan, milliseconds: milliseconds)
        case ("direct", _): return Link(path: .net, milliseconds: milliseconds)
        case ("relay", _): return Link(path: .relay, milliseconds: milliseconds)
        default: return Link(path: .off, milliseconds: 0)
        }
    }

    func highlight(file: String, text: String) -> [TextSpan] { Rust.highlight(file: file, text: text) }
}
