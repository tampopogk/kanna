import Foundation

public struct RunNode: Equatable, Sendable {
    public var id: String
    public var kind: String
    public var who: String
    public var col: Int
    public var row: Int
    public var state: String?
    public var visits: Int

    public init(id: String, kind: String, who: String, col: Int, row: Int, state: String? = nil, visits: Int = 0) {
        self.id = id
        self.kind = kind
        self.who = who
        self.col = col
        self.row = row
        self.state = state
        self.visits = visits
    }
}

public struct RunEdge: Equatable, Sendable {
    public var from: String
    public var to: String
    public var route: String
    public var label: String
    public var fail: Bool
    public var lane: Int
    public var lanes: Int
    public var taken: Bool

    public init(from: String, to: String, route: String, label: String = "", fail: Bool = false, lane: Int = 0, lanes: Int = 1, taken: Bool = false) {
        self.from = from
        self.to = to
        self.route = route
        self.label = label
        self.fail = fail
        self.lane = lane
        self.lanes = lanes
        self.taken = taken
    }
}

public struct RunVisit: Equatable, Sendable {
    public var n: Int
    public var stage: String
    public var visit: Int
    public var kind: String
    public var who: String
    public var startedAt: Double
    public var endedAt: Double?
    public var verdict: String?
    public var turns: Double?
    public var tokens: Double?
    public var note: String

    public init(n: Int, stage: String, visit: Int, kind: String, who: String, startedAt: Double, endedAt: Double? = nil, verdict: String? = nil, turns: Double? = nil, tokens: Double? = nil, note: String = "") {
        self.n = n
        self.stage = stage
        self.visit = visit
        self.kind = kind
        self.who = who
        self.startedAt = startedAt
        self.endedAt = endedAt
        self.verdict = verdict
        self.turns = turns
        self.tokens = tokens
        self.note = note
    }
}

public struct RunWorker: Equatable, Sendable {
    public var stage: String
    public var visit: Int
    public var who: String
    public var terminal: String?
    public var worktree: String?
    public var startedAt: Double

    public init(stage: String, visit: Int, who: String, terminal: String?, worktree: String?, startedAt: Double) {
        self.stage = stage
        self.visit = visit
        self.who = who
        self.terminal = terminal
        self.worktree = worktree
        self.startedAt = startedAt
    }
}

public struct RunCard: Equatable, Sendable {
    public var kind: String
    public var stage: String
    public var title: String
    public var detail: String

    public init(kind: String, stage: String, title: String, detail: String) {
        self.kind = kind
        self.stage = stage
        self.title = title
        self.detail = detail
    }
}

public struct RunNumbers: Equatable, Sendable {
    public var visits = 0
    public var sessions = 0
    public var budget: Int?
    public var waited: Double = 0
    public var waitingSince: Double?
    public var turns: Double?
    public var tokens: Double?
    public var cost: Double?
    public var rework = 0
    public var interventions = 0
    public var lines: (added: Int, removed: Int, files: Int)?

    public init() {}

    public static func == (a: RunNumbers, b: RunNumbers) -> Bool {
        a.visits == b.visits && a.sessions == b.sessions && a.budget == b.budget && a.waited == b.waited && a.waitingSince == b.waitingSince && a.turns == b.turns && a.tokens == b.tokens
            && a.cost == b.cost && a.rework == b.rework && a.interventions == b.interventions && a.lines?.added == b.lines?.added && a.lines?.removed == b.lines?.removed && a.lines?.files == b.lines?.files
    }
}

public struct RunDiffLine: Equatable, Sendable {
    public var sign: String
    public var text: String

    public init(sign: String, text: String) {
        self.sign = sign
        self.text = text
    }
}

public struct RunRevision: Equatable, Sendable {
    public var n: Int?
    public var by: String
    public var at: Double
    public var summary: String?
    public var worst: Int?
    public var budget: Int?
    public var diff: [RunDiffLine]
    public var errors: [String]

