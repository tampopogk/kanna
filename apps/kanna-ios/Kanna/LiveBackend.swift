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
    private var giveUp: (() -> Void)?

    func find(_ instance: String, seconds: Double) async -> String? {
        await withTaskCancellationHandler {
            await look(for: instance, seconds: seconds)
        } onCancel: {
            queue.async { [self] in giveUp?() }
        }
    }

    private func look(for instance: String, seconds: Double) async -> String? {
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
            queue.async { [weak self] in
                if Task.isCancelled { finish(nil) }
                self?.giveUp = { finish(nil) }
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
    private var linked: String?

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
        let result = Paired(name: joined["name"] as? String ?? "", machine: joined["machine"] as? String ?? "", key: joined["key"] as? String ?? "", addrs: joined["addrs"] as? [String] ?? [], good: nil, relay: joined["relay"] as? String)
        paired = result
        Keychain.keepRelayToken(joined["relay_token"] as? String)
        save()
        return result
    }

    private func open(_ key: String, _ addrs: [String], relayed: Bool) async throws -> UInt64 {
        var request: [String: Any] = ["key": key, "addrs": addrs, "header": ["kind": "phone", "phone": 1]]
        if relayed, let relay = paired?.relay {
            request["relay"] = relay
            request["relay_token"] = Keychain.read("relay")
        }
        let text = Rust.text(request)
        return try await Task.detached { () -> UInt64 in
            guard let stream = (try Rust.open(text)["stream"] as? NSNumber)?.uint64Value else { throw Refused(code: "failed", message: "no stream") }
            return stream
        }.value
    }

    func connect(to wanted: Reach?) async throws -> Hello {
        guard var saved = paired else { throw Refused(code: "unpaired", message: "not paired") }
        let machine = wanted ?? Reach(name: saved.machine, key: saved.key, addrs: saved.addrs, good: saved.good)
        try await started()
        let key = machine.key
        let opened = try await dial(machine)
        stream = opened
        linked = key
        listen(opened)
        let hello = try await hello()
        let path = await Task.detached { Rust.path(key) }.value
        let good = path["path"] as? String == "direct" ? path["remote"] as? String : nil
        if let relay = hello.relay {
            saved.relay = relay
            Keychain.keepRelayToken(hello.relay_token)
        }
        if key == saved.key {
            saved.addrs = hello.addrs
            saved.good = good ?? saved.good
        }
        var reached = hello.machines ?? []
        for index in reached.indices {
            reached[index].good = reached[index].key == key ? good : saved.known?.first { $0.key == reached[index].key }?.good
        }
        saved.known = reached
        paired = saved
        save()
        return hello
    }

    func reaches(_ machine: Reach) async -> Bool {
        (try? await dial(machine)) != nil
    }

    private func dial(_ machine: Reach) async throws -> UInt64 {
        let known = [machine.good].compactMap { $0 } + machine.addrs
        let instance = Rust.instance(machine.key)
        let (key, lookup) = (machine.key, self.lookup)
        var refused: Refused?
        let opened: UInt64? = await withTaskGroup(of: Result<UInt64, Error>.self) { group in
            group.addTask { @MainActor in await Result(catching: { try await self.open(key, known, relayed: true) }) }
            group.addTask { @MainActor in
                guard let instance, let found = await lookup.find(instance, seconds: 8) else { return .failure(Refused(code: "unreachable", message: "not found")) }
                return await Result(catching: { try await self.open(key, [found], relayed: false) })
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
        guard let opened else { throw Refused(code: "unreachable", message: "Can't reach \(machine.name)") }
        return opened
    }

    func networkChanged() {
        Task.detached { Rust.networkChanged() }
    }

    func hello() async throws -> Hello {
        try await call("hello", [:])
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
        if line["notice"] as? String == "projects" { return onNotice?(.projects) ?? () }
        guard let repo = line["repo"] as? String else { return }
        switch line["notice"] as? String {
        case "chat":
            if let raw = line["item"], let body = try? JSONSerialization.data(withJSONObject: raw), let item = try? JSONDecoder().decode(Item.self, from: body) {
                onNotice?(.chat(repo: repo, item: item))
            }
        case "planner":
            onNotice?(.planner(repo: repo, status: line["status"] as? String ?? "", since: line["since"] as? Double ?? 0))
        case "tasks":
            onNotice?(.tasks(repo: repo))
        case "quiz":
            onNotice?(.quiz(repo: repo))
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

    private struct Done: Decodable {}

    func tasks(repo: String) async throws -> ShelfPage {
        try await call("tasks.list", ["repo": repo])
    }

    func task(_ id: String) async throws -> TaskPage {
        try await call("task.read", ["task": id])
    }

    func tell(task: String, id: String, text: String) async throws {
        let _: Done = try await call("task.send", ["task": task, "id": id, "text": text])
    }

    func choose(task: String, id: String, seq: Int, choice: String) async throws {
        let _: Done = try await call("task.choose", ["task": task, "id": id, "seq": seq, "choice": choice])
    }

    func pick(task: String, desk: String, room: String?) async throws {
        var args = ["task": task, "desk": desk]
        if let room { args["room"] = room }
        let _: Done = try await call("task.pick", args)
    }

    func quizMap(repo: String) async throws -> MapPage {
        try await call("quiz.map", ["repo": repo])
    }

    func quizRounds(repo: String) async throws -> RoundsPage {
        try await call("quiz.rounds", ["repo": repo])
    }

    func quizAnswer(round: String, n: Int, pick: Int, again: String?) async throws {
        var args: [String: Any] = ["round": round, "n": n, "pick": pick]
        if let again { args["again"] = again }
        let _: Done = try await call("quiz.answer", args)
    }

    func quizAhead(repo: String, about: String?, again: Bool) async throws {
        var args: [String: Any] = ["repo": repo, "again": again]
        if let about { args["about"] = about }
        let _: Done = try await call("quiz.ahead", args)
    }

    func machines(repo: String) async throws -> MachinesPage {
        try await call("machines", ["repo": repo])
    }

    func stats(repo: String) async throws -> StatsPage {
        try await call("stats", ["repo": repo])
    }

    func move(repo: String, machine: String) async throws -> Moved {
        try await call("planner.move", ["repo": repo, "machine": machine])
    }

    func close() {
        stream = nil
        linked = nil
        let failed = waiting
        waiting = [:]
        failed.values.forEach { $0.resume(throwing: Refused(code: "unreachable", message: "closed")) }
        let before = stopping
        stopping = Task.detached {
            await before?.value
            Rust.stop()
        }
    }

    func unlink() {
        let key = linked
        stream = nil
        linked = nil
        let failed = waiting
        waiting = [:]
        failed.values.forEach { $0.resume(throwing: Refused(code: "unreachable", message: "unlinked")) }
        if let key { Task.detached { Rust.forget(key) } }
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
        guard let key = linked, stream != nil else { return Link(path: .off, milliseconds: 0) }
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

    func drawing(_ source: String) -> DrawingScene? { Rust.drawing(source) }
}
