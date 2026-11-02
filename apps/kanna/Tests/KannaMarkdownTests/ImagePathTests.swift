import Foundation
import Testing
@testable import KannaMarkdown

@MainActor
struct ImagePathTests {
    let folder = URL(fileURLWithPath: "/w/docs")

    func resolve(_ source: String) -> String? {
        MarkdownRenderer.localImagePath(source, folder: folder)
    }

    @Test func pathsAreJoinedWithoutTouchingTheDisk() {
        #expect(resolve("img/a.png") == "/w/docs/img/a.png")
        #expect(resolve("img%2Fa.png") == "/w/docs/img/a.png")
        #expect(resolve("../a.png") == "/w/a.png")
        #expect(resolve("/x/./a.png") == "/x/a.png")
        #expect(resolve("file:///x/a.png") == "/x/a.png")
        #expect(resolve("file://localhost/x/a.png") == "/x/a.png")
    }

    @Test func remoteSourcesAreRefused() {
        for source in [
            "http://example.invalid/a.png", "HTTPS://example.invalid/a.png", "smb://example.invalid/a.png",
            "file://example.invalid/share/a.png", "FILE://example.invalid/share/a.png", "",
        ] {
            #expect(resolve(source) == nil, "\(source)")
        }
    }
}
