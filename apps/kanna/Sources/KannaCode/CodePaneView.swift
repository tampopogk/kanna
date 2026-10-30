import AppKit
import KannaOverview

@MainActor
public final class CodePaneView: NSView {
    public var onFocus: (() -> Void)? { didSet { textView.onFocus = onFocus } }
    public var contextMenu: (() -> NSMenu?)? { didSet { textView.contextMenu = contextMenu } }
    public let path: String
    public var keyView: NSView { textView }
    public private(set) var file = CodeFile(text: "")
    public private(set) var failure: String?
    public private(set) var loaded = false
    public private(set) var language: String?
    public private(set) var highlights = CodeHighlights()
    public var onHighlighted: (() -> Void)?
    public var onZoom: (() -> Void)?
    public var onHistory: ((Bool) -> Void)?
    public var zoom: CGFloat { fontSize / CodeTheme.fontSize }
    public var currentLine: Int { textView.currentLine }

    let scrollView = CodeScrollView()
    let textView: CodeTextView
    let gutter: CodeGutter
    let sticky = CodeStickyView()
    public let overview = OverviewView()
    public private(set) var overviewShown = false
    public private(set) var overviewWanted = false
    private var overviewStale = true
    private var overviewQueued = false
    private var settingObserver: NSObjectProtocol?
    private let layoutManager = NSLayoutManager()
    private var fontSize = CodeTheme.fontSize
    private var scrollObserver: NSObjectProtocol?
    private var pendingLine: Int?
    public private(set) var comment: CodeComment?
    private var box: CodeCommentBox?
    public var onTourStep: ((String) -> Void)?
    private var commentUnrevealed = false

