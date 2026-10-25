import AppKit
import Markdown
import Testing
@testable import KannaMarkdown

@MainActor
struct MarkdownDiffTests {
    func diff(_ old: String?, _ new: String) -> [DiffEntry] {
        MarkdownDiff.entries(old: old.map { Document(parsing: $0) }, new: Document(parsing: new))
    }

    func outline(_ entries: [DiffEntry]) -> [String] {
        entries.map { entry in
            let name = String(describing: type(of: entry.node)).components(separatedBy: ".").last!
            switch entry {
            case .same: return "= \(name)"
            case .added: return "+ \(name)"
            case .removed: return "- \(name)"
            case .changed(_, _, nil): return "~ \(name)"
            case .changed(_, _, let children?): return "~ \(name) [\(outline(children).joined(separator: ", "))]"
            }
        }
    }

    func render(_ old: String?, _ new: String) -> NSAttributedString {
        var renderer = MarkdownRenderer(folder: FileManager.default.temporaryDirectory, diagrams: DiagramCache())
        return renderer.render(diff(old, new))
    }

    func runs(_ text: NSAttributedString, _ key: NSAttributedString.Key) -> [String] {
        var out: [String] = []
        text.enumerateAttribute(key, in: NSRange(location: 0, length: text.length)) { value, range, _ in
            if value != nil { out.append((text.string as NSString).substring(with: range)) }
        }
        return out
    }

    @Test func anAddedParagraph() {
        #expect(outline(diff("# Title\n\nFirst.\n", "# Title\n\nFirst.\n\nA new paragraph.\n")) == ["= Heading", "= Paragraph", "+ Paragraph"])
    }

    @Test func aRemovedParagraph() {
        #expect(outline(diff("# Title\n\nFirst.\n\nGone soon.\n\nLast.\n", "# Title\n\nFirst.\n\nLast.\n")) == ["= Heading", "= Paragraph", "- Paragraph", "= Paragraph"])
    }

    @Test func aRewordedParagraphIsChangedWordByWord() {
        let old = "The quick brown fox jumps over the lazy dog.\n", new = "The quick red fox leaps over the lazy dog.\n"
        #expect(outline(diff(old, new)) == ["~ Paragraph"])
        let text = render(old, new)
        #expect(text.string == "The quick brownred fox jumpsleaps over the lazy dog.")
        #expect(runs(text, .strikethroughStyle) == ["brown", "jumps"])
        #expect(runs(text, .backgroundColor) == ["red", "leaps"])
        #expect(MarkdownPaneView.changes(in: text).count == 1)
    }

    @Test func anUnrelatedParagraphIsRemovedAndAdded() {
        #expect(outline(diff("Alpha beta gamma delta.\n", "Something else entirely here.\n")) == ["- Paragraph", "+ Paragraph"])
    }

