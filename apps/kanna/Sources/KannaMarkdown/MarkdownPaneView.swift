import AppKit
import KannaDocs
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
    public let markup: Bool
    public var keyView: NSView { textView }
    public var zoom: CGFloat { scrollView.magnification }
    public private(set) var renderMs: Double = 0
    public private(set) var parseMs: Double = 0

    let scrollView = MarkdownScrollView()
    let textView: MarkdownTextView
    let layoutManager = MarkdownLayoutManager()
    private let diagrams = DiagramCache()
    private var document = Document(parsing: "")
    private var stamp: (Date, Int)?
    private var poll: Timer?
    private var magnifyObserver: NSObjectProtocol?
    let bar = DocBar()
    let panel = ThreadPanel()
    let store: DocStore
    var record: DocRecord
    var storeStamp: Date?
    var source = ""
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

    public init(path: String, zoom: CGFloat = 1, markup: Bool = false) {
        self.path = path
        self.markup = markup
        store = DocStore(file: path)
        record = store.read()
        storeStamp = store.stamp
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
        if markup {
            addSubview(bar)
            addSubview(panel)
            setUpComments()
        }
        textView.delegate = self
        scrollView.onSwipe = { [weak self] back in self?.onHistory?(back) }
        magnifyObserver = NotificationCenter.default.addObserver(forName: NSScrollView.didEndLiveMagnifyNotification, object: scrollView, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.onZoom?() }
        }
        load()
        poll = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.reloadIfChanged() }
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
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
    }

    public func stop() {
        poll?.invalidate()
        poll = nil
        magnifyObserver.map(NotificationCenter.default.removeObserver)
        magnifyObserver = nil
    }

    public override func layout() {
        super.layout()
        let barHeight: CGFloat = markup ? 34 : 0
        let side = markup && threadsVisible ? ThreadPanel.width : 0
        bar.frame = NSRect(x: 0, y: bounds.height - barHeight, width: bounds.width, height: barHeight)
        scrollView.frame = NSRect(x: 0, y: 0, width: max(0, bounds.width - side), height: max(0, bounds.height - barHeight))
        panel.isHidden = side == 0
        if bar.threads.on != (side > 0) { bar.threads.on = side > 0 }
        panel.frame = NSRect(x: bounds.width - side, y: 0, width: side, height: max(0, bounds.height - barHeight))
        updateInset()
        if let y = startScrollY, bounds.height > 0 {
            startScrollY = nil
            scroll(toY: y)
        }
    }

    var threadsVisible: Bool { showThreads && (bounds.width >= ThreadPanel.width * 2 || forcedThreads) }

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
        let inset = max(28, (width - (MarkdownTheme.maxTextWidth + 10)) / 2)
        let size = NSSize(width: inset, height: 22)
        if textView.textContainerInset != size { textView.textContainerInset = size }
        textView.layoutWidth = width
        textView.tail = (scrollView.contentSize.height * 0.4).rounded()
        if textView.frame.width != width { textView.frame.size.width = width }
    }

    private func fileStamp() -> (Date, Int)? {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: path) else { return nil }
        return ((attributes[.modificationDate] as? Date) ?? .distantPast, (attributes[.size] as? Int) ?? 0)
    }

    private func reloadIfChanged() {
        let span = Perf.begin(.markdown, "markdown.check")
        defer { Perf.end(span) }
        reloadThreadsIfChanged()
        refreshPanel()
        guard pending == nil, let now = fileStamp() else { return }
        if let stamp, stamp == now { return }
        load()
    }

    private func load() {
        let span = Perf.begin(.markdown, "markdown.load")
        defer { Perf.end(span) }
        stamp = fileStamp()
        let text = FileManager.default.contents(atPath: path).map { String(decoding: $0, as: UTF8.self) } ?? ""
        source = text
        let start = Date()
        document = Document(parsing: MarkdownRenderer.fencingFrontMatter(text), options: [.disableSmartOpts])
        parseMs = Date().timeIntervalSince(start) * 1000
        render()
    }

    private func render() {
        let start = Date()
        let position = readingPosition()
        var renderer = MarkdownRenderer(folder: URL(fileURLWithPath: path).deletingLastPathComponent(), diagrams: diagrams)
        let text = renderer.render(document)
        diagrams.keep(only: Set(renderer.diagramSources))
        updateInset()
        textView.textStorage?.setAttributedString(text)
        if let position { restoreReadingPosition(position) }
        refreshMarks()
        renderMs = Date().timeIntervalSince(start) * 1000
    }

    public func zoom(by step: Int) {
        let steps = Self.zoomSteps, now = zoom
        let target = step == 0 ? 1 : step > 0 ? (steps.first { $0 > now + 0.001 } ?? steps.last!) : (steps.last { $0 < now - 0.001 } ?? steps.first!)
        let visible = scrollView.contentView.bounds
        scrollView.setMagnification(target, centeredAt: NSPoint(x: visible.midX, y: visible.midY))
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
        case showMarkdown(String)
        case reveal(URL)
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
        let url = (file.hasPrefix("/") ? URL(fileURLWithPath: file)
            : URL(fileURLWithPath: documentPath).deletingLastPathComponent().appendingPathComponent(file)).standardizedFileURL
        let real = url.resolvingSymlinksInPath()
        guard let type = (try? FileManager.default.attributesOfItem(atPath: real.path))?[.type] as? FileAttributeType else { return .ignore }
        if type == .typeRegular, ["md", "markdown", "mdown"].contains(real.pathExtension.lowercased()) {
            return .showMarkdown(real.path)
        }
        return .reveal(url)
    }

    var openWeb: (URL) -> Void = { NSWorkspace.shared.open($0) }
    var reveal: (URL) -> Void = { NSWorkspace.shared.activateFileViewerSelecting([$0]) }

    public func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        follow((link as? URL)?.absoluteString ?? (link as? String) ?? "")
        return true
    }

    func follow(_ target: String) {
        switch Self.action(for: target, from: path) {
        case .scroll(let fragment): scroll(toAnchor: fragment)
        case .openWeb(let url): openWeb(url)
        case .showMarkdown(let file): onOpenMarkdown?(file)
        case .reveal(let url): reveal(url)
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
