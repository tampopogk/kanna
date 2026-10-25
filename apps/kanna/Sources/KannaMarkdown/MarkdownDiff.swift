import Foundation
import Markdown

indirect enum DiffEntry {
    case same(any Markup)
    case added(any Markup)
    case removed(any Markup)
    case changed(any Markup, any Markup, [DiffEntry]?)

    var node: any Markup {
        switch self {
        case .same(let node), .added(let node), .removed(let node), .changed(_, let node, _): node
        }
    }
}

struct WordToken {
    let range: NSRange
    let key: String
}

enum WordOp: Equatable {
    case same(NSRange, NSRange)
    case removed(NSRange)
    case added(NSRange)
}

struct DiffFailure: Error, Equatable {
    let message: String
}

enum MarkdownDiff {
    static let similar = 0.4
    static let limit = 4_000_000

    static func entries(old: Document?, new: Document) -> [DiffEntry] {
        guard let old else { return new.children.map { .added($0) } }
        return blocks(Array(old.children), Array(new.children))
    }

    static func blocks(_ old: [any Markup], _ new: [any Markup]) -> [DiffEntry] {
        let oldKeys = old.map(key), newKeys = new.map(key)
        var out: [DiffEntry] = []
        var i = 0, j = 0
        for (a, b) in common(oldKeys, newKeys) {
            out += gap(Array(old[i..<a]), Array(new[j..<b]))
            out.append(.same(new[b]))
            i = a + 1
            j = b + 1
        }
        out += gap(Array(old[i...]), Array(new[j...]))
        return out
    }

    static func children(of node: any Markup) -> [any Markup]? {
        switch node {
        case is BlockQuote, is UnorderedList, is OrderedList, is ListItem: Array(node.children)
        case let table as Markdown.Table: [table.head] + table.body.rows.map { $0 }
        default: nil
        }
    }

    static func key(_ node: any Markup) -> String { node.detachedFromParent.debugDescription() }

    static func words(of node: any Markup) -> [String] {
        tokens(in: text(of: node) as NSString).map(\.key).filter { $0.first.map { !$0.isWhitespace } ?? false }
    }

    static func text(of node: any Markup) -> String {
        switch node {
        case let text as Markdown.Text: text.string
        case let code as InlineCode: code.code
        case let code as CodeBlock: code.code
        case let html as HTMLBlock: html.rawHTML
        case let html as InlineHTML: html.rawHTML
        case let image as Markdown.Image: ([image.source ?? ""] + image.children.map(text)).joined(separator: " ")
        case let link as Markdown.Link: (node.children.map(text) + [link.destination ?? ""]).joined(separator: " ")
        default: node.children.map(text).joined(separator: " ")
        }
    }

    static func similarity(_ a: [String], _ b: [String]) -> Double {
        guard !a.isEmpty || !b.isEmpty else { return 1 }
        return 2 * Double(common(a, b).count) / Double(a.count + b.count)
    }

    static func pairable(_ old: any Markup, _ new: any Markup, cheap: Bool = false) -> Double? {
        guard type(of: old) == type(of: new) else { return nil }
        if children(of: new) != nil, !(new is ListItem) { return 1 }
        if let a = old as? CodeBlock, let b = new as? CodeBlock, a.language?.lowercased() == "mermaid", b.language?.lowercased() == "mermaid" { return 1 }
        guard !cheap else { return nil }
        let score = similarity(words(of: old), words(of: new))
        return score >= similar ? score : nil
    }

