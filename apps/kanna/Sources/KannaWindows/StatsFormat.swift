import Foundation

public enum StatsFormat {
    public static func duration(_ seconds: Double) -> String {
        let minutes = Int((seconds / 60).rounded())
        if minutes < 60 { return "\(minutes)m" }
        if minutes < 48 * 60 { return minutes % 60 == 0 ? "\(minutes / 60)h" : "\(minutes / 60)h \(minutes % 60)m" }
        return "\(Int((Double(minutes) / 1440).rounded()))d"
    }

    public static func hours(_ hours: Double) -> String {
        if hours < 1 { return "\(Int((hours * 60).rounded()))m" }
        if hours < 10 {
            let tenths = (hours * 10).rounded() / 10
            return tenths == tenths.rounded() ? "\(Int(tenths))h" : "\(tenths)h"
        }
        return "\(Int(hours.rounded()))h"
    }

    public static func figure(_ value: Double?, unit: String?) -> String {
        guard let value else { return "—" }
        switch unit {
        case "%": return "\(Int(value.rounded()))%"
        case "s": return duration(value)
        case "h": return hours(value)
        case "x": return String(format: "%.1f", value)
        default: return "\(Int(value.rounded()))"
        }
    }

    private static let parser: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter
    }()

    private static func printer(_ format: String) -> DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US")
        formatter.dateFormat = format
        return formatter
    }

    private static let short = printer("MMM d")
    private static let long = printer("EEE, MMM d")

    public static func day(_ iso: String, isToday: Bool, long full: Bool = false) -> String {
        if isToday { return full ? "Today" : "today" }
        guard let date = parser.date(from: iso) else { return iso }
        return (full ? long : short).string(from: date)
    }

    public static func across(_ at: Double, from: Double, to: Double) -> Double {
        to > from ? min(max((at - from) / (to - from), 0), 1) : 1
    }

    public static func windowStart(from: Double, to: Double) -> String {
        let days = Int(((to - from) / 86400).rounded(.up))
        return days <= 1 ? "today" : "\(days) days ago"
    }

    public static func nice(_ value: Double) -> Double {
        if value <= 5 { return max(1, value.rounded(.up)) }
        let power = pow(10, (log10(value)).rounded(.down))
        return [1, 2, 2.5, 5, 10].map { $0 * power }.first { $0 >= value } ?? 10 * power
    }

    public static func niceDuration(_ seconds: Double) -> Double {
        if seconds <= 3600 { return nice(max(seconds, 60) / 60) * 60 }
        if seconds <= 48 * 3600 { return nice(seconds / 3600) * 3600 }
        return nice(seconds / 86400) * 86400
    }

    public static func percentile(_ values: [Double], _ share: Double) -> Double? {
        guard !values.isEmpty else { return nil }
        let sorted = values.sorted()
        return sorted[min(sorted.count - 1, Int(Double(sorted.count) * share))]
    }

    public struct Place: Equatable {
        public let x: Double
        public let row: Int
        public let width: Double
    }

    public static func places(_ spans: [Int], width: Double, gap: Double, narrow: Double = 520) -> [Place] {
        let columns = 6
        var row = 0, used = 0
        return spans.map { given in
            let span = width < narrow ? columns : min(columns, max(1, given))
            if used + span > columns {
                row += 1
                used = 0
            }
            let unit = (width + gap) / Double(columns)
            let place = Place(x: Double(used) * unit, row: row, width: Double(span) * unit - gap)
            used += span
            return place
        }
    }
}