    public init(path: String, line: Int?, zoom: CGFloat = 1, comment: CodeComment? = nil) {
        self.path = path
        self.comment = comment
        fontSize = Self.clamp((CodeTheme.fontSize * zoom * 2).rounded() / 2)
        let storage = NSTextStorage()
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        container.lineFragmentPadding = 0
        layoutManager.addTextContainer(container)
        layoutManager.allowsNonContiguousLayout = false
        textView = CodeTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 400), textContainer: container)
        gutter = CodeGutter(scrollView: scrollView, orientation: .verticalRuler)
        super.init(frame: .zero)
        layoutManager.delegate = self
        configure()
        gutter.textView = textView
        pendingLine = line
        commentUnrevealed = comment != nil
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func configure() {
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.importsGraphics = false
        textView.drawsBackground = true
        textView.backgroundColor = CodeTheme.background
        textView.textColor = CodeTheme.text
        textView.font = CodeTheme.font(size: fontSize)
        textView.isVerticallyResizable = false
        textView.isHorizontallyResizable = false
        textView.minSize = .zero
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.autoresizingMask = []
        textView.textContainerInset = NSSize(width: 0, height: 8)
        textView.selectedTextAttributes = [.backgroundColor: NSColor.selectedTextBackgroundColor]
        textView.onMarks = { [weak self] in
            self?.gutter.needsDisplay = true
            self?.updateSticky()
            self?.updateOverviewMarks()
        }
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = true
        scrollView.backgroundColor = CodeTheme.background
        scrollView.verticalRulerView = gutter
        scrollView.hasVerticalRuler = true
        scrollView.rulersVisible = true
        scrollView.contentView.postsBoundsChangedNotifications = true
        scrollObserver = NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: scrollView.contentView, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.gutter.needsDisplay = true
                self?.updateSticky()
                self?.updateOverviewViewport()
            }
        }
        textView.startFinder(in: scrollView)
        addSubview(scrollView)
        overview.background = CodeTheme.background
        overview.edge = CodeTheme.line
        overview.scrollTarget = scrollView
        overview.onScroll = { [weak self] top in self?.scrollOverview(to: top) }
        overview.isHidden = true
        addSubview(overview)
        settingObserver = NotificationCenter.default.addObserver(forName: OverviewSetting.changed, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.needsLayout = true }
        }
        sticky.onPick = { [weak self] line in self?.reveal(line: line, toTop: true) }
        scrollView.onTile = { [weak self] in self?.updateSticky() }
        textView.onMove = { [weak self] move in self?.perform(move) }
        addSubview(sticky)
    }

    public func stop() {
        scrollObserver.map(NotificationCenter.default.removeObserver)
        scrollObserver = nil
        settingObserver.map(NotificationCenter.default.removeObserver)
        settingObserver = nil
    }

    public override func layout() {
        super.layout()
        placeOverview()
        sizeText()
        placeComment()
        updateSticky()
        if loaded, let line = pendingLine, bounds.height > 0 {
            pendingLine = nil
            if comment != nil { revealComment() } else { reveal(line: line) }
        }
        updateOverviewViewport()
    }

    private func placeOverview() {
        var viewport = overviewViewport
        viewport.height = bounds.height - scrollView.contentView.contentInsets.top
        overviewWanted = loaded && failure == nil && OverviewLayout.wanted(enabled: OverviewSetting.enabled, viewport: viewport)
        overviewShown = overviewWanted && !OverviewLayout.shown([.overview], width: bounds.width, columnWidth: OverviewLayout.columnWidth(CodeTheme.font(size: fontSize))).isEmpty
        let width = overviewShown ? OverviewMetrics.width : 0
        scrollView.frame = NSRect(x: 0, y: 0, width: bounds.width - width, height: bounds.height)
        scrollView.hasVerticalScroller = !overviewShown
        overview.frame = NSRect(x: bounds.width - OverviewMetrics.width, y: 0, width: OverviewMetrics.width, height: bounds.height)
        overview.isHidden = !overviewShown
        if overviewShown, overviewStale { updateOverviewContent() }
    }

    private var overviewViewport: OverviewViewport {
        let clip = scrollView.contentView
        let used = layoutManager.usedRect(for: textView.textContainer!).height + textView.textContainerInset.height * 2
        return OverviewViewport(documentHeight: textView.frame.height, contentHeight: used, top: clip.bounds.minY + clip.contentInsets.top,
                                height: clip.bounds.height - clip.contentInsets.top)
    }

    private func updateOverviewViewport() {
        guard overviewShown else { return }
        overview.viewport = overviewViewport
    }

    private func overviewChanged() {
        overviewStale = true
        guard !overviewQueued else { return }
        overviewQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            overviewQueued = false
            if overviewShown, overviewStale { updateOverviewContent() } else { needsLayout = true }
        }
    }

    private func updateOverviewContent() {
        guard let storage = textView.textStorage else { return }
        overviewStale = false
        let string = NSAttributedString(attributedString: storage)
        var lines: [OverviewLine] = []
        lines.reserveCapacity(file.lineCount)
        for line in 1...max(1, file.lineCount) {
            guard let rect = textView.rect(forLine: line) else { continue }
            var range = textView.range(ofLine: line)
            if range.length > 0, (string.string as NSString).character(at: NSMaxRange(range) - 1) == 0x0a { range.length -= 1 }
            lines.append(OverviewLine(y: rect.minY, height: rect.height, range: range, background: textView.comment?.contains(line) == true ? CodeTheme.tint : nil))
        }
        overview.content = .text([OverviewText(string: string, lines: lines)], lineHeight: lineHeight)
        updateOverviewMarks()
        updateOverviewViewport()
    }

    private func updateOverviewMarks() {
        guard overviewShown else { return }
        var marks: [OverviewMark] = []
        func add(_ line: Int, _ kind: OverviewMarkKind, _ color: NSColor) {
            guard let rect = textView.rect(forLine: line) else { return }
            marks.append(OverviewMark(y: rect.minY, height: rect.height, kind: kind, color: color))
        }
        for line in textView.changed.sorted() { add(line, .changed, CodeTheme.changed) }
        if let comment = textView.comment, comment.first <= file.lineCount { for line in comment.first...min(comment.last, file.lineCount) { add(line, .comment, CodeTheme.tintEdge) } }
        var last = 0
        for match in textView.matches {
            let line = file.line(at: match.location)
            if line != last { add(line, .hit, CodeTheme.hit) }
            last = line
        }
        overview.marks = OverviewMark.runs(marks)
    }

    private func scrollOverview(to top: CGFloat) {
        let clip = scrollView.contentView
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: top - clip.contentInsets.top))
        scrollView.reflectScrolledClipView(clip)
    }

    static let smallest: CGFloat = 9
    static let largest: CGFloat = 32

    static func clamp(_ size: CGFloat) -> CGFloat { min(max(size, smallest), largest) }

    public func zoom(by step: Int) {
        let target = step == 0 ? CodeTheme.fontSize : Self.clamp(fontSize + CGFloat(step))
        guard target != fontSize else { return }
        let top = topLine
        fontSize = target
        textView.font = CodeTheme.font(size: fontSize)
        let attributes: [NSAttributedString.Key: Any] = [.font: CodeTheme.font(size: fontSize), .foregroundColor: CodeTheme.text]
        textView.finder.noteClientStringWillChange()
        textView.textStorage?.setAttributedString(NSAttributedString(string: file.text, attributes: attributes))
        apply(highlights)
        textView.gap = 0
        placeComment()
        sizeText()
        if let rect = textView.rect(forLine: top) {
            scrollView.contentView.scroll(to: NSPoint(x: -scrollView.contentView.contentInsets.left, y: max(0, rect.minY - textView.textContainerInset.height)))
            scrollView.reflectScrolledClipView(scrollView.contentView)
        }
        gutter.needsDisplay = true
        overviewChanged()
        needsLayout = true
        onZoom?()
    }

    public override func otherMouseDown(with event: NSEvent) {
        switch event.buttonNumber {
        case 3: onHistory?(true)
        case 4: onHistory?(false)
        default: super.otherMouseDown(with: event)
        }
    }

    public func show(text: String) {
        guard loaded else {
            loaded = true
            load(CodeFile(text: text), failure: nil)
            if let line = pendingLine { textView.currentLine = min(max(line, 1), file.lineCount) }
            needsLayout = true
            return
        }
        guard failure != nil || text != file.text else { return }
        let top = scrollView.contentView.bounds.origin
        let old = file
        load(CodeFile(text: text), failure: nil)
        textView.changed = file.changedLines(from: old)
        scrollView.contentView.scroll(to: top)
        scrollView.reflectScrolledClipView(scrollView.contentView)
    }

    public func show(failure message: String) {
        loaded = true
        pendingLine = nil
        load(CodeFile(text: ""), failure: message)
    }

    private func load(_ read: CodeFile, failure message: String?) {
        failure = message
        file = read
        textView.file = file
        textView.indents = CodeScopes.indents(file)
        textView.indentUnit = CodeScopes.unit(textView.indents)
        let attributes: [NSAttributedString.Key: Any] = [.font: CodeTheme.font(size: fontSize), .foregroundColor: CodeTheme.text]
        textView.finder.noteClientStringWillChange()
        textView.textStorage?.setAttributedString(NSAttributedString(string: file.text, attributes: attributes))
        gutter.file = file
        applyComment()
        sizeText()
        highlight()
        overviewChanged()
        needsLayout = true
    }

    private func applyComment() {
        box?.removeFromSuperview()
        box = nil
        textView.gap = 0
        let shown = comment.flatMap { failure == nil && $0.first <= file.lineCount ? $0 : nil }
        textView.comment = shown
        guard let shown else { return }
        let made = CodeCommentBox(comment: shown) { [weak self] to in self?.onTourStep?(to) }
        box = made
        textView.addSubview(made)
        placeComment()
    }

    private func placeComment() {
        guard let box, let comment = textView.comment, let storage = textView.textStorage else { return }
        let gap = box.height(width: commentWidth) + CodeCommentBox.margin * 2
        if gap != textView.gap {
            let start = file.start(ofLine: comment.first)
            let end = comment.first < file.lineCount ? file.start(ofLine: comment.first + 1) : storage.length
            let style = NSMutableParagraphStyle()
            style.paragraphSpacingBefore = gap
            storage.beginEditing()
            storage.removeAttribute(.paragraphStyle, range: NSRange(location: 0, length: storage.length))
            if end > start { storage.addAttribute(.paragraphStyle, value: style, range: NSRange(location: start, length: end - start)) }
            storage.endEditing()
            textView.gap = gap
            sizeText()
            overviewChanged()
        }
        placeBox()
        textView.needsDisplay = true
        gutter.needsDisplay = true
    }

    private func placeBox() {
        guard let box, let comment = textView.comment, let raw = textView.rawRect(forLine: comment.first) else { return }
        box.place(at: NSPoint(x: textView.textContainerInset.width + 8, y: raw.minY + CodeCommentBox.margin), width: commentWidth)
    }

    private var commentWidth: CGFloat { min(CodeCommentBox.widest, max(200, bounds.width - gutter.ruleThickness - textView.textContainerInset.width - 32)) }

    public func revealComment() {
        guard let comment = textView.comment, let raw = textView.rawRect(forLine: comment.first) else { return }
        let rows = min(CodeScopes.most, CodeScopes.open(at: comment.first, in: highlights.scopes).count)
        let clip = scrollView.contentView
        let y = raw.minY - clip.contentInsets.top - CGFloat(rows) * lineHeight
        clip.scroll(to: NSPoint(x: -clip.contentInsets.left, y: max(-clip.contentInsets.top, y)))
        scrollView.reflectScrolledClipView(clip)
        textView.currentLine = comment.first
    }

    private func highlight() {
        let text = file.text
        let firstLine = String(text.prefix { $0 != "\n" })
        language = CodeSyntax.language(for: path, firstLine: firstLine)
        highlights = CodeHighlights()
        guard let language else { return }
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let found = CodeSyntax.highlight(text, language: language)
            DispatchQueue.main.async {
                guard let self, self.file.text == text else { return }
                self.apply(found)
            }
        }
    }

    private func apply(_ found: CodeHighlights) {
        highlights = found
        guard let storage = textView.textStorage else { return }
        let length = storage.length
        let italic = CodeTheme.italic(size: fontSize)
        storage.beginEditing()
        for span in found.spans where NSMaxRange(span.range) <= length {
            guard let color = CodeTheme.color(capture: span.capture) else { continue }
            storage.addAttribute(.foregroundColor, value: color, range: span.range)
            if span.capture.hasPrefix("comment") { storage.addAttribute(.font, value: italic, range: span.range) }
        }
        storage.endEditing()
        overviewChanged()
        if commentUnrevealed, bounds.height > 0, pendingLine == nil {
            commentUnrevealed = false
            revealComment()
        }
        updateSticky()
        onHighlighted?()
    }

    private func sizeText() {
        guard let container = textView.textContainer else { return }
        layoutManager.ensureLayout(for: container)
        let used = layoutManager.usedRect(for: container)
        let visible = scrollView.contentSize
        let lineHeight = layoutManager.defaultLineHeight(for: CodeTheme.font(size: fontSize))
        let height = max(visible.height, used.height + textView.textContainerInset.height * 2 + max(0, visible.height - lineHeight * 3))
        textView.setFrameSize(NSSize(width: max(visible.width, used.width + 24), height: height))
        let thickness = gutter.thickness(font: CodeTheme.font(size: fontSize))
        if gutter.ruleThickness != thickness {
            gutter.ruleThickness = thickness
            scrollView.tile()
        }
    }

    var lineHeight: CGFloat { layoutManager.defaultLineHeight(for: CodeTheme.font(size: fontSize)) }

    public var pinned: [Int] { sticky.rows.map(\.line) }
    public var pinnedMarks: [(line: Int, marks: CodeMarks)] { sticky.rows.map { ($0.line, $0.marks) } }

    public func select(line: Int, column: Int, length: Int) {
        let span = textView.range(ofLine: min(max(line, 1), file.lineCount))
        let start = span.location + min(max(column, 0), span.length)
        textView.setSelectedRange(NSRange(location: start, length: min(max(length, 0), (textView.textStorage?.length ?? 0) - start)))
    }

    func updateSticky() {
        let top = topLine
        var rows = CodeScopes.pinned(top: top, in: highlights.scopes)
        if let comment = textView.comment, top < comment.first { rows = rows.filter { $0 < comment.first } }
        let clip = scrollView.contentView
        let height = CGFloat(rows.count) * lineHeight
        let edge = convert(clip.frame, from: scrollView).maxY - clip.contentInsets.top
        sticky.frame = NSRect(x: 0, y: edge - height, width: scrollView.frame.width, height: height)
        sticky.isHidden = rows.isEmpty
        sticky.gutterWidth = gutter.ruleThickness
        sticky.textX = clip.frame.minX - clip.bounds.minX + textView.textContainerInset.width
        sticky.lineHeight = lineHeight
        sticky.font = CodeTheme.font(size: fontSize)
        sticky.selectionColor = textView.selectionColor
        guard let storage = textView.textStorage else { return }
        sticky.rows = rows.compactMap { line in
            var range = textView.range(ofLine: line)
            guard range.location < storage.length || range.length == 0 else { return nil }
            if range.length > 0, (storage.string as NSString).character(at: NSMaxRange(range) - 1) == 0x0a { range.length -= 1 }
            let text = NSMutableAttributedString(attributedString: storage.attributedSubstring(from: range))
            text.removeAttribute(.paragraphStyle, range: NSRange(location: 0, length: text.length))
            let marks = textView.marks(forLine: line)
            return CodeStickyView.Row(line: line, text: text, marks: marks, selection: textView.spans(marks.selection, inLine: line), matches: textView.spans(marks.matches, inLine: line))
        }
        sticky.needsDisplay = true
    }

    public func reveal(line: Int, toTop: Bool) {
        guard let rect = textView.rect(forLine: CodeScopes.top(showing: line, in: highlights.scopes)) else { return }
        let clip = scrollView.contentView
        clip.scroll(to: NSPoint(x: -clip.contentInsets.left, y: rect.minY - textView.textContainerInset.height - clip.contentInsets.top))
        scrollView.reflectScrolledClipView(clip)
        textView.currentLine = min(max(line, 1), file.lineCount)
    }

    public func reveal(line: Int) {
        guard let rect = textView.rect(forLine: line) else { return }
        let clip = scrollView.contentView
        let visible = clip.bounds.height - clip.contentInsets.top
        clip.scroll(to: NSPoint(x: -clip.contentInsets.left, y: max(-clip.contentInsets.top, rect.minY - clip.contentInsets.top - visible / 3)))
        scrollView.reflectScrolledClipView(clip)
        textView.currentLine = min(max(line, 1), file.lineCount)
        textView.flash(line: textView.currentLine)
    }

    public func perform(_ move: CodeMove) {
        let clip = scrollView.contentView
        let visible = clip.bounds.height - clip.contentInsets.top
        switch move {
        case .lines(let count): scroll(by: CGFloat(count) * lineHeight)
        case .pages(let pages): scroll(by: CGFloat(pages) * visible)
        case .top: scroll(to: -clip.contentInsets.top)
        case .bottom: scroll(to: .greatestFiniteMagnitude)
        case .blank(let count):
            let lines = file.lines
            var line = anchor
            for _ in 0..<abs(count) {
                repeat { line += count > 0 ? 1 : -1 } while line > 1 && line < lines.count && !lines[line - 1].allSatisfy(\.isWhitespace)
            }
            reveal(line: min(max(line, 1), file.lineCount), toTop: true)
        case .scope(let count):
            let anchor = anchor
            let starts = Array(Set(highlights.scopes.map(\.first))).sorted()
            let after = starts.filter { $0 > anchor }, before = starts.filter { $0 < anchor }
            let target = count > 0 ? after.dropFirst(count - 1).first : before.dropLast(-count - 1).last
            if let target { reveal(line: target, toTop: true) }
        case .find(let forward):
            textView.finderAction(.showFindInterface)
            if !forward { textView.finderAction(.previousMatch) }
        case .match(let forward): textView.finderAction(forward ? .nextMatch : .previousMatch)
        case .stopFinding: textView.finderAction(.hideFindInterface)
        }
    }

    private var anchor: Int {
        let under = topLine + pinned.count
        let clip = scrollView.contentView
        guard let rect = textView.rect(forLine: textView.currentLine), textView.currentLine >= under, rect.maxY <= clip.bounds.maxY else { return under }
        return textView.currentLine
    }

    private func scroll(by delta: CGFloat) { scroll(to: scrollView.contentView.bounds.minY + delta) }

    private func scroll(to y: CGFloat) {
        let clip = scrollView.contentView
        textView.layoutManager?.ensureLayout(for: textView.textContainer!)
        let used = textView.layoutManager?.usedRect(for: textView.textContainer!).height ?? 0
        let end = max(-clip.contentInsets.top, used + textView.textContainerInset.height * 2 - clip.bounds.height)
        let highest = max(end, clip.bounds.minY)
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: min(max(y, -clip.contentInsets.top), highest)))
        scrollView.reflectScrolledClipView(clip)
    }

    public var debugNumbers: [String: Double] {
        let clip = scrollView.contentView
        return ["clip_top": Double(convert(clip.frame, from: scrollView).maxY), "height": Double(bounds.height), "sticky_top": Double(sticky.frame.maxY), "find_bar_height": Double(scrollView.findBarView?.frame.height ?? -1), "find_bar_y": Double(scrollView.findBarView.map { convert($0.frame, from: $0.superview).minY } ?? -1), "find_bar": scrollView.isFindBarVisible ? 1 : 0, "selection_line": Double(file.line(at: textView.selectedRange().location)), "selection_length": Double(textView.selectedRange().length), "zoom": Double(zoom), "font_size": Double(fontSize), "spans": Double(highlights.spans.count), "scopes": Double(highlights.scopes.count), "lines": Double(file.lineCount), "top": Double(topLine), "current": Double(currentLine), "gutter": Double(gutter.ruleThickness),
                "clip_x": Double(clip.frame.minX), "clip_width": Double(clip.frame.width), "scroll_x": Double(clip.bounds.minX),
                "scroll_y": Double(clip.bounds.minY), "changed": Double(textView.changed.min() ?? 0), "pinned": Double(pinned.count), "indent_unit": Double(textView.indentUnit), "text_x": Double(textView.frame.minX), "text_width": Double(textView.frame.width),
                "comment_first": Double(textView.comment?.first ?? 0), "comment_last": Double(textView.comment?.last ?? 0), "comment_gap": Double(textView.gap),
                "comment_box_height": Double(box?.frame.height ?? 0), "comment_box_top": Double(box?.frame.minY ?? -1), "comment_box_bottom": Double(box?.frame.maxY ?? -1),
                "comment_gap_top": Double(textView.comment.flatMap { textView.rawRect(forLine: $0.first)?.minY } ?? -1), "comment_line_top": Double(textView.comment.flatMap { textView.rect(forLine: $0.first)?.minY } ?? -1), "comment_box_visible": box.map { clip.bounds.intersects($0.frame) && $0.frame.minY >= clip.bounds.minY ? 1 : 0 } ?? 0]
    }

    public var commentText: String? { box?.text }

    public var tourFooter: (at: Int, of: Int, back: Bool, next: String, visible: Bool)? {
        guard let footer = box?.footer else { return nil }
        let clip = scrollView.contentView.bounds
        let frame = textView.convert(footer.next.frame, from: footer)
        return (footer.step.at, footer.step.of, footer.back.enabled, footer.next.title, clip.contains(frame))
    }

    public func pressTour(_ to: String) -> Bool {
        guard let footer = box?.footer else { return false }
        let button = to == "back" ? footer.back : footer.next
        guard button.enabled else { return false }
        button.click()
        return true
    }

    public var topLine: Int {
        let y = scrollView.contentView.bounds.minY + scrollView.contentView.contentInsets.top + textView.textContainerInset.height
        guard textView.textStorage?.length ?? 0 > 0 else { return 1 }
        let index = layoutManager.characterIndexForGlyph(at: layoutManager.glyphIndex(for: NSPoint(x: 0, y: y), in: textView.textContainer!))
        let line = file.line(at: index)
        if line == textView.comment?.first, let rect = textView.rect(forLine: line), y < rect.minY { return max(1, line - 1) }
        return line
    }
}

