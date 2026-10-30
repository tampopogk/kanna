import AppKit
import KannaCode

@MainActor
final class DiffScrollView: NSScrollView {
    var onTile: (() -> Void)?

    override func tile() {
        super.tile()
        onTile?()
    }
}

@MainActor
final class DiffColumn {
    enum Numbers {
        case both
        case old
        case new
    }

    let scrollView = DiffScrollView()
    let textView: DiffTextView
    let gutter: DiffGutter
    private let layoutManager = NSLayoutManager()

    init(numbers: Numbers) {
        let storage = NSTextStorage()
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        container.lineFragmentPadding = 0
        layoutManager.addTextContainer(container)
        layoutManager.allowsNonContiguousLayout = true
        textView = DiffTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 400), textContainer: container)
        gutter = DiffGutter(scrollView: scrollView, orientation: .verticalRuler)
        gutter.numbers = numbers
        gutter.textView = textView
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.importsGraphics = false
        textView.drawsBackground = true
        textView.backgroundColor = DiffTheme.background
        textView.isVerticallyResizable = false
        textView.isHorizontallyResizable = false
        textView.minSize = .zero
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.autoresizingMask = []
        textView.textContainerInset = NSSize(width: 0, height: 0)
        textView.selectedTextAttributes = [.backgroundColor: NSColor.selectedTextBackgroundColor]
        textView.usesFindBar = false
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = true
        scrollView.backgroundColor = DiffTheme.background
        scrollView.verticalRulerView = gutter
        scrollView.hasVerticalRuler = true
        scrollView.rulersVisible = true
        scrollView.contentView.postsBoundsChangedNotifications = true
        textView.startFinder(in: scrollView)
    }

    func show(_ rows: [DiffRow], files: [DiffFile], text: NSAttributedString, size: CGFloat) {
        textView.rows = rows
        textView.files = files
        var starts: [Int] = [0]
        let string = text.string as NSString
        var location = 0
        while starts.count < rows.count {
            let found = string.range(of: "\n", options: .literal, range: NSRange(location: location, length: string.length - location))
            guard found.location != NSNotFound else { break }
            location = NSMaxRange(found)
            starts.append(location)
        }
        textView.starts = starts
        textView.font = CodeTheme.font(size: size)
        textView.finder.noteClientStringWillChange()
        textView.textStorage?.setAttributedString(text)
        gutter.needsDisplay = true
    }

    var usedHeight: CGFloat {
        guard let container = textView.textContainer else { return 0 }
        layoutManager.ensureLayout(for: container)
        return layoutManager.usedRect(for: container).height
    }

    func size(minimumHeight: CGFloat, font: NSFont) {
        guard let container = textView.textContainer else { return }
        layoutManager.ensureLayout(for: container)
        let used = layoutManager.usedRect(for: container)
        let visible = scrollView.contentSize
        textView.setFrameSize(NSSize(width: max(visible.width, used.width + 24), height: max(visible.height, used.height + minimumHeight)))
        let thickness = gutter.thickness(font: font)
        if gutter.ruleThickness != thickness {
            gutter.ruleThickness = thickness
            scrollView.tile()
            placed = false
        }
        let clip = scrollView.contentView
        if !placed, clip.contentInsets.left > 0 {
            placed = true
            clip.scroll(to: NSPoint(x: -clip.contentInsets.left, y: max(-clip.contentInsets.top, clip.bounds.minY)))
            scrollView.reflectScrolledClipView(clip)
        }
    }

    private var placed = false
}

@MainActor
final class DiffTextView: NSTextView {
    var rows: [DiffRow] = []
    var files: [DiffFile] = []
    var current: Int? { didSet { if current != oldValue { needsDisplay = true; onMarks?() } } }
    var fresh: Set<Int> = [] { didSet { needsDisplay = true; onMarks?() } }
    var onFocus: (() -> Void)?
    var onMarks: (() -> Void)?
    var onKey: ((String) -> Bool)?
    var onClickRow: ((Int) -> Void)?
    var onHeader: ((Int, Bool) -> Void)?
    var onLink: ((String) -> Void)?
    var contextMenu: (() -> NSMenu?)?
    var onMatches: (() -> Void)?
    let finder = NSTextFinder()
    private lazy var finderClient = CodeFinderClient(textView: self)
    private var matchObserver: NSKeyValueObservation?

    var starts: [Int] = []

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

    func start(ofRow row: Int) -> Int {
        guard !starts.isEmpty else { return 0 }
        return starts[min(max(row, 0), starts.count - 1)]
    }

    func row(at characterIndex: Int) -> Int {
        guard !starts.isEmpty else { return 0 }
        var low = 0
        var high = starts.count - 1
        while low < high {
            let mid = (low + high + 1) / 2
            if starts[mid] <= characterIndex { low = mid } else { high = mid - 1 }
        }
        return low
    }

