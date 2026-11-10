import AppKit
import Testing
@testable import KannaDocs

@MainActor
struct CommentBoxTests {
    let long = String(repeating: "This comment runs on for a while so it wraps across many lines of the box. ", count: 12)

    func type(_ text: String, into field: BoxField) {
        field.setSelectedRange(NSRange(location: (field.string as NSString).length, length: 0))
        field.insertText(text, replacementRange: field.selectedRange())
    }

    func expectLaidOut(_ parts: [String: NSRect]) {
        let box = parts["box"]!, field = parts["field"]!
        #expect(box.insetBy(dx: 1, dy: 1).contains(field))
        for name in ["hint", "post"] {
            guard let part = parts[name] else { continue }
            #expect(box.insetBy(dx: 1, dy: 1).contains(part), "\(name) \(part) is outside the box \(box)")
            #expect(part.minY >= field.maxY, "\(name) \(part) overlaps the field \(field)")
        }
    }

    @Test func composerGrowsWithItsTextThenScrolls() {
        let box = CommentBox(composer: ())
        var resized = 0
        box.onResize = { _ in resized += 1 }
        let empty = box.parts
        #expect(empty["field"]!.height == 54)
        expectLaidOut(empty)
        type("Three\nshort\nlines\nand\na\nfifth", into: box.field)
        let five = box.parts
        #expect(five["field"]!.height > 54)
        #expect(five["box"]!.height > empty["box"]!.height)
        #expect(resized > 0)
        expectLaidOut(five)
        type(long, into: box.field)
        let full = box.parts
        #expect(full["field"]!.height == box.grow.most)
        #expect(full["text"]!.height > full["field"]!.height)
        expectLaidOut(full)
    }

    @Test func aNarrowComposerKeepsPostInside() {
        let box = CommentBox(composer: (), width: 200)
        type(long, into: box.field)
        #expect(box.parts["hint"] == nil)
        expectLaidOut(box.parts)
    }

    @Test func replyGrowsWithItsText() {
        let thread = DocThread(id: "c1", quote: "q", prefix: "", suffix: "", line: 1, state: "open", status: "sent",
                               messages: [DocMessage(who: "user", text: "First", at: "")], error: nil)
        let box = CommentBox(thread: thread, wait: nil, note: nil)
        let before = box.parts
        #expect(before["field"]!.height == 16)
        type(long, into: box.field)
        let after = box.parts
        #expect(after["field"]!.height == box.grow.most)
        #expect(after["box"]!.height - before["box"]!.height == box.grow.most - 16)
        expectLaidOut(after)
    }

    @Test func cardReplyGrowsTheCard() {
        let thread = DocThread(id: "c1", quote: "q", prefix: "", suffix: "", line: 1, state: "open", status: "sent",
                               messages: [DocMessage(who: "user", text: "First", at: "")], error: nil)
        let card = ThreadCard(thread: thread, active: true, wait: nil, note: nil, width: 260)
        let before = card.frame.height
        type(long, into: card.field)
        #expect(card.frame.height - before == card.grow.most - 16)
        #expect(card.bounds.contains(card.convert(card.grow.bounds, from: card.grow)))
    }

    func thread(_ id: String = "c1", answer: String? = nil) -> DocThread {
        DocThread(id: id, quote: "q", prefix: "", suffix: "", line: 1, state: "open", status: "sent",
                  messages: [DocMessage(who: "user", text: "First", at: "")] + (answer.map { [DocMessage(who: "agent", text: $0, at: "")] } ?? []), error: nil)
    }

