import AppKit
import KannaDocs

extension MarkdownPaneView {
    func setUpComments() {
        bar.threads.onPress = { [weak self] in self?.toggleThreads() }
        panel.onSelect = { [weak self] id in self?.reveal(id) }
        panel.onAct = { [weak self] thread in self?.setState(thread.id, open: !thread.isOpen) }
        panel.onReply = { [weak self] id, text in self?.reply(id, text) }
        panel.onTour = { [weak self] to in self?.onTourStep?(to) }
        textView.onCommentKey = { [weak self] in self?.commentOnSelection() ?? false }
        textView.onCommandClick = { [weak self] point in self?.commentOnWord(at: point) ?? false }
        textView.onPlainClick = { [weak self] point in self?.clicked(at: point) }
        agentChanged()
    }

    func tearDownComments() {
        closeBox()
        active = nil
        anchors = [:]
        textView.onCommentKey = nil
        textView.onCommandClick = nil
        textView.onPlainClick = nil
        layoutManager.marks = []
        textView.needsDisplay = true
        overviewChanged()
    }

    func agentChanged() {
        guard markup else { return }
        bar.agent.set(agent.map { "Agent: \(agentName?($0) ?? $0)" } ?? "No agent: comments are saved, not sent")
        bar.agent.warn = agent == nil
        bar.needsLayout = true
        if threadsLoaded, let agent, record.agent != agent { change(.agent(agent)) }
    }

    func change(_ change: DocChange, then: (@MainActor (String?) -> Void)? = nil) {
        guard let changeThreads else { return }
        changeThreads(change) { [weak self] answer in
            guard let self else { return }
            if let error = answer.error {
                bar.agent.set("Not saved: \(error)")
                bar.agent.warn = true
                bar.needsLayout = true
            }
            if let record = answer.record { apply(record) }
            then?(answer.id)
        }
    }

    private func apply(_ record: DocRecord) {
        guard record != self.record else { return }
        self.record = record
        refreshMarks()
    }

    public var threads: [DocThread] { record.threads }
    public var commentBoxText: String? { box?.field.string }
    public var commentBoxParts: [String: NSRect]? { box?.parts }
    public var commentBoxShown: Bool { box.map { scrollView.contentView.bounds.contains($0.frame) } ?? false }
    public var commentBoxThread: String? { box.map { $0.threadID ?? "new" } }
    public var activeThread: String? { active }
    public var threadsShown: Bool { threadsVisible }
    public var marksShown: Bool { markup && showThreads }
    public var commentsButtonShown: Bool { bar.superview != nil && !bar.comments.isHidden }
    public var barText: [String] { [bar.path, bar.diff.label, bar.comments.label, bar.agent.label, bar.threads.label].compactMap { $0.superview?.isHidden == false && !$0.isHidden ? $0.stringValue : nil } }
    public func pressComments() { bar.comments.onPress?() }
    public func pressThreads() { bar.threads.onPress?() }
    public var cards: [String: CardState] { panel.cardStates }

    public var tourFooter: (thread: String, at: Int, of: Int, back: Bool, next: String)? {
        guard let tour, let row = panel.card(tour.thread)?.tourRow else { return nil }
        return (tour.thread, row.step.at, row.step.of, row.back.onPress != nil, row.next.label.stringValue)
    }

    public func pressTour(_ to: String) -> Bool {
        guard let tour, let row = panel.card(tour.thread)?.tourRow else { return false }
        guard let press = (to == "back" ? row.back : row.next).onPress else { return false }
        press()
        return true
    }

    func revealTourStop() {
        guard markup, let tour, tourUnrevealed, anchors[tour.thread] != nil else { return }
        tourUnrevealed = false
        if !threadsVisible { toggleThreads() }
        reveal(tour.thread)
        panel.reveal(tour.thread)
    }

    public func select(_ range: NSRange) {
        window?.makeFirstResponder(textView)
        textView.setSelectedRange(range)
    }

    public func markRect(_ id: String) -> NSRect? {
        guard let range = anchors[id] else { return nil }
        let rects = layoutManager.markRects(range)
        guard let first = rects.first else { return nil }
        let origin = textView.textContainerOrigin
        return textView.convert(first.offsetBy(dx: origin.x, dy: origin.y), to: self)
    }

    func toggleThreads() {
        if !sides(comments: true, forced: false).comments {
            forcedThreads = !threadsVisible
            showThreads = forcedThreads
        } else {
            showThreads.toggle()
            forcedThreads = false
        }
        bar.threads.on = threadsVisible
        needsLayout = true
        if !showThreads { closeBox() }
        refreshMarks()
    }

