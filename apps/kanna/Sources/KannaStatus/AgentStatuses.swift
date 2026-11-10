public struct AgentStatuses: Equatable, Sendable {
    public private(set) var statuses: [String: String] = [:]
    public private(set) var unread: Set<String> = []
    private var seen: Set<String> = []

    public init() {}

    @discardableResult
    public mutating func set(_ pane: String, _ status: String?, looking: Bool) -> Bool {
        let before = statuses[pane]
        statuses[pane] = status
        if status != "idle" {
            unread.remove(pane)
            seen.remove(pane)
            return false
        }
        if before == "working" {
            seen.remove(pane)
            if !looking { unread.insert(pane) }
        }
        return looking && seen.insert(pane).inserted
    }

    @discardableResult
    public mutating func looked(at panes: Set<String>) -> Set<String> {
        unread.subtract(panes)
        let first = panes.filter { statuses[$0] == "idle" }.subtracting(seen)
        seen.formUnion(first)
        return first
    }

    public mutating func keep(_ panes: Set<String>) {
        statuses = statuses.filter { panes.contains($0.key) }
        unread.formIntersection(panes)
        seen.formIntersection(panes)
    }
}