    func key(_ code: UInt16, _ flags: NSEvent.ModifierFlags = [], chars: String = "\r") -> NSEvent {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: 0, context: nil,
                         characters: chars, charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code)!
    }

    @Test func replyStopsGrowingAtEightLines() {
        let card = ThreadCard(thread: thread(), active: true, wait: nil, note: nil, width: 260)
        let line = NSLayoutManager().defaultLineHeight(for: card.field.font!)
        #expect(card.grow.most == ceil(line * 8))
        type((1...7).map { "line \($0)" }.joined(separator: "\n"), into: card.field)
        #expect(card.grow.frame.height == ceil(line * 7))
        type("\nline 8\nline 9\nline 10", into: card.field)
        #expect(card.grow.frame.height == card.grow.most)
        #expect(card.field.frame.height > card.grow.frame.height)
        let caret = card.grow.contentView.convert(card.field.caretRect, from: card.field)
        #expect(card.grow.contentView.bounds.contains(caret), "caret \(caret) is scrolled out of \(card.grow.contentView.bounds)")
    }

    @Test func replyTakesTheCardsWidthWithButtonsBelow() {
        let card = ThreadCard(thread: thread(), active: true, wait: nil, note: nil, width: 260)
        let row = card.row, box = row.boxFrame
        #expect(box.minX == 0 && box.width == row.bounds.width)
        for button in [row.send, row.act] as [NSView] {
            #expect(button.frame.minY >= box.maxY, "\(button.frame) is beside the box \(box), not below it")
            #expect(row.bounds.contains(button.frame))
        }
        #expect(row.act.frame.maxX == row.bounds.width)
        #expect(row.send.frame.maxX < row.act.frame.minX)
        type(long, into: card.field)
        #expect(row.send.frame.minY >= row.boxFrame.maxY)
        #expect(card.bounds.contains(card.convert(row.act.bounds, from: row.act)))
    }

    @Test func panelKeepsTheGrowingReplyInView() {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 280, height: 300), styleMask: [.borderless], backing: .buffered, defer: false)
        let panel = ThreadPanel()
        panel.frame = window.contentView!.bounds
        window.contentView!.addSubview(panel)
        panel.layoutSubtreeIfNeeded()
        panel.show([thread("c1"), thread("c2"), thread("c3", answer: long)], active: "c3", waits: [:], notes: [:])
        let card = panel.card("c3")!
        #expect(card.frame.height > panel.bounds.height)
        window.makeFirstResponder(card.field)
        for _ in 0..<12 { type("more words in the reply to the agent that wrap\n", into: card.field) }
        #expect(card.grow.frame.height == card.grow.most)
        let visible = panel.scroll.contentView.convert(panel.scroll.contentView.bounds, to: nil)
        let row = card.row.convert(card.row.bounds, to: nil)
        let caret = card.field.convert(card.field.caretRect, to: nil)
        #expect(visible.contains(row), "the reply row \(row) is clipped by the panel \(visible)")
        #expect(visible.contains(caret), "the caret \(caret) is clipped by the panel \(visible)")
    }

    @Test func panelShowsTheCaretWhenTheRowIsTallerThanThePanel() {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 280, height: 120), styleMask: [.borderless], backing: .buffered, defer: false)
        let panel = ThreadPanel()
        panel.frame = window.contentView!.bounds
        window.contentView!.addSubview(panel)
        panel.layoutSubtreeIfNeeded()
        panel.show([thread("c1"), thread("c2")], active: "c2", waits: [:], notes: [:])
        let card = panel.card("c2")!
        window.makeFirstResponder(card.field)
        for _ in 0..<12 { type("line\n", into: card.field) }
        let visible = panel.scroll.contentView.convert(panel.scroll.contentView.bounds, to: nil)
        let caret = card.field.convert(card.field.caretRect, to: nil)
        #expect(visible.contains(caret), "the caret \(caret) is clipped by the panel \(visible)")
    }

    @Test func returnTypesANewlineAndCommandReturnPosts() {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 280, height: 400), styleMask: [.borderless], backing: .buffered, defer: false)
        let panel = ThreadPanel()
        panel.frame = window.contentView!.bounds
        window.contentView!.addSubview(panel)
        panel.layoutSubtreeIfNeeded()
        var posted: [String] = []
        panel.onReply = { _, text in posted.append(text) }
        panel.show([thread()], active: "c1", waits: [:], notes: [:])
        let card = panel.card("c1")!
        window.makeFirstResponder(card.field)
        type("one", into: card.field)
        card.field.keyDown(with: key(36))
        type("two", into: card.field)
        card.field.keyDown(with: key(36, .shift))
        #expect(posted.isEmpty)
        #expect(card.field.string.hasPrefix("one\ntwo"))
        #expect(!window.performKeyEquivalent(with: key(36, [.command, .shift])))
        #expect(posted.isEmpty)
        #expect(window.performKeyEquivalent(with: key(36, .command)))
        #expect(posted.count == 1 && posted[0].hasPrefix("one\ntwo"))
    }

    @Test func sendAndCancelKeys() {
        #expect(BoxField.sends(key(36, .command)))
        #expect(BoxField.sends(key(76, .command, chars: "\u{3}")))
        #expect(!BoxField.sends(key(36)))
        #expect(!BoxField.sends(key(36, .shift)))
        #expect(!BoxField.sends(key(36, [.command, .shift])))
        #expect(!BoxField.sends(key(36, [.command, .option])))
        #expect(BoxField.cancels(key(53, chars: "\u{1b}")))
    }
}
