import Foundation

struct MovedCommit: Codable, Equatable, Hashable {
    var hash: String
    var at: Double
    var subject: String
}

struct TileMiss: Codable, Equatable, Hashable {
    var answer: String
    var text: String
    var file: String
}

struct SubsystemTile: Codable, Equatable, Identifiable {
    var id: String { name }
    var name: String
    var paths: [String]
    var lines: Int
    var mastery: Int?
    var level: Int
    var answers: Int
    var asked: Double?
    var was: Int?
    var wasAt: Double?
    var moved: Int
    var slipping: Bool
    var commits: [MovedCommit]
    var misses: [TileMiss]
    var notes: String
}

struct MapPage: Codable, Equatable {
    var subsystems: [SubsystemTile]
}

struct PickLine: Codable, Equatable, Hashable {
    var n: Int
    var text: String
}

struct Question: Codable, Equatable {
    var kind: String
    var file: String
    var line: Int
    var text: String
    var why: String
    var topic: String
    var source: String?
    var choices: [Choice]?
    var lines: [PickLine]?
    var drawings: [String]?
    var answer: Int
    var again: String?
    var subsystem: String?
    var code: Doc?

    var kindName: String {
        if again != nil { return "A miss, back again" }
        switch kind {
        case "architecture": return "Architecture"
        case "drawing": return "Pick the drawing"
        case "behaviour": return "Behaviour"
        case "where": return "Where it lives"
        case "why": return "Why"
        case "bug": return "Spot the bug"
        default: return kind
        }
    }
}

struct Pick: Codable, Equatable {
    var n: Int
    var pick: Int
}

struct QuizRound: Codable, Equatable, Identifiable {
    var id: String { round }
    var round: String
    var title: String
    var about: String?
    var says: String?
    var written: Double
    var taken: Double?
    var head: String
    var questions: [Question]
    var answers: [Pick]
}

struct Writing: Codable, Equatable {
    var since: Double
    var about: String?
}

struct RoundsPage: Codable, Equatable {
    var rounds: [QuizRound]
    var writing: Writing?
    var queued: [String]?
    var failed: String?
}

struct RoundPlay: Codable, Equatable {
    var repo: String
    var round: QuizRound
    var index = 0
    var answers: [Int] = []
    var revealed = false
    var finished = false
    var before: [String: Int] = [:]
    var moves: [MapMove] = []

    var current: Question { round.questions[index] }
    var count: Int { round.questions.count }
    var right: Int { zip(answers, round.questions).filter { $0 == $1.answer }.count }
}

struct MapMove: Codable, Equatable {
    var name: String
    var from: Int
    var to: Int
}

struct PendingAnswer: Codable, Equatable {
    var repo: String
    var round: String
    var n: Int
    var pick: Int
    var again: String?
}

struct QuizKept: Codable, Equatable {
    var play: RoundPlay?
    var pending: [PendingAnswer] = []
    var results: [String: Int] = [:]
}

struct DrawingPoint: Codable, Equatable {
    var x: Double
    var y: Double
}

struct DrawingNode: Codable, Equatable {
    var x: Double
    var y: Double
    var width: Double
    var height: Double
    var label: String
}

struct DrawingEdge: Codable, Equatable {
    var points: [DrawingPoint]
    var dashed: Bool
    var label: String
    var at: DrawingPoint?
}

struct DrawingScene: Codable, Equatable {
    var width: Double
    var height: Double
    var nodes: [DrawingNode]
    var edges: [DrawingEdge]
}
