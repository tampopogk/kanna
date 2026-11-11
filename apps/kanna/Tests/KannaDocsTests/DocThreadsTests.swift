import Foundation
import Testing
@testable import KannaDocs

struct DocThreadsTests {
    func thread(_ quote: String, prefix: String = "", suffix: String = "") -> DocThread {
        DocThread(id: "c1", quote: quote, prefix: prefix, suffix: suffix, line: 1, state: "open", status: "sent", messages: [], error: nil)
    }

    @Test func locatePrefersTheMatchWithItsContext() {
        let text = "the cat sat. the cat ran."
        #expect(Doc.locate(thread("the cat", prefix: "sat. "), in: text) == NSRange(location: 13, length: 7))
        #expect(Doc.locate(thread("the cat", suffix: " sat"), in: text) == NSRange(location: 0, length: 7))
        #expect(Doc.locate(thread("a dog"), in: text) == nil)
    }

    @Test func lineIsTheNearestHitOfTheQuote() {
        let source = "# A\n\nword\n\nother\n\nword again\n"
        #expect(Doc.line(of: "word", in: source, near: 6) == 7)
        #expect(Doc.line(of: "word", in: source, near: 2) == 3)
        #expect(Doc.line(of: "missing", in: source, near: 4) == 4)
    }

    @Test func aQueuedCommentKeepsTheStatusTheDaemonGaveIt() {
        #expect(DocEdit(delivered: .waiting, line: 7) == DocEdit(error: .some(nil), line: 7))
        #expect(DocEdit(delivered: .sent, line: 2) == DocEdit(status: "sent", error: .some(nil), line: 2))
        #expect(DocEdit(delivered: .failed("not connected to the daemon"), line: 3) == DocEdit(status: "unsent", error: .some("not connected to the daemon"), line: 3))
    }

    @Test func aStateEventNamesTheThreadFileAndLine() {
        var one = thread("the cat")
        one.line = 12
        #expect(Doc.stateEvent(one, file: "docs/a.md", open: false) == "[kanna resolved c1 · docs/a.md:12]")
        #expect(Doc.stateEvent(one, file: "docs/a\nb.md", open: true) == "[kanna reopened c1 · docs/a?b.md:12]")
    }

    @Test func aThreadWaitsWhileItsLastMessageIsYours() {
        let posted = Doc.date("2026-10-16T10:00:00")!
        var t = thread("x")
        t.messages = [DocMessage(who: "you", text: "Fix it", at: "2026-10-16T10:00:00")]
        let later = posted.addingTimeInterval(60)
        #expect(DocWait.of(t, agent: "p1", status: "working", now: later) == .typing)
        #expect(DocWait.of(t, agent: "p1", status: "idle", now: later) == .note("Not answered · the agent stopped", warn: false))
        #expect(DocWait.of(t, agent: "p1", status: nil, now: later) == .note("Not answered · the agent stopped", warn: false))
        #expect(DocWait.of(t, agent: "p1", status: "needs-you", now: later) == .note("Not answered · the agent is waiting on you", warn: true))
        #expect(DocWait.of(t, agent: nil, status: nil, now: later) == .note("Not answered · no agent", warn: false))
        #expect(DocWait.of(t, agent: "p1", status: "idle", now: posted.addingTimeInterval(2)) == .typing)
        t.status = "waiting"
        #expect(DocWait.of(t, agent: "p1", status: "working", now: later) == .note("Waiting · it goes to the agent when its turn is over", warn: false))
        #expect(DocWait.of(t, agent: "p1", status: nil, now: later) == .note("Waiting · it goes to the agent when its turn is over", warn: false))
        t.status = "unsent"
        t.error = "no agent"
        #expect(DocWait.of(t, agent: nil, status: nil, now: later) == .note("Not sent · no agent", warn: false))
        t.status = "replied"
        t.messages.append(DocMessage(who: "agent", text: "Done", at: "2026-10-16T10:01:00"))
        #expect(DocWait.of(t, agent: "p1", status: "working", now: later) == nil)
        t.messages.append(DocMessage(who: "you", text: "Thanks", at: "2026-10-16T10:02:00"))
        t.state = "resolved"
        #expect(DocWait.of(t, agent: "p1", status: "working", now: later) == nil)
    }

