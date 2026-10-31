import AppKit
import KannaDocs
import KannaOverview
import KannaPerf
import Markdown

@MainActor
public final class MarkdownPaneView: NSView, NSTextViewDelegate {
    public static let zoomSteps: [CGFloat] = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3]

    public var onFocus: (() -> Void)? { didSet { textView.onFocus = onFocus } }
    public var onOpenMarkdown: ((String) -> Void)?
    public var onHistory: ((Bool) -> Void)?
    public var onZoom: (() -> Void)?
    public var startScrollY: CGFloat?
    public var contextMenu: (() -> NSMenu?)? { didSet { textView.contextMenu = contextMenu } }
    public let path: String
    public private(set) var markup: Bool
    public private(set) var loads = 0
    public let diffBase: String?
    public private(set) var diffError: String?
    public private(set) var changes: [NSRange] = []
    public var diffRange: (from: String, to: String)? { diffBase.flatMap(MarkdownDiff.range) }
    public var keyView: NSView { textView }
    public var zoom: CGFloat { scrollView.magnification }
    public private(set) var renderMs: Double = 0
    public private(set) var parseMs: Double = 0

    let scrollView = MarkdownScrollView()
    let textView: MarkdownTextView
    let layoutManager = MarkdownLayoutManager()
    public let overview = OverviewView()
    public internal(set) var overviewShown = false
    public internal(set) var overviewWanted = false
    public internal(set) var pictures = 0
    var revision = 0
    var drawnKey: MarkdownPictureKey?
    var pictureMarks: [OverviewMark] = []
    var overviewQueued = false
    var overviewObservers: [NSObjectProtocol] = []
    private let diagrams = DiagramCache()
    private var document = Document(parsing: "")
    private var entries: [DiffEntry]?
    private var disk: (text: String, compared: MarkdownComparison?)?
    private var held: (text: String, compared: MarkdownComparison?)?
    private var poll: Timer?
    private var magnifyObserver: NSObjectProtocol?
    let bar = DocBar()
    let panel = ThreadPanel()
    public var changeThreads: DocChanger?
    public var readFile: ((String, Bool, @escaping @MainActor (Result<MarkdownFile, MarkdownFile.Failure>) -> Void) -> Void)?
    public var canReveal = true
    public private(set) var docName: String
    var record: DocRecord
    var threadsLoaded = false
    var source = ""
    private var images: [String: (data: Data, image: NSImage)] = [:]
    private var fetchImages = false
    private var rerender = false
    var anchors: [String: NSRange] = [:]
    var active: String?
    var box: CommentBox?
    var pending: (quote: String, prefix: String, suffix: String, line: Int, range: NSRange)?
    var showThreads = true
    var forcedThreads = false
    public var agent: String? { didSet { agentChanged() } }
    public var agentStatus: String? { didSet { if agentStatus != oldValue { refreshPanel() } } }
    public var onSend: ((String, String, @escaping (String?) -> Void) -> Void)?
    public var onTourStep: ((String) -> Void)?
    var tourUnrevealed = false
    public var tour: DocTourStep? {
        didSet {
            guard tour != oldValue else { return }
            tourUnrevealed = tour != nil
            refreshMarks()
        }
    }

    public init(path: String, zoom: CGFloat = 1, markup: Bool = false, diff: String? = nil) {
        self.path = path
        self.markup = markup && diff.flatMap(MarkdownDiff.range) == nil
        diffBase = diff
        docName = (path as NSString).lastPathComponent
        record = DocRecord(file: path)
        let storage = NSTextStorage()
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: 600, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = true
        layoutManager.addTextContainer(container)
        textView = MarkdownTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 400), textContainer: container)
        super.init(frame: .zero)
        configureTextView()
        scrollView.contentView = CenteringClipView()
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = true
        scrollView.allowsMagnification = true
        scrollView.minMagnification = Self.zoomSteps.first!
        scrollView.maxMagnification = 4
        scrollView.magnification = zoom
        scrollView.backgroundColor = MarkdownTheme.background
        addSubview(scrollView)
        setUpOverview()
        bar.path.stringValue = docName
        bar.diff.isHidden = diff == nil
        bar.diff.onPress = { [weak self] in self?.jump(by: 1) }
        textView.onChangeKey = diff == nil ? nil : { [weak self] step in self?.jump(by: step) }
        showComments()
        textView.delegate = self
        scrollView.onSwipe = { [weak self] back in self?.onHistory?(back) }
        magnifyObserver = NotificationCenter.default.addObserver(forName: NSScrollView.didEndLiveMagnifyNotification, object: scrollView, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.needsLayout = true
                self?.onZoom?()
            }
        }
        poll = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.check() }
        }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func configureTextView() {
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = true
        textView.importsGraphics = false
        textView.drawsBackground = true
        textView.backgroundColor = MarkdownTheme.background
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.minSize = .zero
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.autoresizingMask = [.width]
        textView.linkTextAttributes = [.foregroundColor: MarkdownTheme.accent, .cursor: NSCursor.pointingHand]
        textView.selectedTextAttributes = [.backgroundColor: NSColor.selectedTextBackgroundColor]
        textView.startFinder(in: scrollView)
    }

    public func set(markup on: Bool) {
        let on = on && diffRange == nil
        guard on != markup else { return }
        let position = readingPosition()
        markup = on
        showComments()
        layoutSubtreeIfNeeded()
        if let position { restoreReadingPosition(position) }
    }

    private func showComments() {
        if markup || diffBase != nil {
            if bar.superview == nil { addSubview(bar) }
        } else {
            bar.removeFromSuperview()
        }
        bar.agent.isHidden = !markup
        bar.threads.isHidden = !markup
        if markup {
            if panel.superview == nil { addSubview(panel) }
            setUpComments()
            refreshMarks()
        } else {
            tearDownComments()
            panel.removeFromSuperview()
        }
        needsLayout = true
    }

    func showHeld() {
        if let held { show(text: held.text, compared: held.compared) }
    }

    public func stop() {
        poll?.invalidate()
        poll = nil
        magnifyObserver.map(NotificationCenter.default.removeObserver)
        magnifyObserver = nil
        stopOverview()
    }

    public override func layout() {
        super.layout()
        let barHeight: CGFloat = markup || diffBase != nil ? 34 : 0
        let height = max(0, bounds.height - barHeight)
        let shown = sides(height: height)
        let side = shown.comments ? ThreadPanel.width : 0
        let rule = shown.overview ? OverviewMetrics.width : 0
        overviewShown = shown.overview
        bar.frame = NSRect(x: 0, y: bounds.height - barHeight, width: bounds.width, height: barHeight)
        scrollView.frame = NSRect(x: 0, y: 0, width: max(0, bounds.width - side - rule), height: height)
        scrollView.hasVerticalScroller = !shown.overview
        overview.frame = NSRect(x: bounds.width - side - OverviewMetrics.width, y: 0, width: OverviewMetrics.width, height: height)
        overview.isHidden = !shown.overview
        panel.isHidden = side == 0
        if bar.threads.on != (side > 0) { bar.threads.on = side > 0 }
        panel.frame = NSRect(x: bounds.width - side, y: 0, width: side, height: height)
        updateInset()
        if overviewShown {
            if drawnKey != pictureKey { overviewChanged() }
            updateOverviewViewport()
        }
        if let y = startScrollY, bounds.height > 0 {
            startScrollY = nil
            scroll(toY: y)
        }
    }

    var threadsVisible: Bool { showThreads && sides().comments }

    public override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        overviewChanged()
    }

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(textView)
    }

    public override func otherMouseDown(with event: NSEvent) {
        switch event.buttonNumber {
        case 3: onHistory?(true)
        case 4: onHistory?(false)
        default: super.otherMouseDown(with: event)
        }
    }

    private func updateInset() {
        let width = scrollView.contentSize.width
        let inset = MarkdownOverviewLayout.inset(width)
        let size = NSSize(width: inset, height: 22)
        if textView.textContainerInset != size { textView.textContainerInset = size }
        textView.layoutWidth = width
        textView.tail = (scrollView.contentSize.height * 0.4).rounded()
        if textView.frame.width != width { textView.frame.size.width = width }
    }

    public func show(text: String, compared: MarkdownComparison? = nil) {
        guard pending == nil else { return held = (text, compared) }
        held = nil
        guard disk.map({ diffRange == nil && ($0.text != text || $0.compared != compared) }) ?? true else { return }
        disk = (text, compared)
        load(text, compared: compared)
    }

    public func show(name: String) {
        guard name != docName else { return }
        docName = name
        bar.path.stringValue = name
        bar.needsLayout = true
    }

    private func check() {
        let span = Perf.begin(.markdown, "markdown.check")
        defer { Perf.end(span) }
        refreshPanel()
    }

    private func load(_ text: String, compared: MarkdownComparison?) {
        let span = Perf.begin(.markdown, "markdown.load")
        defer { Perf.end(span) }
        var old: String??
        diffError = nil
        if diffRange != nil {
            switch compared {
            case .range(let before)?: old = .some(before)
            case .failed(let message)?: diffError = message
            default: break
            }
        }
        source = text
        loads += 1
        let start = Date()
        document = Document(parsing: MarkdownRenderer.fencingFrontMatter(text), options: [.disableSmartOpts])
        entries = nil
        if let old {
            entries = MarkdownDiff.entries(old: old.map { Document(parsing: MarkdownRenderer.fencingFrontMatter($0), options: [.disableSmartOpts]) }, new: document)
        } else if diffBase != nil, diffRange == nil {
            switch compared {
            case .base(let base)?:
                let old = base.map { Document(parsing: MarkdownRenderer.fencingFrontMatter($0), options: [.disableSmartOpts]) }
                entries = MarkdownDiff.entries(old: old, new: document)
            case .failed(let message)?: diffError = message
            default: break
            }
        }
        parseMs = Date().timeIntervalSince(start) * 1000
        fetchImages = true
        render()
    }

    private func render() {
        let start = Date()
        let position = readingPosition()
        var renderer = MarkdownRenderer(folder: URL(fileURLWithPath: path).deletingLastPathComponent(), diagrams: diagrams)
        renderer.images = images.mapValues(\.image)
        let text = entries.map { renderer.render($0) } ?? renderer.render(document)
        diagrams.keep(only: Set(renderer.diagramSources))
        if fetchImages {
            fetchImages = false
            fetch(Set(renderer.imagePaths))
        }
        updateInset()
        textView.finder.noteClientStringWillChange()
        textView.textStorage?.setAttributedString(text)
        revision += 1
        if let position { restoreReadingPosition(position) }
        refreshMarks()
        refreshChanges()
        needsLayout = true
        overviewChanged()
        renderMs = Date().timeIntervalSince(start) * 1000
    }

    private func fetch(_ paths: Set<String>) {
        guard let readFile else { return }
        for path in paths {
            readFile(path, true) { [weak self] result in
                guard let self else { return }
                switch result {
                case .success(let file):
                    guard let data = file.data, images[path]?.data != data else { return }
                    guard let image = NSImage(data: data) else {
                        guard images.removeValue(forKey: path) != nil else { return }
                        return renderSoon()
                    }
                    images[path] = (data, image)
                case .failure:
                    guard images.removeValue(forKey: path) != nil else { return }
                }
                renderSoon()
            }
        }
    }

    private func renderSoon() {
        guard !rerender else { return }
        rerender = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            rerender = false
            render()
        }
    }

    private func refreshChanges() {
        guard let diffBase, let storage = textView.textStorage else { return }
        changes = Self.changes(in: storage)
        let count = changes.count == 0 ? "no changes" : changes.count == 1 ? "1 change" : "\(changes.count) changes"
        let label = diffRange == nil ? "vs \(diffBase)" : diffBase
        bar.diff.set(diffError.map { "\(label) · \($0)" } ?? "\(label) · \(count)")
        bar.diff.warn = diffError != nil
        bar.needsLayout = true
    }

    static func changes(in text: NSAttributedString) -> [NSRange] {
        var out: [NSRange] = []
        let string = text.string as NSString
        text.enumerateAttribute(.kannaChange, in: NSRange(location: 0, length: text.length)) { value, range, _ in
            guard value != nil else { return }
            if let last = out.last, string.range(of: "\n", range: NSRange(location: NSMaxRange(last), length: range.location - NSMaxRange(last))).location == NSNotFound {
                out[out.count - 1] = NSUnionRange(last, range)
            } else {
                out.append(range)
            }
        }
        return out
    }

    public func jump(by step: Int) {
        guard step != 0, let container = textView.textContainer, !changes.isEmpty else { return }
        layoutManager.ensureLayout(for: container)
        let now = scrollView.contentView.bounds.minY
        let tops = changes.map { max(0, lineTop(at: $0.location) - 24) }
        let target = step > 0 ? tops.filter { $0 > now + 1 }.dropFirst(step - 1).first ?? tops.filter { $0 > now + 1 }.last
            : tops.filter { $0 < now - 1 }.dropLast(-step - 1).last ?? tops.filter { $0 < now - 1 }.first
        guard let target else { return }
        textView.scroll(NSPoint(x: 0, y: target))
    }

    public func zoom(by step: Int) {
        let steps = Self.zoomSteps, now = zoom
        let target = step == 0 ? 1 : step > 0 ? (steps.first { $0 > now + 0.001 } ?? steps.last!) : (steps.last { $0 < now - 0.001 } ?? steps.first!)
        let visible = scrollView.contentView.bounds
        scrollView.setMagnification(target, centeredAt: NSPoint(x: visible.midX, y: visible.midY))
        needsLayout = true
        onZoom?()
    }

    private struct ReadingPosition {
        let index: Int
        let offset: CGFloat
    }

    private func lineTop(at index: Int) -> CGFloat {
        let length = textView.textStorage?.length ?? 0
        guard length > 0 else { return 0 }
        let glyph = layoutManager.glyphIndexForCharacter(at: min(index, length - 1))
        return layoutManager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil).minY + textView.textContainerOrigin.y
    }

    private func readingPosition() -> ReadingPosition? {
        guard let container = textView.textContainer, (textView.textStorage?.length ?? 0) > 0 else { return nil }
        let visible = scrollView.contentView.bounds
        guard visible.minY > 1 else { return nil }
        layoutManager.ensureLayout(for: container)
        let point = NSPoint(x: 0, y: max(0, visible.minY - textView.textContainerOrigin.y))
        let index = layoutManager.characterIndexForGlyph(at: layoutManager.glyphIndex(for: point, in: container))
        return ReadingPosition(index: index, offset: lineTop(at: index) - visible.minY)
    }

    private func restoreReadingPosition(_ position: ReadingPosition) {
        guard let container = textView.textContainer else { return }
        layoutManager.ensureLayout(for: container)
        textView.scroll(NSPoint(x: 0, y: max(0, lineTop(at: position.index) - position.offset)))
    }

    @discardableResult
    public func scroll(toAnchor fragment: String) -> Bool {
        guard let storage = textView.textStorage else { return false }
        let wanted = fragment.removingPercentEncoding ?? fragment
        var found: Int?
        storage.enumerateAttribute(.kannaHeading, in: NSRange(location: 0, length: storage.length)) { value, range, stop in
            if value as? String == wanted { found = range.location; stop.pointee = true }
        }
        guard let found, let container = textView.textContainer else { return false }
        layoutManager.ensureLayout(for: container)
        textView.scroll(NSPoint(x: 0, y: max(0, lineTop(at: found) - 8)))
        return true
    }

    public enum LinkAction: Equatable {
        case scroll(String)
        case openWeb(URL)
        case file(URL)
        case ignore
    }

    public nonisolated static func action(for target: String, from documentPath: String) -> LinkAction {
        if target.hasPrefix("#") { return .scroll(String(target.dropFirst())) }
        if let url = URL(string: target), let scheme = url.scheme?.lowercased(), scheme != "file" {
            return ["http", "https", "mailto"].contains(scheme) ? .openWeb(url) : .ignore
        }
        var file = target.hasPrefix("file://") ? (URL(string: target)?.path ?? "") : target
        if let hash = file.firstIndex(of: "#") { file = String(file[..<hash]) }
        file = file.removingPercentEncoding ?? file
        guard !file.isEmpty else { return .ignore }
        return .file((file.hasPrefix("/") ? URL(fileURLWithPath: file)
            : URL(fileURLWithPath: documentPath).deletingLastPathComponent().appendingPathComponent(file)).standardizedFileURL)
    }

    var openWeb: (URL) -> Void = { NSWorkspace.shared.open($0) }
    var reveal: (URL) -> Void = { NSWorkspace.shared.activateFileViewerSelecting([$0]) }

    public func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        follow((link as? URL)?.absoluteString ?? (link as? String) ?? "")
        return true
    }

    public func follow(_ target: String) {
        switch Self.action(for: target, from: path) {
        case .scroll(let fragment): scroll(toAnchor: fragment)
        case .openWeb(let url): openWeb(url)
        case .file(let url):
            readFile?(url.path, false) { [weak self] result in
                guard let self, case .success(let file) = result else { return }
                if file.type == "file", ["md", "markdown", "mdown"].contains((file.path as NSString).pathExtension.lowercased()) {
                    onOpenMarkdown?(file.path)
                } else if canReveal {
                    reveal(url)
                }
            }
        case .ignore: break
        }
    }

    public var text: String {
        guard let storage = textView.textStorage else { return "" }
        var out = ""
        storage.enumerateAttributes(in: NSRange(location: 0, length: storage.length)) { attributes, range, _ in
            if let attachment = attributes[.attachment] as? NSTextAttachment {
                out += attachment.attachmentCell is DiagramCell ? "[diagram]" : "[image]"
            } else {
                out += (storage.string as NSString).substring(with: range)
            }
        }
        return out
    }

    public var drawnDiagrams: [(type: String, labels: [String])] {
        guard let storage = textView.textStorage else { return [] }
        var out: [(String, [String])] = []
        storage.enumerateAttribute(.attachment, in: NSRange(location: 0, length: storage.length)) { value, _, _ in
            if let cell = (value as? NSTextAttachment)?.attachmentCell as? DiagramCell { out.append((cell.diagramType, cell.scene.texts)) }
        }
        return out
    }

    public var layoutWidth: CGFloat { textView.textContainer?.size.width ?? 0 }

    public var scrollTop: CGFloat { scrollView.contentView.bounds.minY }

    public func scroll(toY y: CGFloat) { textView.scroll(NSPoint(x: 0, y: max(0, y))) }
}

