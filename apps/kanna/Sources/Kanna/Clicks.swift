import AppKit
import KannaPerf

@MainActor
enum Clicks {
    private static var picking: [String: (span: PerfSpan, before: Set<String>)] = [:]
    private static var activating: [String: PerfSpan] = [:]

    static func pick(_ task: String, desk: [String]) -> PerfSpan {
        let span = begin("click.pick", "click.pick.wait", task)
        picking[task] = (span, Set(desk))
        return span
    }

    static func activate(_ task: String) {
        activating[task] = begin("click.activate", "click.activate.wait", task)
    }

    private static func begin(_ name: StaticString, _ wait: String, _ task: String) -> PerfSpan {
        let now = Perf.now()
        let event = NSApp.currentEvent.flatMap { [.leftMouseDown, .leftMouseUp].contains($0.type) ? Perf.uptime($0.timestamp) : nil }
        let at = event.flatMap { $0 <= now && now - $0 < 5_000_000_000 ? $0 : nil } ?? now
        Perf.record(wait, start: at, end: now)
        return Perf.begin(.click, name, note: task, at: at)
    }

    static func sent(_ span: PerfSpan) -> @Sendable () -> Void { { Perf.mark(span, "sent") } }

    static func laid(desk: [String], shelf: [String]) {
        for (task, click) in picking where !shelf.contains(task) && !Set(desk).isSubset(of: click.before) {
            picking[task] = nil
            HangWatch.afterDrawing { Perf.end(click.span, as: "shown") }
        }
    }

    static func activated(_ task: String) {
        guard let span = activating.removeValue(forKey: task) else { return }
        HangWatch.afterDrawing { Perf.end(span, as: "shown") }
    }

    static func failed(_ task: String) {
        guard let click = picking.removeValue(forKey: task) else { return }
        Perf.end(click.span, as: "failed")
    }
}
