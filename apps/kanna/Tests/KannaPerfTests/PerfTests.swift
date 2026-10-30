import Testing
@testable import KannaPerf

struct PerfTests {
    static let ms: UInt64 = 1_000_000

    @Test func statsGiveCountPercentilesAndMax() {
        var book = Book()
        for duration in 1...100 { book.record("lay", start: 0, end: UInt64(duration) * Self.ms, detail: "\(duration) rows") }
        let stats = book.stats()
        #expect(stats.map(\.name) == ["lay"])
        #expect(stats[0].count == 100)
        #expect(stats[0].p50 == 50)
        #expect(stats[0].p95 == 95)
        #expect(stats[0].max == 100)
        #expect(stats[0].last == 100)
        #expect(stats[0].detail == "100 rows")
    }

    @Test func theRingKeepsOnlyTheRecentSamples() {
        var book = Book()
        for duration in 1...(Book.kept + 50) { book.record("check", start: 0, end: UInt64(duration) * Self.ms, detail: nil) }
        let stats = book.stats()[0]
        #expect(stats.count == Book.kept + 50)
        #expect(stats.recent == Book.kept)
        #expect(stats.max == Double(Book.kept + 50))
        #expect(stats.p50 > 50)
    }

    @Test func aHangNamesWhatWasOpenAndWhatRanInIt() {
        var book = Book()
        let click = book.opened("click.shelve", at: 0)
        let message = book.opened("message.layout", at: 10 * Self.ms)
        book.watched = Book.Watched(since: 10 * Self.ms, open: book.openKeys(before: .max))
        book.record("DeskView.lay", start: 20 * Self.ms, end: 200 * Self.ms, detail: "40 rows")
        book.record("markdown.check", start: 1 * Self.ms, end: 2 * Self.ms, detail: nil)
        book.closed(message)
        let hang = book.hung(from: 10 * Self.ms, to: 210 * Self.ms)
        #expect(hang.ms == 200)
        #expect(hang.open == ["click.shelve", "message.layout"])
        #expect(hang.ran == ["DeskView.lay (40 rows) 180ms"])
        #expect(book.hangs == [hang])
        #expect(book.watched == nil)
        book.closed(click)
        #expect(book.openKeys(before: .max).isEmpty)
    }

    @Test func aHangTheWatchMissedTakesWhatWasOpenBeforeIt() {
        var book = Book()
        _ = book.opened("command.debug.hang", at: 5 * Self.ms)
        _ = book.opened("later", at: 50 * Self.ms)
        let hang = book.hung(from: 10 * Self.ms, to: 150 * Self.ms)
        #expect(hang.open == ["command.debug.hang"])
    }
}
