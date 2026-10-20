import Darwin
import Foundation
import Testing

@testable import KmuxCore

@MainActor
final class FakeHost: PaneHost {
    weak var core: Core?
    var fail: Set<String> = []
    var stopped: [String] = []

    func start(_ pane: Pane) {
        let id = pane.id, failing = fail.contains(pane.command ?? "")
        Task { @MainActor in
            if failing { self.core?.update(id, state: .failed, error: "command not found") } else { self.core?.update(id, state: .running) }
        }
    }

    func stop(_ pane: Pane) { stopped.append(pane.id) }

    var sent: [String] = []
    func send(_ pane: Pane, text: String) { sent.append("\(pane.id): \(text)") }
}

@MainActor
func makeCore() -> (Core, FakeHost) {
    let core = Core(), host = FakeHost()
    host.core = core
    core.host = host
    return (core, host)
}

/// Native-only behaviour. Protocol behaviour shared with the reference model
/// lives in tests/kmux-protocol/cases (see ProtocolCasesTests).
@MainActor
@Suite struct CoreTests {
    @Test func capabilities() async {
        let (core, _) = makeCore()
        let reply = await core.handle(["id": 1, "cmd": "capabilities"])
        #expect(reply["ok"] == true)
        #expect(reply["id"] == 1)
        #expect(reply["mux"] == "kmux")
    }

    @Test func autoSplitFollowsThePaneShape() async {
        let (core, _) = makeCore()
        core.paneIsWide = { _ in false }
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "a"]])
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "b"]])
        let list = await core.handle(["cmd": "list"])
        if case .array(let windows) = list["windows"], case .array(let tabs) = windows[0]["tabs"] {
            #expect(tabs[0]["layout"]?["split"] == "column")
        } else {
            Issue.record("no windows in \(list)")
        }
    }

    @Test func openReportsStartFailures() async {
        let (core, host) = makeCore()
        host.fail = ["nope"]
        let reply = await core.handle(["cmd": "open", "args": ["type": "term", "cmd": "nope"]])
        #expect(reply["ok"] == false)
        #expect(reply["error"]?["code"] == "start_failed")
        #expect(reply["error"]?["message"] == "command not found")
    }

    @Test func startTimeoutOnlyFailsPanesStillStarting() async throws {
        let (core, host) = makeCore()
        core.startTimeout = .milliseconds(50)
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "ok"]])
        core.host = nil
        let reply = await core.handle(["cmd": "open", "args": ["type": "term", "name": "stuck"]])
        #expect(reply["error"]?["code"] == "start_failed")
        try await Task.sleep(for: .milliseconds(100))
        #expect(core.model.pane("ok")?.state == .running)
        _ = host
    }

    @Test func closeRedistributesAndDropsEmptyWindows() async {
        let (core, host) = makeCore()
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "a"]])
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "b", "split": "right", "size": "1/4"]])
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "solo", "window": "new"]])
        #expect(core.model.windows.count == 2)

        let closed = await core.handle(["cmd": "close", "args": ["pane": "solo"]])
        #expect(closed["closed"] == ["term-3"])
        #expect(core.model.windows.map(\.id) == ["win-1"])
        #expect(core.model.key == "win-1")

        _ = await core.handle(["cmd": "close", "args": ["pane": "a"]])
        #expect(core.model.tree(core.model.windows[0].tabs[0].root) == ["pane": "b"])
        #expect(core.model.windows[0].focused == "term-2")
        #expect(host.stopped == ["term-3", "term-1"])
    }

    @Test func focusZoomAndCycling() async {
        let (core, _) = makeCore()
        for name in ["a", "b", "c"] { _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": .string(name), "split": "right"]]) }
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "t2", "tab": true]])
        let window = core.model.windows[0]
        #expect(window.tabs.count == 2)
        #expect(core.cycleTab(in: window, by: 1) == "tab-1")

        _ = await core.handle(["cmd": "focus", "args": ["tab": "tab-1"]])
        #expect(window.active == "tab-1")
        #expect(window.focused == "term-3")
        #expect(core.cyclePane(in: window, by: 1) == "term-1")
        #expect(core.cyclePane(in: window, by: -1) == "term-2")

        let zoomed = await core.handle(["cmd": "zoom", "args": ["pane": "a"]])
        #expect(zoomed["zoomed"] == true)
        #expect(window.zoomed == "term-1")
        _ = await core.handle(["cmd": "focus", "args": ["pane": "b"]])
        #expect(window.zoomed == "term-2")
        #expect(window.focused == "term-2")
        _ = await core.handle(["cmd": "focus", "args": ["pane": .string(core.cyclePane(in: window, by: 1)!)]])
        #expect(window.zoomed == "term-3")
        _ = await core.handle(["cmd": "focus", "args": ["pane": "t2"]])
        #expect(window.zoomed == "term-4")
        _ = await core.handle(["cmd": "focus", "args": ["tab": "tab-1"]])
        #expect(window.zoomed == "term-3")
        _ = await core.handle(["cmd": "zoom", "args": ["pane": "a"]])
        #expect(window.zoomed == "term-1")
        _ = await core.handle(["cmd": "zoom", "args": ["pane": "a"]])
        #expect(window.zoomed == nil)
        #expect(window.focused == "term-1")

        let missing = await core.handle(["cmd": "focus"])
        #expect(missing["error"]?["code"] == "bad_request")
    }

    @Test func restartStartsANewTerminalInItsPlace() async {
        let (core, host) = makeCore()
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "a"]])
        core.update("term-1", state: .exited, exitCode: 0)
        let reply = await core.handle(["cmd": "restart", "args": ["pane": "a"]])
        #expect(reply["pane"]?["id"] == "term-2")
        #expect(reply["pane"]?["state"] == "running")
        #expect(reply["pane"]?["exitCode"] == nil)
        #expect(host.stopped == ["term-1"])
        #expect(core.model.paneOrder == ["term-2"])
        #expect(core.model.windows[0].focused == "term-2")
        #expect(core.model.tree(core.model.windows[0].tabs[0].root) == ["pane": "a"])
    }

    @Test func dockedPaneCyclesInReadingOrder() async {
        let (core, _) = makeCore()
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "a"]])
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "b", "split": "right"]])
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "agent", "dock": "left"]])
        let window = core.model.windows[0]
        #expect(window.shownPanes == ["term-3", "term-1", "term-2"])
        #expect(core.cyclePane(in: window, by: 1) == "term-1")
        #expect(core.cyclePane(in: window, by: -1) == "term-2")
        _ = await core.handle(["cmd": "move", "args": ["pane": "agent", "dock": "right"]])
        #expect(window.shownPanes == ["term-1", "term-2", "term-3"])
        #expect(core.cyclePane(in: window, by: 1) == "term-1")
        _ = await core.handle(["cmd": "focus", "args": ["pane": "b"]])
        #expect(core.cyclePane(in: window, by: 1) == "term-3")
    }

    @Test func dockedPaneStaysWhenItsProgramExits() async {
        let (core, host) = makeCore()
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "agent", "dock": "left"]])
        core.update("term-1", state: .exited, exitCode: 2)
        #expect(core.model.windows.first?.dock?.pane == "term-1")
        #expect(core.model.pane("agent")?.state == .exited)
        let reply = await core.handle(["cmd": "restart", "args": ["pane": "agent"]])
        #expect(reply["pane"]?["state"] == "running")
        #expect(core.model.windows.first?.dock == Dock(pane: "term-2", edge: .left, width: 0.4))
        #expect(host.stopped == ["term-1"])
    }

    @Test func workspacesSavedWithOldIDsLoadRenamed() async {
        let (core, _) = makeCore()
        let state: JSON = ["kmux": "state", "version": 1, "panes": [], "windows": [], "workspaces": [["id": "s2", "name": "old", "hidden": false]]]
        #expect(await core.handle(["cmd": "debug.restoreState", "args": ["state": state]])["ok"] == true)
        let made = await core.handle(["cmd": "workspace", "args": ["action": "new"]])
        #expect(made["workspace"]?["id"] == "ws-3")
        let list = await core.handle(["cmd": "list"])
        if case .array(let workspaces) = list["workspaces"] {
            #expect(workspaces.compactMap { $0["id"]?.string } == ["ws-2", "ws-3"])
        } else {
            Issue.record("no workspaces in \(list)")
        }
    }

    @Test func fractions() {
        #expect(Fraction.parse("1/3")! == 1.0 / 3)
        #expect(Fraction.parse("25%") == 0.25)
        #expect(Fraction.parse(0.5) == 0.5)
        #expect(Fraction.parse("x") == nil)
        #expect(Fraction.format(2.0 / 3) == "2/3")
        #expect(Fraction.format(0.37) == "37%")
    }
}

