import Foundation

public struct UpdateOffer: Equatable, Codable, Sendable {
    public let version: String
    public let build: String
    public let speaks: Int?

    public init(version: String, build: String, speaks: Int?) {
        self.version = version
        self.build = build
        self.speaks = speaks
    }
}

public enum UpdatePhase: Equatable, Sendable {
    case idle
    case asking(UpdateOffer)
    case waiting(UpdateOffer)
    case downloading(UpdateOffer)
    case installing(UpdateOffer)
}

public enum UpdateAnswer: Equatable, Sendable {
    case ask
    case dismiss
    case install
}

public struct UpdateState: Sendable {
    public private(set) var phase: UpdatePhase
    private var asked: Set<String> = []
    private var installRequested = false

    public init(pending: UpdateOffer? = nil) {
        phase = pending.map { .waiting($0) } ?? .idle
    }

    public var offer: UpdateOffer? {
        switch phase {
        case .idle: return nil
        case .asking(let offer), .waiting(let offer), .downloading(let offer), .installing(let offer): return offer
        }
    }

    public var pending: UpdateOffer? {
        if case .idle = phase { return nil }
        return offer
    }

    public var menuTitle: String {
        switch phase {
        case .idle: return "Check for Updates…"
        case .asking(let offer), .waiting(let offer): return "Install Update \(offer.version)…"
        case .downloading(let offer): return "Downloading \(offer.version)…"
        case .installing(let offer): return "Installing \(offer.version)…"
        }
    }

    public var menuEnabled: Bool {
        switch phase {
        case .idle, .asking, .waiting: return true
        case .downloading, .installing: return false
        }
    }

    public var wantsInstallFromMenu: Bool {
        if case .waiting = phase { return true }
        return false
    }

    public mutating func requestInstall() {
        installRequested = true
    }

    public mutating func found(_ offer: UpdateOffer, byHand: Bool) -> UpdateAnswer {
        if installRequested {
            installRequested = false
            phase = .downloading(offer)
            return .install
        }
        if !byHand, asked.contains(offer.build) {
            phase = .waiting(offer)
            return .dismiss
        }
        asked.insert(offer.build)
        phase = .asking(offer)
        return .ask
    }

    public mutating func notFound() {
        installRequested = false
        phase = .idle
    }

    public mutating func accepted() {
        if case .asking(let offer) = phase { phase = .downloading(offer) }
    }

    public mutating func later() {
        if case .asking(let offer) = phase { phase = .waiting(offer) }
    }

    public mutating func stopped() {
        installRequested = false
        if case .downloading(let offer) = phase { phase = .waiting(offer) }
        if case .installing(let offer) = phase { phase = .waiting(offer) }
    }

    public mutating func installing() {
        if case .downloading(let offer) = phase { phase = .installing(offer) }
    }
}

public enum UpdateText {
    public static func available(_ version: String, app: String) -> String { "\(app) \(version) is available" }
    public static func downloading(_ version: String, app: String) -> String { "Downloading \(app) \(version)" }
    public static func installing(_ version: String, app: String) -> String { "Installing \(app) \(version)" }
    public static func updated(_ version: String, app: String) -> String { "Updated to \(app) \(version)" }
    public static func failed(_ version: String?, app: String) -> String { version.map { "\(app) \($0) didn't install" } ?? "Couldn't check for updates" }
    public static let keepsRunning = "Relaunching keeps your terminals and running agents."
    public static let carriedOn = "Every terminal and agent carried on."

    public static func size(received: UInt64, total: UInt64) -> String {
        let megabytes = { (bytes: UInt64) in Int((Double(bytes) / 1_000_000).rounded()) }
        return total > 0 ? "\(megabytes(received)) of \(megabytes(total)) MB" : "\(megabytes(received)) MB"
    }

    public static func fallsBehind(_ machines: [String]) -> String? {
        switch machines.count {
        case 0: return nil
        case 1: return "\(machines[0]) will show Incompatible until you update it."
        case 2: return "\(machines[0]) and \(machines[1]) will show Incompatible until you update them."
        default: return "\(machines.count) machines will show Incompatible until you update them."
        }
    }

    public static func machinesBehind(speaks: Int?, mine: Int, machines: [String: Int?]) -> [String] {
        guard let speaks, speaks != mine else { return [] }
        return machines.filter { $0.value != speaks }.keys.sorted()
    }

    public static func notes(_ version: String) -> URL {
        URL(string: "https://github.com/tampopogk/kanna/releases/tag/v\(version)")!
    }
}
