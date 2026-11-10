import Foundation
import Testing
@testable import KannaLinks

struct MachineFileURLTests {
    @Test func aFilePageLoadsThroughTheSchemeAndReportsItsFileURL() throws {
        let page = try #require(MachineFileURL.page("file:///home/u/repo/docs/My%20Page.html?x=1#top"))
        #expect(page.absoluteString == "kanna-file://files/home/u/repo/docs/My%20Page.html?x=1#top")
        #expect(MachineFileURL.path(of: page) == "/home/u/repo/docs/My Page.html")
        #expect(MachineFileURL.file(page).absoluteString == "file:///home/u/repo/docs/My%20Page.html?x=1#top")
    }

    @Test func aRelativeImageResolvesToTheSameMachinesFolder() throws {
        let page = try #require(MachineFileURL.page("file:///w/site/index.html"))
        let image = try #require(URL(string: "img/logo.png", relativeTo: page)?.absoluteURL)
        #expect(MachineFileURL.path(of: image) == "/w/site/img/logo.png")
        let up = try #require(URL(string: "../style.css", relativeTo: page)?.absoluteURL)
        #expect(MachineFileURL.path(of: up) == "/w/style.css")
    }

    @Test(arguments: ["https://example.com/", "http://localhost:5173/", "about:blank", "kanna-file://other/x.html"])
    func otherPagesAreNotFiles(_ value: String) {
        #expect(MachineFileURL.page(value) == nil)
        #expect(MachineFileURL.path(of: URL(string: value)) == nil)
        #expect(MachineFileURL.file(URL(string: value)!).absoluteString == value)
    }

    @Test func aPageReadsOnlyInsideItsFolderByTheTextOfItsPath() throws {
        let folder = try #require(MachineFileURL.folder(of: "/w/site/index.html"))
        #expect(folder == ["w", "site"])
        #expect(MachineFileURL.inside("/w/site/img/logo.png", folder))
        #expect(MachineFileURL.inside("/w/site/./index.html", folder))
        #expect(!MachineFileURL.inside("/w/site/../docs/spec.md", folder))
        #expect(!MachineFileURL.inside("/w/sitemap.html", folder))
        #expect(!MachineFileURL.inside("/Users/u/.ssh/id_ed25519", folder))
        #expect(!MachineFileURL.inside("/../../w/site/x", folder))
        #expect(!MachineFileURL.inside("~/w/site/x", folder))
        #expect(!MachineFileURL.inside("/w/sit\u{0065}\u{0301}/x", ["w", "sit\u{00e9}"]))
        #expect(MachineFileURL.inside("/etc/hosts", try #require(MachineFileURL.folder(of: "/index.html"))))
    }

    @Test(arguments: [("a.html", "text/html"), ("app.js", "text/javascript"), ("s.css", "text/css"), ("i.png", "image/png"), ("d.svg", "image/svg+xml"), ("x.unknownext", "application/octet-stream")])
    func eachFileGetsItsType(_ path: String, _ mime: String) {
        #expect(MachineFileURL.mime(of: path) == mime)
    }
}
