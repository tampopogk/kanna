public struct RowMotion: Equatable, Sendable {
    public enum Why: String, Sendable {
        case animated
        case unchanged
        case first
        case quiet
        case hidden
        case still = "reduce-motion"
        case burst
    }

    public static let duration = 0.2
    public static let curve: (Float, Float, Float, Float) = (0.2, 0, 0, 1)
    public static let most = 3

    public let why: Why
    public let arrived: [String]
    public let left: [String]
    public let moved: [String]

    public var animated: Bool { why == .animated }
    public var changed: Int { arrived.count + left.count + moved.count }

    public static func plan(from before: [String]?, to after: [String], shown: Bool = true, still: Bool = false, quiet: Bool = false) -> RowMotion {
        let old = Dictionary((before ?? []).enumerated().map { ($1, $0) }, uniquingKeysWith: { first, _ in first })
        let now = Set(after)
        let arrived = after.filter { old[$0] == nil }
        let left = (before ?? []).filter { !now.contains($0) }
        let kept = after.filter { old[$0] != nil }
        let stay = Set(steady(kept.map { old[$0]! }).map { kept[$0] })
        let moved = kept.filter { !stay.contains($0) }
        let why: Why
        if before == nil { why = .first }
        else if arrived.isEmpty && left.isEmpty && moved.isEmpty { why = .unchanged }
        else if quiet { why = .quiet }
        else if !shown { why = .hidden }
        else if still { why = .still }
        else if arrived.count + left.count + moved.count > most { why = .burst }
        else { why = .animated }
        return RowMotion(why: why, arrived: arrived, left: left, moved: moved)
    }

    static func steady(_ places: [Int]) -> [Int] {
        var tails: [Int] = []
        var back = Array(repeating: -1, count: places.count)
        for (index, place) in places.enumerated() {
            var low = 0, high = tails.count
            while low < high {
                let middle = (low + high) / 2
                if places[tails[middle]] < place { low = middle + 1 } else { high = middle }
            }
            if low > 0 { back[index] = tails[low - 1] }
            if low == tails.count { tails.append(index) } else { tails[low] = index }
        }
        var out: [Int] = []
        var at = tails.last ?? -1
        while at >= 0 {
            out.append(at)
            at = back[at]
        }
        return out.reversed()
    }
}
