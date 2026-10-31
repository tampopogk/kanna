public enum Origin: Equatable, Sendable {
    case app
    case user
    case agent
}

public enum Placement: Equatable, Sendable {
    case front
    case behindApp
    case behindKanna
    case waiting

    public static func of(_ origin: Origin, active: Bool, background: Bool) -> Placement {
        if active { return .front }
        if origin == .agent { return background ? .behindKanna : .waiting }
        return background ? .behindApp : .front
    }

    public var recorded: Bool { self == .front || self == .behindApp }
}

public struct WindowOrder: Equatable, Sendable {
    public private(set) var waiting: [String] = []

    public init() {}

    public mutating func place(_ window: String, _ origin: Origin, active: Bool, background: Bool) -> Placement {
        let placement = Placement.of(origin, active: active, background: background)
        waiting.removeAll { $0 == window }
        if placement == .waiting { waiting.append(window) }
        return placement
    }

    public mutating func forget(_ window: String) {
        waiting.removeAll { $0 == window }
    }

    public mutating func activated(front: [String]) -> [String] {
        let rank = { (id: String) in front.firstIndex(of: id) ?? front.count }
        let released = waiting.enumerated().sorted { (rank($0.element), $0.offset) < (rank($1.element), $1.offset) }.map(\.element)
        waiting = []
        return released
    }
}