    private static func gap(_ old: [any Markup], _ new: [any Markup]) -> [DiffEntry] {
        guard !old.isEmpty, !new.isEmpty, old.count * new.count <= limit else { return old.map { .removed($0) } + new.map { .added($0) } }
        let rows = old.count + 1, columns = new.count + 1
        var score = [Double](repeating: 0, count: rows * columns)
        var pairs: [Int: Double] = [:]
        let cheap = old.count * new.count > 2500
        for a in stride(from: old.count - 1, through: 0, by: -1) {
            for b in stride(from: new.count - 1, through: 0, by: -1) {
                var best = max(score[(a + 1) * columns + b], score[a * columns + b + 1])
                if let pair = pairable(old[a], new[b], cheap: cheap) {
                    pairs[a * columns + b] = pair
                    best = max(best, 1 + pair + score[(a + 1) * columns + b + 1])
                }
                score[a * columns + b] = best
            }
        }
        var out: [DiffEntry] = []
        var removed: [DiffEntry] = [], added: [DiffEntry] = []
        var a = 0, b = 0
        while a < old.count, b < new.count {
            let here = score[a * columns + b]
            if let pair = pairs[a * columns + b], here == 1 + pair + score[(a + 1) * columns + b + 1] {
                out += removed + added
                removed = []
                added = []
                out.append(.changed(old[a], new[b], children(of: new[b]).map { blocks(children(of: old[a]) ?? [], $0) }))
                a += 1
                b += 1
            } else if here == score[(a + 1) * columns + b] {
                removed.append(.removed(old[a]))
                a += 1
            } else {
                added.append(.added(new[b]))
                b += 1
            }
        }
        removed += old[a...].map { .removed($0) }
        added += new[b...].map { .added($0) }
        return out + removed + added
    }

    static func common<T: Equatable>(_ a: [T], _ b: [T]) -> [(Int, Int)] {
        var head = 0
        while head < a.count, head < b.count, a[head] == b[head] { head += 1 }
        var tail = 0
        while tail < a.count - head, tail < b.count - head, a[a.count - 1 - tail] == b[b.count - 1 - tail] { tail += 1 }
        let x = Array(a[head..<(a.count - tail)]), y = Array(b[head..<(b.count - tail)])
        var middle: [(Int, Int)] = []
        if !x.isEmpty, !y.isEmpty {
            guard x.count * y.count <= limit else { return (0..<head).map { ($0, $0) } + (0..<tail).reversed().map { (a.count - 1 - $0, b.count - 1 - $0) } }
            let columns = y.count + 1
            var length = [Int32](repeating: 0, count: (x.count + 1) * columns)
            for i in stride(from: x.count - 1, through: 0, by: -1) {
                for j in stride(from: y.count - 1, through: 0, by: -1) {
                    length[i * columns + j] = x[i] == y[j] ? length[(i + 1) * columns + j + 1] + 1 : max(length[(i + 1) * columns + j], length[i * columns + j + 1])
                }
            }
            var i = 0, j = 0
            while i < x.count, j < y.count {
                if x[i] == y[j] {
                    middle.append((head + i, head + j))
                    i += 1
                    j += 1
                } else if length[(i + 1) * columns + j] >= length[i * columns + j + 1] {
                    i += 1
                } else {
                    j += 1
                }
            }
        }
        return (0..<head).map { ($0, $0) } + middle + (0..<tail).reversed().map { (a.count - 1 - $0, b.count - 1 - $0) }
    }

    static func tokens(in text: NSString, key: @escaping (Int) -> String? = { _ in nil }) -> [WordToken] {
        var out: [WordToken] = []
        var run: (start: Int, kind: Int)?
        let close = { (end: Int) in
            if let open = run {
                let range = NSRange(location: open.start, length: end - open.start)
                out.append(WordToken(range: range, key: text.substring(with: range)))
            }
            run = nil
        }
        text.enumerateSubstrings(in: NSRange(location: 0, length: text.length), options: .byComposedCharacterSequences) { piece, range, _, _ in
            guard let piece, let scalar = piece.unicodeScalars.first else { return }
            let kind = scalar == "\u{FFFC}" || scalar == "\n" ? 0
                : CharacterSet.alphanumerics.contains(scalar) || scalar == "_" ? 1
                : CharacterSet.whitespaces.contains(scalar) ? 2 : 0
            if kind != 0, run?.kind == kind { return }
            close(range.location)
            if kind == 0 {
                out.append(WordToken(range: range, key: piece + (key(range.location) ?? "")))
            } else {
                run = (range.location, kind)
            }
        }
        close(text.length)
        return out
    }

    static func words(_ old: [WordToken], _ new: [WordToken]) -> [WordOp] {
        let pairs = common(old.map(\.key), new.map(\.key))
        var ops: [WordOp] = []
        var i = 0, j = 0
        let flush = { (a: Int, b: Int) in
            ops += old[i..<a].map { .removed($0.range) }
            ops += new[j..<b].map { .added($0.range) }
        }
        for (a, b) in pairs {
            flush(a, b)
            ops.append(.same(old[a].range, new[b].range))
            i = a + 1
            j = b + 1
        }
        flush(old.count, new.count)
        return tidy(ops, old: old, new: new)
    }

