import Foundation
import KannaIOS
import Network
import SwiftUI

struct Target: Codable {
    let key: String
    let addrs: [String]
    let instance: String
    var daemonKey: String?
    var daemonInstance: String?
}

struct Check: Identifiable {
    let id = UUID()
    let name: String
    let result: String
    let good: Bool
}

enum Rust {
    static func take(_ pointer: UnsafeMutablePointer<CChar>?) -> [String: Any] {
        guard let pointer else { return ["error": ["message": "no reply"]] }
        defer { kanna_phone_free(pointer) }
        let data = Data(String(cString: pointer).utf8)
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }

    static func text(_ value: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) else { return "\(value)" }
        return String(decoding: data, as: UTF8.self)
    }

    static func failed(_ reply: [String: Any]) -> String? {
        (reply["error"] as? [String: Any]).map { ($0["message"] as? String) ?? "failed" }
    }

    static func start(_ secret: String?) -> [String: Any] {
        guard let secret else { return take(kanna_phone_start(nil)) }
        return secret.withCString { take(kanna_phone_start($0)) }
    }

    static func open(_ key: String, _ addrs: [String], _ header: [String: Any]) -> [String: Any] {
        text(["key": key, "addrs": addrs, "header": header]).withCString { take(kanna_phone_open($0)) }
    }

    static func echo(_ stream: UInt64, _ line: [String: Any]) -> [String: Any] {
        let wrote = text(line).withCString { take(kanna_phone_write(stream, $0)) }
        if failed(wrote) != nil { return wrote }
        return take(kanna_phone_read(stream))
    }

    static func path(_ key: String) -> [String: Any] {
        key.withCString { take(kanna_phone_path($0)) }
    }
}

final class Lookup {
    private var browser: NWBrowser?
    private var connections: [NWConnection] = []
    private let queue = DispatchQueue(label: "probe.lookup")

    func find(_ instance: String, seconds: Double = 45, seen: @escaping ([String]) -> Void, done: @escaping (String?, [String]) -> Void) {
        let browser = NWBrowser(for: .bonjour(type: "_kanna._udp", domain: "local."), using: NWParameters())
        self.browser = browser
        connections = []
        var finished = false
        var names: [String] = []
        let finish: (String?) -> Void = { [weak self] address in
            guard !finished else { return }
            finished = true
            self?.browser?.cancel()
            self?.connections.forEach { $0.cancel() }
            done(address, names)
        }
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            guard self?.connections.isEmpty ?? true else { return }
            names = results.compactMap {
                if case let .service(name, _, _, _) = $0.endpoint { return name }
                return nil
            }
            seen(names)
            guard let self, let match = results.first(where: {
                if case let .service(name, _, _, _) = $0.endpoint { return name == instance }
                return false
            }) else { return }
            names = ["matched \(instance)"]
            let v4 = NWParameters.udp
            (v4.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options)?.version = .v4
            for parameters in [v4, NWParameters.udp] {
                let connection = NWConnection(to: match.endpoint, using: parameters)
                connection.stateUpdateHandler = { state in
                    names.append("resolve \(state)")
                    if case .ready = state, case let .hostPort(host, port)? = connection.currentPath?.remoteEndpoint {
                        var text = "\(host)"
                        if let percent = text.firstIndex(of: "%") {
                            let scope = if_nametoindex(String(text[text.index(after: percent)...]))
                            let bare = String(text[..<percent])
                            text = bare.contains(":") && scope > 0 ? "\(bare)%\(scope)" : bare
                        }
                        finish(text.contains(":") ? "[\(text)]:\(port)" : "\(text):\(port)")
                    }
                }
                self.connections.append(connection)
                connection.start(queue: self.queue)
            }
        }
        browser.stateUpdateHandler = { state in
            if case let .failed(error) = state { names.append("browser failed: \(error)"); finish(nil) }
            if case let .waiting(error) = state { names.append("browser waiting: \(error)") }
        }
        browser.start(queue: queue)
        queue.asyncAfter(deadline: .now() + seconds) { finish(nil) }
    }
}

@MainActor
final class Probe: ObservableObject {
    @Published var checks: [Check] = []
    @Published var running = false
    @Published var backs: [Int] = []
    @Published var stage = "idle"
    @Published var finished = 0
    @Published var failedBacks = 0
    private(set) var target: Target?
    private var secret: String?
    private var good: String?
    private var stream: UInt64?
    private var leftAt: Date?
    private let lookup = Lookup()

    private func note(_ name: String, _ result: String, _ good: Bool = true) async {
        print("[probe] \(good ? "ok" : "FAILED") \(name): \(result)")
        checks.append(Check(name: name, result: result, good: good))
        await report(["check": name, "result": result, "good": good])
    }