    @Test func listItems() {
        #expect(outline(diff("- one\n- two\n- three\n", "- one\n- three\n- four\n"))
            == ["~ UnorderedList [= ListItem, - ListItem, = ListItem, + ListItem]"])
        #expect(outline(diff("1. install the app\n2. run it\n", "1. install the new app\n2. run it\n"))
            == ["~ OrderedList [~ ListItem [~ Paragraph], = ListItem]"])
    }

    @Test func aRemovedListItemShowsWholeWithItsMarker() {
        let text = render("1. one\n2. two\n3. three\n", "1. one\n2. three\n")
        #expect(text.string == "1.\tone\n2.\ttwo\n2.\tthree")
        #expect(runs(text, .strikethroughStyle) == ["2.\ttwo"])
    }

    @Test func tableRows() {
        let old = "| a | b |\n|---|---|\n| 1 | one |\n| 2 | two |\n| 3 | three |\n"
        let new = "| a | b |\n|---|---|\n| 1 | one |\n| 3 | three point five |\n| 4 | four |\n"
        #expect(outline(diff(old, new)) == ["~ Table [= Head, = Row, - Row, ~ Row, + Row]"])
        let text = render(old, new)
        #expect(runs(text, .strikethroughStyle) == ["2", "two"])
        #expect(runs(text, .backgroundColor).contains(" point five"))
        #expect(MarkdownPaneView.changes(in: text).count == 1)
    }

    @Test func headings() {
        #expect(outline(diff("# Kanna\n\n## Old name\n\nText.\n", "# Kanna\n\n## New name\n\nText.\n")) == ["= Heading", "~ Heading", "= Paragraph"])
        #expect(outline(diff("# Kanna\n\nText.\n", "# Kanna\n\n## Added section\n\nText.\n")) == ["= Heading", "+ Heading", "= Paragraph"])
        #expect(outline(diff("# Kanna\n\n## Going\n\nText.\n", "# Kanna\n\nText.\n")) == ["= Heading", "- Heading", "= Paragraph"])
    }

    @Test func aRenamedHeadingKeepsItsAnchor() {
        let text = render("## Old name\n", "## New name\n")
        #expect(runs(text, .kannaHeading).allSatisfy { !$0.isEmpty })
        var slugs = Set<String>()
        text.enumerateAttribute(.kannaHeading, in: NSRange(location: 0, length: text.length)) { value, _, _ in if let value = value as? String { slugs.insert(value) } }
        #expect(slugs == ["new-name"])
    }

    @Test func aChangedDiagramShowsBeforeAndAfter() {
        let old = "```mermaid\ngraph TD\n  A --> B\n```\n", new = "```mermaid\ngraph TD\n  A --> C\n```\n"
        #expect(outline(diff(old, new)) == ["~ CodeBlock"])
        let text = render(old, new).string
        #expect(text.components(separatedBy: "\u{FFFC}").count == 3)
        #expect(text.hasPrefix("Before\n\u{FFFC}"))
        #expect(text.contains("After\n\u{FFFC}"))
    }

    @Test func aNewFileIsAllAdded() {
        #expect(outline(diff(nil, "# New\n\nText.\n\n- item\n")) == ["+ Heading", "+ Paragraph", "+ UnorderedList"])
        let text = render(nil, "# New\n\nText.\n")
        #expect(MarkdownPaneView.changes(in: text).count == 1)
        #expect(runs(text, .backgroundColor) == ["New", "Text."])
    }

    @Test func noChanges() {
        let text = render("# Same\n\nText.\n", "# Same\n\nText.\n")
        #expect(MarkdownPaneView.changes(in: text).isEmpty)
        #expect(text.string == "Same\nText.")
    }

    @Test func separateChangesCountApart() {
        let old = "One.\n\nTwo.\n\nThree.\n\nFour.\n", new = "One changed.\n\nTwo.\n\nThree.\n\nFour changed.\n"
        #expect(MarkdownPaneView.changes(in: render(old, new)).count == 2)
    }

    @Test func wordTokens() {
        let tokens = MarkdownDiff.tokens(in: "it's a  test-case\n" as NSString).map(\.key)
        #expect(tokens == ["it", "'", "s", " ", "a", "  ", "test", "-", "case", "\n"])
    }

    @Test func spacesBetweenChangesJoinThem() {
        let old = MarkdownDiff.tokens(in: "keep one two keep" as NSString), new = MarkdownDiff.tokens(in: "keep three four keep" as NSString)
        #expect(MarkdownDiff.words(old, new) == [.same(NSRange(location: 0, length: 4), NSRange(location: 0, length: 4)),
                                                 .same(NSRange(location: 4, length: 1), NSRange(location: 4, length: 1)),
                                                 .removed(NSRange(location: 5, length: 7)), .added(NSRange(location: 5, length: 10)),
                                                 .same(NSRange(location: 12, length: 1), NSRange(location: 15, length: 1)),
                                                 .same(NSRange(location: 13, length: 4), NSRange(location: 16, length: 4))])
    }

    @Test func aHugeRewriteStaysCheap() {
        let old = (0..<2500).map { "Old paragraph \($0).\n" }.joined(separator: "\n"), new = (0..<2500).map { "New line \($0) here.\n" }.joined(separator: "\n")
        let start = Date()
        let entries = diff(old, new)
        #expect(entries.count == 5000)
        #expect(Date().timeIntervalSince(start) < 5)
    }

    @Test func aRangeNamesTwoCommits() {
        #expect(MarkdownDiff.range("v1..main")! == ("v1", "main"))
        #expect(MarkdownDiff.range("v1..")! == ("v1", "HEAD"))
        #expect(MarkdownDiff.range("..work")! == ("HEAD", "work"))
        #expect(MarkdownDiff.range("main") == nil)
        #expect(MarkdownDiff.range("main...work") == nil)
    }

    @Test func aRangeReadsBothVersionsFromGit() throws {
        let repo = FileManager.default.temporaryDirectory.appendingPathComponent("kanna-range-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: repo, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: repo) }
        let git = { (args: [String]) in MarkdownDiff.git(repo.path, ["-c", "user.name=t", "-c", "user.email=t@example.invalid"] + args) }
        let doc = repo.appendingPathComponent("doc.md"), path = doc.path
        _ = git(["init", "-q", "-b", "main"])
        try "One.\n".write(to: doc, atomically: true, encoding: .utf8)
        _ = git(["add", "doc.md"]); _ = git(["commit", "-qm", "a"]); _ = git(["tag", "a"])
        try "Two.\n".write(to: doc, atomically: true, encoding: .utf8)
        _ = git(["commit", "-qam", "b"]); _ = git(["tag", "b"])
        try "On disk.\n".write(to: doc, atomically: true, encoding: .utf8)
        let both = try MarkdownDiff.versions(of: path, from: "a", to: "b").get()
        #expect(both.old == "One.\n" && both.new == "Two.\n")
        try "x\n".write(to: repo.appendingPathComponent("later.md"), atomically: true, encoding: .utf8)
        _ = git(["add", "later.md"]); _ = git(["commit", "-qm", "c"])
        #expect(try MarkdownDiff.versions(of: repo.appendingPathComponent("later.md").path, from: "a", to: "HEAD").get().old == nil)
        #expect(throws: DiffFailure(message: "later.md isn't in b")) { try MarkdownDiff.versions(of: repo.appendingPathComponent("later.md").path, from: "a", to: "b").get() }
        #expect(throws: DiffFailure(message: "no commit or branch \"nope\" in its repo")) { try MarkdownDiff.versions(of: path, from: "nope", to: "b").get() }
    }
}