@Suite struct SocketServerTests {
    @Test func answersNDJSONRequestsInOrder() async throws {
        let path = "/tmp/claude/kmux-test-\(getpid()).sock"
        try? FileManager.default.createDirectory(atPath: "/tmp/claude", withIntermediateDirectories: true)
        let server = SocketServer(path: path) { request in
            ["id": request["id"] ?? nil, "ok": true, "echo": request["cmd"] ?? nil]
        }
        try server.start()
        defer { server.stop() }

        let lines = try await Task.detached {
            try roundTrip(path, "{\"id\":1,\"cmd\":\"a\"}\n{\"id\":2,\"cmd\":\"b\"}\nnot json\n", replies: 3)
        }.value
        #expect(lines.count == 3)
        #expect(try JSON.parse(Data(lines[0].utf8)) == ["id": 1, "ok": true, "echo": "a"])
        #expect(try JSON.parse(Data(lines[1].utf8)) == ["id": 2, "ok": true, "echo": "b"])
        #expect(try JSON.parse(Data(lines[2].utf8))["error"]?["code"] == "bad_request")
        #expect(SocketServer.isListening(path))
    }
}

func roundTrip(_ path: String, _ text: String, replies: Int) throws -> [String] {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    defer { close(fd) }
    var address = SocketServer.address(path)
    let connected = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
    }
    guard connected == 0 else { throw KmuxError("connect", String(cString: strerror(errno))) }
    _ = text.withCString { write(fd, $0, strlen($0)) }
    var received = Data()
    var chunk = [UInt8](repeating: 0, count: 4096)
    while received.filter({ $0 == UInt8(ascii: "\n") }).count < replies {
        let count = read(fd, &chunk, chunk.count)
        if count <= 0 { break }
        received.append(chunk, count: count)
    }
    return String(decoding: received, as: UTF8.self).split(separator: "\n").map(String.init)
}
