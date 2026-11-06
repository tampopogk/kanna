import Foundation

enum LinkPath { case lan, net, relay, off }

enum Planner: Equatable {
    case idle, asks, stopped, starting
    case working(Date)
    case elsewhere(Date)
    case blocked

    init(status: String, since: Double) {
        let date = since > 0 ? Date(timeIntervalSince1970: since) : Date()
        switch status {
        case "working": self = .working(date)
        case "elsewhere": self = .elsewhere(date)
        case "asks": self = .asks
        case "blocked": self = .blocked
        case "starting": self = .starting
        case "idle": self = .idle
        default: self = .stopped
        }
    }
}

enum Receipt: Equatable {
    case none, sent, read
    case waitingTurn, waitingPlanner, waitingStart, dayOld
    case waitingMachine(String)
}

struct CodeLine: Identifiable {
    let id = UUID()
    let n: Int
    let text: String
    var hot = false
    var spans: [TextSpan] = []
    init(_ n: Int, _ text: String, hot: Bool = false, spans: [TextSpan] = []) { self.n = n; self.text = text; self.hot = hot; self.spans = spans }
}

struct TextSpan: Equatable {
    let start: Int
    let end: Int
    let capture: String
}

struct Snippet: Identifiable {
    let id: String
    let file: String
    let lines: [CodeLine]
    let commit: String
    var first: Int { lines.first?.n ?? 0 }
    var base: String { file.components(separatedBy: "/").last ?? file }
    var folder: String { String(file.dropLast(base.count)) }
}

enum Block: Identifiable {
    case p(String)
    case list([String])
    case code(Snippet)
    var id: String {
        switch self {
        case .p(let t): "p" + t
        case .list(let l): "l" + l.joined()
        case .code(let c): c.id
        }
    }
}

struct AskOption: Identifiable {
    var id: String { label }
    let label: String
    let why: String
}

enum ChatKind {
    case when(String)
    case me(String, Receipt)
    case planner([Block])
    case ask([AskOption])
    case answered(String)
    case sys(String)
}

struct ChatItem: Identifiable {
    let id: String
    var kind: ChatKind
    var seq = 0
}

enum Status {
    case working, needs, idle, ready, stale, stopped, waiting, blocked

    init(wire: String) {
        switch wire {
        case "working": self = .working
        case "needs": self = .needs
        case "idle": self = .idle
        case "ready": self = .ready
        case "stale": self = .stale
        case "waiting": self = .waiting
        case "blocked": self = .blocked
        default: self = .stopped
        }
    }
}

struct Message: Identifiable {
    let id: String
    let from: String
    let time: String
    let text: String
    var you = false
    var receipt: Receipt = .none
    var at = 0.0
    var said = false
}

struct WorkItem: Identifiable {
    let id: String
    var name: String
    var status: Status
    var desk: String?
    var machine: String?
    var agent = "claude"
    var age = ""
    var after = ""
    var branch: String?
    var firstMessage: String?
    var thread: [Message] = []
    var unreachable = false
    var picking = false
}

enum AppTab { case chat, shelf }

enum PairStep { case start, scan, refused, confirm, done }

struct Repo: Identifiable {
    let id: String
    var name: String
    var plannerMachine: String
    var planner: Planner
    var lastLine = ""
    var chat: [ChatItem] = []
    var tasks: [WorkItem] = []
    var desks: [Desk] = []
    var done: [String] = []
    var down = false
    var lastSeen = ""
}

struct FullCode: Identifiable {
    let id = UUID()
    let snippet: Snippet
}
