import Testing
@testable import KannaWindows

struct StatsFormatTests {
    @Test func aRunSitsAcrossItsWindow() {
        #expect(StatsFormat.across(50, from: 0, to: 100) == 0.5)
        #expect(StatsFormat.across(-5, from: 0, to: 100) == 0)
        #expect(StatsFormat.across(500, from: 0, to: 100) == 1)
        #expect(StatsFormat.across(5, from: 5, to: 5) == 1)
        #expect(StatsFormat.windowStart(from: 0, to: 30 * 86400) == "30 days ago")
        #expect(StatsFormat.windowStart(from: 0, to: 3600) == "today")
    }

    @Test func numbersReadAsTheTileSaysTheyAre() {
        #expect(StatsFormat.figure(79.3, unit: "%") == "79%")
        #expect(StatsFormat.figure(12780, unit: "s") == "3h 33m")
        #expect(StatsFormat.figure(28, unit: nil) == "28")
        #expect(StatsFormat.figure(nil, unit: "%") == "—")
        #expect(StatsFormat.duration(59 * 60) == "59m")
        #expect(StatsFormat.duration(3 * 3600) == "3h")
        #expect(StatsFormat.duration(50 * 3600) == "2d")
    }

    @Test func daysReadAsMonthAndDateWithTodayNamed() {
        #expect(StatsFormat.day("2026-10-02", isToday: false) == "Oct 2")
        #expect(StatsFormat.day("2026-10-27", isToday: false, long: true) == "Tue, Oct 27")
        #expect(StatsFormat.day("2026-10-31", isToday: true) == "today")
        #expect(StatsFormat.day("2026-10-31", isToday: true, long: true) == "Today")
    }

    @Test func scalesEndOnARoundNumber() {
        #expect(StatsFormat.nice(0) == 1)
        #expect(StatsFormat.nice(4.2) == 5)
        #expect(StatsFormat.nice(17) == 20)
        #expect(StatsFormat.nice(38) == 50)
        #expect(StatsFormat.nice(120) == 200)
        #expect(StatsFormat.niceDuration(40 * 60) == 50 * 60)
        #expect(StatsFormat.niceDuration(17 * 3600) == 20 * 3600)
        #expect(StatsFormat.niceDuration(3 * 86400) == 3 * 86400)
        #expect(StatsFormat.niceDuration(7 * 86400) == 10 * 86400)
        #expect(StatsFormat.percentile([5, 1, 3, 2, 4, 9, 7, 8, 6, 100], 0.9) == 100)
        #expect(StatsFormat.percentile([], 0.9) == nil)
    }

    @Test func tilesFillRowsOfSixthsAndStackWhenNarrow() {
        let wide = StatsFormat.places([3, 3, 2, 2, 2, 4], width: 638, gap: 22)
        #expect(wide.map(\.row) == [0, 0, 1, 1, 1, 2])
        #expect(wide[1].x == 330)
        #expect(wide[1].width == 308)
        #expect(wide[5].width == 418)
        let narrow = StatsFormat.places([2, 2, 3], width: 400, gap: 22)
        #expect(narrow.map(\.row) == [0, 1, 2])
        #expect(narrow.allSatisfy { $0.x == 0 && $0.width == 400 })
    }
}
