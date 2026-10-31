import Foundation
import Testing
@testable import KannaLinks

struct PortTunnelURLTests {
    @Test(arguments: [("http://127.0.0.1:5173/", 5173), ("http://localhost:3000/a?b=1#c", 3000), ("http://[::1]:8080/", 8080),
                      ("https://localhost/", 443), ("http://0.0.0.0:4321", 4321), ("http://app.localhost:5000/", 5000)])
    func aLoopbackPageIsTunnelled(_ value: String, _ port: Int) {
        #expect(PortTunnelURL(value)?.port == port)
    }

    @Test(arguments: ["https://example.com/", "file:///tmp/a.html", "http://192.168.1.4:5173/", "about:blank", "ftp://localhost/"])
    func otherPagesAreNot(_ value: String) {
        #expect(PortTunnelURL(value) == nil)
    }

    @Test func theAppLoadsItOnItsLoopbackKeepingThePathAndTheSchemeAndReportsTheOriginal() throws {
        let tunnel = try #require(PortTunnelURL("http://localhost:5173/app/?q=1#top"))
        let local = tunnel.local(61234)
        #expect(local.absoluteString == "http://127.0.0.1:61234/app/?q=1#top")
        #expect(tunnel.remote(URL(string: "http://127.0.0.1:61234/other?x=2")!, local: 61234).absoluteString == "http://localhost:5173/other?x=2")
        #expect(tunnel.remote(URL(string: "https://example.com/")!, local: 61234).absoluteString == "https://example.com/")
        #expect(tunnel.remote(URL(string: "http://127.0.0.1:9999/")!, local: 61234).absoluteString == "http://127.0.0.1:9999/")
        #expect(tunnel.address == "localhost:5173")
        #expect(try #require(PortTunnelURL("https://127.0.0.1/")).local(5000).absoluteString == "https://127.0.0.1:5000/")
    }
}
