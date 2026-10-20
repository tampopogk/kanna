import Foundation
import os
import Synchronization

public enum HangWatch {
    public static let threshold: UInt64 = 100_000_000
    static let stuck: UInt64 = 1_000_000_000
    static let busySince = Atomic<UInt64>(0)
    static let log = Logger(subsystem: Perf.subsystem, category: "hang")
    nonisolated(unsafe) static var drawn: [() -> Void] = []
    nonisolated(unsafe) static var timer: DispatchSourceTimer?

    @MainActor public static func start(report: @escaping @Sendable (PerfHang) -> Void) {
        guard timer == nil else { return }
        let main = CFRunLoopGetMain()
        let awake = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.allActivities.subtracting(.beforeWaiting).rawValue, true, .min) { _, _ in
            boundary(busy: true, report: report)
        }
        let idle = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.beforeWaiting.rawValue, true, .max) { _, _ in
            let waiting = drawn
            drawn = []
            waiting.forEach { $0() }
            boundary(busy: false, report: report)
        }
        CFRunLoopAddObserver(main, awake, .commonModes)
        CFRunLoopAddObserver(main, idle, .commonModes)
        busySince.store(Perf.now(), ordering: .relaxed)
        let timer = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
        timer.schedule(deadline: .now(), repeating: .milliseconds(50), leeway: .milliseconds(25))
        timer.setEventHandler { @Sendable in watch() }
        timer.resume()
        self.timer = timer
    }

    @MainActor public static func afterDrawing(_ action: @escaping () -> Void) { drawn.append(action) }

    static func boundary(busy: Bool, report: @Sendable (PerfHang) -> Void) {
        let now = Perf.now()
        let since = busySince.exchange(busy ? now : 0, ordering: .relaxed)
        guard since != 0, now &- since > threshold else { return }
        let hang = Perf.book.withLock { $0.hung(from: since, to: now) }
        signal(hang)
        report(hang)
    }

    static func watch() {
        let since = busySince.load(ordering: .relaxed)
        guard since != 0 else { return }
        let waited = Perf.now() &- since
        guard waited > threshold else { return }
        let stuckWith = Perf.book.withLock { book -> [String]? in
            if book.watched?.since != since { book.watched = Book.Watched(since: since, open: book.openKeys(before: .max)) }
            guard waited > stuck, !book.watched!.told else { return nil }
            book.watched!.told = true
            return book.watched!.open
        }
        if let stuckWith {
            let open = stuckWith.joined(separator: ", ")
            log.fault("the main thread hasn't answered for \(waited / 1_000_000)ms; open: \(open, privacy: .public)")
            FileHandle.standardError.write(Data("kanna: the main thread hasn't answered for \(waited / 1_000_000)ms; open: \(open)\n".utf8))
        }
    }

    static func signal(_ hang: PerfHang) {
        let open = hang.open.joined(separator: ", ")
        let ran = hang.ran.joined(separator: ", ")
        log.error("the main thread hung \(Int(hang.ms))ms; open: \(open, privacy: .public); ran: \(ran, privacy: .public)")
        Perf.signposters[.hang]!.emitEvent("hang", "\(Int(hang.ms))ms; open: \(open, privacy: .public)")
    }

    public static func line(_ hang: PerfHang) -> String {
        "the main thread hung \(Int(hang.ms))ms; open: \(hang.open.isEmpty ? "nothing" : hang.open.joined(separator: ", "))"
            + (hang.ran.isEmpty ? "" : "; ran: \(hang.ran.joined(separator: ", "))")
    }
}
