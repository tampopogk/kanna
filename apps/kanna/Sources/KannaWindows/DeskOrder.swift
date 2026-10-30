public enum DeskOrder {
    public static func byRecency<T>(_ recent: [T], needsYou: (T) -> Bool) -> [T] {
        recent.filter(needsYou) + recent.filter { !needsYou($0) }
    }

    public static func byAge<T, Made: Comparable>(_ tasks: [T], made: (T) -> Made?) -> [T] {
        tasks.enumerated().sorted { a, b in
            switch (made(a.element), made(b.element)) {
            case let (x?, y?) where x != y: return x < y
            case (_?, nil): return true
            case (nil, _?): return false
            default: return a.offset < b.offset
            }
        }.map(\.element)
    }

    public static func keys(_ keyed: [String]) -> [String: String] {
        Dictionary(uniqueKeysWithValues: keyed.prefix(2).enumerated().map { ($0.element, "⌘\($0.offset + 1)") })
    }
}