@MainActor
final class CodeScrollView: NSScrollView {
    var onTile: (() -> Void)?

    override func tile() {
        super.tile()
        onTile?()
    }
}

@MainActor
final class CodeTextView: NSTextView {
    var onFocus: (() -> Void)?
    var onMarks: (() -> Void)?
    var onMove: ((CodeMove) -> Void)?
    private var keys = CodeKeys()
    var contextMenu: (() -> NSMenu?)?
    var file = CodeFile(text: "")
    var indents: [Int] = []
    var indentUnit = 4
    var currentLine = 1 { didSet { if currentLine != oldValue { needsDisplay = true; onMarks?() } } }
    var comment: CodeComment? { didSet { needsDisplay = true; onMarks?() } }
    var changed: Set<Int> = [] { didSet { needsDisplay = true; onMarks?() } }
    var gap: CGFloat = 0
    let finder = NSTextFinder()
    private lazy var finderClient = CodeFinderClient(textView: self)
    private var matchObserver: NSKeyValueObservation?
    private var keyObservers: [NSObjectProtocol] = []
    private var flashing: Int? { didSet { needsDisplay = true; onMarks?() } }
    private var flashTask: DispatchWorkItem?

    func startFinder(in container: NSScrollView) {
        finder.client = finderClient
        finder.findBarContainer = container
        finder.isIncrementalSearchingEnabled = true
        finder.incrementalSearchingShouldDimContentView = false
        matchObserver = finder.observe(\.incrementalMatchRanges) { [weak self] _, _ in
            DispatchQueue.main.async {
                self?.needsDisplay = true
                self?.onMarks?()
            }
        }
    }

