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

enum Notice {
    case chat(repo: String, item: Item)
    case planner(repo: String, status: String, since: Double)
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
    func close()
    func forget()
    func link() async -> Link
    func highlight(file: String, text: String) -> [TextSpan]
}
