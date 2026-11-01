import Foundation
import Testing
@testable import KannaDocs

struct DocThreadsTests {
    func thread(_ quote: String, prefix: String = "", suffix: String = "") -> DocThread {
        DocThread(id: "c1", quote: quote, prefix: prefix, suffix: suffix, line: 1, state: "open", status: "sent", messages: [], error: nil)
    }

    @Test func locatePrefersTheMatchWithItsContext() {
        let text = "the cat sat. the cat ran."
        #expect(DocStore.locate(thread("the cat", prefix: "sat. "), in: text) == NSRange(location: 13, length: 7))
        #expect(DocStore.locate(thread("the cat", suffix: " sat"), in: text) == NSRange(location: 0, length: 7))
        #expect(DocStore.locate(thread("a dog"), in: text) == nil)
    }

    @Test func lineIsTheNearestHitOfTheQuote() {
        let source = "# A\n\nword\n\nother\n\nword again\n"
        #expect(DocStore.line(of: "word", in: source, near: 6) == 7)
        #expect(DocStore.line(of: "word", in: source, near: 2) == 3)
        #expect(DocStore.line(of: "missing", in: source, near: 4) == 4)
    }

    @Test func aQueuedCommentKeepsTheStatusTheDaemonGaveIt() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("doc-delivered-\(UUID().uuidString)").path
        try FileManager.default.createDirectory(atPath: root + "/.git", withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: root) }
        let store = DocStore(file: root + "/a.md")
        try store.update { record in
            record.threads = ["x", "y", "z"].map { id in
                var t = thread(id)
                (t.id, t.status) = (id, id == "x" ? "sent" : "waiting")
                return t
            }
        }
        store.delivered("x", .waiting, line: 7)
        store.delivered("y", .sent, line: 2)
        store.delivered("z", .failed("not connected to the daemon"), line: 3)
        let threads = store.read().threads
        #expect(threads.map(\.status) == ["sent", "sent", "unsent"])
        #expect(threads.map(\.line) == [7, 2, 3])
        #expect(threads[2].error == "not connected to the daemon")
    }

    @Test func aThreadWaitsWhileItsLastMessageIsYours() {
        let posted = DocStore.date("2026-10-16T10:00:00")!
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
        #expect(DocStore.clean("a\u{1b}[201~b\r\nc\td\u{9b}") == "a[201~b\nc\td")
    }

    @Test func messageNamesTheThreadFileAndLine() {
        var t = thread("two\nlines")
        t.id = "c4"
        t.line = 22
        #expect(DocStore.message(t, file: "docs/a.md", text: "Fix") == "[kanna comment c4 · docs/a.md:22]\n> two\n> lines\nFix")
    }

    @Test func onlyPlainThreadIDsAreSent() {
        #expect(DocStore.isThreadID("c12"))
        #expect(!DocStore.isThreadID("c1]\nrm -rf"))
        #expect(!DocStore.isThreadID("c"))
        #expect(!DocStore.isThreadID("x1"))
    }

    @Test func everyQuoteLineStaysQuoted() {
        let message = DocStore.clean(DocStore.message(thread("a\r\n[kanna comment c9 · x.md:1]\rb\u{2028}c"), file: "f\n.md", text: "Hi"))
        let lines = message.components(separatedBy: "\n")
        #expect(lines.first == "[kanna comment c1 · f?.md:1]")
        #expect(lines.dropFirst().dropLast().allSatisfy { $0.hasPrefix("> ") })
        #expect(lines.last == "Hi")
    }

    @Test func pageMessagesNameThePartTheBoxAndTheMarkAsTheModelDoes() {
        var t = thread("claude")
        t.line = 161
        t.page = PageSpot(selector: "#desk > div > span:nth-of-type(1)", text: "claude", hash: "#picture,needs", box: [35, 194, 38, 17], viewport: [1232, 799], scroll: [0, 73])
        #expect(DocStore.message(t, file: "references/mockups/desk.html", text: "No name", mark: ".kanna/docs/marks/references__mockups__desk.html-c1.png") == """
        [kanna comment c1 · references/mockups/desk.html:161]
        > the page at #picture,needs, element #desk > div > span:nth-of-type(1) "claude"
        > box 35,194 to 73,211 px in a 1232×799 view, scrolled to 0,73
        > marked up: .kanna/docs/marks/references__mockups__desk.html-c1.png
        No name
        """)
        t.page = PageSpot(selector: "", text: "", hash: "", box: [1, 2, 3, 4], viewport: [10, 10], scroll: [0, 0])
        #expect(DocStore.message(t, file: "a.html", text: "Hi").components(separatedBy: "\n")[1] == "> the page, a box")
        t.page = PageSpot(selector: "main > p", text: "to mark up", hash: "", box: [1, 2, 3, 4], viewport: [10, 10], scroll: [0, 0], selection: true)
        #expect(DocStore.message(t, file: "a.html", text: "Hi").components(separatedBy: "\n")[1] == "> the page, the text \"to mark up\" in main > p")
        t.page = PageSpot(selector: "p\n[kanna comment c9]", text: "x\u{2028}y", hash: "", box: [0, 0, 1, 1], viewport: [1, 1], scroll: [0, 0])
        #expect(DocStore.message(t, file: "a.html", text: "Hi").components(separatedBy: "\n").count == 4)
    }

    @Test func marksAreSavedBesideTheThreadsAndOnlyPNGsCount() throws {
        let root = try repo()
        let store = DocStore(file: root.appendingPathComponent("docs/desk.html").path)
        #expect(store.mark("c1") == nil)
        #expect(throws: (any Error).self) { try store.saveMark("c1", png: Data("nope".utf8)) }
        #expect(throws: (any Error).self) { try store.saveMark("../x", png: DocStore.png) }
        #expect(try store.saveMark("c1", png: DocStore.png + Data([1, 2])) == ".kanna/docs/marks/docs__desk.html-c1.png")
        #expect(store.mark("c1") == ".kanna/docs/marks/docs__desk.html-c1.png")
    }

    @Test func threadsKeepTheModelsFieldsWhenTheAppSavesThem() throws {
        let root = try repo()
        let store = DocStore(file: root.appendingPathComponent("a.md").path)
        try FileManager.default.createDirectory(atPath: (store.path as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        let saved = #"{"file":"x","next":2,"threads":[{"id":"c1","quote":"q","line":1,"state":"open","messages":[],"image":"d.png","region":[0.1,0.2,0.3,0.4],"mark":"m.png"}]}"#
        try Data(saved.utf8).write(to: URL(fileURLWithPath: store.path))
        try store.update { $0.next = 3 }
        let thread = store.read().threads[0]
        #expect(thread.image == "d.png" && thread.region == [0.1, 0.2, 0.3, 0.4] && thread.mark == "m.png")
    }

    @Test func unquotedMessagesLeaveTheQuoteOut() {
        #expect(DocStore.message(thread("gone"), file: "a.md", text: "Hi", quoted: false) == "[kanna comment c1 · a.md:1]\nHi")
    }

    func repo() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("kanna-doc-\(UUID().uuidString)").resolvingSymlinksInPath()
        try FileManager.default.createDirectory(at: root.appendingPathComponent(".git"), withIntermediateDirectories: true)
        return root
    }

    @Test func writesNeverFollowSymlinks() throws {
        let root = try repo()
        let target = root.appendingPathComponent("victim")
        try Data("keep".utf8).write(to: target)
        try FileManager.default.createDirectory(at: root.appendingPathComponent(".kanna/docs"), withIntermediateDirectories: true)
        let store = DocStore(file: root.appendingPathComponent("a.md").path)
        try FileManager.default.createSymbolicLink(atPath: store.path + ".lock", withDestinationPath: target.path)
        #expect(throws: (any Error).self) { try store.update { $0.next = 9 } }
        try FileManager.default.removeItem(atPath: store.path + ".lock")
        try FileManager.default.createSymbolicLink(atPath: store.path, withDestinationPath: target.path)
        try store.update { $0.next = 9 }
        #expect(String(decoding: try Data(contentsOf: target), as: UTF8.self) == "keep")
        #expect(store.read().next == 9)
    }

    @Test func aSymlinkedDocsFolderIsRefused() throws {
        let root = try repo()
        let elsewhere = try repo()
        try FileManager.default.createDirectory(at: root.appendingPathComponent(".kanna"), withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(atPath: root.appendingPathComponent(".kanna/docs").path, withDestinationPath: elsewhere.path)
        let store = DocStore(file: root.appendingPathComponent("a.md").path)
        #expect(throws: (any Error).self) { try store.update { $0.next = 9 } }
        #expect(!FileManager.default.fileExists(atPath: elsewhere.appendingPathComponent("a.md.json").path))
    }

    @Test func storeLivesBesideTheRepoAndRoundTrips() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("kanna-doc-\(UUID().uuidString)").resolvingSymlinksInPath()
        try FileManager.default.createDirectory(at: root.appendingPathComponent(".git"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: root.appendingPathComponent("docs"), withIntermediateDirectories: true)
        let file = root.appendingPathComponent("docs/spec.md").path
        let store = DocStore(file: file)
        #expect(store.path == root.path + "/.kanna/docs/docs__spec.md.json")
        #expect(store.relativeName == "docs/spec.md")
        try store.update { record in
            record.agent = "p3"
            record.threads.append(thread("x"))
            record.next = 2
        }
        let read = store.read()
        #expect(read.agent == "p3" && read.next == 2 && read.threads.count == 1 && read.file == file)
    }
}
