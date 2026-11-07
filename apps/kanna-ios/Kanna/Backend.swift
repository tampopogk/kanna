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
    var machine: String?
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
    var plannerKey: String?
    var status: String?
    var since: Double?
}

struct Reach: Codable, Equatable {
    var name: String
    var key: String
    var addrs: [String]
    var good: String?
}

struct Hello: Codable {
    var name: String
    var key: String
    var addrs: [String]
    var relay: String?
    var relay_token: String?
    var projects: [Project]
    var machines: [Reach]?
}

struct Paired: Codable {
    var name: String
    var machine: String
    var key: String
    var addrs: [String]
    var good: String?
    var known: [Reach]?
    var relay: String?
}

struct MachineInfo: Codable, Equatable, Identifiable {
    var id: String { key }
    var name: String
    var key: String
    var me: Bool?
    var os: String?
    var arch: String?
    var cores: Int?
    var version: String?
    var has: Bool
    var planner: Bool
    var up: Bool
    var path: String?
    var lan: Bool?
    var ms: Double?
    var since: Double?
    var seen: Double?
}

struct MachinesPage: Codable, Equatable {
    var from: String
    var machines: [MachineInfo]
}

struct Moved: Codable, Equatable {
    var planner: String
    var plannerKey: String
}

struct StatsPoint: Codable, Equatable {
    var day: Int
    var value: Double?
    var label: String?
}

struct StatsRow: Codable, Equatable {
    var label: String
    var value: Double?
    var note: String?
    var detail: String?
    var figure: String?
}

struct StatsChart: Codable, Equatable {
    var kind: String
    var values: [Double?]?
    var max: Double?
    var made: [Double?]?
    var done: [Double?]?
    var what: String?
    var points: [StatsPoint]?
    var line: [Double?]?
    var rows: [StatsRow]?
    var empty: String?
}

struct StatsTile: Codable, Equatable, Identifiable {
    var id: String
    var label: String
    var value: Double?
    var unit: String?
    var note: String?
    var span: Int?
    var chart: StatsChart?
}

struct StatsSection: Codable, Equatable, Identifiable {
    var id: String
    var title: String
    var note: String?
    var tiles: [StatsTile]?
    var empty: String?
    var placeholder: String?
}

struct StatsPage: Codable, Equatable {
    var days: [String]
    var sections: [StatsSection]
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
    case projects
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
    func connect(to machine: Reach?) async throws -> Hello
    func networkChanged()
    func askLocalNetwork()
    func hello() async throws -> Hello
    func reaches(_ machine: Reach) async -> Bool
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
    func machines(repo: String) async throws -> MachinesPage
    func stats(repo: String) async throws -> StatsPage
    func move(repo: String, machine: String) async throws -> Moved
    func close()
    func forget()
    func link() async -> Link
    func highlight(file: String, text: String) -> [TextSpan]
    func drawing(_ source: String) -> DrawingScene?
}
