import Foundation

public struct InkBlock: Equatable, Sendable {
    public var line: Int
    public var text: String
    public var selector: String?

    public init(line: Int, text: String, selector: String? = nil) {
        self.line = line
        self.text = text
        self.selector = selector
    }
}

extension DocInk {
    public var isPage: Bool { view != nil }

    public var line: Int { marks.first?.anchor.line ?? 1 }

    public static func lines(_ ink: DocInk) -> [String] {
        var lines: [String] = []
        if let view = ink.view { lines.append("> " + describe(view)) }
        lines.append("> ink, \(ink.marks.count) mark\(ink.marks.count == 1 ? "" : "s"), numbered in the picture:")
        for (index, mark) in ink.marks.enumerated() {
            lines.append("> \(index + 1) " + describe(mark, page: ink.isPage))
            if let note = mark.note.map({ flat($0, 400) }), !note.isEmpty { lines.append(">   note: " + note) }
        }
        return lines
    }

    public static func describe(_ view: InkView) -> String {
        let hash = flat(view.hash, 100)
        let place = hash.isEmpty ? "the page" : "the page at \(hash)"
        guard view.viewport.count == 2, view.scroll.count == 2 else { return place }
        return "\(place), in a \(view.viewport[0])×\(view.viewport[1]) view, scrolled to \(view.scroll[0]),\(view.scroll[1])"
    }

    public static func describe(_ mark: InkMark, page: Bool) -> String {
        let verb: [InkKind: String] = [.circle: "circle around", .underline: "underline under", .strike: "strike through", .cross: "cross over"]
        let words = mark.words.map { flat($0, 120) } ?? ""
        let to = mark.kind == .arrow ? mark.to : nil
        if page {
            let element = { (place: InkPlace) in "element \(flat(place.selector ?? "", 200)) \"\(flat(place.text, 120))\"" }
            if let verb = verb[mark.kind], !words.isEmpty {
                let text = words == flat(mark.anchor.text, 120) ? "" : ", the text \"\(words)\""
                return "\(verb) \(element(mark.anchor))\(text)" + at(mark.anchor.box)
            }
            if let to { return "arrow from \(element(mark.anchor)) to \((mark.side ?? .after).rawValue) \(element(to))" }
            return "\(mark.kind.rawValue) near \(element(mark.anchor))\(at(bounds(mark)))" + (mark.kind == .writing ? ", read it in the picture" : "")
        }
        let line = { (place: InkPlace) in "line \(place.line)" }
        if let verb = verb[mark.kind], !words.isEmpty { return "\(verb) \"\(words)\" (\(line(mark.anchor)))" }
        if let to {
            return "arrow from \"\(trim(mark.anchor.text))\" (\(line(mark.anchor))) to \((mark.side ?? .after).rawValue) \"\(trim(to.text))\" (\(line(to)))"
        }
        if mark.kind == .writing { return "writing beside \(line(mark.anchor)), read it in the picture" }
        return "\(mark.kind.rawValue) at \(line(mark.anchor)), around nothing"
    }

    public static func bounds(_ mark: InkMark) -> [Int]? {
        guard let box = mark.anchor.box, box.count == 4 else { return nil }
        let points = mark.strokes.flatMap { $0 }.filter { $0.count >= 2 }
        guard !points.isEmpty else { return nil }
        let xs = points.map { $0[0] }, ys = points.map { $0[1] }
        let left = Int((Double(box[0]) + xs.min()!).rounded()), top = Int((Double(box[1]) + ys.min()!).rounded())
        let right = Int((Double(box[0]) + xs.max()!).rounded()), bottom = Int((Double(box[1]) + ys.max()!).rounded())
        return [left, top, right - left, bottom - top]
    }

    public static func find(_ place: InkPlace, in blocks: [InkBlock]) -> Int? {
        let text = flat(place.text, .max)
        let hits = blocks.indices.filter { blocks[$0].selector == place.selector && flat(blocks[$0].text, .max) == text }
        return hits.min { abs(blocks[$0].line - place.line) < abs(blocks[$1].line - place.line) }
    }

    public static func stays(_ mark: InkMark, in blocks: [InkBlock]) -> Bool {
        guard let from = find(mark.anchor, in: blocks) else { return false }
        guard mark.kind == .arrow, let place = mark.to else { return true }
        guard let to = find(place, in: blocks) else { return false }
        guard from != to else { return true }
        return from != to + ((mark.side ?? .after) == .after ? 1 : -1)
    }

    static func flat(_ text: String, _ cap: Int) -> String {
        let one = Doc.oneLine(text.split(whereSeparator: \.isWhitespace).joined(separator: " "))
        return one.count <= cap ? one : String(one.prefix(cap - 1)) + "…"
    }

    static func trim(_ text: String, _ cap: Int = 34) -> String {
        let one = flat(text, .max)
        guard one.count > cap else { return one }
        var cut = String(one.prefix(cap - 1))
        while let last = cut.last, last.isWhitespace || ",;:.".contains(last) { cut.removeLast() }
        return cut + "…"
    }

    static func at(_ box: [Int]?) -> String {
        guard let box, box.count == 4 else { return "" }
        return " at \(box[0]),\(box[1]) to \(box[0] + box[2]),\(box[1] + box[3]) px"
    }
}
