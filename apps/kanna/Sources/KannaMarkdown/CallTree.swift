import AppKit
import KannaCallTreeC

extension NSAttributedString.Key {
    static let kannaCallRow = NSAttributedString.Key("kanna.callRow")
    static let kannaCallGuides = NSAttributedString.Key("kanna.callGuides")
}

struct CallTree: Equatable {
    enum Guide: String { case none, through, tee, end }

    struct Row: Equatable {
        enum Kind: Equatable { case call, hop }
        var kind: Kind
        var process: Int?
        var depth = 0
        var name = ""
        var note = ""
        var number: Int?
        var laneStart = false
        var guides: [Guide] = []
        var sourceLine = 0
        var external = false
        var package = ""
        var location = ""
        var path = ""
        var line = 0
        var label = ""
    }

    var processes: [String] = []
    var rows: [Row] = []
    var errors: [(line: Int, message: String)] = []

    static func == (a: CallTree, b: CallTree) -> Bool {
        a.processes == b.processes && a.rows == b.rows && a.errors.map(\.line) == b.errors.map(\.line) && a.errors.map(\.message) == b.errors.map(\.message)
    }

    static func parse(_ source: String, doc: String) -> CallTree {
        let bytes = Array(source.utf8), docBytes = Array(doc.utf8)
        let raw = bytes.withUnsafeBufferPointer { text in
            docBytes.withUnsafeBufferPointer { path in
                text.withMemoryRebound(to: CChar.self) { text in
                    path.withMemoryRebound(to: CChar.self) { path in kanna_calltree_parse(text.baseAddress, text.count, path.baseAddress, path.count) }
                }
            }
        }
        guard let raw else { return CallTree() }
        defer { kanna_calltree_free(raw) }
        guard let json = try? JSONSerialization.jsonObject(with: Data(bytes: raw, count: strlen(raw))) as? [String: Any] else { return CallTree() }
        var tree = CallTree()
        tree.processes = json["processes"] as? [String] ?? []
        tree.errors = (json["errors"] as? [[String: Any]] ?? []).map { ($0["line"] as? Int ?? 0, $0["message"] as? String ?? "") }
        for row in json["rows"] as? [[String: Any]] ?? [] {
            let guides = (row["guides"] as? [String] ?? []).map { Guide(rawValue: $0) ?? .none }
            if row["kind"] as? String == "hop" {
                tree.rows.append(Row(kind: .hop, guides: guides, sourceLine: row["sourceLine"] as? Int ?? 0, label: row["label"] as? String ?? ""))
                continue
            }
            tree.rows.append(Row(kind: .call, process: row["process"] as? Int, depth: row["depth"] as? Int ?? 0, name: row["name"] as? String ?? "",
                                 note: row["note"] as? String ?? "", number: row["number"] as? Int, laneStart: row["laneStart"] as? Bool ?? false,
                                 guides: guides, sourceLine: row["sourceLine"] as? Int ?? 0, external: row["external"] as? Bool ?? false,
                                 package: row["package"] as? String ?? "", location: row["location"] as? String ?? "", path: row["path"] as? String ?? "",
                                 line: row["line"] as? Int ?? 0))
        }
        return tree
    }

    static let fence = "calltree"
    static let guideWidth: CGFloat = 22
    static let rowPad: CGFloat = 7
    static let hopPad: CGFloat = 2

    static func laneColor(_ index: Int) -> NSColor {
        let colors: [(UInt32, UInt32)] = [(0x7aa2ff, 0x2f63d8), (0xc792ea, 0x8839c4), (0xe5c07b, 0xa15c00), (0x7fd1b9, 0x1a7f6a), (0xf78c6c, 0xc4421a)]
        let (dark, light) = colors[index % colors.count]
        return NSColor(name: nil) { appearance in
            let value = MarkdownTheme.isDark(appearance) ? dark : light
            return NSColor(srgbRed: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255, blue: CGFloat(value & 0xff) / 255, alpha: 1)
        }
    }

    static func laneTint(_ index: Int) -> NSColor {
        let base = laneColor(index)
        return NSColor(name: nil) { appearance in
            var color = NSColor.clear
            appearance.performAsCurrentDrawingAppearance { color = base.withAlphaComponent(MarkdownTheme.isDark(appearance) ? 0.10 : 0.07) }
            return color
        }
    }
}

final class CallRowMark: NSObject {
    let id: Int
    init(id: Int) { self.id = id }
}

