import Darwin
import Foundation
import Testing

@testable import KmuxCore

/// The saved layout on disk: where it lives, atomic writes, and refusing
/// files kmux didn't write (docs/kmux-spec.md §3.6).
@MainActor
@Suite struct StateFileTests {
    let folder: String

    init() throws {
        folder = (NSTemporaryDirectory() as NSString).appendingPathComponent("kmux-state-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    }

    func file(_ name: String = "state-default.json") -> StateFile { StateFile(path: (folder as NSString).appendingPathComponent(name)) }

    func mode(_ path: String) -> mode_t {
        var info = stat()
        lstat(path, &info)
        return info.st_mode & 0o777
    }

    @Test func pathsSitNextToTheSocket() {
        #expect(StateFile.path(for: Instance(name: "default")).hasSuffix("/kmux/state-default.json"))
        #expect(StateFile.path(for: Instance(name: "work")).hasSuffix("/kmux/state-work.json"))
        #expect(StateFile.path(for: Instance(name: "default", socketPath: "/tmp/kmux-lp-1.sock")) == "/tmp/kmux-lp-1.state.json")
    }

    @Test func writesAtomicallyAndPrivately() throws {
        let state = file()
        #expect(state.load() == .none)
        try state.write(["version": 1, "panes": []])
        try state.write(["version": 1, "panes": ["second"]])
        #expect(state.load() == .state(["version": 1, "panes": ["second"]]))
        #expect(mode(state.path) == 0o600)
        let left = try FileManager.default.contentsOfDirectory(atPath: folder)
        #expect(left == ["state-default.json"], "no temporary files left behind: \(left)")
    }

    @Test func refusesFilesKmuxDidNotWrite() throws {
        let state = file()
        try Data("{\"version\": 1}".utf8).write(to: URL(fileURLWithPath: state.path))
        chmod(state.path, 0o644)
        guard case .refused(let why) = state.load() else { Issue.record("a file others can read should be refused"); return }
        #expect(why.contains("other users"))

        chmod(state.path, 0o600)
        #expect(state.load() == .state(["version": 1]))

        let link = file("link.json")
        symlink(state.path, link.path)
        guard case .refused(let linkWhy) = link.load() else { Issue.record("a symlink should be refused"); return }
        #expect(linkWhy.contains("not a regular file"))

        chmod(folder, 0o777)
        defer { chmod(folder, 0o700) }
        guard case .refused(let folderWhy) = state.load() else { Issue.record("a folder others can write to should be refused"); return }
        #expect(folderWhy.contains("written by other users"))
    }

    @Test func badFilesAreRefusedAndSetAside() throws {
        let state = file()
        try state.write(["version": 1])
        try Data("{ not json".utf8).write(to: URL(fileURLWithPath: state.path))
        guard case .refused = state.load() else { Issue.record("bad JSON should be refused"); return }
        #expect(state.setAside() == state.path + ".bad")
        #expect(state.load() == .none)
        #expect(FileManager.default.fileExists(atPath: state.path + ".bad"))
    }

    @Test func aDockedPaneRoundTripsThroughTheFile() async throws {
        let (core, _) = makeCore()
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "a"]])
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "agent", "dock": "right"]])
        _ = await core.handle(["cmd": "resize", "args": ["pane": "agent", "size": "3/10"]])
        let state = file()
        try state.write(core.exportState())
        guard case .state(let saved) = state.load() else { Issue.record("the state should load"); return }
        let (restored, _) = makeCore()
        try restored.restoreState(saved)
        #expect(restored.model.windows.first?.dock == Dock(pane: "term-2", edge: .right, width: 0.3))
        #expect(restored.model.windows.first?.focused == "term-2")
        #expect(restored.exportState() == core.exportState())
    }

    @Test func oldOrBrokenStatesDoNotRestore() async throws {
        let (core, _) = makeCore()
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "keep"]])
        for bad: JSON in [["version": 0, "windows": [], "panes": []], ["version": 3, "windows": [], "panes": []], ["version": 1], ["version": 2], "x"] {
            #expect(throws: KmuxError.self) { try core.restoreState(bad) }
        }
        #expect(core.model.pane("keep") != nil)
    }

    @Test func roundTripsThroughTheFile() async throws {
        let (core, _) = makeCore()
        _ = await core.handle(["cmd": "open", "args": ["type": "term", "name": "a", "cwd": "/tmp", "cmd": "ls"]])
        _ = await core.handle(["cmd": "open", "args": ["type": "md", "name": "doc", "path": "/x.md", "split": "right", "size": "1/3"]])
        let state = file()
        try state.write(core.exportState())
        guard case .state(let saved) = state.load() else { Issue.record("no state"); return }

        let (restored, host) = makeCore()
        try restored.restoreState(saved)
        #expect(restored.exportState() == core.exportState())
        #expect(restored.model.pane("a")?.command == "ls")
        _ = host
    }

    /// Where windows were and markdown zoom are the app's, in a file of its
    /// own; a version-1 state kept them, and its upgrade still has them, with
    /// the new IDs, for the app to take.
    @Test func whereWindowsAreIsTheAppsOwn() throws {
        let instance = Instance(name: "work", socketPath: Instance.socketPath(for: "work"))
        #expect(StateFile.appPath(for: instance).hasSuffix("/app-work.json"))
        #expect(StateFile.appPath(for: Instance(name: "t", socketPath: "/tmp/x/k.sock")) == "/tmp/x/k.app.json")
        let old: JSON = [
            "kmux": "state", "version": 1, "key": "w1", "counters": ["pane": 1, "tab": 1, "window": 1],
            "windows": [["id": "w1", "frame": ["x": 10, "y": 20, "w": 800, "h": 500], "active": "t1", "focused": "p1", "zoomed": nil, "tabCount": 1,
                         "tabs": [["id": "t1", "title": "a", "lastFocus": "p1", "layout": ["pane": "p1"]]]]],
            "panes": [["id": "p1", "name": "doc", "type": "md", "path": "/x.md", "zoom": 1.25]],
        ]
        func first(_ state: JSON, _ key: String) -> JSON? {
            if case .array(let items)? = state[key] { return items.first }
            return nil
        }
        let upgraded = Core.upgrade(old)
        #expect(first(upgraded, "windows")?["id"] == "win-1")
        #expect(first(upgraded, "windows")?["frame"]?["w"] == 800)
        #expect(first(upgraded, "panes")?["id"] == "file-1")
        #expect(first(upgraded, "panes")?["zoom"] == 1.25)
        let (core, _) = makeCore()
        try core.restoreState(old)
        let saved = core.exportState()
        #expect(first(saved, "windows")?["frame"] == nil)
        #expect(first(saved, "panes")?["zoom"] == nil)
    }
}