    private func report(_ line: [String: Any]) async {
        guard let stream else { return }
        let text = Rust.text(line)
        await off {
            let parsed = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]
            _ = Rust.echo(stream, parsed)
        }
    }

    private func step(_ text: String) {
        stage = text
        print("[probe] \(Date().timeIntervalSince1970) \(text)")
    }

    private func off<T: Sendable>(_ work: @escaping @Sendable () -> T) async -> T {
        await Task.detached(priority: .userInitiated, operation: work).value
    }

    private func dial(_ name: String, _ key: String, _ addrs: [String]) async -> Bool {
        let opened = await off { Rust.open(key, addrs, ["kind": "probe", "phone": 1]) as NSDictionary }
        let reply = opened as? [String: Any] ?? [:]
        if let error = Rust.failed(reply) {
            await note(name, "failed: \(error)", false)
            return false
        }
        stream = (reply["stream"] as? NSNumber)?.uint64Value
        let path = await off { Rust.path(key) as NSDictionary }
        let ms = Int((reply["ms"] as? Double) ?? 0)
        await note(name, "\(ms) ms · \(Rust.text(path))")
        return true
    }

    func run(_ target: Target) async {
        self.target = target
        running = true
        checks = []
        defer {
            running = false
            finished += 1
        }
        let started = await off { Rust.start(nil) as NSDictionary }
        let reply = started as? [String: Any] ?? [:]
        if let error = Rust.failed(reply) {
            await note("Endpoint", "failed: \(error)", false)
            return
        }
        secret = reply["secret"] as? String
        let bound = Int((reply["ms"] as? Double) ?? 0)
        guard await dial("By address", target.key, target.addrs) else {
            await note("Endpoint", "bound in \(bound) ms, but no dial worked", false)
            return
        }
        good = target.addrs.first
        await note("Endpoint", "bound in \(bound) ms")
        await report(["device": UIDevice.current.model, "system": UIDevice.current.systemVersion])
        await browse("Browse: probe server", target, key: target.key, instance: target.instance)
        if let key = target.daemonKey, let instance = target.daemonInstance {
            await browse("Browse: this Mac's daemon", target, key: key, instance: instance)
        }
        step("done")
    }

    private func restart() async {
        stream = nil
        let saved = secret
        await off { kanna_phone_stop() }
        _ = await off { Rust.start(saved) as NSDictionary }
    }

    private func browse(_ name: String, _ target: Target, key: String, instance: String) async {
        await restart()
        step("browsing \(instance)")
        let found: (String?, [String]) = await withCheckedContinuation { continuation in
            lookup.find(instance, seconds: 30, seen: { _ in }) { address, names in
                continuation.resume(returning: (address, names))
            }
        }
        var result = "not found; saw \(found.1)"
        var good = false
        if let address = found.0 {
            await restart()
            let opened = await off { Rust.open(key, [address], ["kind": "probe", "phone": 1]) as NSDictionary }
            let reply = opened as? [String: Any] ?? [:]
            if let error = Rust.failed(reply) {
                good = error.contains("no stream kind")
                result = "\(address) answered: \(error)"
            } else {
                good = true
                result = "\(address) opened in \(Int((reply["ms"] as? Double) ?? 0)) ms"
            }
        }
        await restart()
        _ = await dial("Reporting", target.key, target.addrs)
        await note(name, result, good)
    }

    func left() {
        guard target != nil else { return }
        leftAt = Date()
        stream = nil
        var task = UIBackgroundTaskIdentifier.invalid
        task = UIApplication.shared.beginBackgroundTask { UIApplication.shared.endBackgroundTask(task) }
        Task.detached {
            kanna_phone_stop()
            await MainActor.run { UIApplication.shared.endBackgroundTask(task) }
        }
    }

    func back() async {
        guard let target, let left = leftAt, !running else { return }
        leftAt = nil
        let away = Int(Date().timeIntervalSince(left))
        let began = Date()
        let secret = self.secret
        let started = await off { Rust.start(secret) as NSDictionary }
        if let error = Rust.failed(started as? [String: Any] ?? [:]) {
            await note("Back", "failed to bind: \(error)", false)
            failedBacks += 1
            return
        }
        let addrs = [good].compactMap { $0 } + target.addrs
        let key = target.key
        let opened = await off { Rust.open(key, addrs, ["kind": "probe", "phone": 1]) as NSDictionary }
        let reply = opened as? [String: Any] ?? [:]
        if let error = Rust.failed(reply) {
            await note("Back", "failed after \(away)s away: \(error)", false)
            failedBacks += 1
            return
        }
        guard let stream = (reply["stream"] as? NSNumber)?.uint64Value else { return }
        let echoed = await off { Rust.echo(stream, ["ping": true]) as NSDictionary }
        let ms = Int(Date().timeIntervalSince(began) * 1000)
        self.stream = stream
        let path = await off { Rust.path(key) as NSDictionary }
        backs.append(ms)
        await note("Back after \(away)s", Rust.failed(echoed as? [String: Any] ?? [:]).map { "failed: \($0)" } ?? "\(ms) ms · \(Rust.text(path))", ms < 1000)
    }
}
