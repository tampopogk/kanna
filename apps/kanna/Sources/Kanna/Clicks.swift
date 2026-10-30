import AppKit
import KannaPerf

@MainActor
enum Clicks {
    private static var shelving: [String: (span: PerfSpan, before: Set<String>)] = [:]
    private static var pulling: [String: PerfSpan] = [:]

    static func shelve(_ task: String, shelf: [String]) -> PerfSpan {
        let span = begin("click.shelve", "click.shelve.wait", task)
        shelving[task] = (span, Set(shelf))
        return span
    }

    static func pull(_ task: String) {
        pulling[task] = begin("click.pull", "click.pull.wait", task)
    }

    private static func begin(_ name: StaticString, _ wait: String, _ task: String) -> PerfSpan {
        let now = Perf.now()
        let event = NSApp.currentEvent.flatMap { [.leftMouseDown, .leftMouseUp].contains($0.type) ? Perf.uptime($0.timestamp) : nil }
        let at = event.flatMap { $0 <= now && now - $0 < 5_000_000_000 ? $0 : nil } ?? now
        Perf.record(wait, start: at, end: now)
        return Perf.begin(.click, name, note: task, at: at)
    }

    static func sent(_ span: PerfSpan) -> @Sendable () -> Void { { Perf.mark(span, "sent") } }

    static func laid(shelf: [String], backlog: [String]) {
        for (task, click) in shelving where !backlog.contains(task) && !Set(shelf).isSubset(of: click.before) {
            shelving[task] = nil
            HangWatch.afterDrawing { Perf.end(click.span, as: "shown") }
        }
    }

    static func pulled(_ task: String) {
        guard let span = pulling.removeValue(forKey: task) else { return }
        HangWatch.afterDrawing { Perf.end(span, as: "shown") }
    }

    static func failed(_ task: String) {
        guard let click = shelving.removeValue(forKey: task) else { return }
        Perf.end(click.span, as: "failed")
    }
}