final class CallGuides: NSObject {
    let guides: [CallTree.Guide]
    let pad: CGFloat
    init(_ guides: [CallTree.Guide], pad: CGFloat) {
        self.guides = guides
        self.pad = pad
    }
}

extension MarkdownLayoutManager {
    func drawCallGuides(_ shown: NSRange, at origin: NSPoint) {
        guard let storage = textStorage, storage.length > 0 else { return }
        let whole = NSRange(location: 0, length: storage.length)
        var location = shown.location
        while location < NSMaxRange(shown) {
            var range = NSRange()
            let value = storage.attribute(.kannaCallGuides, at: location, longestEffectiveRange: &range, in: whole)
            location = max(location + 1, NSMaxRange(range))
            guard let guides = value as? CallGuides, !guides.guides.isEmpty else { continue }
            let glyph = glyphIndexForCharacter(at: range.location)
            let fragment = lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
            let rect = fragment.offsetBy(dx: origin.x, dy: origin.y).insetBy(dx: 0, dy: -guides.pad)
            MarkdownTheme.guide.setFill()
            for (column, guide) in guides.guides.enumerated() {
                let x = (rect.minX + CGFloat(column) * CallTree.guideWidth + CallTree.guideWidth / 2).rounded()
                switch guide {
                case .none: continue
                case .through: NSRect(x: x, y: rect.minY, width: 1, height: rect.height).fill()
                case .tee:
                    NSRect(x: x, y: rect.minY, width: 1, height: rect.height).fill()
                    NSRect(x: x, y: rect.midY.rounded(), width: CallTree.guideWidth / 2 - 2, height: 1).fill()
                case .end:
                    NSRect(x: x, y: rect.minY, width: 1, height: (rect.height / 2).rounded()).fill()
                    NSRect(x: x, y: rect.midY.rounded(), width: CallTree.guideWidth / 2 - 2, height: 1).fill()
                }
            }
        }
    }

    func callRowRect(containing index: Int) -> NSRect? {
        guard let storage = textStorage, index < storage.length else { return nil }
        var range = NSRange()
        guard storage.attribute(.kannaCallRow, at: index, longestEffectiveRange: &range, in: NSRange(location: 0, length: storage.length)) != nil else { return nil }
        let glyphs = glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        var rect = NSRect.null
        enumerateLineFragments(forGlyphRange: glyphs) { fragment, _, _, _, _ in rect = rect.union(fragment) }
        return rect.isNull ? nil : rect
    }
}

