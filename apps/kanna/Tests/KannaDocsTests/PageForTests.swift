import Foundation
import Testing
@testable import KannaDocs

@MainActor
struct PageForTests {
    @Test func aFilePageIsMarkedByItsPathFromTheScheme() {
        #expect(PageMarkupView.page(for: URL(string: "kanna-file://files/w/site/My%20Page.html#top")) == "/w/site/My Page.html")
        #expect(PageMarkupView.page(for: URL(string: "kanna-file://files/w/site/INDEX.HTM")) == "/w/site/INDEX.HTM")
    }

    @Test(arguments: ["file:///w/site/index.html", "kanna-file://files/w/site/style.css", "https://example.com/index.html", "kanna-file://other/w/index.html"])
    func otherURLsAreNotPages(_ url: String) {
        #expect(PageMarkupView.page(for: URL(string: url)) == nil)
    }
}
