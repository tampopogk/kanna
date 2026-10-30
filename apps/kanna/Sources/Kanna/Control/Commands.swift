import Foundation
import KannaPerf

struct KannaError: Error, Equatable {
    let code: String
    let message: String

    init(_ code: String, _ message: String) {
        self.code = code
        self.message = message
    }
}

@MainActor
final class Commands {
    typealias Command = @MainActor (JSON) async throws -> [String: JSON]

    private var commands: [String: Command] = [:]

    subscript(name: String) -> Command? {
        get { commands[name] }
        set { commands[name] = newValue }
    }

    func handle(_ request: JSON) async -> JSON {
        let id = request["id"] ?? .null
        do {
            guard let cmd = request["cmd"]?.string else { throw KannaError("bad_request", "missing cmd") }
            guard let command = commands[cmd] else { throw KannaError("bad_request", "unknown command \"\(cmd)\"") }
            let span = Perf.begin(.commands, "command", cmd)
            defer { Perf.end(span) }
            var reply = try await command(request["args"] ?? [:])
            reply["id"] = id
            reply["ok"] = true
            return .object(reply)
        } catch let error as KannaError {
            return ["id": id, "ok": false, "error": ["code": .string(error.code), "message": .string(error.message)]]
        } catch {
            return ["id": id, "ok": false, "error": ["code": "bad_request", "message": .string("\(error)")]]
        }
    }
}
