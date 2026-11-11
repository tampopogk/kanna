import Foundation

public struct DocMessage: Codable, Equatable {
    public var who: String
    public var text: String
    public var at: String

    package init(who: String, text: String, at: String) {
        self.who = who
        self.text = text
        self.at = at
    }
}

public struct PageSpot: Codable, Equatable {
    public var selector: String
    public var text: String
    public var hash: String
    public var box: [Int]
    public var viewport: [Int]
    public var scroll: [Int]
    public var selection: Bool?

    package init(selector: String, text: String, hash: String, box: [Int], viewport: [Int], scroll: [Int], selection: Bool? = nil) {
        self.selector = selector
        self.text = text
        self.hash = hash
        self.box = box
        self.viewport = viewport
        self.scroll = scroll
        self.selection = selection
    }
}

public enum InkKind: String, Codable, CaseIterable, Sendable {
    case circle, underline, strike, cross, arrow, writing

    public init(from decoder: Decoder) throws {
        self = InkKind(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .writing
    }
}

public enum InkSide: String, Codable, Sendable {
    case before, after

    public init(from decoder: Decoder) throws {
        self = InkSide(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .after
    }
}

public struct InkPlace: Codable, Equatable, Sendable {
    public var line: Int
    public var text: String
    public var selector: String?
    public var box: [Int]?

    public init(line: Int, text: String, selector: String? = nil, box: [Int]? = nil) {
        self.line = line
        self.text = text
        self.selector = selector
        self.box = box
    }
}

public struct InkMark: Codable, Equatable, Sendable {
    public var kind: InkKind
    public var strokes: [[[Double]]]
    public var anchor: InkPlace
    public var words: String?
    public var to: InkPlace?
    public var side: InkSide?
    public var tip: Int?
    public var note: String?

    public init(kind: InkKind, strokes: [[[Double]]], anchor: InkPlace, words: String? = nil, to: InkPlace? = nil, side: InkSide? = nil,
                tip: Int? = nil, note: String? = nil) {
        self.kind = kind
        self.strokes = strokes
        self.anchor = anchor
        self.words = words
        self.to = to
        self.side = side
        self.tip = tip
        self.note = note
    }
}

public struct InkView: Codable, Equatable, Sendable {
    public var hash: String
    public var viewport: [Int]
    public var scroll: [Int]

    public init(hash: String, viewport: [Int], scroll: [Int]) {
        self.hash = hash
        self.viewport = viewport
        self.scroll = scroll
    }
}

public struct DocInk: Codable, Equatable, Sendable {
    public var view: InkView?
    public var marks: [InkMark]

    public init(view: InkView? = nil, marks: [InkMark]) {
        self.view = view
        self.marks = marks
    }
}

public struct DocThread: Codable, Equatable {
    public var id: String
    public var quote: String
    public var prefix: String?
    public var suffix: String?
    public var line: Int
    public var state: String
    public var status: String?
    public var messages: [DocMessage]
    public var error: String?
    public var image: String?
    public var region: [Double]?
    public var page: PageSpot?
    public var mark: String?
    public var ink: DocInk?

    package init(id: String, quote: String, prefix: String?, suffix: String?, line: Int, state: String, status: String?, messages: [DocMessage],
                 error: String?, page: PageSpot? = nil, ink: DocInk? = nil) {
        self.id = id
        self.quote = quote
        self.prefix = prefix
        self.suffix = suffix
        self.line = line
        self.state = state
        self.status = status
        self.messages = messages
        self.error = error
        self.page = page
        self.ink = ink
    }

    public var isOpen: Bool { state == "open" }
}

public struct DocRecord: Codable, Equatable {
    public var file: String
    public var agent: String?
    public var socket: String?
    public var next: Int
    public var threads: [DocThread]

    package init(file: String) {
        self.file = file
        next = 1
        threads = []
    }
}

public enum DocDelivery: Equatable, Sendable {
    case sent
    case waiting
    case failed(String)
}

public typealias DocSend = (_ agent: String, _ text: String, _ doc: String, _ thread: String, _ done: @escaping (DocDelivery) -> Void) -> Void

public typealias DocThreadEvent = (_ agent: String, _ text: String, _ doc: String, _ thread: String, _ state: String) -> Void

public struct DocEdit: Equatable {
    public var state: String?
    public var status: String?
    public var error: String??
    public var line: Int?
    public var message: DocMessage?

    public init(state: String? = nil, status: String? = nil, error: String?? = nil, line: Int? = nil, message: DocMessage? = nil) {
        self.state = state
        self.status = status
        self.error = error
        self.line = line
        self.message = message
    }

    public init(delivered: DocDelivery, line: Int) {
        switch delivered {
        case .sent: self.init(status: "sent", error: .some(nil), line: line)
        case .waiting: self.init(error: .some(nil), line: line)
        case .failed(let why): self.init(status: "unsent", error: .some(why), line: line)
        }
    }
}

public enum DocChange: Equatable {
    case post(DocThread, mark: Data?)
    case edit(String, DocEdit)
    case agent(String)
}

public struct DocAnswer {
    public var id: String?
    public var record: DocRecord?
    public var error: String?