    var matches: [NSRange] { finder.incrementalMatchRanges.map(\.rangeValue) }

    var selectionColor: NSColor {
        guard let window, window.isKeyWindow, window.firstResponder === self else { return .unemphasizedSelectedTextBackgroundColor }
        return selectedTextAttributes[.backgroundColor] as? NSColor ?? .selectedTextBackgroundColor
    }

    func range(ofLine line: Int) -> NSRange {
        let length = textStorage?.length ?? 0
        let start = min(file.start(ofLine: line), length)
        let end = line < file.lineCount ? min(file.start(ofLine: line + 1), length) : length
        return NSRange(location: start, length: max(0, end - start))
    }

    func marks(forLine line: Int) -> CodeMarks {
        let span = range(ofLine: line)
        func local(_ ranges: [NSRange]) -> [NSRange] {
            ranges.compactMap { range in
                guard range.length > 0, let overlap = range.intersection(span), overlap.length > 0 else { return nil }
                return NSRange(location: overlap.location - span.location, length: overlap.length)
            }
        }
        return CodeMarks(selection: local(selectedRanges.map(\.rangeValue)), matches: local(matches), current: line == currentLine,
                         commented: comment?.contains(line) == true, dimmed: comment.map { !$0.contains(line) } ?? false,
                         changed: changed.contains(line), flashing: line == flashing)
    }

