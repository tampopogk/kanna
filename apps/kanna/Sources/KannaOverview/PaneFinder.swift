import AppKit

@MainActor
public final class PaneFinder: NSObject {
    public weak var textView: NSTextView?
    public weak var container: NSScrollView?
    public var onMatches: (() -> Void)?
    public private(set) var matches: [NSRange] = []
    public private(set) var current: Int?
    let bar = FindBar()

    public override init() {
        super.init()
        bar.finder = self
    }

    public func attach(_ textView: NSTextView, in container: NSScrollView) {
        self.textView = textView
        self.container = container
        container.findBarPosition = .aboveContent
    }

    public var query: FindQuery { FindQuery(bar.field.stringValue) }

    public var isVisible: Bool { container?.isFindBarVisible == true && container?.findBarView === bar }

    public var label: String { bar.label.stringValue }

    public func performAction(_ action: NSTextFinder.Action) {
        switch action {
        case .showFindInterface: show()
        case .hideFindInterface: hide()
        case .nextMatch: step(forward: true)
        case .previousMatch: step(forward: false)
        case .setSearchString: useSelection()
        default: break
        }
    }

    public func validateAction(_ action: NSTextFinder.Action) -> Bool {
        switch action {
        case .showFindInterface: return textView != nil
        case .hideFindInterface: return isVisible
        case .nextMatch, .previousMatch: return !query.isEmpty
        case .setSearchString: return (textView?.selectedRange().length ?? 0) > 0
        default: return false
        }
    }

    private var pending = false

    public func noteClientStringWillChange() {
        guard !pending else { return }
        pending = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.pending = false
            guard self.isVisible else { return }
            self.search(scroll: false)
        }
    }

    public func show() {
        guard let container else { return }
        if container.findBarView !== bar {
            bar.frame = NSRect(x: 0, y: 0, width: container.bounds.width, height: FindBar.height)
            container.findBarView = bar
        }
        if !container.isFindBarVisible { keepingScroll(container) { container.isFindBarVisible = true } }
        bar.window?.makeFirstResponder(bar.field)
        bar.field.currentEditor()?.selectAll(nil)
        search(scroll: false)
    }

    public func hide() {
        let shown = isVisible
        if let container, container.isFindBarVisible { keepingScroll(container) { container.isFindBarVisible = false } }
        clear()
        if shown, let textView { textView.window?.makeFirstResponder(textView) }
    }

    private func keepingScroll(_ container: NSScrollView, _ change: () -> Void) {
        let clip = container.contentView
        let origin = clip.bounds.origin
        change()
        guard clip.bounds.origin != origin else { return }
        clip.scroll(to: origin)
        container.reflectScrolledClipView(clip)
    }

    func edited() {
        search(scroll: true)
    }

    func search(scroll: Bool) {
        guard isVisible else { return clear() }
        let ranges = query.ranges(in: textView?.string ?? "")
        let start = textView?.selectedRange().location ?? 0
        matches = ranges
        current = ranges.isEmpty ? nil : ranges.firstIndex { $0.location >= start } ?? 0
        if scroll, let current, let textView { textView.scrollRangeToVisible(ranges[current]) }
        updateLabel()
        onMatches?()
    }

    public func step(forward: Bool) {
        guard let textView else { return }
        guard !query.isEmpty else { return show() }
        let ranges = isVisible ? matches : query.ranges(in: textView.string)
        guard !ranges.isEmpty else { return NSSound.beep() }
        let selection = textView.selectedRange()
        let index: Int
        if forward {
            index = ranges.firstIndex { $0.location >= selection.location && $0 != selection } ?? 0
        } else {
            index = ranges.lastIndex { $0.location < selection.location } ?? ranges.count - 1
        }
        let range = ranges[index]
        textView.setSelectedRange(range)
        textView.scrollRangeToVisible(range)
        textView.showFindIndicator(for: range)
        if isVisible {
            current = index
            updateLabel()
            onMatches?()
        }
    }

    private func useSelection() {
        guard let textView else { return }
        let selection = textView.selectedRange()
        guard selection.length > 0 else { return }
        bar.field.stringValue = (textView.string as NSString).substring(with: selection)
        if isVisible { search(scroll: false) }
    }

    private func clear() {
        let had = !matches.isEmpty
        matches = []
        current = nil
        updateLabel()
        if had { onMatches?() }
    }

    private func updateLabel() {
        if query.isEmpty || !isVisible {
            bar.label.stringValue = ""
        } else if matches.isEmpty {
            bar.label.stringValue = "no match"
        } else {
            bar.label.stringValue = "\((current ?? 0) + 1) of \(matches.count)"
        }
    }
}