public struct MarkdownFile: Equatable, Sendable {
    public var path: String
    public var type: String
    public var data: Data?

    public init(path: String, type: String, data: Data?) {
        self.path = path
        self.type = type
        self.data = data
    }

    public struct Failure: Error, Equatable {
        public var message: String

        public init(_ message: String) { self.message = message }
    }
}

public enum MarkdownComparison: Equatable, Sendable {
    case base(String?)
    case range(String?)
    case failed(String)
}

final class MarkdownScrollView: NSScrollView {
    var onSwipe: ((Bool) -> Void)?

    static func swipes(back: Bool, visible: NSRect, page: NSRect) -> Bool {
        back ? visible.minX <= page.minX + 1 : visible.maxX >= page.maxX - 1
    }

    override func scrollWheel(with event: NSEvent) {
        guard event.phase == .began, NSEvent.isSwipeTrackingFromScrollEventsEnabled, onSwipe != nil,
              abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY), let page = documentView?.frame else {
            return super.scrollWheel(with: event)
        }
        let back = (event.isDirectionInvertedFromDevice ? event.scrollingDeltaX : -event.scrollingDeltaX) > 0
        guard Self.swipes(back: back, visible: contentView.bounds, page: page) else { return super.scrollWheel(with: event) }
        event.trackSwipeEvent(options: [.lockDirection, .clampGestureAmount], dampenAmountThresholdMin: -1, max: 1) { [weak self] _, phase, _, _ in
            guard phase == .ended else { return }
            MainActor.assumeIsolated { self?.onSwipe?(back) }
        }
    }
}