    func rect(ofRow row: Int) -> NSRect? {
        guard let manager = layoutManager, let storage = textStorage, row >= 0, row < rows.count else { return nil }
        guard storage.length > 0 else { return nil }
        let glyph = manager.glyphIndexForCharacter(at: min(start(ofRow: row), storage.length - 1))
        var rect = manager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
        rect.origin.x = 0
        rect.size.width = max(bounds.width, visibleRect.maxX)
        return rect
    }

    func visibleRows(in rect: NSRect) -> ClosedRange<Int>? {
        guard let manager = layoutManager, let container = textContainer, (textStorage?.length ?? 0) > 0, !rows.isEmpty else { return nil }
        let glyphs = manager.glyphRange(forBoundingRect: rect, in: container)
        let characters = manager.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        let first = row(at: characters.location)
        let last = row(at: max(characters.location, NSMaxRange(characters) - 1))
        return first...max(first, last)
    }

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard let visible = visibleRows(in: rect) else { return }
        var cache: [Int: Int] = [:]
        for index in visible where index < rows.count {
            guard let fragment = rowRect(index, cache: &cache), fragment.intersects(rect) else { continue }
            let row = rows[index]
            switch row.kind {
            case .added: DiffTheme.addedLine.setFill(); fragment.fill(using: .sourceOver)
            case .removed: DiffTheme.removedLine.setFill(); fragment.fill(using: .sourceOver)
            case .header: DiffTheme.panel.setFill(); fragment.fill()
            case .fold, .note: DiffTheme.fold.setFill(); fragment.fill()
            case .blank: drawHatch(fragment)
            case .context: break
            }
            if let word = row.word, let manager = layoutManager, let container = textContainer, let storage = textStorage,
               start(ofRow: index) + NSMaxRange(word) <= storage.length {
                let range = NSRange(location: start(ofRow: index) + word.location, length: word.length)
                var box = manager.boundingRect(forGlyphRange: manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil), in: container)
                box.origin.y = fragment.minY
                box.size.height = fragment.height
                (row.kind == .added ? DiffTheme.addedWord : DiffTheme.removedWord).setFill()
                NSBezierPath(roundedRect: box.insetBy(dx: -1, dy: 1), xRadius: 2, yRadius: 2).fill()
            }
            if row.kind == .header {
                DiffTheme.line.setFill()
                NSRect(x: fragment.minX, y: fragment.maxY - 1, width: fragment.width, height: 1).fill()
                NSRect(x: fragment.minX, y: fragment.minY, width: fragment.width, height: 1).fill()
            }
            if index == current {
                DiffTheme.tint.setFill()
                fragment.fill(using: .sourceOver)
            }
            if fresh.contains(index) {
                DiffTheme.accent.setFill()
                NSRect(x: 0, y: fragment.minY, width: 3, height: fragment.height).fill()
            }
        }
        drawMatches(in: rect)
    }

    private func drawMatches(in rect: NSRect) {
        guard let manager = layoutManager, let container = textContainer else { return }
        let glyphs = manager.glyphRange(forBoundingRect: rect, in: container)
        let visible = manager.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        for match in matches where NSIntersectionRange(match, visible).length > 0 {
            manager.enumerateEnclosingRects(forGlyphRange: manager.glyphRange(forCharacterRange: match, actualCharacterRange: nil),
                                            withinSelectedGlyphRange: NSRange(location: NSNotFound, length: 0), in: container) { box, _ in
                NSTextFinder.drawIncrementalMatchHighlight(in: box)
            }
        }
    }

    private func rowRect(_ index: Int, cache: inout [Int: Int]) -> NSRect? {
        guard let manager = layoutManager, let storage = textStorage, storage.length > 0 else { return nil }
        let start = start(ofRow: index)
        cache[index] = start
        let glyph = manager.glyphIndexForCharacter(at: min(start, storage.length - 1))
        var rect = manager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
        rect.origin.x = 0
        rect.size.width = max(bounds.width, visibleRect.maxX)
        return rect
    }

    private func drawHatch(_ rect: NSRect) {
        NSGraphicsContext.saveGraphicsState()
        rect.clip()
        DiffTheme.fold.setStroke()
        let path = NSBezierPath()
        path.lineWidth = 5
        var x = rect.minX - rect.height
        while x < rect.maxX {
            path.move(to: NSPoint(x: x, y: rect.maxY))
            path.line(to: NSPoint(x: x + rect.height, y: rect.minY))
            x += 10
        }
        path.stroke()
        NSGraphicsContext.restoreGraphicsState()
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let manager = layoutManager, let container = textContainer, (textStorage?.length ?? 0) > 0 {
            let glyph = manager.glyphIndex(for: NSPoint(x: 0, y: point.y), in: container)
            let index = row(at: manager.characterIndexForGlyph(at: glyph))
            if let rect = rect(ofRow: index), rect.minY <= point.y, point.y < rect.maxY, rows[safe: index]?.kind == .header {
                onHeader?(index, event.modifierFlags.contains(.option))
                return
            }
        }
        super.mouseDown(with: event)
        if selectedRange().length == 0 { onClickRow?(row(at: selectedRange().location)) }
    }

    override func clicked(onLink link: Any, at charIndex: Int) {
        if let link = link as? String { onLink?(link) }
    }

    override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags.intersection([.command, .control, .option])
        if modifiers.isEmpty, let characters = event.characters, onKey?(event.keyCode == 36 ? "\r" : characters) == true { return }
        super.keyDown(with: event)
    }

    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result { onFocus?() }
        return result
    }

    override func menu(for event: NSEvent) -> NSMenu? {
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
final class DiffGutter: NSRulerView {
    weak var textView: DiffTextView?
    var numbers: DiffColumn.Numbers = .both

    override init(scrollView: NSScrollView?, orientation: NSRulerView.Orientation) {
        super.init(scrollView: scrollView, orientation: orientation)
        clientView = nil
    }

    required init(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }
    override var requiredThickness: CGFloat { ruleThickness }

    private var digits: Int {
        let largest = textView?.rows.reduce(0) { max($0, $1.old ?? 0, $1.new ?? 0) } ?? 0
        return max(3, String(largest).count)
    }

    private func digitWidth(_ font: NSFont) -> CGFloat { ("0" as NSString).size(withAttributes: [.font: font]).width }

    func thickness(font: NSFont) -> CGFloat {
        let column = digitWidth(font) * CGFloat(digits) + 16
        return ceil(column * (numbers == .both ? 2 : 1) + digitWidth(font) * 2 + 4)
    }

    override func draw(_ dirtyRect: NSRect) {
        DiffTheme.background.setFill()
        bounds.fill()
        drawHashMarksAndLabels(in: dirtyRect)
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let textView, let visible = textView.visibleRows(in: textView.visibleRect) else { return }
        let font = textView.font ?? CodeTheme.font(size: CodeTheme.fontSize)
        let column = digitWidth(font) * CGFloat(digits) + 16
        let signX = numbers == .both ? column * 2 : column
        for index in visible {
            guard index < textView.rows.count, let fragment = textView.rect(ofRow: index) else { continue }
            let y = convert(NSPoint(x: 0, y: fragment.minY), from: textView).y
            let box = NSRect(x: 0, y: y, width: bounds.width, height: fragment.height)
            let row = textView.rows[index]
            var color = DiffTheme.number
            switch row.kind {
            case .added:
                DiffTheme.addedGutter.setFill(); box.fill(using: .sourceOver); color = DiffTheme.ok
            case .removed:
                DiffTheme.removedGutter.setFill(); box.fill(using: .sourceOver); color = DiffTheme.bad
            case .header:
                DiffTheme.panel.setFill(); box.fill()
                DiffTheme.line.setFill()
                NSRect(x: 0, y: box.maxY - 1, width: box.width, height: 1).fill()
                NSRect(x: 0, y: box.minY, width: box.width, height: 1).fill()
            case .fold, .note:
                DiffTheme.fold.setFill(); box.fill()
            case .blank, .context: break
            }
            if index == textView.current {
                DiffTheme.tint.setFill(); box.fill(using: .sourceOver)
                color = DiffTheme.text
            }
            let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color]
            func put(_ text: String, right: CGFloat) {
                let size = (text as NSString).size(withAttributes: attributes)
                (text as NSString).draw(at: NSPoint(x: right - size.width, y: y + (fragment.height - size.height) / 2), withAttributes: attributes)
            }
            if row.kind == .header, let file = textView.files[safe: row.file] {
                let status: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedSystemFont(ofSize: font.pointSize - 1, weight: .semibold), .foregroundColor: DiffTheme.status(file.status)]
                let size = (file.status as NSString).size(withAttributes: status)
                (file.status as NSString).draw(at: NSPoint(x: bounds.width - size.width - 8, y: y + (fragment.height - size.height) / 2), withAttributes: status)
                continue
            }
            switch numbers {
            case .both:
                if let old = row.old, row.kind != .added { put(String(old), right: column - 8) }
                if let new = row.new, row.kind != .removed { put(String(new), right: column * 2 - 8) }
            case .old:
                if let old = row.old, row.kind != .added { put(String(old), right: column - 8) }
            case .new:
                if let new = row.new, row.kind != .removed { put(String(new), right: column - 8) }
            }
            let sign = row.kind == .added ? "+" : row.kind == .removed ? "−" : ""
            if !sign.isEmpty { (sign as NSString).draw(at: NSPoint(x: signX + 2, y: y + (fragment.height - font.boundingRectForFont.height) / 2), withAttributes: attributes) }
        }
    }
}

public extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}
