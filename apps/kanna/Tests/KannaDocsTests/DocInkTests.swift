import Foundation
import Testing
@testable import KannaDocs

struct DocInkTests {
    static let title = InkPlace(line: 3, text: "A tab container is a slot in a split that holds several panes as tabs and shows one at a time.")
    static let panesOnly = InkPlace(line: 7, text: "Panes only: a container holds panes, not splits or other containers.")
    static let click = InkPlace(line: 14, text: "Click a tab to show it; middle-click or × to close it.")
    static let inside = InkPlace(line: 23, text: "The panes inside stay ordinary panes, with their own IDs, names and processes, so every pane command still works on them.")

    static let specInk = DocInk(marks: [
        InkMark(kind: .circle, strokes: [[[2, 1], [80, 0], [84, 14], [1, 16], [2, 1]]], anchor: panesOnly, words: "Panes only:"),
        InkMark(kind: .writing, strokes: [[[96, 0], [102, 4], [98, 10]], [[98, 16], [98, 17]]], anchor: panesOnly),
        InkMark(kind: .arrow, strokes: [[[-12, 4], [-30, -200], [-10, -390]]], anchor: inside, to: title, side: .after, tip: 2, note: "say this up front"),
        InkMark(kind: .strike, strokes: [[[180, 8], [360, 9]]], anchor: click, words: "middle-click or × to close it."),
    ])

    func thread(_ ink: DocInk, id: String = "c2") -> DocThread {
        DocThread(id: id, quote: "", prefix: nil, suffix: nil, line: ink.line, state: "open", status: "sent", messages: [], error: nil, ink: ink)
    }

