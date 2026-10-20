import Foundation
import os
import Synchronization

public enum PerfArea: String, Sendable, CaseIterable {
    case click, daemon, render, shelf, markdown, commands, hang
}

public struct PerfSpan: @unchecked Sendable {
    let serial: Int
    let area: PerfArea
    let name: StaticString
    let id: OSSignpostID
    let state: OSSignpostIntervalState
    public let key: String
    public let start: UInt64
}

public struct PerfStats: Sendable, Equatable {
    public let name: String
    public let count: Int
    public let recent: Int
    public let p50: Double
    public let p95: Double
    public let max: Double
    public let last: Double
    public let detail: String?
}

public struct PerfHang: Sendable, Equatable {
    public let at: Date
    public let ms: Double
    public let open: [String]
    public let ran: [String]
}

public enum Perf {
    public static let subsystem = "dev.kanna.app"
    static let signposters = Dictionary(uniqueKeysWithValues: PerfArea.allCases.map { ($0, OSSignposter(subsystem: subsystem, category: $0.rawValue)) })
    static let book = Mutex(Book())

    public static func now() -> UInt64 { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }

    public static func uptime(_ seconds: TimeInterval) -> UInt64 { UInt64(max(0, seconds) * 1_000_000_000) }

    public static func begin(_ area: PerfArea, _ name: StaticString, _ label: String? = nil, note: String? = nil, at start: UInt64? = nil) -> PerfSpan {
        let signposter = signposters[area]!
        let id = signposter.makeSignpostID()
        let message = [label, note].compactMap { $0 }.joined(separator: " ")
        let state = message.isEmpty ? signposter.beginInterval(name, id: id) : signposter.beginInterval(name, id: id, "\(message, privacy: .public)")
        let key = label.map { "\(name).\($0)" } ?? "\(name)"
        let start = start ?? now()
        let serial = book.withLock { $0.opened(key, at: start) }
        return PerfSpan(serial: serial, area: area, name: name, id: id, state: state, key: key, start: start)
    }

    public static func end(_ span: PerfSpan, _ detail: String? = nil, as event: StaticString? = nil) {
        let signposter = signposters[span.area]!
        let message = [event.map { "\($0)" }, detail].compactMap { $0 }.joined(separator: " ")
        if message.isEmpty { signposter.endInterval(span.name, span.state) } else { signposter.endInterval(span.name, span.state, "\(message, privacy: .public)") }
        let end = now()
        let key = event.map { "\(span.key).\($0)" } ?? span.key
        book.withLock {
            $0.closed(span.serial)
            $0.record(key, start: span.start, end: end, detail: detail)
        }
    }

    public static func mark(_ span: PerfSpan, _ event: StaticString) {
        signposters[span.area]!.emitEvent(event, id: span.id)
        let end = now()
        book.withLock { $0.record("\(span.key).\(event)", start: span.start, end: end, detail: nil) }
    }

    public static func measure<T>(_ area: PerfArea, _ name: StaticString, _ label: String? = nil, _ body: () throws -> T) rethrows -> T {
        let span = begin(area, name, label)
        defer { end(span) }
        return try body()
    }

    public static func record(_ key: String, start: UInt64, end: UInt64, detail: String? = nil) {
        book.withLock { $0.record(key, start: start, end: end, detail: detail) }
    }

    public static var open: [String] { book.withLock { $0.openKeys(before: .max) } }

    public static func stats() -> [PerfStats] { book.withLock { $0.stats() } }

    public static func hangs() -> [PerfHang] { book.withLock { Array($0.hangs) } }

    public static func reset() { book.withLock { $0.reset() } }
}

struct Book {
    static let kept = 200
    static let hangsKept = 20

    struct Sample {
        let start: UInt64
        let end: UInt64
        let detail: String?
        var ms: Double { Double(end &- start) / 1_000_000 }
    }

    struct Ring {
        var samples: [Sample] = []
        var next = 0
        var count = 0

        mutating func add(_ sample: Sample) {
            count += 1
            if samples.count < Book.kept { samples.append(sample) } else { samples[next] = sample }
            next = (next + 1) % Book.kept
        }

        var latest: Sample? { samples.isEmpty ? nil : samples[(next + samples.count - 1) % samples.count] }
    }

    var serial = 0
    var spans: [Int: (key: String, start: UInt64)] = [:]
    var rings: [String: Ring] = [:]
    var hangs: [PerfHang] = []
    struct Watched {
        let since: UInt64
        let open: [String]
        var told = false
    }

    var watched: Watched?

    mutating func opened(_ key: String, at start: UInt64) -> Int {
        serial += 1
        spans[serial] = (key, start)
        return serial
    }

    mutating func closed(_ serial: Int) { spans[serial] = nil }

    mutating func record(_ key: String, start: UInt64, end: UInt64, detail: String?) {
        rings[key, default: Ring()].add(Sample(start: start, end: end, detail: detail))
    }

    func openKeys(before time: UInt64) -> [String] {
        spans.sorted { ($0.value.start, $0.key) < ($1.value.start, $1.key) }.filter { $0.value.start <= time }.map(\.value.key)
    }

    func ran(from start: UInt64, to end: UInt64, besides open: Set<String>) -> [String] {
        var found: [(String, Double)] = []
        for (key, ring) in rings where !open.contains(key) {
            for sample in ring.samples where sample.end >= start && sample.end <= end && sample.start >= start {
                found.append((sample.detail.map { "\(key) (\($0))" } ?? key, sample.ms))
            }
        }
        return found.sorted { $0.1 > $1.1 }.prefix(8).map { String(format: "%@ %.0fms", $0.0, $0.1) }
    }

    mutating func hung(from start: UInt64, to end: UInt64) -> PerfHang {
        let open = watched?.since == start ? watched!.open : openKeys(before: start)
        let ms = Double(end &- start) / 1_000_000
        let hang = PerfHang(at: Date(timeIntervalSinceNow: -ms / 1000), ms: ms, open: open, ran: ran(from: start, to: end, besides: Set(open)))
        hangs.append(hang)
        if hangs.count > Self.hangsKept { hangs.removeFirst(hangs.count - Self.hangsKept) }
        watched = nil
        return hang
    }

    func stats() -> [PerfStats] {
        rings.keys.sorted().compactMap { key in
            guard let ring = rings[key], let latest = ring.latest else { return nil }
            let sorted = ring.samples.map(\.ms).sorted()
            let at = { (q: Double) in sorted[min(sorted.count - 1, Int((Double(sorted.count) * q).rounded(.up)) - 1)] }
            return PerfStats(name: key, count: ring.count, recent: sorted.count, p50: at(0.5), p95: at(0.95), max: sorted.last!, last: latest.ms, detail: latest.detail)
        }
    }

    mutating func reset() {
        rings = [:]
        hangs = []
    }
}