    func spans(_ ranges: [NSRange], inLine line: Int) -> [NSRect] {
        guard let manager = layoutManager, let container = textContainer, let storage = textStorage else { return [] }
        let span = range(ofLine: line)
        let newline = span.length > 0 && (storage.string as NSString).character(at: NSMaxRange(span) - 1) == 0x0a
        return ranges.map { local in
            let range = NSRange(location: span.location + local.location, length: local.length)
            var rect = manager.boundingRect(forGlyphRange: manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil), in: container)
            if newline, NSMaxRange(range) == NSMaxRange(span) { rect.size.width = max(bounds.width, visibleRect.maxX) - rect.minX }
            return rect
        }
    }

    private func lines(in rect: NSRect) -> ClosedRange<Int> {
        guard let manager = layoutManager, let container = textContainer, (textStorage?.length ?? 0) > 0 else { return 1...1 }
        let glyphs = manager.glyphRange(forBoundingRect: rect.offsetBy(dx: -textContainerOrigin.x, dy: -textContainerOrigin.y), in: container)
        let characters = manager.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        let first = file.line(at: characters.location)
        return first...max(first, file.line(at: max(characters.location, NSMaxRange(characters) - 1)))
    }

    func rect(forLine line: Int) -> NSRect? {
        guard var rect = rawRect(forLine: line) else { return nil }
        if line == comment?.first, gap > 0 {
            rect.origin.y += gap
            rect.size.height -= gap
        }
        return rect
    }

    func rawRect(forLine line: Int) -> NSRect? {
        guard let manager = layoutManager else { return nil }
        let length = textStorage?.length ?? 0
        guard length > 0 else { return NSRect(x: 0, y: textContainerInset.height, width: bounds.width, height: 0) }
        let start = min(file.start(ofLine: line), length - 1)
        let glyph = manager.glyphIndexForCharacter(at: start)
        var rect = manager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
        rect.origin.y += textContainerInset.height
        rect.origin.x = 0
        rect.size.width = max(bounds.width, visibleRect.maxX)
        return rect
    }

    func flash(line: Int) {
        flashing = line
        needsDisplay = true
        flashTask?.cancel()
        let task = DispatchWorkItem { [weak self] in self?.flashing = nil }
        flashTask = task
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.1, execute: task)
    }

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        for line in lines(in: rect) {
            guard let fragment = self.rect(forLine: line), fragment.intersects(rect) else { continue }
            let marks = marks(forLine: line)
            marks.fillLine(fragment)
            marks.drawMatches(spans(marks.matches, inLine: line).map { NSRect(x: textContainerOrigin.x + $0.minX, y: fragment.minY, width: $0.width, height: fragment.height) })
        }
        drawGuides(in: rect)
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard let comment, let top = rawRect(forLine: comment.first), let bottom = rect(forLine: comment.last) else { return }
        CodeTheme.dim.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: top.minY).intersection(dirtyRect).fill(using: .sourceOver)
        NSRect(x: 0, y: bottom.maxY, width: bounds.width, height: max(0, bounds.height - bottom.maxY)).intersection(dirtyRect).fill(using: .sourceOver)
    }

    private func drawGuides(in rect: NSRect) {
        guard !indents.isEmpty, (textStorage?.length ?? 0) > 0 else { return }
        let width = ("0" as NSString).size(withAttributes: [.font: font ?? CodeTheme.font(size: CodeTheme.fontSize)]).width
        CodeTheme.guide.setFill()
        for line in lines(in: rect) where line <= indents.count {
            guard let fragment = self.rect(forLine: line) else { continue }
            var column = 0
            while column < indents[line - 1] {
                NSRect(x: textContainerInset.width + CGFloat(column) * width, y: fragment.minY, width: 1, height: fragment.height).fill()
                column += indentUnit
            }
        }
    }

    override func mouseDown(with event: NSEvent) {
        super.mouseDown(with: event)
        if selectedRange().length == 0 { currentLine = file.line(at: selectedRange().location) }
    }

    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result {
            onFocus?()
            onMarks?()
        }
        return result
    }

    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        if result { onMarks?() }
        return result
    }

    override func setSelectedRanges(_ ranges: [NSValue], affinity: NSSelectionAffinity, stillSelecting: Bool) {
        super.setSelectedRanges(ranges, affinity: affinity, stillSelecting: stillSelecting)
        onMarks?()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        keyObservers.forEach(NotificationCenter.default.removeObserver)
        keyObservers = []
        guard let window else { return }
        keyObservers = [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification].map { name in
            NotificationCenter.default.addObserver(forName: name, object: window, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.onMarks?() }
            }
        }
    }

    func finderAction(_ action: NSTextFinder.Action) {
        finder.performAction(action)
    }

    override func performTextFinderAction(_ sender: Any?) {
        guard let tag = (sender as? NSValidatedUserInterfaceItem)?.tag, let action = NSTextFinder.Action(rawValue: tag) else { return }
        finder.performAction(action)
    }

    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(performTextFinderAction(_:)) { return NSTextFinder.Action(rawValue: item.tag).map(finder.validateAction) ?? false }
        return super.validateUserInterfaceItem(item)
    }

    override func validateMenuItem(_ item: NSMenuItem) -> Bool {
        if item.action == #selector(performTextFinderAction(_:)) { return validateUserInterfaceItem(item) }
        return super.validateMenuItem(item)
    }

    override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
        guard !modifiers.contains(.command), !modifiers.contains(.option) else { return super.keyDown(with: event) }
        if event.keyCode == 49, modifiers.contains(.shift) { return onMove?(.pages(-1)) ?? () }
        let control = modifiers.contains(.control)
        let characters = (control ? event.charactersIgnoringModifiers : event.characters) ?? ""
        if let move = keys.press(characters, control: control) { return onMove?(move) ?? () }
        if characters.count == 1, let key = characters.first, key.isLetter || key.isNumber || "[]{}/?".contains(key) { return }
        super.keyDown(with: event)
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        window?.makeFirstResponder(self)
        let menu = super.menu(for: event) ?? NSMenu()
        if let extra = contextMenu?() {
            menu.addItem(.separator())
            for item in extra.items {
                extra.removeItem(item)
                menu.addItem(item)
            }
        }
        return menu
    }
}