private final class CenteringClipView: NSClipView {
    override func constrainBoundsRect(_ proposedBounds: NSRect) -> NSRect {
        var rect = super.constrainBoundsRect(proposedBounds)
        if let page = documentView?.frame, rect.width > page.width { rect.origin.x = (page.width - rect.width) / 2 }
        return rect
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(documentView)
        super.mouseDown(with: event)
    }
}

public final class MarkdownTextView: NSTextView {
    var onFocus: (() -> Void)?
    var onCommandClick: ((NSPoint) -> Bool)?
    var onPlainClick: ((NSPoint) -> Void)?
    var onCommentKey: (() -> Bool)?
    var onChangeKey: ((Int) -> Void)?
    var onMatches: (() -> Void)?
    let finder = NSTextFinder()
    private lazy var finderClient = TextFinderClient(textView: self)
    private var matchObserver: NSKeyValueObservation?
    private var bracket: Character?
    private var count = ""

    func startFinder(in container: NSScrollView) {
        finder.client = finderClient
        finder.findBarContainer = container
        finder.isIncrementalSearchingEnabled = true
        finder.incrementalSearchingShouldDimContentView = false
        matchObserver = finder.observe(\.incrementalMatchRanges) { [weak self] _, _ in
            DispatchQueue.main.async {
                self?.needsDisplay = true
                self?.onMatches?()
            }
        }
    }