    public func show(threads: DocRecord) {
        if !threadsLoaded {
            threadsLoaded = true
            if let agent, threads.agent != agent { change(.agent(agent)) }
        }
        guard threads != record else { return }
        let known = Set(record.threads.map(\.id))
        record = threads
        refreshMarks()
        if markup, let started = record.threads.last(where: { !known.contains($0.id) && $0.isOpen && $0.messages.first?.who == "agent" }) {
            if !threadsVisible { toggleThreads() }
            reveal(started.id)
        }
    }

    func refreshMarks() {
        guard markup else { return }
        let text = textView.textStorage?.string ?? ""
        anchors = [:]
        for thread in record.threads {
            if let range = Doc.locate(thread, in: text) { anchors[thread.id] = range }
        }
        var marks = showThreads ? record.threads.compactMap { thread in anchors[thread.id].map { DocMark(range: $0, kind: thread.isOpen ? .open : .resolved) } } : []
        if let pending { marks.append(DocMark(range: pending.range, kind: .active)) } else if showThreads, let active, let range = anchors[active] { marks.append(DocMark(range: range, kind: .active)) }
        layoutManager.marks = marks
        textView.needsDisplay = true
        overviewChanged()
        refreshPanel()
        bar.threads.set("Threads (\(record.threads.filter(\.isOpen).count))")
        bar.needsLayout = true
        revealTourStop()
    }

    func refreshPanel() {
        guard markup else { return }
        panel.tour = tour
        panel.show(record.threads, active: active, waits: waits(), notes: notes())
    }

    func waits() -> [String: DocWait] { DocWait.all(record.threads, agent: agent, status: agentStatus) }

    func notes() -> [String: String] {
        var out: [String: String] = [:]
        for thread in record.threads where thread.isOpen {
            if anchors[thread.id] == nil {
                out[thread.id] = "its text is no longer in the doc"
            }
        }
        return out
    }

