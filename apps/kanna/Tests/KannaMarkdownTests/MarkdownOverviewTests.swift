import AppKit
import Markdown
import Testing
import KannaOverview
@testable import KannaMarkdown

@MainActor
struct MarkdownOverviewTests {
    let digit = MarkdownOverviewLayout.digit

    func cutoff() -> CGFloat {
        var width: CGFloat = 0
        while MarkdownOverviewLayout.column(width) < OverviewMetrics.columns * digit { width += 1 }
        return width + OverviewMetrics.width
    }

    func shown(_ width: CGFloat, comments: Bool = false, forced: Bool = false) -> (overview: Bool, comments: Bool) {
        MarkdownOverviewLayout.shown(overview: true, comments: comments, forced: forced, width: width, digit: digit)
    }

    @Test func eightyColumnsAreCountedInTheTextColumn() {
        let width = cutoff()
        #expect(shown(width).overview)
        #expect(!shown(width - 1).overview)
        #expect(MarkdownOverviewLayout.column(width - OverviewMetrics.width) >= 80 * digit)
    }

    @Test func aWidePaneGivesTheOverviewOnlyMargin() {
        let room: CGFloat = 1400
        #expect(MarkdownOverviewLayout.column(room) == MarkdownTheme.maxTextWidth)
        #expect(MarkdownOverviewLayout.column(room - OverviewMetrics.width) == MarkdownTheme.maxTextWidth)
    }

    @Test func theDocIsMeasuredAtFullSizeAtItsWidth() {
        let paragraph = String(repeating: "The redirect is checked before the app trusts it. ", count: 40)
        let storage = NSTextStorage(string: paragraph, attributes: [.font: NSFont.systemFont(ofSize: MarkdownTheme.bodySize)])
        let layout = MarkdownFullSizeLayout(storage: storage, padding: 5)
        let wide = layout.height(key: [1], width: 1200, insetHeight: 22)
        let narrow = layout.height(key: [1], width: 600, insetHeight: 22)
        #expect(narrow > wide)
        #expect(wide > 44)
        storage.append(NSAttributedString(string: paragraph, attributes: [.font: NSFont.systemFont(ofSize: MarkdownTheme.bodySize)]))
        #expect(layout.height(key: [1], width: 600, insetHeight: 22) == narrow)
        #expect(layout.height(key: [2], width: 600, insetHeight: 22) > narrow)
        layout.detach()
        #expect(storage.layoutManagers.isEmpty)
    }

    @Test func itShowsForADocTallerThanThePaneAtFullSize() {
        #expect(MarkdownOverviewLayout.wanted(enabled: true, contentHeight: 701, paneHeight: 700))
        #expect(!MarkdownOverviewLayout.wanted(enabled: true, contentHeight: 700, paneHeight: 700))
        #expect(!MarkdownOverviewLayout.wanted(enabled: false, contentHeight: 5000, paneHeight: 700))
    }

    @Test func theCommentsListLeavesTheOverviewAlone() {
        for width in stride(from: CGFloat(1600), through: 300, by: -10) {
            #expect(shown(width, comments: true).overview == shown(width).overview)
            #expect(shown(width, comments: true, forced: true).overview == shown(width).overview)
        }
    }

    @Test func theListShowsAtTwiceItsWidthAlone() {
        #expect(MarkdownOverviewLayout.shown(overview: false, comments: true, forced: false, width: 560, digit: digit).comments)
        #expect(!MarkdownOverviewLayout.shown(overview: false, comments: true, forced: false, width: 559, digit: digit).comments)
    }

    @Test func aListOpenedOnPurposeStays() {
        let now = shown(400, comments: true, forced: true)
        #expect(now.comments)
        #expect(!now.overview)
    }

