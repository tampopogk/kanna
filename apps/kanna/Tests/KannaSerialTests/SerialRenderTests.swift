import AppKit
import Testing
@testable import KannaSerial

@Suite struct SerialRenderTests {
    @Test func ansiColoursShowAndOtherSequencesGo() {
        let text = SerialRender.ansi("\u{1b}[0;31mred\u{1b}[0m plain\u{1b}[2J", [.font: NSFont.systemFont(ofSize: 12)])
        #expect(text.string == "red plain")
        let red = text.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor
        #expect(red == SerialTheme.ansi[31])
    }

    @Test func aLevelRowDropsItsAnsiAndTakesTheLevelColour() {
        let row = SerialRow(n: 1, t: 0, dir: "<", text: "\u{1b}[0;31mE (5) wifi: bad\u{1b}[0m", level: "E")
        let text = SerialRender.row(row, time: false)
        #expect(text.string == "▎E (5) wifi: bad")
        #expect(text.attribute(.foregroundColor, at: 1, effectiveRange: nil) as? NSColor == SerialTheme.error)
    }

    @Test func sentAndEventRowsAreMarked() {
        #expect(SerialRender.row(SerialRow(n: 1, t: 0, dir: ">", text: "help"), time: false).string == "→ help")
        #expect(SerialRender.row(SerialRow(n: 2, t: 0, dir: "#", text: "unplugged · gone"), time: false).string == "── unplugged · gone ──")
    }

    @Test func escapesComeBackAsBytesAndShowFaint() {
        #expect(SerialRender.bytes("a\\x00\\\\b\\xff") == [0x61, 0, 0x5c, 0x62, 0xff])
        let text = SerialRender.row(SerialRow(n: 1, t: 0, dir: "<", text: "x\\x00y"), time: false)
        #expect(text.string == " x\\x00y")
        #expect(text.attribute(.foregroundColor, at: 2, effectiveRange: nil) as? NSColor == SerialTheme.faint)
    }

    @Test func theHexDumpHasOffsetsBytesAndText() {
        let lines = SerialRender.hexLines([SerialRow(n: 1, t: 0, dir: "<", text: "ESP-ROM:esp32s3\\x00"), SerialRow(n: 2, t: 0, dir: ">", text: "ignored")])
        #expect(lines.count == 2)
        #expect(lines[0] == "00000000  45 53 50 2d 52 4f 4d 3a 65 73 70 33 32 73 33 00  ESP-ROM:esp32s3·")
        #expect(lines[1].hasPrefix("00000010  0a"))
    }

    @MainActor @Test func historyWalksAndNewRowsAppend() {
        let view = SerialPaneView()
        var sent: [String] = []
        view.onSend = { text, _ in sent.append(text) }
        view.load(rows: [SerialRow(n: 1, t: 0, dir: "<", text: "one")], partial: SerialRow(n: 0, t: 0, dir: "<", text: "esp> "), state: SerialState(), history: ["help"])
        #expect(view.text.hasSuffix("one\n" + SerialRender.clock(0) + "   esp> "))
        view.receive(rows: [SerialRow(n: 1, t: 0, dir: "<", text: "one"), SerialRow(n: 2, t: 0, dir: "<", text: "two")], partial: nil, state: nil, rx: 9)
        #expect(view.rows.count == 2)
        #expect(view.text.hasSuffix("one\n" + SerialRender.clock(0) + "   two\n"))
        view.type("wifi status")
        #expect(sent == ["wifi status"])
        #expect(view.history == ["help", "wifi status"])
    }
}
