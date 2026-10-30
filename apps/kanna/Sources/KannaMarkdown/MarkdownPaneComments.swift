import AppKit
import KannaDocs

extension MarkdownPaneView {
    func setUpComments() {
        bar.path.stringValue = store.relativeName
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

    func agentChanged() {
        guard markup else { return }
        bar.agent.set(agent.map { "Agent: \($0)" } ?? "No agent: comments are saved, not sent")
        bar.agent.warn = agent == nil
        bar.needsLayout = true
        if let agent, record.agent != agent {
            try? store.update { $0.agent = agent }
            storeStamp = store.stamp
            record = store.read()
        }
    }

    public var threads: [DocThread] { record.threads }
    public var commentBoxText: String? { box?.field.string }
    public var commentBoxParts: [String: NSRect]? { box?.parts }
    public var commentBoxShown: Bool { box.map { scrollView.contentView.bounds.contains($0.frame) } ?? false }
    public var commentBoxThread: String? { box.map { $0.threadID ?? "new" } }
    public var activeThread: String? { active }
    public var threadsShown: Bool { threadsVisible }
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
        if bounds.width < ThreadPanel.width * 2 {
            forcedThreads = !threadsVisible
            showThreads = forcedThreads
        } else {
            showThreads.toggle()
            forcedThreads = false
        }
        bar.threads.on = threadsVisible
        needsLayout = true
    }

    func reloadThreadsIfChanged() {
        let stamp = store.stamp
        guard stamp != storeStamp else { return }
        storeStamp = stamp
        let known = Set(record.threads.map(\.id))
        record = store.read()
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
            if let range = DocStore.locate(thread, in: text) { anchors[thread.id] = range }
        }
        var marks = record.threads.compactMap { thread in anchors[thread.id].map { DocMark(range: $0, kind: thread.isOpen ? .open : .resolved) } }
        if let pending { marks.append(DocMark(range: pending.range, kind: .active)) } else if let active, let range = anchors[active] { marks.append(DocMark(range: range, kind: .active)) }
        layoutManager.marks = marks
        textView.needsDisplay = true
        refreshPanel()
        bar.threads.set("Threads \(record.threads.filter(\.isOpen).count)")
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
        pending = (quote, prefix, suffix, DocStore.line(of: quote, in: source, near: hint), range)
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
        guard let pending, let text = box.map({ DocStore.clean($0.field.trimmed) }), !text.isEmpty else { return }
        closeBox()
        let id = try? store.update { record -> String in
            let id = "c\(record.next)"
            record.next += 1
            record.threads.append(DocThread(id: id, quote: DocStore.clean(pending.quote), prefix: pending.prefix, suffix: pending.suffix,
                                            line: pending.line, state: "open", status: "sent",
                                            messages: [DocMessage(who: "you", text: text, at: DocStore.now())], error: nil))
            return id
        }
        guard let id else { return }
        active = id
        reloadThreads()
        deliver(id, text)
    }

    func reply(_ id: String, _ text: String) {
        let text = DocStore.clean(text)
        try? store.update { record in
            guard let index = record.threads.firstIndex(where: { $0.id == id }) else { return }
            record.threads[index].messages.append(DocMessage(who: "you", text: text, at: DocStore.now()))
            record.threads[index].state = "open"
        }
        reloadThreads()
        deliver(id, text)
    }

    func setState(_ id: String, open: Bool) {
        try? store.update { record in
            guard let index = record.threads.firstIndex(where: { $0.id == id }) else { return }
            record.threads[index].state = open ? "open" : "resolved"
        }
        reloadThreads()
    }

    func reveal(_ id: String) {
        active = id
        closeBox()
        refreshMarks()
        guard let range = anchors[id], let rect = layoutManager.markRects(range).first else { return }
        let top = rect.minY + textView.textContainerOrigin.y - scrollView.contentView.bounds.height / 3
        textView.scroll(NSPoint(x: 0, y: max(0, top)))
    }

    private func reloadThreads() {
        record = store.read()
        storeStamp = store.stamp
        refreshMarks()
    }

    private func deliver(_ id: String, _ text: String) {
        guard var thread = record.threads.first(where: { $0.id == id }), DocStore.isThreadID(id) else { return }
        thread.line = DocStore.line(of: thread.quote, in: source, near: thread.line)
        let shown = anchors[id] != nil
        let message = DocStore.clean(DocStore.message(thread, file: store.relativeName, text: text, quoted: shown))
        let finish: (String?) -> Void = { [weak self] error in
            guard let self else { return }
            try? store.update { record in
                guard let index = record.threads.firstIndex(where: { $0.id == id }) else { return }
                record.threads[index].status = error == nil ? "sent" : "unsent"
                record.threads[index].error = error
                record.threads[index].line = thread.line
            }
            reloadThreads()
        }
        guard let agent, let onSend else { return finish("no agent") }
        onSend(agent, message, finish)
    }
}
