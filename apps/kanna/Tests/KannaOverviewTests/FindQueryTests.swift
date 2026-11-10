import AppKit
import Testing
@testable import KannaOverview

struct FindQueryTests {
    let file = """
    enum Shutdown { Now, Later }
    fn shutdown(reason: Shutdown) {
        log("shutdown requested");
        if reason == Shutdown::Now { SHUTDOWN.store(true) }
        let shutdowns = 0;
    }
    """

    @Test func withoutACapitalIgnoresCase() {
        #expect(FindQuery("shutdown").ranges(in: file).count == 7)
    }

    @Test func withACapitalMatchesCase() {
        #expect(FindQuery("Shutdown").ranges(in: file).count == 3)
        #expect(FindQuery("SHUTDOWN").ranges(in: file).count == 1)
        #expect(FindQuery("ShutDown").ranges(in: file).isEmpty)
    }

    @Test func rangesAreWhereTheMatchesAre() {
        let text = file as NSString
        #expect(FindQuery("Shutdown").ranges(in: file).allSatisfy { text.substring(with: $0) == "Shutdown" })
    }

    @Test func emptyFindsNothing() {
        #expect(FindQuery("").ranges(in: file).isEmpty)
        #expect(!FindQuery("").matches(file))
    }

    @Test func matchesFollowsTheSameRule() {
        #expect(FindQuery("fix login").matches("Fix login redirect"))
        #expect(!FindQuery("Fix Login").matches("Fix login redirect"))
        #expect(FindQuery("Fix login").matches("Fix login redirect"))
    }

    @MainActor @Test func paneFinderCountsAndSteps() {
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
        let text = NSTextView(frame: scroll.bounds)
        text.string = file
        scroll.documentView = text
        let finder = PaneFinder()
        finder.attach(text, in: scroll)
        finder.show()
        finder.bar.field.stringValue = "Shutdown"
        finder.edited()
        #expect(finder.matches.count == 3)
        #expect(finder.label == "1 of 3")
        finder.bar.field.stringValue = "shutdown"
        finder.edited()
        #expect(finder.matches.count == 7)
        finder.step(forward: true)
        finder.step(forward: true)
        #expect(text.selectedRange() == finder.matches[1])
        #expect(finder.label == "2 of 7")
        finder.step(forward: false)
        finder.step(forward: false)
        #expect(text.selectedRange() == finder.matches[6])
        finder.hide()
        #expect(finder.matches.isEmpty)
        #expect(!finder.isVisible)
    }
}
