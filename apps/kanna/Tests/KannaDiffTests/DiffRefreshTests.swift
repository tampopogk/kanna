import AppKit
import Testing
@testable import KannaDiff

@MainActor
struct DiffRefreshTests {
    func diff(_ lines: [String], files extra: [DiffFile] = [], error: String? = nil) -> DiffResult {
        let hunk = DiffHunk(old: 1, new: 1, scope: "", lines: lines.enumerated().map { DiffLine(kind: .added, text: $1, old: nil, new: $0 + 1) })
        let file = DiffFile(path: "a.rs", oldPath: nil, status: "M", added: lines.count, removed: 0, binary: nil, untracked: nil, fold: nil, note: nil, hunks: [hunk])
        let files = error == nil ? [file] + extra : []
        return DiffResult(folder: "/r", args: [], command: "git diff", top: "/r", files: files, added: lines.count, removed: 0, error: error)
    }

    func pane() -> DiffPaneView {
        let pane = DiffPaneView(view: "unified")
        pane.frame = NSRect(x: 0, y: 0, width: 900, height: 600)
        return pane
    }

    @Test func aNewerDiffIsHeldUntilARefresh() {
        let pane = pane()
        pane.receive(diff(["one"]), serial: 1)
        let rows = pane.debugRows.count
        pane.receive(diff(["one", "two"]), serial: 2)
        #expect(pane.stale && pane.serial == 1 && pane.pending?.serial == 2)
        #expect(pane.debugRows.count == rows && pane.fresh.isEmpty)
        pane.refresh()
        #expect(!pane.stale && pane.serial == 2)
        #expect(pane.debugRows.count == rows + 1 && !pane.fresh.isEmpty)
    }

    @Test func aDiffEqualToTheShownOneIsNotNews() {
        let pane = pane()
        pane.receive(diff(["one"]), serial: 1)
        pane.receive(diff(["one", "two"]), serial: 2)
        pane.receive(diff(["one"]), serial: 3)
        #expect(!pane.stale && pane.serial == 1)
    }

    @Test func gitFailingWhileReadingIsANotice() {
        let pane = pane()
        pane.receive(diff(["one"]), serial: 1)
        pane.receive(diff([], error: "fatal: not a git repository\nmore"), serial: 2)
        #expect(pane.notice?.hasPrefix("fatal: not a git repository") == true && pane.result?.error == nil && pane.result?.files.count == 1)
        pane.receive(diff(["one"]), serial: 3)
        #expect(pane.notice == nil && !pane.stale)
    }

    @Test func aPaneWithNothingToReadTakesTheNewerDiffAtOnce() {
        let pane = pane()
        pane.receive(diff([], error: "fatal: bad flag"), serial: 1)
        #expect(pane.result?.error == "fatal: bad flag")
        pane.receive(diff(["one"]), serial: 2)
        #expect(!pane.stale && pane.serial == 2)
        let empty = DiffResult(folder: "/r", args: [], command: "git diff", top: "/r", files: [], added: 0, removed: 0, error: nil)
        let other = self.pane()
        other.receive(empty, serial: 1)
        other.receive(diff(["two"]), serial: 2)
        #expect(!other.stale && other.serial == 2)
    }

    @Test func showingAFoldedFileTakesTheDiffThatUnfoldsIt() {
        let pane = pane()
        let lock = DiffFile(path: "Cargo.lock", oldPath: nil, status: "M", added: 1, removed: 0, binary: nil, untracked: nil, fold: "lock", note: nil, hunks: [])
        pane.receive(diff(["one"], files: [lock]), serial: 1)
        pane.showFolded("Cargo.lock")
        pane.receive(diff(["one", "two"], files: [lock]), serial: 2)
        #expect(!pane.stale && pane.serial == 2)
        var open = lock
        open.fold = nil
        pane.receive(diff(["one", "two"], files: [open]), serial: 3)
        #expect(!pane.stale && pane.serial == 3)
        pane.receive(diff(["one", "two", "three"], files: [open]), serial: 4)
        #expect(pane.stale && pane.serial == 3)
    }
}
