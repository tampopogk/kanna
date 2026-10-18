import Foundation

public enum DiffLineKind: String, Sendable, Equatable {
    case context = " "
    case added = "+"
    case removed = "-"
}

public struct DiffLine: Sendable, Equatable, Decodable {
    public var kind: DiffLineKind
    public var text: String
    public var old: Int?
    public var new: Int?

    public init(kind: DiffLineKind, text: String, old: Int?, new: Int?) {
        self.kind = kind
        self.text = text
        self.old = old
        self.new = new
    }

    public init(from decoder: Decoder) throws {
        var fields = try decoder.unkeyedContainer()
        kind = DiffLineKind(rawValue: try fields.decode(String.self)) ?? .context
        text = try fields.decode(String.self)
        old = try fields.decodeIfPresent(Int.self)
        new = try fields.decodeIfPresent(Int.self)
    }
}

public struct DiffHunk: Sendable, Equatable, Decodable {
    public var old: Int
    public var new: Int
    public var scope: String
    public var lines: [DiffLine]

    public var oldEnd: Int { old + lines.filter { $0.kind != .added }.count }
}

public struct DiffFile: Sendable, Equatable, Decodable {
    public var path: String
    public var oldPath: String?
    public var status: String
    public var added: Int
    public var removed: Int
    public var binary: Bool?
    public var untracked: Bool?
    public var fold: String?
    public var note: String?
    public var hunks: [DiffHunk]
}

public struct DiffResult: Sendable, Equatable, Decodable {
    public var folder: String
    public var args: [String]
    public var command: String
    public var top: String
    public var files: [DiffFile]
    public var added: Int
    public var removed: Int
    public var error: String?

    public static func decode(_ data: Data) -> DiffResult? {
        try? JSONDecoder().decode(DiffResult.self, from: data)
    }

    public static func failed(folder: String, args: [String], error: String) -> DiffResult {
        DiffResult(folder: folder, args: args, command: (["git diff"] + args).joined(separator: " "), top: "", files: [], added: 0, removed: 0, error: error)
    }
}