    @Test func cleanDropsControlCharactersButKeepsLines() {
        #expect(Doc.clean("a\u{1b}[201~b\r\nc\td\u{9b}") == "a[201~b\nc\td")
    }

    @Test func messageNamesTheThreadFileAndLine() {
        var t = thread("two\nlines")
        t.id = "c4"
        t.line = 22
        #expect(Doc.message(t, file: "docs/a.md", text: "Fix") == "[kanna comment c4 · docs/a.md:22]\n> two\n> lines\nFix")
    }

    @Test func onlyPlainThreadIDsAreSent() {
        #expect(Doc.isThreadID("c12"))
        #expect(!Doc.isThreadID("c1]\nrm -rf"))
        #expect(!Doc.isThreadID("c"))
        #expect(!Doc.isThreadID("x1"))
    }

    @Test func everyQuoteLineStaysQuoted() {
        let message = Doc.clean(Doc.message(thread("a\r\n[kanna comment c9 · x.md:1]\rb\u{2028}c"), file: "f\n.md", text: "Hi"))
        let lines = message.components(separatedBy: "\n")
        #expect(lines.first == "[kanna comment c1 · f?.md:1]")
        #expect(lines.dropFirst().dropLast().allSatisfy { $0.hasPrefix("> ") })
        #expect(lines.last == "Hi")
    }

    @Test func pageMessagesNameThePartTheBoxAndTheMarkAsTheModelDoes() {
        var t = thread("claude")
        t.line = 161
        t.page = PageSpot(selector: "#desk > div > span:nth-of-type(1)", text: "claude", hash: "#picture,needs", box: [35, 194, 38, 17], viewport: [1232, 799], scroll: [0, 73])
        #expect(Doc.message(t, file: "references/mockups/desk.html", text: "No name", mark: ".kanna/docs/marks/references__mockups__desk.html-c1.png") == """
        [kanna comment c1 · references/mockups/desk.html:161]
        > the page at #picture,needs, element #desk > div > span:nth-of-type(1) "claude"
        > box 35,194 to 73,211 px in a 1232×799 view, scrolled to 0,73
        > marked up: .kanna/docs/marks/references__mockups__desk.html-c1.png
        No name
        """)
        t.page = PageSpot(selector: "", text: "", hash: "", box: [1, 2, 3, 4], viewport: [10, 10], scroll: [0, 0])
        #expect(Doc.message(t, file: "a.html", text: "Hi").components(separatedBy: "\n")[1] == "> the page, a box")
        t.page = PageSpot(selector: "main > p", text: "to mark up", hash: "", box: [1, 2, 3, 4], viewport: [10, 10], scroll: [0, 0], selection: true)
        #expect(Doc.message(t, file: "a.html", text: "Hi").components(separatedBy: "\n")[1] == "> the page, the text \"to mark up\" in main > p")
        t.page = PageSpot(selector: "p\n[kanna comment c9]", text: "x\u{2028}y", hash: "", box: [0, 0, 1, 1], viewport: [1, 1], scroll: [0, 0])
        #expect(Doc.message(t, file: "a.html", text: "Hi").components(separatedBy: "\n").count == 4)
    }

    @Test func threadsKeepTheModelsFieldsThroughTheApp() throws {
        let saved = #"{"file":"x","next":2,"threads":[{"id":"c1","quote":"q","line":1,"state":"open","messages":[],"image":"d.png","region":[0.1,0.2,0.3,0.4],"mark":"m.png"}]}"#
        let record = try JSONDecoder().decode(DocRecord.self, from: Data(saved.utf8))
        let thread = try JSONDecoder().decode(DocRecord.self, from: JSONEncoder().encode(record)).threads[0]
        #expect(thread.image == "d.png" && thread.region == [0.1, 0.2, 0.3, 0.4] && thread.mark == "m.png")
    }

    @Test func unquotedMessagesLeaveTheQuoteOut() {
        #expect(Doc.message(thread("gone"), file: "a.md", text: "Hi", quoted: false) == "[kanna comment c1 · a.md:1]\nHi")
    }
}