    public init(id: String? = nil, record: DocRecord? = nil, error: String? = nil) {
        self.id = id
        self.record = record
        self.error = error
    }
}

public typealias DocChanger = (DocChange, @escaping @MainActor (DocAnswer) -> Void) -> Void

public enum Doc {
    public static func isThreadID(_ id: String) -> Bool {
        id.count > 1 && id.count < 12 && id.first == "c" && id.dropFirst().allSatisfy(\.isASCII) && id.dropFirst().allSatisfy(\.isNumber)
    }

    public static func now() -> String { stamp.string(from: Date()) }

    public static func date(_ text: String) -> Date? { stamp.date(from: text) }

    private static var stamp: DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        return formatter
    }

    public static func clean(_ text: String) -> String {
        let normalized = text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        return String(String.UnicodeScalarView(normalized.unicodeScalars.filter { scalar in
            let value = scalar.value
            return value == 0x0a || value == 0x09 || !(value < 0x20 || (0x7f...0x9f).contains(value))
        }))
    }

    public static func oneLine(_ text: String) -> String {
        String(String.UnicodeScalarView(text.unicodeScalars.map { scalar in
            let value = scalar.value
            return value < 0x20 || (0x7f...0x9f).contains(value) || value == 0x2028 || value == 0x2029 ? "?" : scalar
        }))
    }

    public static func message(_ thread: DocThread, file: String, text: String, quoted: Bool = true, mark: String? = nil) -> String {
        let header = "[kanna comment \(oneLine(thread.id)) · \(oneLine(file)):\(thread.line)]"
        if let ink = thread.ink {
            var lines = DocInk.lines(ink)
            if let mark { lines.append("> marked up: " + oneLine(mark)) }
            return ([header] + lines + (text.isEmpty ? [] : [text])).joined(separator: "\n")
        }
        if let page = thread.page {
            var lines = ["> " + describe(page)]
            if page.box.count == 4, page.viewport.count == 2, page.scroll.count == 2 {
                let b = page.box
                lines.append("> box \(b[0]),\(b[1]) to \(b[0] + b[2]),\(b[1] + b[3]) px in a \(page.viewport[0])×\(page.viewport[1]) view, scrolled to \(page.scroll[0]),\(page.scroll[1])")
            }
            if let mark { lines.append("> marked up: " + oneLine(mark)) }
            return "\(header)\n\(lines.joined(separator: "\n"))\n\(text)"
        }
        guard quoted else { return "\(header)\n\(text)" }
        let lines = clean(thread.quote).components(separatedBy: "\n").map { "> " + oneLine($0) }
        return "\(header)\n\(lines.joined(separator: "\n"))\n\(text)"
    }

    public static func stateEvent(_ thread: DocThread, file: String, open: Bool) -> String {
        "[kanna \(open ? "reopened" : "resolved") \(oneLine(thread.id)) · \(oneLine(file)):\(thread.line)]"
    }

    public static func describe(_ page: PageSpot) -> String {
        let flat = { (text: String, cap: Int) -> String in
            let one = oneLine(text.split(whereSeparator: \.isWhitespace).joined(separator: " "))
            return one.count <= cap ? one : String(one.prefix(cap - 1)) + "…"
        }
        let hash = flat(page.hash, 100), selector = flat(page.selector, 200), text = flat(page.text, 120)
        let place = hash.isEmpty ? "the page" : "the page at \(hash)"
        guard !selector.isEmpty else { return "\(place), a box" }
        if page.selection == true { return "\(place), the text \"\(text)\" in \(selector)" }
        return "\(place), element \(selector)" + (text.isEmpty ? "" : " \"\(text)\"")
    }

    public static func line(of quote: String, in source: String, near hint: Int) -> Int {
        let words = quote.split(whereSeparator: \.isWhitespace)
        for probe in [quote, words.prefix(3).joined(separator: " "), words.prefix(1).joined(separator: " ")] where !probe.isEmpty {
            var hits: [Int] = []
            var search = source.startIndex..<source.endIndex
            while let found = source.range(of: probe, range: search) {
                hits.append(source[..<found.lowerBound].reduce(1) { $1 == "\n" ? $0 + 1 : $0 })
                search = source.index(after: found.lowerBound)..<source.endIndex
            }
            if let best = hits.min(by: { abs($0 - hint) < abs($1 - hint) }) { return best }
        }
        return hint
    }

    public static func locate(_ thread: DocThread, in text: String) -> NSRange? {
        let hay = text as NSString
        let quote = thread.quote
        guard !quote.isEmpty else { return nil }
        let prefix = Array((thread.prefix ?? "").utf16), suffix = Array((thread.suffix ?? "").utf16)
        var best: (at: Int, score: Int)?
        var from = 0
        while from < hay.length {
            let found = hay.range(of: quote, options: [.literal], range: NSRange(location: from, length: hay.length - from))
            guard found.location != NSNotFound else { break }
            let before = Array(hay.substring(with: NSRange(location: max(0, found.location - 32), length: min(32, found.location))).utf16)
            let afterStart = NSMaxRange(found)
            let after = Array(hay.substring(with: NSRange(location: afterStart, length: min(32, hay.length - afterStart))).utf16)
            var score = 0
            while score < min(before.count, prefix.count), before[before.count - 1 - score] == prefix[prefix.count - 1 - score] { score += 1 }
            var tail = 0
            while tail < min(after.count, suffix.count), after[tail] == suffix[tail] { tail += 1 }
            if best == nil || score + tail > best!.score { best = (found.location, score + tail) }
            from = found.location + 1
        }
        return best.map { NSRange(location: $0.at, length: (quote as NSString).length) }
    }
}
