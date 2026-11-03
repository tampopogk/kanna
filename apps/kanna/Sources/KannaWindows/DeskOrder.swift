public enum DeskOrder {
    public static func byAge<T, Made: Comparable>(_ tasks: [T], made: (T) -> Made) -> [T] {
        tasks.enumerated().sorted { a, b in
            let (x, y) = (made(a.element), made(b.element))
            return x != y ? x < y : a.offset < b.offset
        }.map(\.element)
    }

    public static func task<T>(_ keyed: [T], key: Int) -> T? {
        switch key {
        case 1: keyed.first
        case 2: keyed.last
        default: nil
        }
    }

    public static func strip<T>(_ keyed: [T]) -> [T] {
        guard keyed.count > 2, let first = keyed.first, let last = keyed.last else { return keyed }
        return [first, last] + keyed.dropFirst().dropLast()
    }

    public static func keys(_ keyed: [String]) -> [String: String] {
        guard let first = keyed.first, let last = keyed.last else { return [:] }
        return first == last ? [first: "⌘1"] : [first: "⌘1", last: "⌘2"]
    }
}
