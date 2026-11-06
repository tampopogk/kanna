import Foundation
import KannaIOS

enum Rust {
    static func take(_ pointer: UnsafeMutablePointer<CChar>?) throws -> Data {
        guard let pointer else { throw Refused(code: "failed", message: "no reply") }
        defer { kanna_phone_free(pointer) }
        let data = Data(String(cString: pointer).utf8)
        if let reply = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any], reply["id"] == nil, let error = reply["error"] as? [String: Any] {
            throw Refused(code: (error["code"] as? String) ?? "failed", message: (error["message"] as? String) ?? "failed")
        }
        return data
    }

    static func object(_ data: Data) -> [String: Any] {
        ((try? JSONSerialization.jsonObject(with: data)) as? [String: Any]) ?? [:]
    }

    static func text(_ value: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: value) else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }

    static func start(secret: String) throws -> [String: Any] {
        try object(secret.withCString { try take(kanna_phone_start($0)) })
    }

    static func join(_ offer: String) throws -> [String: Any] {
        try object(offer.withCString { try take(kanna_phone_join($0)) })
    }

    static func open(_ request: String) throws -> [String: Any] {
        try object(request.withCString { try take(kanna_phone_open($0)) })
    }

    static func write(_ stream: UInt64, _ line: String) throws {
        _ = try line.withCString { try take(kanna_phone_write(stream, $0)) }
    }

    static func highlight(file: String, text: String) -> [TextSpan] {
        guard let data = try? file.withCString({ file in try text.withCString { try take(kanna_phone_highlight(file, $0)) } }) else { return [] }
        return (object(data)["spans"] as? [[String: Any]] ?? []).compactMap {
            guard let start = $0["start"] as? Int, let end = $0["end"] as? Int, let capture = $0["capture"] as? String else { return nil }
            return TextSpan(start: start, end: end, capture: capture)
        }
    }

    static func read(_ stream: UInt64) throws -> Data {
        try take(kanna_phone_read(stream))
    }

    static func path(_ key: String) -> [String: Any] {
        ((try? key.withCString { try take(kanna_phone_path($0)) }).map(object) ?? [:])["path"] as? [String: Any] ?? [:]
    }

    static func instance(_ key: String) -> String? {
        (try? key.withCString { try take(kanna_phone_instance($0)) }).map(object)?["instance"] as? String
    }

    static func key(secret: String) -> String? {
        (try? secret.withCString { try take(kanna_phone_key($0)) }).map(object)?["key"] as? String
    }

    static func stop() {
        kanna_phone_stop()
    }
}