@MainActor
final class CodeGutter: NSRulerView {
    weak var textView: CodeTextView?
    var file = CodeFile(text: "") { didSet { needsDisplay = true } }

    override init(scrollView: NSScrollView?, orientation: NSRulerView.Orientation) {
        super.init(scrollView: scrollView, orientation: orientation)
        clientView = nil
    }

    required init(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }
    override var requiredThickness: CGFloat { ruleThickness }

    func thickness(font: NSFont) -> CGFloat {
        let digits = max(3, String(file.lineCount).count)
        let width = ("0" as NSString).size(withAttributes: [.font: font]).width
        return ceil(width * CGFloat(digits) + CodeTheme.gutterPadding * 2)
    }

    override func draw(_ dirtyRect: NSRect) {
        CodeTheme.background.setFill()
        bounds.fill()
        drawHashMarksAndLabels(in: dirtyRect)
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let textView, let manager = textView.layoutManager, let container = textView.textContainer, let storage = textView.textStorage else { return }
        let font = textView.font ?? CodeTheme.font(size: CodeTheme.fontSize)
        let visible = textView.visibleRect
        let glyphs = manager.glyphRange(forBoundingRect: visible, in: container)
        let characters = manager.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        let first = file.line(at: characters.location)
        let last = storage.length == 0 ? 1 : file.line(at: max(characters.location, NSMaxRange(characters) - 1))
        for line in first...max(first, last) {
            guard let fragment = textView.rect(forLine: line) else { continue }
            let y = convert(NSPoint(x: 0, y: fragment.minY), from: textView).y
            textView.marks(forLine: line).drawGutter(line: line, in: NSRect(x: 0, y: y, width: bounds.width, height: fragment.height), font: font)
        }
    }
}

extension CodePaneView: NSLayoutManagerDelegate {
    public nonisolated func layoutManager(_ manager: NSLayoutManager, didCompleteLayoutFor textContainer: NSTextContainer?, atEnd layoutFinishedFlag: Bool) {
        MainActor.assumeIsolated {
            guard let comment = textView.comment, self.layoutManager.firstUnlaidCharacterIndex() > file.start(ofLine: comment.first) else { return }
            placeBox()
        }
    }
}