    private static func tidy(_ ops: [WordOp], old: [WordToken], new: [WordToken]) -> [WordOp] {
        let blank = Set(new.filter { $0.key.allSatisfy { $0 == " " || $0 == "\t" } }.map(\.range.location))
        var out: [WordOp] = []
        var removed: [WordOp] = [], added: [WordOp] = []
        var index = 0
        func changedAround(_ at: Int) -> Bool {
            let before = ops[..<at].last.map { if case .same = $0 { false } else { true } } ?? false
            let after = ops[(at + 1)...].first.map { if case .same = $0 { false } else { true } } ?? false
            return before && after
        }
        while index < ops.count {
            let op = ops[index]
            if case .same(let a, let b) = op, blank.contains(b.location), changedAround(index) {
                removed.append(.removed(a))
                added.append(.added(b))
            } else if case .same = op {
                out += merge(removed) + merge(added)
                removed = []
                added = []
                out.append(op)
            } else if case .removed = op {
                removed.append(op)
            } else {
                added.append(op)
            }
            index += 1
        }
        return out + merge(removed) + merge(added)
    }

    private static func merge(_ ops: [WordOp]) -> [WordOp] {
        var out: [WordOp] = []
        for op in ops {
            switch (out.last, op) {
            case (.removed(let a)?, .removed(let b)) where NSMaxRange(a) == b.location:
                out[out.count - 1] = .removed(NSUnionRange(a, b))
            case (.added(let a)?, .added(let b)) where NSMaxRange(a) == b.location:
                out[out.count - 1] = .added(NSUnionRange(a, b))
            default:
                out.append(op)
            }
        }
        return out
    }

    static func base(of path: String, against ref: String) -> Result<(commit: String, text: String?), DiffFailure> {
        let file = URL(fileURLWithPath: path).resolvingSymlinksInPath()
        let folder = file.deletingLastPathComponent().path
        guard git(folder, ["rev-parse", "--show-toplevel"]) != nil else { return .failure(DiffFailure(message: "\(file.lastPathComponent) isn't in a git repo, so it has nothing to be compared with")) }
        guard git(folder, ["rev-parse", "--verify", "--quiet", "\(ref)^{commit}"]) != nil else { return .failure(DiffFailure(message: "no commit or branch \"\(ref)\" in its repo")) }
        guard let commit = mergeBase(folder, ref) else { return .failure(DiffFailure(message: "\(ref) and HEAD share no history")) }
        return .success((commit, git(folder, ["show", "\(commit):./\(file.lastPathComponent)"], trim: false)))
    }

    static func mergeBase(_ folder: String, _ ref: String) -> String? { git(folder, ["merge-base", ref, "HEAD"]) }

    static func range(_ ref: String) -> (from: String, to: String)? {
        guard let dots = ref.range(of: ".."), !ref.contains("...") else { return nil }
        let from = String(ref[..<dots.lowerBound]), to = String(ref[dots.upperBound...])
        return (from.isEmpty ? "HEAD" : from, to.isEmpty ? "HEAD" : to)
    }

    static func versions(of path: String, from: String, to: String) -> Result<(old: String?, new: String), DiffFailure> {
        let file = URL(fileURLWithPath: path).resolvingSymlinksInPath()
        let folder = file.deletingLastPathComponent().path, name = file.lastPathComponent
        guard git(folder, ["rev-parse", "--show-toplevel"]) != nil else { return .failure(DiffFailure(message: "\(name) isn't in a git repo, so it has nothing to be compared with")) }
        for ref in [from, to] where git(folder, ["rev-parse", "--verify", "--quiet", "\(ref)^{commit}"]) == nil {
            return .failure(DiffFailure(message: "no commit or branch \"\(ref)\" in its repo"))
        }
        guard let new = git(folder, ["show", "\(to):./\(name)"], trim: false) else { return .failure(DiffFailure(message: "\(name) isn't in \(to)")) }
        return .success((git(folder, ["show", "\(from):./\(name)"], trim: false), new))
    }

    static func git(_ folder: String, _ args: [String], trim: Bool = true) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        process.arguments = ["-C", folder] + args
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        process.standardInput = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        let text = String(decoding: data, as: UTF8.self)
        return trim ? text.trimmingCharacters(in: .whitespacesAndNewlines) : text
    }
}
