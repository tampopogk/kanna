import Foundation

public struct LogRow: Sendable, Equatable, Decodable {
    public var hash: String
    public var parents: [String]
    public var author: String
    public var time: Int
    public var refs: [String]
    public var subject: String
    public var spine: Bool
    public var side: String?
    public var fold: Int

    public init(hash: String, parents: [String], author: String = "", time: Int = 0, refs: [String] = [], subject: String = "", spine: Bool = false, side: String? = nil, fold: Int = 0) {
        self.hash = hash
        self.parents = parents
        self.author = author
        self.time = time
        self.refs = refs
        self.subject = subject
        self.spine = spine
        self.side = side
        self.fold = fold
    }

    public var short: String { String(hash.prefix(8)) }
    public var isMerge: Bool { parents.count > 1 }
}

public struct LogTask: Sendable, Equatable, Decodable {
    public var id: String
    public var name: String
    public var place: String
    public var branch: String
    public var tip: String
    public var ahead: Int
    public var behind: Int
    public var merged: Bool

    public init(id: String, name: String, place: String, branch: String, tip: String, ahead: Int = 0, behind: Int = 0, merged: Bool = false) {
        self.id = id
        self.name = name
        self.place = place
        self.branch = branch
        self.tip = tip
        self.ahead = ahead
        self.behind = behind
        self.merged = merged
    }
}

public struct LogResult: Sendable, Equatable, Decodable {
    public var folder: String
    public var args: [String]
    public var top: String
    public var main: String
    public var range: [String]
    public var command: String
    public var rows: [LogRow]
    public var tasks: [LogTask]
    public var more: Bool
    public var error: String?

    public init(folder: String = "", args: [String] = [], top: String = "", main: String = "main", range: [String] = [], command: String = "git log", rows: [LogRow], tasks: [LogTask] = [], more: Bool = false, error: String? = nil) {
        self.folder = folder
        self.args = args
        self.top = top
        self.main = main
        self.range = range
        self.command = command
        self.rows = rows
        self.tasks = tasks
        self.more = more
        self.error = error
    }

    public static func decode(_ data: Data) -> LogResult? {
        try? JSONDecoder().decode(LogResult.self, from: data)
    }

    public static func failed(folder: String, args: [String], error: String) -> LogResult {
        LogResult(folder: folder, args: args, command: (["git log"] + args).joined(separator: " "), rows: [], error: error)
    }
}

public struct CommitFile: Sendable, Equatable, Decodable {
    public var path: String
    public var old: String?
    public var status: String
    public var added: Int?
    public var removed: Int?
}

public struct CommitDetail: Sendable, Equatable, Decodable {
    public var hash: String
    public var parents: [String]
    public var author: String
    public var email: String
    public var time: Int
    public var message: String
    public var files: [CommitFile]

    public static func decode(_ data: Data) -> CommitDetail? {
        try? JSONDecoder().decode(CommitDetail.self, from: data)
    }
}

public enum LogAge {
    public static func text(_ time: Int, now: Int) -> String {
        let seconds = max(0, now - time)
        if seconds < 60 { return "just now" }
        if seconds < 3600 { return "\(Int((Double(seconds) / 60).rounded()))m ago" }
        if seconds < 86400 { return "\(Int((Double(seconds) / 3600).rounded()))h ago" }
        if seconds < 86400 * 30 { return "\(Int((Double(seconds) / 86400).rounded()))d ago" }
        if seconds < 86400 * 365 { return "\(Int((Double(seconds) / 86400 / 30).rounded()))mo ago" }
        return "\(Int((Double(seconds) / 86400 / 365).rounded()))y ago"
    }
}
