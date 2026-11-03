import Foundation

public struct LaneLine: Sendable, Equatable {
    public var lane: Int
    public var color: Int
}

public struct LaneEdge: Sendable, Equatable {
    public var lane: Int
    public var color: Int
    public var hook = false
    public var stub = false
}

public struct LaneRow: Sendable, Equatable {
    public var row: Int
    public var hash: String
    public var lane: Int
    public var color: Int
    public var incoming: [LaneLine]
    public var through: [LaneLine]
    public var outgoing: [LaneEdge]
    public var folded: Bool
    public var open: Bool
    public var labels: [LogTask]
}

public struct LogGraph: Sendable, Equatable {
    public static let hookOver = 6
    public var rows: [LaneRow]
    public var lanes: Int

    public static func make(_ result: LogResult, open: Set<String>) -> LogGraph {
        let shown = result.rows.indices.filter { index in result.rows[index].side.map { open.contains($0) } ?? true }
        let byHash = Dictionary(result.rows.enumerated().map { ($1.hash, $0) }, uniquingKeysWith: { first, _ in first })
        var labels: [String: [LogTask]] = [:]
        for task in result.tasks {
            guard let at = byHash[task.tip] else { continue }
            let row = result.rows[at]
            let where_ = row.side.flatMap { open.contains($0) ? nil : $0 } ?? row.hash
            labels[where_, default: []].append(task)
        }
        return layout(result.rows, shown: shown, open: open, labels: labels)
    }

    static func layout(_ all: [LogRow], shown: [Int], open: Set<String>, labels: [String: [LogTask]]) -> LogGraph {
        let place = Dictionary(shown.enumerated().map { (all[$1].hash, $0) }, uniquingKeysWith: { first, _ in first })
        let spine = Set(shown.filter { all[$0].spine }.map { all[$0].hash })
        var lanes: [String?] = []
        var colors: [Int] = []
        var next = 1
        var widest = 0
        var out: [LaneRow] = []
        out.reserveCapacity(shown.count)
        func free() -> Int {
            var lane = 1
            while lane < lanes.count, lanes[lane] != nil { lane += 1 }
            return lane
        }
        func set(_ lane: Int, _ hash: String?) {
            while lanes.count <= lane { lanes.append(nil) }
            while colors.count <= lane { colors.append(0) }
            lanes[lane] = hash
        }
        for (position, index) in shown.enumerated() {
            let row = all[index]
            let waiting = lanes.indices.filter { lanes[$0] == row.hash }
            let lane = row.spine ? 0 : waiting.min() ?? free()
            set(lane, lanes[safe: lane] ?? nil)
            if row.spine {
                colors[0] = 0
            } else if waiting.isEmpty {
                colors[lane] = next
                next += 1
            }
            let incoming = waiting.map { LaneLine(lane: $0, color: colors[$0]) }
            let through = lanes.indices.compactMap { k -> LaneLine? in
                guard let hash = lanes[k], hash != row.hash else { return nil }
                return LaneLine(lane: k, color: colors[k])
            }
            for k in waiting { lanes[k] = nil }
            let folded = row.fold > 0 && !open.contains(row.hash)
            let parents = (folded ? Array(row.parents.prefix(1)) : row.parents).filter { place[$0] != nil }
            var outgoing: [LaneEdge] = []
            for (j, parent) in parents.enumerated() {
                if let existing = lanes.firstIndex(where: { $0 == parent }) {
                    outgoing.append(LaneEdge(lane: existing, color: colors[existing]))
                    continue
                }
                if j > 0, !row.spine, spine.contains(parent), let at = place[parent], at - position > hookOver {
                    outgoing.append(LaneEdge(lane: 0, color: 0, hook: true))
                    continue
                }
                let target: Int
                if row.spine && spine.contains(parent) {
                    target = 0
                } else if j == 0 && (lanes[safe: lane] ?? nil) == nil {
                    target = lane
                } else {
                    target = free()
                    set(target, nil)
                    colors[target] = next
                    next += 1
                }
                set(target, parent)
                outgoing.append(LaneEdge(lane: target, color: colors[target]))
            }
            if parents.isEmpty, !row.parents.isEmpty, !folded {
                outgoing.append(LaneEdge(lane: lane, color: colors[lane], stub: true))
            }
            widest = max(widest, lane, through.map(\.lane).max() ?? 0, outgoing.map(\.lane).max() ?? 0)
            out.append(LaneRow(row: index, hash: row.hash, lane: lane, color: colors[lane], incoming: incoming, through: through, outgoing: outgoing, folded: folded,
                               open: row.fold > 0 && open.contains(row.hash), labels: labels[row.hash] ?? []))
        }
        return LogGraph(rows: out, lanes: out.isEmpty ? 0 : widest + 1)
    }
}

extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}