    @Test func thePictureKeyChangesWithTheLook() {
        let key = MarkdownPictureKey(revision: 1, width: 900, height: 4000, dark: true, comments: [])
        var reload = key
        reload.revision += 1
        var wider = key
        wider.width = 1000
        var taller = key
        taller.height = 4200
        var light = key
        light.dark = false
        var commented = key
        commented.comments = MarkdownPictureKey.comments([DocMark(range: NSRange(location: 4, length: 3), kind: .open)])
        for changed in [reload, wider, taller, light, commented] { #expect(changed != key) }
        var resolved = commented
        resolved.comments = MarkdownPictureKey.comments([DocMark(range: NSRange(location: 4, length: 3), kind: .resolved)])
        #expect(resolved != commented)
    }

    func laidOut(_ old: String?, _ new: String) -> (MarkdownLayoutManager, NSTextContainer, NSTextStorage) {
        var renderer = MarkdownRenderer(folder: FileManager.default.temporaryDirectory, diagrams: DiagramCache())
        let text = renderer.render(MarkdownDiff.entries(old: old.map { Document(parsing: $0) }, new: Document(parsing: new)))
        let storage = NSTextStorage(attributedString: text)
        let manager = MarkdownLayoutManager()
        storage.addLayoutManager(manager)
        let container = NSTextContainer(size: NSSize(width: 600, height: CGFloat.greatestFiniteMagnitude))
        manager.addTextContainer(container)
        manager.ensureLayout(for: container)
        return (manager, container, storage)
    }

    func used(_ manager: MarkdownLayoutManager, _ storage: NSTextStorage, _ needle: String) -> NSRect {
        let range = (storage.string as NSString).range(of: needle)
        var rect = NSRect.null
        manager.enumerateLineFragments(forGlyphRange: manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil)) { _, used, _, _, _ in rect = rect.union(used) }
        return rect
    }

    @Test func changesAreMarkedAtTheirLines() {
        let old = "# Doc\n\nKeep.\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | old |\n"
        let new = "# Doc\n\n## Added heading\n\nKeep.\n\n- one\n\n| a | b |\n|---|---|\n| 1 | new |\n"
        let (manager, _, storage) = laidOut(old, new)
        let marks = MarkdownOverviewMarks.changes(in: storage) { manager.lineRect(for: $0) }
        let kinds = Set(marks.map(\.kind))
        #expect(kinds == [.added, .removed, .changed])
        let heading = used(manager, storage, "Added heading")
        let added = marks.first { $0.kind == .added }!
        #expect(added.y <= heading.minY + 0.5 && added.y + added.height >= heading.maxY - 0.5)
        let two = used(manager, storage, "two")
        let removed = marks.first { $0.kind == .removed }!
        #expect(removed.y <= two.minY + 0.5 && removed.y + removed.height >= two.maxY - 0.5)
        let keep = used(manager, storage, "Keep.")
        #expect(!marks.contains { $0.y < keep.maxY - 0.5 && $0.y + $0.height > keep.minY + 0.5 })
    }

    @Test func aCommentIsMarkedAtItsText() {
        let (manager, _, storage) = laidOut(nil, "# Doc\n\nFirst paragraph.\n\nCheck it against the allow list.\n")
        let range = (storage.string as NSString).range(of: "allow list")
        let marks = MarkdownOverviewMarks.ranges([range], kind: .comment, color: .systemBlue) { manager.lineRect(for: $0) }
        let line = used(manager, storage, "allow list")
        #expect(marks.count == 1)
        #expect(marks[0].kind == .comment && abs(marks[0].y - line.minY) < 0.5 && abs(marks[0].height - line.height) < 0.5)
    }

    @Test func thePictureIsTheThumbnailsWidth() {
        let (manager, container, storage) = laidOut(nil, (1...60).map { "Paragraph \($0) of the doc." }.joined(separator: "\n\n"))
        let height = manager.usedRect(for: container).height
        #expect(storage.length > 0)
        guard let (image, scale) = manager.picture(origin: NSPoint(x: 28, y: 22), contentHeight: height, backing: 2, appearance: NSAppearance(named: .darkAqua)!) else {
            Issue.record("no picture")
            return
        }
        #expect(image.width == Int(OverviewMetrics.thumbWidth * 2))
        #expect(abs(scale - OverviewMetrics.thumbWidth / (container.size.width - container.lineFragmentPadding * 2 + MarkdownOverviewLayout.gutter)) < 0.001)
        #expect(abs(CGFloat(image.height) - height * scale * 2) <= 1)
    }
}