    public init(n: Int?, by: String, at: Double, summary: String? = nil, worst: Int? = nil, budget: Int? = nil, diff: [RunDiffLine] = [], errors: [String] = []) {
        self.n = n
        self.by = by
        self.at = at
        self.summary = summary
        self.worst = worst
        self.budget = budget
        self.diff = diff
        self.errors = errors
    }

    public var who: String {
        if by == "file" { return "from the pipeline file" }
        if by.hasPrefix("stage:") { return "by stage \(by.dropFirst(6))" }
        return by == "agent" ? "by the task's agent" : "by you"
    }

    public var title: String {
        if let n { return "Revision \(n)" }
        return by.hasPrefix("stage:") ? "Refused: a kojo block" : "Refused: an edit"
    }

    public var detail: String {
        var parts = ["\(who) at \(RunFormat.time(at))" + (summary.map { ": \($0)" } ?? "")]
        if let worst, n != nil { parts.append("worst case \(worst)" + (budget.map { " of \($0)" } ?? "") + " sessions") }
        return parts.joined(separator: " · ")
    }

    public var changed: Bool { diff.contains { $0.sign != " " } }
}

public struct RunNotice: Equatable, Sendable {
    public var run = ""
    public var task = ""
    public var name: String?
    public var pipeline = ""
    public var branch = ""
    public var state = ""
    public var outcome: String?
    public var why: String?
    public var startedAt: Double = 0
    public var endedAt: Double?
    public var now: Double = 0
    public var startedBy = "you"
    public var revision: (n: Int, by: String, at: Double)?
    public var revisions: [RunRevision] = []
    public var place: (stage: String, at: Int, of: Int) = ("", 0, 0)
    public var columns = 0
    public var rows = 0
    public var nodes: [RunNode] = []
    public var edges: [RunEdge] = []
    public var visits: [RunVisit] = []
    public var numbers = RunNumbers()
    public var workers: [RunWorker] = []
    public var card: RunCard?

    public init() {}

    public var live: Bool { ["running", "waiting", "paused"].contains(state) }
    public var badge: String { state == "done" ? "done" : "\(place.at)/\(place.of)" }

    public static func == (a: RunNotice, b: RunNotice) -> Bool {
        a.run == b.run && a.task == b.task && a.name == b.name && a.pipeline == b.pipeline && a.branch == b.branch && a.state == b.state && a.outcome == b.outcome && a.why == b.why
            && a.startedAt == b.startedAt && a.endedAt == b.endedAt && a.startedBy == b.startedBy && a.revision?.n == b.revision?.n && a.revision?.by == b.revision?.by && a.revisions == b.revisions
            && a.place.stage == b.place.stage && a.place.at == b.place.at && a.place.of == b.place.of && a.columns == b.columns && a.rows == b.rows && a.nodes == b.nodes
            && a.edges == b.edges && a.visits == b.visits && a.numbers == b.numbers && a.workers == b.workers && a.card == b.card
    }
}

public enum RunFormat {
    public static func clock(_ seconds: Double) -> String {
        let s = Int(max(0, seconds).rounded())
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m" }
        return "\(s / 3600)h \(String(format: "%02d", (s % 3600) / 60))m"
    }

    public static func count(_ value: Double?) -> String {
        guard let value else { return "—" }
        if value >= 1_000_000 { return trimmed(value / 1_000_000) + "M" }
        if value >= 1_000 { return "\(Int((value / 1_000).rounded()))k" }
        return "\(Int(value))"
    }

    public static func money(_ value: Double?) -> String? {
        value.map { String(format: "$%.2f", $0) }
    }

    private static func trimmed(_ value: Double) -> String {
        let text = String(format: "%.1f", value)
        return text.hasSuffix(".0") ? String(text.dropLast(2)) : text
    }

    public static func time(_ at: Double) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: Date(timeIntervalSince1970: at))
    }

    public static func verdict(_ visit: RunVisit, state: String) -> String {
        if visit.endedAt == nil { return state == "paused" || state == "waiting" ? "waiting" : "running" }
        switch visit.verdict {
        case "done"?, nil: return "—"
        case let verdict?: return verdict
        }
    }
}