    var matches: [NSRange] { finder.incrementalMatchRanges.map(\.rangeValue) }

    public override func performTextFinderAction(_ sender: Any?) {
        guard let tag = (sender as? NSValidatedUserInterfaceItem)?.tag, let action = NSTextFinder.Action(rawValue: tag) else { return }
        finder.performAction(action)
    }

    public override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(performTextFinderAction(_:)) { return NSTextFinder.Action(rawValue: item.tag).map(finder.validateAction) ?? false }
        return super.validateUserInterfaceItem(item)
    }

    public override func validateMenuItem(_ item: NSMenuItem) -> Bool {
        if item.action == #selector(performTextFinderAction(_:)) { return validateUserInterfaceItem(item) }
        return super.validateMenuItem(item)
    }

    public override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags.intersection([.command, .control, .option])
        guard let onChangeKey, modifiers.isEmpty, let characters = event.characters, characters.count == 1, let key = characters.first else {
            bracket = nil
            count = ""
            return super.keyDown(with: event)
        }
        if let open = bracket {
            bracket = nil
            let times = max(1, Int(count) ?? 1)
            count = ""
            if key == "c" { return onChangeKey(open == "]" ? times : -times) }
        }
        if key == "]" || key == "[" { return bracket = key }
        if key.isNumber, key != "0" || !count.isEmpty { return count.append(key) }
        count = ""
        super.keyDown(with: event)
    }

    public override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if event.modifierFlags.contains(.command), onCommandClick?(point) == true { return }
        super.mouseDown(with: event)
        if selectedRange().length == 0 { onPlainClick?(point) }
    }

    public override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.keyCode == 36, event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command, window?.firstResponder === self, onCommentKey?() == true { return true }
        return super.performKeyEquivalent(with: event)
    }
    var contextMenu: (() -> NSMenu?)?
    var layoutWidth: CGFloat = 0
    var tail: CGFloat = 0

    public override func setFrameSize(_ newSize: NSSize) {
        var height = newSize.height
        if let manager = layoutManager, let container = textContainer {
            height = max(height, ceil(manager.usedRect(for: container).height + textContainerInset.height * 2 + tail))
        }
        super.setFrameSize(NSSize(width: layoutWidth > 0 ? layoutWidth : newSize.width, height: height))
    }

    public override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        (layoutManager as? MarkdownLayoutManager)?.drawChangeBars(in: rect, origin: textContainerOrigin)
        drawMatches(in: rect)
    }

    private func drawMatches(in rect: NSRect) {
        guard let manager = layoutManager, let container = textContainer, !matches.isEmpty else { return }
        let origin = textContainerOrigin
        let glyphs = manager.glyphRange(forBoundingRect: rect.offsetBy(dx: -origin.x, dy: -origin.y), in: container)
        let visible = manager.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        for match in matches where NSIntersectionRange(match, visible).length > 0 {
            manager.enumerateEnclosingRects(forGlyphRange: manager.glyphRange(forCharacterRange: match, actualCharacterRange: nil),
                                            withinSelectedGlyphRange: NSRange(location: NSNotFound, length: 0), in: container) { box, _ in
                NSTextFinder.drawIncrementalMatchHighlight(in: box.offsetBy(dx: origin.x, dy: origin.y))
            }
        }
    }

    public override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result { onFocus?() }
        return result
    }

    public override func menu(for event: NSEvent) -> NSMenu? {
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