    func characterIndex(at point: NSPoint) -> Int? {
        guard let container = textView.textContainer, let storage = textView.textStorage, storage.length > 0 else { return nil }
        let origin = textView.textContainerOrigin
        let inner = NSPoint(x: point.x - origin.x, y: point.y - origin.y)
        let glyph = layoutManager.glyphIndex(for: inner, in: container)
        guard layoutManager.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1), in: container).insetBy(dx: -1, dy: -2).contains(inner) else { return nil }
        return layoutManager.characterIndexForGlyph(at: glyph)
    }

    func commentOnSelection() -> Bool {
        let range = textView.selectedRange()
        guard range.length > 0 else { return false }
        openComposer(range)
        return true
    }

    func commentOnWord(at point: NSPoint) -> Bool {
        guard let index = characterIndex(at: point) else { return false }
        let word = textView.selectionRange(forProposedRange: NSRange(location: index, length: 0), granularity: .selectByWord)
        guard word.length > 0 else { return false }
        openComposer(word)
        return true
    }

    func clicked(at point: NSPoint) {
        if let box, box.threadID == nil {
            if box.field.trimmed.isEmpty { closeBox() }
            return
        }
        guard let index = characterIndex(at: point) else { return closeBox() }
        let hits = record.threads.filter { thread in anchors[thread.id].map { index >= $0.location && index <= NSMaxRange($0) } ?? false }
        guard let thread = hits.first(where: \.isOpen) ?? hits.first else { return closeBox() }
        active = thread.id
        refreshMarks()
        panel.reveal(thread.id)
        openPop(thread.id)
    }

    var boxWidth: CGFloat { max(200, min(CommentBox.width, scrollView.contentView.bounds.width - 16)) }

    func boxOrigin(for range: NSRange, height: CGFloat) -> NSPoint {
        let origin = textView.textContainerOrigin
        let rects = layoutManager.markRects(range).map { $0.offsetBy(dx: origin.x, dy: origin.y) }
        let union = rects.reduce(NSRect.null) { $0.union($1) }
        let visible = scrollView.contentView.bounds
        let x = max(visible.minX + 8, min(union.minX, visible.maxX - boxWidth - 8))
        return NSPoint(x: x, y: union.maxY + 6)
    }

    func openComposer(_ selection: NSRange) {
        guard let storage = textView.textStorage else { return }
        if !showThreads { toggleThreads() }
        let whole = storage.string as NSString
        let raw = whole.substring(with: selection)
        let quote = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !quote.isEmpty else { return }
        let start = selection.location + (raw as NSString).range(of: quote).location
        let range = NSRange(location: start, length: (quote as NSString).length)
        closeBox()
        let hint = storage.attribute(.kannaLine, at: start, effectiveRange: nil) as? Int ?? 1
        let prefix = whole.substring(with: NSRange(location: max(0, start - 32), length: min(32, start)))
        let end = NSMaxRange(range)
        let suffix = whole.substring(with: NSRange(location: end, length: min(32, whole.length - end)))
        pending = (quote, prefix, suffix, Doc.line(of: quote, in: source, near: hint), range)
        textView.setSelectedRange(NSRange(location: start, length: 0))
        let box = CommentBox(composer: (), width: boxWidth)
        anchor(box, to: range)
        (box.button("post") as? Chip)?.onPress = { [weak self] in self?.post() }
        box.field.onKey = { [weak self] event in
            if event.keyCode == 36, event.modifierFlags.contains(.command) { self?.post(); return true }
            if event.keyCode == 53 { self?.closeBox(); return true }
            return false
        }
        show(box)
        refreshMarks()
    }

    func openPop(_ id: String) {
        guard let thread = record.threads.first(where: { $0.id == id }), let range = anchors[id] else { return }
        closeBox()
        let box = CommentBox(thread: thread, wait: waits()[id], note: notes()[id], width: boxWidth)
        anchor(box, to: range)
        (box.button("act") as? ActButton)?.onPress = { [weak self] in
            self?.closeBox()
            self?.setState(id, open: !thread.isOpen)
        }
        let send: () -> Void = { [weak self, weak box] in
            guard let box else { return }
            let text = box.field.trimmed
            guard !text.isEmpty else { return }
            self?.reply(id, text)
            self?.openPop(id)
        }
        (box.button("send") as? Chip)?.onPress = send
        box.field.onKey = { [weak self] event in
            if event.keyCode == 53 { self?.closeBox(); return true }
            guard event.keyCode == 36, !event.modifierFlags.contains(.shift) else { return false }
            send()
            return true
        }
        show(box)
    }

    private func anchor(_ box: CommentBox, to range: NSRange) {
        box.frame.origin = boxOrigin(for: range, height: box.frame.height)
        box.onResize = { [weak self] box in
            guard let self else { return }
            box.frame.origin = boxOrigin(for: range, height: box.frame.height)
            keepInView(box)
        }
    }

    private func keepInView(_ box: CommentBox) {
        textView.scrollToVisible(box.frame.insetBy(dx: 0, dy: -8))
        let visible = scrollView.contentView.bounds
        if box.frame.maxY > visible.maxY - 8 {
            box.frame.origin.y = max(visible.minY + 8, visible.maxY - 8 - box.frame.height)
        }
    }

    private func show(_ box: CommentBox) {
        self.box = box
        textView.addSubview(box)
        textView.frame.size = textView.frame.size
        keepInView(box)
        window?.makeFirstResponder(box.field)
        DispatchQueue.main.async { [weak self, weak box] in
            guard let box, self?.box === box else { return }
            self?.window?.makeFirstResponder(box.field)
        }
    }

    func closeBox() {
        guard let box else { return }
        self.box = nil
        let hadPending = pending != nil
        pending = nil
        box.removeFromSuperview()
        window?.makeFirstResponder(textView)
        if hadPending { refreshMarks() }
        showHeld()
    }

    func post() {
        guard let pending, let text = box.map({ Doc.clean($0.field.trimmed) }), !text.isEmpty else { return }
        closeBox()
        let thread = DocThread(id: "", quote: Doc.clean(pending.quote), prefix: pending.prefix, suffix: pending.suffix,
                               line: pending.line, state: "open", status: "waiting",
                               messages: [DocMessage(who: "you", text: text, at: Doc.now())], error: nil)
        change(.post(thread, mark: nil)) { [weak self] id in
            guard let self, let id else { return }
            active = id
            refreshMarks()
            deliver(id, text)
        }
    }

    func reply(_ id: String, _ text: String) {
        let text = Doc.clean(text)
        change(.edit(id, DocEdit(state: "open", status: "waiting", error: .some(nil), message: DocMessage(who: "you", text: text, at: Doc.now())))) { [weak self] _ in self?.deliver(id, text) }
    }

    func setState(_ id: String, open: Bool) {
        change(.edit(id, DocEdit(state: open ? "open" : "resolved")))
    }

    func reveal(_ id: String) {
        active = id
        closeBox()
        refreshMarks()
        guard let range = anchors[id], let rect = layoutManager.markRects(range).first else { return }
        let top = rect.minY + textView.textContainerOrigin.y - scrollView.contentView.bounds.height / 3
        textView.scroll(NSPoint(x: 0, y: max(0, top)))
    }

    private func deliver(_ id: String, _ text: String) {
        guard var thread = record.threads.first(where: { $0.id == id }), Doc.isThreadID(id) else { return }
        thread.line = Doc.line(of: thread.quote, in: source, near: thread.line)
        let shown = anchors[id] != nil
        let message = Doc.clean(Doc.message(thread, file: docName, text: text, quoted: shown))
        let finish: (DocDelivery) -> Void = { [weak self] delivery in
            self?.change(.edit(id, DocEdit(delivered: delivery, line: thread.line)))
        }
        guard let agent, let onSend else { return finish(.failed("no agent")) }
        onSend(agent, message, path, id, finish)
    }
}