    @Test func theSpecsExampleIsTypedAsTheSpecShowsIt() {
        let message = DocStore.message(thread(Self.specInk), file: "docs/specs/tab-container.md", text: "",
                                       mark: ".kanna/docs/marks/docs__specs__tab-container.md-c2.png")
        #expect(message == """
        [kanna comment c2 · docs/specs/tab-container.md:7]
        > ink, 4 marks, numbered in the picture:
        > 1 circle around "Panes only:" (line 7)
        > 2 writing beside line 7, read it in the picture
        > 3 arrow from "The panes inside stay ordinary pa…" (line 23) to after "A tab container is a slot in a sp…" (line 3)
        >   note: say this up front
        > 4 strike through "middle-click or × to close it." (line 14)
        > marked up: .kanna/docs/marks/docs__specs__tab-container.md-c2.png
        """)
    }

    @Test func pageMarksNamePartsByCSSPathAndPagePixels() {
        let name = InkPlace(line: 15, text: "Name Jeremy Hale", selector: "#settings .row:nth-of-type(1)", box: [40, 100, 600, 40])
        let theme = InkPlace(line: 18, text: "Theme Dark", selector: "#settings .row:nth-of-type(3)", box: [40, 180, 600, 40])
        let save = InkPlace(line: 33, text: "Save", selector: "#settings .btns .btn:nth-of-type(2)", box: [560, 300, 80, 30])
        let ink = DocInk(view: InkView(hash: "#general", viewport: [1220, 816], scroll: [0, 0]), marks: [
            InkMark(kind: .circle, strokes: [[[-6, -4], [90, -2], [86, 36], [-4, 34]]], anchor: save, words: "Save", note: "make this the default"),
            InkMark(kind: .underline, strokes: [[[60, 30], [200, 31]]], anchor: name, words: "Jeremy Hale"),
            InkMark(kind: .arrow, strokes: [[[-10, 20], [-30, -60]]], anchor: theme, to: name, side: .before),
            InkMark(kind: .writing, strokes: [[[620, 4], [640, 12]], [[630, 20], [650, 24]]], anchor: name),
            InkMark(kind: .cross, strokes: [[[700, 0], [720, 20]], [[720, 0], [700, 20]]], anchor: theme),
        ])
        let lines = DocStore.message(thread(ink, id: "c1"), file: "references/mockups/settings.html", text: "",
                                     mark: ".kanna/docs/marks/references__mockups__settings.html-c1.png").components(separatedBy: "\n")
        #expect(lines == [
            "[kanna comment c1 · references/mockups/settings.html:33]",
            "> the page at #general, in a 1220×816 view, scrolled to 0,0",
            "> ink, 5 marks, numbered in the picture:",
            "> 1 circle around element #settings .btns .btn:nth-of-type(2) \"Save\" at 560,300 to 640,330 px",
            ">   note: make this the default",
            "> 2 underline under element #settings .row:nth-of-type(1) \"Name Jeremy Hale\", the text \"Jeremy Hale\" at 40,100 to 640,140 px",
            "> 3 arrow from element #settings .row:nth-of-type(3) \"Theme Dark\" to before element #settings .row:nth-of-type(1) \"Name Jeremy Hale\"",
            "> 4 writing near element #settings .row:nth-of-type(1) \"Name Jeremy Hale\" at 660,104 to 690,124 px, read it in the picture",
            "> 5 cross near element #settings .row:nth-of-type(3) \"Theme Dark\" at 740,180 to 760,200 px",
            "> marked up: .kanna/docs/marks/references__mockups__settings.html-c1.png",
        ])
    }

    @Test func oneMarkAroundNothingAndAReplyUnderIt() {
        let ink = DocInk(marks: [InkMark(kind: .circle, strokes: [[[0, 0], [10, 10]]], anchor: Self.click, note: "two\nlines\u{1b}[2J")])
        #expect(DocStore.message(thread(ink), file: "a.md", text: "And this?") == """
        [kanna comment c2 · a.md:14]
        > ink, 1 mark, numbered in the picture:
        > 1 circle at line 14, around nothing
        >   note: two lines?[2J
        And this?
        """)
    }

    @Test func inkThreadsRoundTripThroughTheThreadFile() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("kanna-ink-\(UUID().uuidString)").resolvingSymlinksInPath()
        try FileManager.default.createDirectory(at: root.appendingPathComponent(".git"), withIntermediateDirectories: true)
        let store = DocStore(file: root.appendingPathComponent("docs/specs/tab-container.md").path)
        try store.update { record in
            record.threads.append(thread(Self.specInk))
            record.next = 3
        }
        let saved = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: store.path))) as! [String: Any]
        let first = (saved["threads"] as! [[String: Any]])[0]
        let marks = (first["ink"] as! [String: Any])["marks"] as! [[String: Any]]
        #expect(first["quote"] as? String == "")
        #expect(marks.map { $0["kind"] as? String } == ["circle", "writing", "arrow", "strike"])
        #expect(marks[2]["side"] as? String == "after" && marks[2]["note"] as? String == "say this up front")
        #expect((marks[0]["anchor"] as! [String: Any])["line"] as? Int == 7)
        #expect(store.read().threads == [thread(Self.specInk)])
    }

    @Test func oldThreadsLoadWithoutInkAndUnknownKindsStillLoad() throws {
        let old = #"{"file":"x","next":2,"threads":[{"id":"c1","quote":"q","line":1,"state":"open","messages":[]}]}"#
        let record = try JSONDecoder().decode(DocRecord.self, from: Data(old.utf8))
        #expect(record.threads[0].ink == nil && record.threads[0].quote == "q")
        let later = #"{"file":"x","next":2,"threads":[{"id":"c1","quote":"","line":4,"state":"open","messages":[],"ink":{"marks":[{"kind":"lasso","strokes":[[[1,2]]],"anchor":{"line":4,"text":"t"},"side":"over"}]}}]}"#
        let mark = try JSONDecoder().decode(DocRecord.self, from: Data(later.utf8)).threads[0].ink!.marks[0]
        #expect(mark.kind == .writing && mark.side == .after && mark.strokes == [[[1, 2]]])
    }

    @Test func aMarkLeavesWhenTheBlockUnderItChanges() {
        let blocks = [Self.title, Self.panesOnly, Self.click, Self.inside].map { InkBlock(line: $0.line, text: $0.text) }
        #expect(Self.specInk.marks.allSatisfy { DocInk.stays($0, in: blocks) })
        var edited = blocks
        edited[2].text = "Click a tab to show it."
        edited.remove(at: 3)
        edited.insert(InkBlock(line: 5, text: Self.inside.text), at: 1)
        #expect(Self.specInk.marks.map { DocInk.stays($0, in: edited) } == [true, true, false, false])
        let shifted = blocks.map { InkBlock(line: $0.line + 40, text: $0.text.replacingOccurrences(of: " ", with: "  ")) }
        #expect(Self.specInk.marks.allSatisfy { DocInk.stays($0, in: shifted) })
        #expect(!DocInk.stays(Self.specInk.marks[2], in: blocks.filter { $0.line != 3 }))
    }

    @Test func aBlockIsFoundNearestItsLineAndAPartByItsPath() {
        let blocks = [InkBlock(line: 2, text: "Same"), InkBlock(line: 9, text: "Same"), InkBlock(line: 20, text: "Same")]
        #expect(DocInk.find(InkPlace(line: 12, text: "Same"), in: blocks) == 1)
        #expect(DocInk.find(InkPlace(line: 12, text: "Other"), in: blocks) == nil)
        let row = InkPlace(line: 15, text: "Theme Dark", selector: ".row:nth-of-type(3)")
        #expect(DocInk.find(row, in: [InkBlock(line: 15, text: "Theme Dark")]) == nil)
        #expect(DocInk.find(row, in: [InkBlock(line: 15, text: "Theme Dark", selector: ".row:nth-of-type(3)")]) == 0)
        #expect(!DocInk.stays(InkMark(kind: .cross, strokes: [], anchor: row), in: [InkBlock(line: 15, text: "Name", selector: ".row:nth-of-type(3)")]))
    }
}
