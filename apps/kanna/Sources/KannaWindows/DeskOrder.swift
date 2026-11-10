public enum DeskOrder {
    public static func byAge<T, Made: Comparable>(_ tasks: [T], made: (T) -> Made) -> [T] {
        byAge(tasks, made: made, tie: { _ in 0 })
    }

    public static func byAge<T, Made: Comparable, Tie: Comparable>(_ tasks: [T], made: (T) -> Made, tie: (T) -> Tie) -> [T] {
        tasks.enumerated().sorted { a, b in
            let (x, y) = (made(a.element), made(b.element))
            if x != y { return x < y }
            let (p, q) = (tie(a.element), tie(b.element))
            return p != q ? p < q : a.offset < b.offset
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
