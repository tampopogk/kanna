import Foundation

struct Choice: Codable, Equatable {
    var label: String
    var why: String?
}

struct Doc: Codable, Equatable {
    var text: String?
    var choices: [Choice]?
    var chosen: String?
    var choice: Int?
    var file: String?
    var line: Int?
    var commit: String?
    var hot: [Int]?
    var lines: [String]?
    var round: String?
    var title: String?
    var about: String?
}

struct Item: Codable, Equatable {
    var seq: Int
    var at: Double
    var kind: String
    var client: String?
    var read: Double?
    var doc: Doc
}

struct Page: Codable {
    var items: [Item]
    var status: String
    var since: Double
}

struct Project: Codable, Equatable {
    var repo: String
    var name: String
    var planner: String?
    var status: String
    var since: Double
}

struct Hello: Codable {
    var name: String
    var key: String
    var addrs: [String]
    var projects: [Project]
}

struct Paired: Codable {
    var name: String
    var machine: String
    var key: String
    var addrs: [String]
    var good: String?
}

struct Named: Codable, Equatable {
    var id: String
    var name: String?
}

struct ShelfTask: Codable, Equatable {
    var id: String
    var name: String
    var status: String
    var since: Double
    var desk: String?
    var machine: String?
    var agent: String?
    var branch: String?
    var after: [Named]?
    var message: String?
    var unreachable: Bool?
}

struct Desk: Codable, Equatable, Identifiable {
    var id: String { machine }
    var machine: String
    var open: Bool
}

struct ShelfPage: Codable, Equatable {
    var tasks: [ShelfTask]
    var desks: [Desk]
    var done: [String]
}

struct ThreadLine: Codable, Equatable {
    var at: Double
    var from: String
    var you: Bool
    var text: String
    var read: Double?
    var client: String?
}

struct Said: Codable, Equatable {
    var at: Double
    var asking: Bool
    var text: String
}

struct TaskPage: Codable, Equatable {
    var task: ShelfTask
    var thread: [ThreadLine]
    var said: [Said]
    var items: [Item]
}

enum Notice {
    case chat(repo: String, item: Item)
    case planner(repo: String, status: String, since: Double)
    case tasks(repo: String)
    case quiz(repo: String)
}

struct Link {
    var path: LinkPath
    var milliseconds: Int
}

struct Refused: Error, LocalizedError {
    var code: String
    var message: String
    var errorDescription: String? { message }
}

@MainActor
protocol Backend: AnyObject {
    var onNotice: ((Notice) -> Void)? { get set }
    var onLost: (() -> Void)? { get set }
    var phoneName: String { get }
    var paired: Paired? { get }
    func pair(scanned: String) async throws -> Paired
    func connect() async throws -> Hello
    func read(repo: String, after: Int) async throws -> Page
    func send(repo: String, id: String, text: String) async throws -> Item
    func choose(repo: String, id: String, seq: Int, choice: String) async throws -> Item
    func tasks(repo: String) async throws -> ShelfPage
    func task(_ id: String) async throws -> TaskPage
    func tell(task: String, id: String, text: String) async throws
    func choose(task: String, id: String, seq: Int, choice: String) async throws
    func pick(task: String, desk: String, room: String?) async throws
    func quizMap(repo: String) async throws -> MapPage
    func quizRounds(repo: String) async throws -> RoundsPage
    func quizAnswer(round: String, n: Int, pick: Int, again: String?) async throws
    func quizAhead(repo: String, about: String?, again: Bool) async throws
    func close()
    func forget()
    func link() async -> Link
    func highlight(file: String, text: String) -> [TextSpan]
    func drawing(_ source: String) -> DrawingScene?
}