extension MarkdownRenderer {
    func callTreeText(_ tree: CallTree, fenceLine: Int?, indent: CGFloat, firstID: Int) -> NSMutableAttributedString {
        let out = NSMutableAttributedString()
        let small = NSFont.systemFont(ofSize: 11.5, weight: .medium)
        let mono = NSFont.monospacedSystemFont(ofSize: MarkdownTheme.monoSize + 0.5, weight: .regular)
        let note = NSFont.systemFont(ofSize: MarkdownTheme.bodySize - 1)
        let legendStyle = NSMutableParagraphStyle()
        legendStyle.firstLineHeadIndent = indent
        legendStyle.headIndent = indent
        legendStyle.paragraphSpacingBefore = 6
        legendStyle.paragraphSpacing = 6
        for (index, name) in tree.processes.enumerated() {
            out.append(NSAttributedString(string: "■ ", attributes: [.font: small, .foregroundColor: CallTree.laneColor(index), .paragraphStyle: legendStyle]))
            out.append(NSAttributedString(string: name + (index < tree.processes.count - 1 ? "     " : "\n"),
                                          attributes: [.font: small, .foregroundColor: MarkdownTheme.muted, .paragraphStyle: legendStyle]))
        }
        let laneWidth = (tree.processes.map { ($0 as NSString).size(withAttributes: [.font: small]).width }.max() ?? 0) + 28
        let grid = NSTextTable()
        grid.numberOfColumns = 3
        grid.layoutAlgorithm = .automaticLayoutAlgorithm
        grid.collapsesBorders = true
        grid.hidesEmptyCells = false
        var process = 0
        for (rowIndex, row) in tree.rows.enumerated() {
            if row.kind == .call, let p = row.process { process = p }
            let lane = row.kind == .hop ? tree.rows[(rowIndex + 1)...].first(where: { $0.kind == .call })?.process ?? process : process
            let rowStart = out.length
            let pad = row.kind == .hop ? CallTree.hopPad : CallTree.rowPad
            let cell = { (column: Int, width: CGFloat?, alignment: NSTextAlignment, headIndent: CGFloat) -> NSMutableParagraphStyle in
                let block = NSTextTableBlock(table: grid, startingRow: rowIndex, rowSpan: 1, startingColumn: column, columnSpan: 1)
                block.backgroundColor = CallTree.laneTint(lane)
                if let width { block.setValue(width, type: .absoluteValueType, for: .width) }
                block.setWidth(column == 0 ? 12 : 4, type: .absoluteValueType, for: .padding, edge: .minX)
                block.setWidth(column == 2 ? 12 : 4, type: .absoluteValueType, for: .padding, edge: .maxX)
                block.setWidth(pad, type: .absoluteValueType, for: .padding, edge: .minY)
                block.setWidth(pad, type: .absoluteValueType, for: .padding, edge: .maxY)
                if column == 0 {
                    block.setWidth(3, type: .absoluteValueType, for: .border, edge: .minX)
                    block.setBorderColor(CallTree.laneColor(lane), for: .minX)
                }
                let style = NSMutableParagraphStyle()
                style.textBlocks = [block]
                style.alignment = alignment
                style.firstLineHeadIndent = headIndent
                style.headIndent = headIndent
                style.lineBreakMode = .byTruncatingTail
                return style
            }
            let line = fenceLine.map { $0 + row.sourceLine } ?? 0
            let laneStyle = cell(0, laneWidth, .left, 0)
            let laneText = row.kind == .call && row.laneStart ? tree.processes[lane] : ""
            out.append(NSAttributedString(string: laneText + "\n", attributes: [.font: small, .foregroundColor: CallTree.laneColor(lane), .paragraphStyle: laneStyle, .kannaLine: line]))
            let guides = CGFloat(row.guides.count) * CallTree.guideWidth
            let body = cell(1, nil, .left, guides + 2)
            let main = NSMutableAttributedString()
            switch row.kind {
            case .hop:
                main.append(NSAttributedString(string: "⇣ ", attributes: [.font: note, .foregroundColor: MarkdownTheme.text]))
                main.append(NSAttributedString(string: row.label, attributes: [.font: note, .foregroundColor: MarkdownTheme.muted]))
            case .call:
                if let number = row.number {
                    main.append(NSAttributedString(string: "\(number) ", attributes: [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: MarkdownTheme.muted]))
                }
                main.append(NSAttributedString(string: row.name, attributes: [.font: mono, .foregroundColor: MarkdownTheme.accent]))
                if !row.note.isEmpty {
                    main.append(NSAttributedString(string: "  " + row.note, attributes: [.font: note, .foregroundColor: MarkdownTheme.muted]))
                }
            }
            main.append(NSAttributedString(string: "\n", attributes: [.font: note]))
            main.addAttributes([.paragraphStyle: body, .kannaLine: line, .kannaCallGuides: CallGuides(row.guides, pad: pad)], range: NSRange(location: 0, length: main.length))
            out.append(main)
            let place = NSMutableAttributedString()
            if row.kind == .call {
                if !row.package.isEmpty {
                    place.append(NSAttributedString(string: row.package + " ", attributes: [.font: mono, .foregroundColor: MarkdownTheme.muted.withAlphaComponent(0.7)]))
                }
                var attributes: [NSAttributedString.Key: Any] = [.font: mono, .foregroundColor: row.external ? MarkdownTheme.muted : MarkdownTheme.text]
                if !row.external { attributes[.link] = Self.codeLink(path: row.path, line: row.line) }
                attributes[.toolTip] = row.external ? row.path : "\(row.path):\(row.line)"
                place.append(NSAttributedString(string: row.location, attributes: attributes))
            }
            place.append(NSAttributedString(string: "\n", attributes: [.font: mono]))
            place.addAttributes([.paragraphStyle: cell(2, nil, .right, 0), .kannaLine: line], range: NSRange(location: 0, length: place.length))
            out.append(place)
            out.addAttribute(.kannaCallRow, value: CallRowMark(id: firstID + rowIndex), range: NSRange(location: rowStart, length: out.length - rowStart))
        }
        out.append(NSAttributedString(string: "\n", attributes: [.font: NSFont.systemFont(ofSize: 4), .paragraphStyle: { () -> NSParagraphStyle in
            let style = NSMutableParagraphStyle()
            style.paragraphSpacing = 10
            return style
        }()]))
        return out
    }

    nonisolated static func codeLink(path: String, line: Int) -> String {
        (URL(fileURLWithPath: path).absoluteString) + "#L\(line)"
    }
}
