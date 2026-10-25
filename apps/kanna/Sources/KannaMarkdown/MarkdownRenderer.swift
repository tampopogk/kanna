import AppKit
import KannaDiagram
import Markdown

extension NSAttributedString.Key {
    static let kannaHeading = NSAttributedString.Key("kanna.heading")
    static let kannaBlock = NSAttributedString.Key("kanna.block")
    static let kannaLine = NSAttributedString.Key("kanna.line")
    static let kannaChange = NSAttributedString.Key("kanna.change")
    static let kannaSource = NSAttributedString.Key("kanna.source")
}

enum ChangeKind: String {
    case added, removed, changed
}

final class BlockDecoration: NSObject {
    enum Kind { case code, quote, rule }
    let kind: Kind
    let id: Int
    let indent: CGFloat
    init(_ kind: Kind, id: Int, indent: CGFloat) {
        self.kind = kind
        self.id = id
        self.indent = indent
    }
}

@MainActor
final class DiagramCache {
    private var layouts: [String: Result<DiagramLayout, DiagramError>] = [:]
    private(set) var lastLayoutMs: Double = 0

    func layout(_ source: String) -> Result<DiagramLayout, DiagramError> {
        if let cached = layouts[source] { return cached }
        let start = Date()
        let result: Result<DiagramLayout, DiagramError>
        do { result = .success(try Diagram.layout(source: source)) } catch let error as DiagramError { result = .failure(error) } catch {
            result = .failure(DiagramError(message: "\(error)"))
        }
        lastLayoutMs = Date().timeIntervalSince(start) * 1000
        layouts[source] = result
        return result
    }

    func keep(only sources: Set<String>) { layouts = layouts.filter { sources.contains($0.key) } }
}

@MainActor
struct MarkdownRenderer {
    var folder: URL
    var diagrams: DiagramCache

    private var blockCount = 0
    private var old = false
    private var slugs: [String: Int] = [:]
    private(set) var diagramSources: [String] = []

    init(folder: URL, diagrams: DiagramCache) {
        self.folder = folder
        self.diagrams = diagrams
    }

    private struct Context {
        var indent: CGFloat = 0
        var quote: BlockDecoration?
        var color: NSColor = MarkdownTheme.text
        var tight = false
    }

    var bodyFont: NSFont { .systemFont(ofSize: MarkdownTheme.bodySize) }
    var monoFont: NSFont { .monospacedSystemFont(ofSize: MarkdownTheme.monoSize, weight: .regular) }

    private func paragraph(_ context: Context, before: CGFloat = 0, after: CGFloat = 10, lineSpacing: CGFloat = 3,
                           headIndent: CGFloat? = nil) -> NSMutableParagraphStyle {
        let style = NSMutableParagraphStyle()
        style.lineSpacing = lineSpacing
        style.paragraphSpacingBefore = before
        style.paragraphSpacing = context.tight ? min(after, 4) : after
        style.firstLineHeadIndent = context.indent
        style.headIndent = headIndent ?? context.indent
        return style
    }

    private func attributes(_ context: Context, font: NSFont? = nil) -> [NSAttributedString.Key: Any] {
        var attributes: [NSAttributedString.Key: Any] = [.font: font ?? bodyFont, .foregroundColor: context.color]
        if let quote = context.quote { attributes[.kannaBlock] = quote }
        return attributes
    }

    mutating func render(_ document: Document) -> NSAttributedString {
        render(document.children.map { .same($0) })
    }

