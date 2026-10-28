public enum Origin: Equatable, Sendable {
    case app
    case user
    case agent
}

public enum Placement: Equatable, Sendable {
    case front
    case behindApp
    case behindKanna

    public static func of(_ origin: Origin, active: Bool, background: Bool) -> Placement {
        if active { return .front }
        if origin == .agent { return .behindKanna }
        return background ? .behindApp : .front
    }

    public var recorded: Bool { self != .behindKanna }
}