@MainActor
final class FindBar: NSView, NSSearchFieldDelegate {
    static let height: CGFloat = 30
    weak var finder: PaneFinder?
    let field = NSSearchField()
    let label = NSTextField(labelWithString: "")
    private let steps = NSSegmentedControl()
    private let done = NSButton(title: "Done", target: nil, action: nil)

    override init(frame: NSRect) {
        super.init(frame: frame)
        clipsToBounds = true
        autoresizingMask = [.width]
        field.placeholderString = "Find"
        field.delegate = self
        field.sendsSearchStringImmediately = true
        field.sendsWholeSearchString = false
        field.target = self
        field.action = #selector(changed)
        field.controlSize = .small
        field.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        label.font = .systemFont(ofSize: 11)
        label.textColor = .secondaryLabelColor
        label.alignment = .right
        steps.segmentCount = 2
        steps.trackingMode = .momentary
        steps.controlSize = .small
        steps.setImage(NSImage(systemSymbolName: "chevron.left", accessibilityDescription: "Previous"), forSegment: 0)
        steps.setImage(NSImage(systemSymbolName: "chevron.right", accessibilityDescription: "Next"), forSegment: 1)
        steps.setWidth(22, forSegment: 0)
        steps.setWidth(22, forSegment: 1)
        steps.target = self
        steps.action = #selector(step)
        done.controlSize = .small
        done.bezelStyle = .push
        done.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        done.target = self
        done.action = #selector(close)
        [field, label, steps, done].forEach(addSubview)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        bounds.fill()
        NSColor.separatorColor.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
    }

    override func layout() {
        super.layout()
        let h = bounds.height
        done.sizeToFit()
        let doneWidth = max(done.frame.width, 52)
        var x = bounds.width - 8 - doneWidth
        done.frame = NSRect(x: x, y: (h - done.frame.height) / 2, width: doneWidth, height: done.frame.height)
        steps.sizeToFit()
        x -= 8 + steps.frame.width
        steps.frame = NSRect(x: x, y: (h - steps.frame.height) / 2, width: steps.frame.width, height: steps.frame.height)
        x -= 8 + 72
        label.frame = NSRect(x: x, y: (h - 16) / 2, width: 72, height: 16)
        field.frame = NSRect(x: 8, y: (h - 22) / 2, width: max(x - 16, 60), height: 22)
    }

    @objc private func changed() { finder?.edited() }

    @objc private func step() { finder?.step(forward: steps.selectedSegment == 1) }

    @objc private func close() { finder?.hide() }

    func controlTextDidChange(_ notification: Notification) { finder?.edited() }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            finder?.step(forward: !(NSApp.currentEvent?.modifierFlags.contains(.shift) ?? false))
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            finder?.hide()
            return true
        default:
            return false
        }
    }

    override func performTextFinderAction(_ sender: Any?) {
        guard let tag = (sender as? NSValidatedUserInterfaceItem)?.tag, let action = NSTextFinder.Action(rawValue: tag) else { return }
        finder?.performAction(action)
    }

    func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        guard item.action == #selector(performTextFinderAction(_:)) else { return false }
        return NSTextFinder.Action(rawValue: item.tag).map { finder?.validateAction($0) ?? false } ?? false
    }

    @objc func validateMenuItem(_ item: NSMenuItem) -> Bool { validateUserInterfaceItem(item) }
}