    mutating func render(_ entries: [DiffEntry]) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for entry in entries { self.entry(entry, Context(), into: out) }
        while out.length > 0, out.string.hasSuffix("\n") { out.deleteCharacters(in: NSRange(location: out.length - 1, length: 1)) }
        return out
    }

    private mutating func entry(_ entry: DiffEntry, _ context: Context, into out: NSMutableAttributedString) {
        let start = out.length
        switch entry {
        case .same(let node):
            block(node, context, into: out)
        case .added(let node):
            block(node, context, into: out)
            if node is CodeBlock, out.length > start, out.attribute(.attachment, at: start, effectiveRange: nil) != nil {
                label("Added", color: MarkdownTheme.addedBar, context, at: start, into: out)
            }
            Self.mark(.added, NSRange(location: start, length: out.length - start), in: out)
        case .removed(let node):
            asOld { $0.block(node, context, into: out) }
            if out.length > start, out.attribute(.attachment, at: start, effectiveRange: nil) != nil {
                label("Removed", color: MarkdownTheme.removedText, context, at: start, into: out)
            }
            Self.mark(.removed, NSRange(location: start, length: out.length - start), in: out)
        case .changed(let was, let node, let children?):
            block(node, context, into: out, diff: (was, children))
        case .changed(let was, let node, nil):
            changed(was, node, context, into: out)
        }
    }

    private mutating func asOld(_ body: (inout MarkdownRenderer) -> Void) {
        let before = old
        old = true
        body(&self)
        old = before
    }

    private mutating func changed(_ was: any Markup, _ node: any Markup, _ context: Context, into out: NSMutableAttributedString) {
        let start = out.length
        let before = NSMutableAttributedString(), after = NSMutableAttributedString()
        asOld { $0.block(was, context, into: before) }
        block(node, context, into: after)
        if let words = Self.compose(before, after) {
            out.append(words)
        } else {
            note("Before", color: MarkdownTheme.removedText, context, into: out)
            out.append(before)
            note("After", color: MarkdownTheme.addedBar, context, into: out)
            out.append(after)
        }
        Self.mark(.changed, NSRange(location: start, length: out.length - start), in: out)
    }

    private func label(_ text: String, color: NSColor, _ context: Context, at index: Int, into out: NSMutableAttributedString) {
        let note = NSMutableAttributedString()
        self.note(text, color: color, context, into: note)
        out.insert(note, at: index)
    }

    static func mark(_ kind: ChangeKind, _ range: NSRange, in out: NSMutableAttributedString) {
        guard range.length > 0 else { return }
        out.addAttribute(.kannaChange, value: kind.rawValue, range: range)
        guard kind != .changed else { return }
        let text = out.string as NSString
        var index = range.location
        while index < NSMaxRange(range) {
            let character = text.character(at: index)
            var end = index + 1
            while end < NSMaxRange(range), (text.character(at: end) == 10) == (character == 10) { end += 1 }
            if character != 10 { style(kind, NSRange(location: index, length: end - index), in: out) }
            index = end
        }
    }

    private static func style(_ kind: ChangeKind, _ range: NSRange, in out: NSMutableAttributedString) {
        switch kind {
        case .added:
            out.addAttribute(.backgroundColor, value: MarkdownTheme.addedTint, range: range)
        case .removed:
            out.addAttributes([.strikethroughStyle: NSUnderlineStyle.single.rawValue, .strikethroughColor: MarkdownTheme.removedText,
                               .foregroundColor: MarkdownTheme.removedText], range: range)
        case .changed:
            break
        }
    }

    static func compose(_ old: NSAttributedString, _ new: NSAttributedString) -> NSMutableAttributedString? {
        let source = { (text: NSAttributedString) in { (index: Int) in text.attribute(.kannaSource, at: index, effectiveRange: nil) as? String } }
        let ops = MarkdownDiff.words(MarkdownDiff.tokens(in: old.string as NSString, key: source(old)),
                                     MarkdownDiff.tokens(in: new.string as NSString, key: source(new)))
        let touches = { (text: NSAttributedString, range: NSRange) in (text.string as NSString).range(of: "\u{FFFC}", range: range).location != NSNotFound }
        for op in ops {
            switch op {
            case .removed(let range) where touches(old, range): return nil
            case .added(let range) where touches(new, range): return nil
            default: continue
            }
        }
        let out = NSMutableAttributedString()
        var at = 0
        for op in ops {
            switch op {
            case .same(_, let range):
                out.append(new.attributedSubstring(from: range))
                at = NSMaxRange(range)
            case .added(let range):
                let start = out.length
                out.append(new.attributedSubstring(from: range))
                mark(.added, NSRange(location: start, length: range.length), in: out)
                at = NSMaxRange(range)
            case .removed(let range):
                let piece = NSMutableAttributedString(attributedString: old.attributedSubstring(from: range))
                let whole = NSRange(location: 0, length: piece.length)
                let near = new.length == 0 ? nil : min(at, new.length - 1)
                piece.removeAttribute(.kannaHeading, range: whole)
                if let near {
                    if let block = new.attribute(.kannaBlock, at: near, effectiveRange: nil) { piece.addAttribute(.kannaBlock, value: block, range: whole) }
                    if !piece.string.contains("\n"), let style = new.attribute(.paragraphStyle, at: near, effectiveRange: nil) {
                        piece.addAttribute(.paragraphStyle, value: style, range: whole)
                    }
                    if let heading = new.attribute(.kannaHeading, at: near, effectiveRange: nil) { piece.addAttribute(.kannaHeading, value: heading, range: whole) }
                }
                let start = out.length
                out.append(piece)
                mark(.removed, NSRange(location: start, length: piece.length), in: out)
            }
        }
        return out
    }

    private mutating func block(_ markup: any Markup, _ context: Context, into out: NSMutableAttributedString,
                                diff: (was: any Markup, children: [DiffEntry])? = nil) {
        let start = out.length
        defer {
            if !old, let line = markup.range?.lowerBound.line, out.length > start {
                let added = NSRange(location: start, length: out.length - start)
                out.enumerateAttribute(.kannaLine, in: added) { value, range, _ in
                    if value == nil { out.addAttribute(.kannaLine, value: line, range: range) }
                }
            }
        }
        switch markup {
        case let heading as Heading:
            let level = min(max(heading.level, 1), 6)
            let font = NSFont.systemFont(ofSize: MarkdownTheme.headingSizes[level - 1], weight: level <= 2 ? .bold : .semibold)
            let text = inline(heading.children, attributes(context, font: font))
            let style = paragraph(context, before: level <= 2 ? 14 : 10, after: level <= 2 ? 10 : 6, lineSpacing: 2)
            text.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: text.length))
            if !old { text.addAttribute(.kannaHeading, value: slug(heading.plainText), range: NSRange(location: 0, length: text.length)) }
            out.append(text)
            out.append(newline(style, font, context))

        case let para as Paragraph:
            let text = inline(para.children, attributes(context))
            let style = paragraph(context)
            text.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: text.length))
            out.append(text)
            out.append(newline(style, bodyFont, context))

        case let quote as BlockQuote:
            blockCount += 1
            var inner = context
            inner.indent += 16
            inner.color = MarkdownTheme.muted
            inner.quote = BlockDecoration(.quote, id: blockCount, indent: context.indent)
            for child in diff?.children ?? quote.children.map({ .same($0) }) { entry(child, inner, into: out) }

        case let list as UnorderedList:
            listItems(diff?.children ?? list.listItems.map { .same($0) }, ordered: nil, was: nil, context, into: out)

        case let list as OrderedList:
            listItems(diff?.children ?? list.listItems.map { .same($0) }, ordered: Int(list.startIndex),
                      was: (diff?.was as? OrderedList).map { Int($0.startIndex) }, context, into: out)

        case let code as CodeBlock:
            if code.language?.lowercased() == "mermaid" {
                diagram(code.code, context, into: out)
            } else {
                codeBlock(code.code, context, into: out)
            }

        case is ThematicBreak:
            blockCount += 1
            let style = paragraph(context, before: 6, after: 12)
            out.append(NSAttributedString(string: "\u{00A0}\n", attributes: [
                .font: bodyFont, .paragraphStyle: style, .kannaBlock: BlockDecoration(.rule, id: blockCount, indent: context.indent),
            ]))

        case let table as Markdown.Table:
            self.table(table, rows: diff?.children ?? MarkdownDiff.children(of: table)!.map { .same($0) }, context, into: out)

        case let html as HTMLBlock:
            codeBlock(html.rawHTML, context, into: out)

        default:
            for child in markup.children { block(child, context, into: out) }
        }
    }

    private func newline(_ style: NSParagraphStyle, _ font: NSFont, _ context: Context) -> NSAttributedString {
        var attributes: [NSAttributedString.Key: Any] = [.paragraphStyle: style, .font: font]
        if let quote = context.quote { attributes[.kannaBlock] = quote }
        return NSAttributedString(string: "\n", attributes: attributes)
    }

    private mutating func listItems(_ entries: [DiffEntry], ordered start: Int?, was: Int?, _ context: Context, into out: NSMutableAttributedString) {
        let depth = Int((context.indent / 24).rounded())
        let bullets = ["•", "◦", "▪"]
        let widest = start.map { "\($0 + entries.count - 1)." } ?? "☐"
        let markerWidth = max(18, (widest + " ").size(withAttributes: [.font: bodyFont]).width + 6)
        var number = start ?? 1
        for entry in entries {
            guard let item = entry.node as? ListItem else { continue }
            let itemStart = out.length
            var marker = start.map { _ in "\(number)." } ?? bullets[depth % bullets.count]
            if case .removed = entry {
                marker = start.map { "\((was ?? $0) + item.indexInParent)." } ?? marker
            } else {
                number += 1
            }
            switch item.checkbox {
            case .checked: marker = "☑"
            case .unchecked: marker = "☐"
            case nil: break
            }
            var inner = context
            inner.indent = context.indent + markerWidth
            inner.tight = true
            var children: [DiffEntry] = item.children.map { .same($0) }
            if case .changed(_, _, let inside?) = entry { children = inside }
            let removed: Bool = if case .removed = entry { true } else { false }
            for (position, child) in children.enumerated() {
                let start = out.length
                if removed { asOld { $0.entry(child, inner, into: out) } } else { self.entry(child, inner, into: out) }
                guard position == 0, child.node is Paragraph else { continue }
                let style = (out.attribute(.paragraphStyle, at: start, effectiveRange: nil) as? NSParagraphStyle)?.mutableCopy() as? NSMutableParagraphStyle
                    ?? paragraph(inner)
                style.firstLineHeadIndent = context.indent
                style.tabStops = [NSTextTab(textAlignment: .left, location: inner.indent)]
                let markerText = NSAttributedString(string: marker + "\t", attributes: [
                    .font: bodyFont, .foregroundColor: item.checkbox == .checked ? MarkdownTheme.accent : MarkdownTheme.muted, .paragraphStyle: style,
                ])
                out.insert(markerText, at: start)
                let range = (out.string as NSString).paragraphRange(for: NSRange(location: start, length: 0))
                out.addAttribute(.paragraphStyle, value: style, range: range)
            }
            if children.isEmpty || !(children[0].node is Paragraph) {
                out.append(NSAttributedString(string: marker + "\n", attributes: [.font: bodyFont, .foregroundColor: MarkdownTheme.muted, .paragraphStyle: paragraph(context)]))
            }
            let range = NSRange(location: itemStart, length: out.length - itemStart)
            switch entry {
            case .added: Self.mark(.added, range, in: out)
            case .removed: Self.mark(.removed, range, in: out)
            default: break
            }
        }
        if out.length > 0, let style = (out.attribute(.paragraphStyle, at: out.length - 1, effectiveRange: nil) as? NSParagraphStyle)?.mutableCopy() as? NSMutableParagraphStyle {
            style.paragraphSpacing = context.tight ? 4 : 10
            out.addAttribute(.paragraphStyle, value: style, range: (out.string as NSString).paragraphRange(for: NSRange(location: out.length - 1, length: 0)))
        }
    }

    private mutating func codeBlock(_ code: String, _ context: Context, into out: NSMutableAttributedString) {
        blockCount += 1
        let decoration = BlockDecoration(.code, id: blockCount, indent: context.indent)
        let pad: CGFloat = 12
        var lines = code.components(separatedBy: "\n")
        if lines.count > 1, lines.last == "" { lines.removeLast() }
        for (index, line) in lines.enumerated() {
            var inner = context
            inner.indent += pad
            inner.tight = false
            let style = paragraph(inner, before: index == 0 ? 10 : 0, after: index == lines.count - 1 ? 20 : 0, lineSpacing: 2)
            style.tailIndent = -pad
            style.lineBreakMode = .byWordWrapping
            out.append(NSAttributedString(string: line + "\n", attributes: [
                .font: monoFont, .foregroundColor: MarkdownTheme.text, .paragraphStyle: style, .kannaBlock: decoration,
            ]))
        }
    }

    private mutating func diagram(_ source: String, _ context: Context, into out: NSMutableAttributedString) {
        diagramSources.append(source)
        switch diagrams.layout(source) {
        case .success(let layout) where layout.scene != nil:
            let attachment = NSTextAttachment()
            attachment.attachmentCell = DiagramCell(scene: layout.scene!, type: layout.type)
            let text = NSMutableAttributedString(attachment: attachment)
            let style = paragraph(context, before: 6, after: 14)
            style.alignment = .center
            text.addAttributes([.paragraphStyle: style, .font: bodyFont, .kannaSource: source], range: NSRange(location: 0, length: text.length))
            out.append(text)
            out.append(NSAttributedString(string: "\n", attributes: [.paragraphStyle: style, .font: bodyFont]))
        case .success(let layout):
            note("Unsupported diagram type: \(layout.type)", color: MarkdownTheme.muted, context, into: out)
            codeBlock(source, context, into: out)
        case .failure(let error):
            note("Diagram error: \(error.message)", color: MarkdownTheme.bad, context, into: out)
            codeBlock(source, context, into: out)
        }
    }

    private func note(_ text: String, color: NSColor, _ context: Context, into out: NSMutableAttributedString) {
        out.append(NSAttributedString(string: text + "\n", attributes: [
            .font: NSFont.systemFont(ofSize: 12.5, weight: .medium), .foregroundColor: color, .paragraphStyle: paragraph(context, after: 0),
        ]))
    }

    private mutating func table(_ table: Markdown.Table, rows: [DiffEntry], _ context: Context, into out: NSMutableAttributedString) {
        let grid = NSTextTable()
        grid.numberOfColumns = max(1, rows.map { $0.node.childCount }.max() ?? 1)
        grid.layoutAlgorithm = .automaticLayoutAlgorithm
        grid.collapsesBorders = true
        grid.hidesEmptyCells = false
        let alignments = table.columnAlignments
        for (rowIndex, row) in rows.enumerated() {
            let rowStart = out.length
            let head = row.node is Markdown.Table.Head
            let cells = Array(row.node.children)
            var before: [any Markup] = []
            if case .changed(let was, _, _) = row { before = Array(was.children) }
            for column in 0..<max(cells.count, before.count) {
                let block = NSTextTableBlock(table: grid, startingRow: rowIndex, rowSpan: 1, startingColumn: column, columnSpan: 1)
                block.setWidth(1, type: .absoluteValueType, for: .border)
                block.setBorderColor(MarkdownTheme.line)
                block.setWidth(6, type: .absoluteValueType, for: .padding, edge: .minY)
                block.setWidth(6, type: .absoluteValueType, for: .padding, edge: .maxY)
                block.setWidth(12, type: .absoluteValueType, for: .padding, edge: .minX)
                block.setWidth(12, type: .absoluteValueType, for: .padding, edge: .maxX)
                if head { block.backgroundColor = MarkdownTheme.panel }
                let style = NSMutableParagraphStyle()
                style.textBlocks = [block]
                style.lineSpacing = 2
                switch column < alignments.count ? alignments[column] : nil {
                case .center: style.alignment = .center
                case .right: style.alignment = .right
                default: style.alignment = .left
                }
                let font = head ? NSFontManager.shared.convert(bodyFont, toHaveTrait: .boldFontMask) : bodyFont
                let renderer = self
                let cell = { (node: (any Markup)?) in
                    let text = renderer.inline(node.map { Array($0.children) } ?? [], renderer.attributes(context, font: font))
                    text.append(NSAttributedString(string: "\n", attributes: [.font: font]))
                    text.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: text.length))
                    return text
                }
                let now = column < cells.count ? cells[column] : nil
                if case .changed = row {
                    let was = column < before.count ? before[column] : nil
                    out.append(Self.compose(cell(was), cell(now)) ?? cell(now))
                } else {
                    out.append(cell(now))
                }
            }
            let range = NSRange(location: rowStart, length: out.length - rowStart)
            switch row {
            case .added: Self.mark(.added, range, in: out)
            case .removed: Self.mark(.removed, range, in: out)
            case .changed: Self.mark(.changed, range, in: out)
            case .same: break
            }
        }
        out.append(NSAttributedString(string: "\n", attributes: [.font: NSFont.systemFont(ofSize: 4), .paragraphStyle: paragraph(context, after: 6)]))
    }

    private func inline(_ children: some Sequence<any Markup>, _ base: [NSAttributedString.Key: Any]) -> NSMutableAttributedString {
        let out = NSMutableAttributedString()
        for child in children { inline(child, base, into: out) }
        return out
    }

    private func inline(_ markup: any Markup, _ base: [NSAttributedString.Key: Any], into out: NSMutableAttributedString) {
        var attributes = base
        let font = base[.font] as? NSFont ?? bodyFont
        switch markup {
        case let text as Markdown.Text:
            out.append(NSAttributedString(string: text.string, attributes: base))
            return
        case is SoftBreak:
            out.append(NSAttributedString(string: " ", attributes: base))
            return
        case is LineBreak:
            out.append(NSAttributedString(string: "\u{2028}", attributes: base))
            return
        case let code as InlineCode:
            attributes[.font] = NSFont.monospacedSystemFont(ofSize: font.pointSize * 0.9, weight: .regular)
            attributes[.backgroundColor] = MarkdownTheme.panel2
            out.append(NSAttributedString(string: code.code, attributes: attributes))
            return
        case let html as InlineHTML:
            out.append(NSAttributedString(string: html.rawHTML, attributes: base))
            return
        case let image as Markdown.Image:
            if let attachment = imageAttachment(image.source) {
                let text = NSMutableAttributedString(attachment: attachment)
                text.addAttribute(.kannaSource, value: image.source ?? "", range: NSRange(location: 0, length: text.length))
                out.append(text)
            } else {
                attributes[.foregroundColor] = MarkdownTheme.muted
                if let source = image.source { attributes[.link] = source }
                out.append(NSAttributedString(string: image.plainText.isEmpty ? (image.source ?? "image") : image.plainText, attributes: attributes))
            }
            return
        case is Emphasis:
            attributes[.font] = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask)
        case is Strong:
            attributes[.font] = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
        case is Strikethrough:
            attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
        case let link as Markdown.Link:
            if let destination = link.destination { attributes[.link] = destination }
        default:
            break
        }
        for child in markup.children { inline(child, attributes, into: out) }
    }

    private func imageAttachment(_ source: String?) -> NSTextAttachment? {
        guard let source, let path = Self.localImagePath(source, folder: folder),
              let image = NSImage(contentsOf: URL(fileURLWithPath: path)) else { return nil }
        let attachment = NSTextAttachment()
        attachment.attachmentCell = ImageCell(image: image)
        return attachment
    }

    nonisolated static func fencingFrontMatter(_ text: String) -> String {
        var lines = text.components(separatedBy: "\n")
        let plain = { (line: String) in line.replacingOccurrences(of: "\r", with: "").trimmingCharacters(in: .whitespaces) }
        guard lines.count > 2, plain(lines[0]) == "---",
              let end = lines.indices.dropFirst().first(where: { ["---", "..."].contains(plain(lines[$0])) }) else { return text }
        let longest = lines[1..<end].map { $0.split(separator: "`", omittingEmptySubsequences: false).count - 1 }.max() ?? 0
        let fence = String(repeating: "`", count: max(3, longest + 1))
        lines[0] = fence + "yaml"
        lines[end] = fence
        return lines.joined(separator: "\n")
    }

    nonisolated static func localImagePath(_ source: String, folder: URL) -> String? {
        var raw = source
        if raw.lowercased().hasPrefix("file:") {
            guard let parts = URLComponents(string: raw), parts.scheme?.lowercased() == "file",
                  (parts.host ?? "").isEmpty || parts.host?.lowercased() == "localhost" else { return nil }
            raw = parts.percentEncodedPath
        } else if raw.range(of: #"^[A-Za-z][A-Za-z0-9+.-]*:"#, options: .regularExpression) != nil {
            return nil
        }
        guard let decoded = raw.removingPercentEncoding, !decoded.isEmpty, !decoded.contains("\0") else { return nil }
        let start = decoded.hasPrefix("/") ? decoded : folder.path + "/" + decoded
        guard let resolved = resolveLocally(start) else { return nil }
        var info = statfs()
        guard statfs(resolved, &info) == 0, info.f_flags & UInt32(MNT_LOCAL) != 0 else { return nil }
        return resolved
    }

    nonisolated private static let automountRoots: Set<String> = ["net", "Network", "home"]

    nonisolated private static func resolveLocally(_ path: String) -> String? {
        var pending = path.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        var done: [String] = []
        var hops = 0
        while !pending.isEmpty {
            let part = pending.removeFirst()
            if part == "." { continue }
            if part == ".." { if !done.isEmpty { done.removeLast() }; continue }
            done.append(part)
            if isAutomount(done) { return nil }
            let current = "/" + done.joined(separator: "/")
            var info = stat()
            guard lstat(current, &info) == 0 else { return nil }
            guard info.st_mode & S_IFMT == S_IFLNK else { continue }
            hops += 1
            guard hops <= 32, let target = try? FileManager.default.destinationOfSymbolicLink(atPath: current) else { return nil }
            done.removeLast()
            let targetParts = target.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
            if target.hasPrefix("/") { done = [] }
            pending = targetParts + pending
        }
        return "/" + done.joined(separator: "/")
    }

    nonisolated private static func isAutomount(_ parts: [String]) -> Bool {
        var rest = parts[...]
        if rest.starts(with: ["System", "Volumes", "Data"]) { rest = rest.dropFirst(3) }
        if rest.first == "private" { rest = rest.dropFirst() }
        guard let root = rest.first else { return false }
        return automountRoots.contains(root) && rest.count >= 2
    }

    private mutating func slug(_ text: String) -> String {
        let base = Self.slugBase(text)
        let count = slugs[base, default: 0]
        slugs[base] = count + 1
        return count == 0 ? base : "\(base)-\(count)"
    }

    static func slugBase(_ text: String) -> String {
        text.lowercased().replacingOccurrences(of: #"[^\p{L}\p{M}\p{N} _-]"#, with: "", options: .regularExpression)
            .replacingOccurrences(of: " ", with: "-")
    }
}
