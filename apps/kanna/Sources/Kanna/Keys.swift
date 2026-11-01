import AppKit
import KannaPerf

@MainActor
enum Keys {
    private static var typing: [String: PerfSpan] = [:]

    static func pressed(_ event: NSEvent) -> UInt64 {
        let now = Perf.now()
        let stamp = Perf.uptime(event.timestamp)
        let at = stamp <= now && now - stamp < 5_000_000_000 ? stamp : now
        Perf.record("key.wait", start: at, end: now)
        return at
    }

    static func typed(_ terminal: String, at: UInt64) -> (@Sendable () -> Void)? {
        if let old = typing[terminal] {
            guard Perf.now() &- old.start > 1_000_000_000 else { return nil }
            Perf.end(old, as: "lost")
        }
        let span = Perf.begin(.keys, "key", note: terminal, at: at)
        typing[terminal] = span
        return { Perf.mark(span, "sent") }
    }

    static func echoing(_ terminal: String) -> PerfSpan? {
        guard let span = typing.removeValue(forKey: terminal) else { return nil }
        Perf.mark(span, "echo")
        return span
    }

    static func echoed(_ span: PerfSpan) {
        HangWatch.afterDrawing { Perf.end(span, as: "drawn") }
    }

    static func forget(_ terminal: String) {
        typing.removeValue(forKey: terminal).map { Perf.end($0, as: "lost") }
    }
}
