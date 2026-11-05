import Foundation

enum AppTab: Hashable { case chat, shelf, quiz, stats, machines }

enum Net: String, CaseIterable, Identifiable {
    case lan, internet, relay, machineDown, noNetwork
    var id: String { rawValue }
    var label: String {
        switch self {
        case .lan: "Direct · LAN"
        case .internet: "Direct · internet"
        case .relay: "Relayed"
        case .machineDown: "Offline: machine asleep"
        case .noNetwork: "Offline: no network"
        }
    }
}

enum LinkPath { case lan, net, relay, off }

enum Planner: Equatable {
    case idle, asks, stopped, starting
    case working(Date)
    case elsewhere(Date)
}

enum Receipt: Equatable {
    case none, sent, read
    case waitingTurn, waitingPlanner, waitingStart
    case waitingMachine(String)
    case dayOld
}

struct CodeLine: Identifiable {
    let id = UUID()
    let n: Int
    let text: String
    var hot = false
    init(_ n: Int, _ text: String, hot: Bool = false) { self.n = n; self.text = text; self.hot = hot }
}

struct Snippet: Identifiable {
    let id = UUID()
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
        case .code(let c): c.id.uuidString
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
    case round(String, Int?)
}

struct ChatItem: Identifiable {
    let id = UUID()
    var kind: ChatKind
}

enum Status {
    case working, needs, idle, ready, stale, stopped, waiting, blocked
}

struct Message: Identifiable {
    let id = UUID()
    let from: String
    let time: String
    let text: String
    var you = false
    var receipt: Receipt = .none
}

struct WorkItem: Identifiable {
    let id = UUID()
    var name: String
    var status: Status
    var desk: String?
    var machine: String?
    var agent = "claude"
    var age = ""
    var after = ""
    var branch: String?
    var firstMessage: String?
    var lastWords: [String] = []
    var lastAt = ""
    var thread: [Message] = []
    var picking = false
}

struct Machine: Identifiable {
    var id: String { name }
    let name: String
    let about: String
    let linux: Bool
    let ms: Int
    let lastSeen: String?
    let since: String
    var alwaysRelayed = false
}

struct Desk: Identifiable {
    var id: String { machine }
    let machine: String
    var open: Bool
}

struct Subsystem: Identifiable {
    var id: String { name }
    let name: String
    let span: (Int, Int)
    var pct: Int?
    var moved = 0
    var paths = ""
    var was = ""
    var notes = ""
    var movedRows: [(String, String)] = []
    var misses: [(String, String)] = []
    var level: Int {
        guard let pct else { return 0 }
        if pct >= 80 { return 4 }
        if pct >= 60 { return 3 }
        if pct >= 40 { return 2 }
        return 1
    }
}

struct Choice {
    let title: String
    var sub: String = ""
}

struct PickLine {
    let n: Int
    let text: String
}

enum QuestionBody {
    case choices([Choice])
    case lines([PickLine])
    case drawings
}

struct Question {
    let kind: String
    let source: String
    let text: String
    let body: QuestionBody
    let correct: Int
    let why: String
    var code: Snippet?
    let endLabel: String
    let endFile: String
    let subsystem: String
    var small = false
}

struct RoundDef: Identifiable {
    let id: String
    let title: String
    let about: String
    var questions: [Question]
}

struct RoundPlay {
    var def: RoundDef
    var index = 0
    var answers: [Int] = []
    var revealed = false
    var earned = 0
    var run = 0
    var finished = false
    var moves: [(String, Int, Int)] = []
    var startScore = 0
    var cardItem: UUID?
    var current: Question { def.questions[index] }
    var count: Int { def.questions.count }
    var right: Int { zip(answers, def.questions).filter { $0 == $1.correct }.count }
}

enum ScoreMode: String, CaseIterable, Identifiable {
    case streak, points, both, neither
    var id: String { rawValue }
    var label: String { rawValue.capitalized }
    var showsStreak: Bool { self == .streak || self == .both }
    var showsPoints: Bool { self == .points || self == .both }
}

enum NextRound: Equatable { case ready, writing }

enum PairStep { case start, scan, refused, confirm, done }

enum Appearance: String, CaseIterable, Identifiable {
    case system, light, dark
    var id: String { rawValue }
    var label: String { rawValue.capitalized }
}

struct Repo: Identifiable {
    let id: String
    var plannerMachine: String
    var planner: Planner
    var lastLine: String
    var chat: [ChatItem]
    var tasks: [WorkItem]
    var done: [String]
    var desks: [Desk]
    var machines: [String]
    var without: [String]
    var map: [Subsystem]
    var seed: Double
    var down = false
    var lastSeen = "9:12"
    var streak = 6
    var points = 1240
    var playedToday = false
    var nextRound: NextRound = .ready
    var nextIndex = 0
    var extraRound: RoundDef?
    var quizMeWriting: String?
    var missedOnce = false
}
