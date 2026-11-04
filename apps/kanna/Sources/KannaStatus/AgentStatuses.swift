public struct AgentStatuses: Equatable, Sendable {
    public private(set) var statuses: [String: String] = [:]
    public private(set) var unread: Set<String> = []

    public init() {}

    public mutating func set(_ pane: String, _ status: String?, looking: Bool) {
        let before = statuses[pane]
        statuses[pane] = status
        if status != "idle" {
            unread.remove(pane)
        } else if before == "working" && !looking {
            unread.insert(pane)
        }
    }

    public mutating func looked(at panes: Set<String>) {
        unread.subtract(panes)
    }

    public mutating func keep(_ panes: Set<String>) {
        statuses = statuses.filter { panes.contains($0.key) }
        unread.formIntersection(panes)
    }
}
