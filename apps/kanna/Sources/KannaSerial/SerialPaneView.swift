import AppKit

@MainActor
public final class SerialPaneView: NSView, NSTextFieldDelegate {
    public static let bauds = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 74880, 115_200, 230_400, 460_800, 921_600, 1_500_000, 2_000_000, 3_000_000]
    public static let endings = ["lf", "crlf", "cr"]
    public static let keep = 10_000

    public var onFocus: (() -> Void)? { didSet { textView.onFocus = onFocus } }
    public var contextMenu: (() -> NSMenu?)? { didSet { textView.contextMenu = contextMenu } }
    public var onSend: ((String, Bool) -> Void)?
    public var onReset: (() -> Void)?
    public var onSet: ((Int?, String?) -> Void)?
    public var keyView: NSView { textView }

    public private(set) var rows: [SerialRow] = []
    public private(set) var partial: SerialRow?
    public private(set) var state = SerialState()
    public private(set) var history: [String] = []
    public private(set) var showTime = true
    public private(set) var hex = false
    public private(set) var paused = false
    public private(set) var following = true
    public private(set) var unseen = 0
    public private(set) var failure: String?
    public var text: String { textView.string }
    public var sendEnabled: Bool { field.isEnabled }
    public var statusText: String { status.stringValue }

    private let toolbar = NSView()
    private let device = NSTextField(labelWithString: "")
    private let baud = NSPopUpButton()
    private let ending = NSPopUpButton()
    private let view = NSSegmentedControl(labels: ["Text", "Hex"], trackingMode: .selectOne, target: nil, action: nil)
    private let timeButton = NSButton(title: "Time", target: nil, action: nil)
    private let pauseButton = NSButton()
    private let clearButton = NSButton()
    private let findButton = NSButton()
    private let resetButton = NSButton()
    private let scroll = NSScrollView()
    private let textView = SerialTextView()
    private let pill = NSButton()
    private let sendBar = NSView()
    private let field = NSTextField()
    private let hint = NSTextField(labelWithString: "↩ sends + LF · ⌥↩ without")
    private let status = NSTextField(labelWithString: "")
    private var lengths: [Int] = []
    private var committed = 0
    private var historyAt: Int?
    private var draft = ""
    private var hexQueued = false
    private var scrolling = false

    public init() {
        super.init(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
        wantsLayer = true
        build()
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func symbol(_ name: String, _ title: String) -> NSImage? {
        NSImage(systemSymbolName: name, accessibilityDescription: title)?.withSymbolConfiguration(.init(pointSize: 11, weight: .regular))
    }

    private func tool(_ button: NSButton, _ title: String, _ image: String?, _ action: Selector, tip: String, toggles: Bool = false) {
        button.title = title
        button.image = image.flatMap { symbol($0, title) }
        button.imagePosition = title.isEmpty ? .imageOnly : .imageLeading
        button.bezelStyle = .accessoryBarAction
        button.setButtonType(toggles ? .pushOnPushOff : .momentaryPushIn)
        button.font = .systemFont(ofSize: 12)
        button.controlSize = .small
        button.target = self
        button.action = action
        button.toolTip = tip
    }

    private func build() {
        device.font = .systemFont(ofSize: 12, weight: .semibold)
        device.lineBreakMode = .byTruncatingTail
        device.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        for (popup, titles, tip) in [(baud, Self.bauds.map(String.init), "Baud rate"), (ending, Self.endings.map { $0.uppercased() }, "Line ending sent after each line")] {
            popup.addItems(withTitles: titles)
            popup.controlSize = .small
            popup.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
            popup.bezelStyle = .accessoryBarAction
            popup.target = self
            popup.action = #selector(changed(_:))
            popup.toolTip = tip
        }
        view.selectedSegment = 0
        view.controlSize = .small
        view.font = .systemFont(ofSize: 11)
        view.target = self
        view.action = #selector(toggleHex)
        tool(timeButton, "Time", nil, #selector(toggleTime), tip: "Timestamps", toggles: true)
        timeButton.state = .on
        tool(pauseButton, "Pause", "pause.fill", #selector(togglePause), tip: "Stop following (the log keeps recording)", toggles: true)
        tool(clearButton, "Clear", "circle.slash", #selector(clear), tip: "Clear this view (the log file keeps everything)")
        tool(findButton, "", "magnifyingglass", #selector(find), tip: "Find (⌘F)")
        tool(resetButton, "Reset", "arrow.counterclockwise", #selector(reset), tip: "Reset the device (toggles DTR/RTS)")
        let spacer = NSView()
        spacer.setContentHuggingPriority(.init(1), for: .horizontal)
        let tools = NSStackView(views: [device, baud, ending, view, spacer, timeButton, pauseButton, clearButton, findButton, resetButton])
        tools.spacing = 6
        tools.edgeInsets = NSEdgeInsets(top: 0, left: 12, bottom: 0, right: 8)
        tools.setCustomSpacing(4, after: device)
        toolbar.wantsLayer = true
        toolbar.addSubview(tools)
        tools.translatesAutoresizingMaskIntoConstraints = false

        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = true
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.textContainerInset = NSSize(width: 4, height: 6)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true
        textView.font = SerialRender.font(SerialRender.fontSize)
        textView.layoutManager?.allowsNonContiguousLayout = true
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = true
        scroll.findBarPosition = .aboveContent
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification, object: scroll.contentView)

        pill.bezelStyle = .push
        pill.controlSize = .small
        pill.font = .systemFont(ofSize: 11)
        pill.target = self
        pill.action = #selector(follow)
        pill.isHidden = true

        field.placeholderString = "Send a line to the device (↑ for history)"
        field.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        field.delegate = self
        field.bezelStyle = .roundedBezel
        field.focusRingType = .none
        hint.font = .systemFont(ofSize: 11)
        hint.textColor = SerialTheme.muted
        hint.setContentCompressionResistancePriority(.defaultHigh, for: .horizontal)
        let send = NSStackView(views: [field, hint])
        send.spacing = 8
        send.edgeInsets = NSEdgeInsets(top: 0, left: 10, bottom: 0, right: 10)
        sendBar.addSubview(send)
        send.translatesAutoresizingMaskIntoConstraints = false

        status.font = .systemFont(ofSize: 11)
        status.lineBreakMode = .byTruncatingTail
        status.wantsLayer = true

        for part in [toolbar, scroll, pill, sendBar, status] {
            part.translatesAutoresizingMaskIntoConstraints = false
            addSubview(part)
        }
        NSLayoutConstraint.activate([
            toolbar.topAnchor.constraint(equalTo: topAnchor), toolbar.leadingAnchor.constraint(equalTo: leadingAnchor), toolbar.trailingAnchor.constraint(equalTo: trailingAnchor), toolbar.heightAnchor.constraint(equalToConstant: 32),
            tools.topAnchor.constraint(equalTo: toolbar.topAnchor), tools.bottomAnchor.constraint(equalTo: toolbar.bottomAnchor), tools.leadingAnchor.constraint(equalTo: toolbar.leadingAnchor), tools.trailingAnchor.constraint(equalTo: toolbar.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: toolbar.bottomAnchor, constant: 1), scroll.leadingAnchor.constraint(equalTo: leadingAnchor), scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            sendBar.topAnchor.constraint(equalTo: scroll.bottomAnchor, constant: 1), sendBar.leadingAnchor.constraint(equalTo: leadingAnchor), sendBar.trailingAnchor.constraint(equalTo: trailingAnchor), sendBar.heightAnchor.constraint(equalToConstant: 36),
            send.topAnchor.constraint(equalTo: sendBar.topAnchor), send.bottomAnchor.constraint(equalTo: sendBar.bottomAnchor), send.leadingAnchor.constraint(equalTo: sendBar.leadingAnchor), send.trailingAnchor.constraint(equalTo: sendBar.trailingAnchor),
            status.topAnchor.constraint(equalTo: sendBar.bottomAnchor, constant: 5), status.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12), status.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            status.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -5),
            pill.trailingAnchor.constraint(equalTo: scroll.trailingAnchor, constant: -18), pill.bottomAnchor.constraint(equalTo: scroll.bottomAnchor, constant: -10),
        ])
    }

    public override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    public override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        SerialTheme.line.setFill()
        for y in [toolbar.frame.maxY, scroll.frame.maxY, sendBar.frame.maxY] where y > 0 {
            NSRect(x: 0, y: y, width: bounds.width, height: 1).fill()
        }
    }

    public override var isFlipped: Bool { true }

    private func paint() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = SerialTheme.bar.cgColor
            toolbar.layer?.backgroundColor = SerialTheme.bar.cgColor
            scroll.backgroundColor = SerialTheme.background
            textView.backgroundColor = SerialTheme.background
            sendBar.wantsLayer = true
            sendBar.layer?.backgroundColor = SerialTheme.background.cgColor
        }
        rerender()
        showState()
    }

    public func load(rows: [SerialRow], partial: SerialRow?, state: SerialState, history: [String]) {
        failure = nil
        self.rows = Array(rows.suffix(Self.keep))
        self.partial = partial
        self.history = history
        self.state = state
        unseen = 0
        following = !paused
        rerender()
        showState()
    }

    public func receive(rows new: [SerialRow], partial: SerialRow?, state: SerialState?, rx: Int?) {
        if let state { self.state = state }
        if let rx { self.state.rx = rx }
        let fresh = new.filter { $0.n > (rows.last?.n ?? 0) }
        rows += fresh
        self.partial = partial
        if !following || paused { unseen += fresh.count }
        if rows.count > Self.keep {
            let drop = rows.count - Self.keep
            rows.removeFirst(drop)
            if !hex { trim(drop) }
        }
        if hex {
            queueHex()
        } else {
            append(fresh)
        }
        if state != nil || rx != nil { showState() }
        showPill()
    }

    public func remember(_ line: String) {
        history.removeAll { $0 == line }
        history.append(line)
    }

    public func show(failure: String) {
        self.failure = failure
        textView.textStorage?.setAttributedString(NSAttributedString(string: failure, attributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: SerialTheme.muted]))
        lengths = []
        committed = 0
        field.isEnabled = false
    }

    public func flash(_ message: String) {
        hint.stringValue = message
        hint.textColor = SerialTheme.error
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) { [weak self] in
            guard let self else { return }
            hint.textColor = SerialTheme.muted
            showState()
        }
    }

    public func stop() {
        NotificationCenter.default.removeObserver(self)
    }

    private func rendered(_ row: SerialRow) -> NSAttributedString {
        SerialRender.row(row, time: showTime)
    }

    private func rerender() {
        guard failure == nil else { return }
        if hex { return renderHex() }
        let out = NSMutableAttributedString()
        lengths = rows.map { row in
            let line = NSMutableAttributedString(attributedString: rendered(row))
            line.append(NSAttributedString(string: "\n"))
            out.append(line)
            return line.length
        }
        committed = out.length
        if let partial { out.append(rendered(partial)) }
        textView.textStorage?.setAttributedString(out)
        textView.typingAttributes = [.font: SerialRender.font(SerialRender.fontSize)]
        if following && !paused { scrollToEnd() }
    }

    private func append(_ fresh: [SerialRow]) {
        guard failure == nil, let storage = textView.textStorage else { return }
        storage.beginEditing()
        if storage.length > committed { storage.deleteCharacters(in: NSRange(location: committed, length: storage.length - committed)) }
        for row in fresh {
            let line = NSMutableAttributedString(attributedString: rendered(row))
            line.append(NSAttributedString(string: "\n"))
            storage.append(line)
            lengths.append(line.length)
        }
        committed = storage.length
        if let partial { storage.append(rendered(partial)) }
        storage.endEditing()
        if following && !paused { scrollToEnd() }
    }

    private func trim(_ count: Int) {
        guard let storage = textView.textStorage, count > 0 else { return }
        let length = lengths.prefix(count).reduce(0, +)
        lengths.removeFirst(min(count, lengths.count))
        storage.deleteCharacters(in: NSRange(location: 0, length: min(length, storage.length)))
        committed -= length
    }

    private func queueHex() {
        guard !hexQueued else { return }
        hexQueued = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
            guard let self else { return }
            hexQueued = false
            if hex { renderHex() }
        }
    }

    private func renderHex() {
        let all = rows + (partial.map { [$0] } ?? [])
        textView.textStorage?.setAttributedString(SerialRender.hex(all))
        lengths = []
        committed = textView.textStorage?.length ?? 0
        if following && !paused { scrollToEnd() }
    }

    private func scrollToEnd() {
        scrolling = true
        textView.scrollToEndOfDocument(nil)
        scrolling = false
    }

    private var atEnd: Bool {
        let visible = scroll.contentView.bounds
        return visible.maxY >= textView.frame.height - 4
    }

    @objc private func scrolled() {
        guard !scrolling else { return }
        let was = following
        following = atEnd
        if following && !was && !paused { unseen = 0 }
        showPill()
    }

    private func showPill() {
        let show = paused || (!following && unseen > 0)
        pill.isHidden = !show
        guard show else { return }
        let new = unseen == 1 ? "1 new line" : "\(unseen) new lines"
        pill.title = paused ? "Paused · \(new) · click to follow" : "\(new) ↓"
    }

    private func showState() {
        let ok = state.state == "connected"
        field.isEnabled = ok && failure == nil
        resetButton.isEnabled = ok
        field.placeholderString = switch state.state {
        case "connected": "Send a line to the device (↑ for history)"
        case "lent": "Lent to \(state.cmd ?? "a command"): back when it exits"
        default: "Unplugged: lines can't be sent until the device is back"
        }
        device.stringValue = state.product ?? state.name
        device.toolTip = state.device
        if let at = Self.bauds.firstIndex(of: state.baud) {
            baud.selectItem(at: at)
        } else {
            if baud.item(withTitle: String(state.baud)) == nil { baud.addItem(withTitle: String(state.baud)) }
            baud.selectItem(withTitle: String(state.baud))
        }
        ending.selectItem(at: Self.endings.firstIndex(of: state.lineEnding) ?? 0)
        hint.stringValue = "↩ sends + \(state.lineEnding.uppercased()) · ⌥↩ without"
        let line = NSMutableAttributedString()
        let small: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: SerialTheme.muted]
        let (dot, color, words): (String, NSColor, String) = switch state.state {
        case "connected": ("●", SerialTheme.ok, "connected")
        case "lent": ("●", SerialTheme.warning, "lent to \(state.cmd ?? "a command") · back when it exits")
        default: ("○", SerialTheme.error, "unplugged" + (state.device.map { " · waiting for \($0)" } ?? ""))
        }
        line.append(NSAttributedString(string: dot + " ", attributes: small.merging([.foregroundColor: color]) { $1 }))
        line.append(NSAttributedString(string: words, attributes: small.merging([.foregroundColor: state.state == "unplugged" ? SerialTheme.error : SerialTheme.text]) { $1 }))
        var parts = [state.path, "\(state.baud) 8N1", state.lineEnding.uppercased()]
        if let format = state.format { parts.append("\(format) log levels") }
        parts.append(String(format: "rx %.1f KB", Double(state.rx) / 1024))
        parts.append((state.log as NSString).lastPathComponent)
        line.append(NSAttributedString(string: "   " + parts.joined(separator: "   "), attributes: small))
        status.attributedStringValue = line
        status.toolTip = state.log
    }

    @objc private func changed(_ sender: NSPopUpButton) {
        if sender === baud, let value = Int(sender.titleOfSelectedItem ?? "") {
            onSet?(value, nil)
        } else if sender === ending {
            onSet?(nil, Self.endings[sender.indexOfSelectedItem])
        }
    }

    @objc public func toggleHex() {
        hex = view.selectedSegment == 1
        rerender()
    }

    public func set(hex on: Bool) {
        view.selectedSegment = on ? 1 : 0
        toggleHex()
    }

    @objc public func toggleTime() {
        showTime.toggle()
        timeButton.state = showTime ? .on : .off
        rerender()
    }

    @objc public func togglePause() {
        paused.toggle()
        pauseButton.state = paused ? .on : .off
        if !paused { follow() } else { showPill() }
    }

    @objc public func follow() {
        paused = false
        pauseButton.state = .off
        following = true
        unseen = 0
        if hex { renderHex() } else { scrollToEnd() }
        showPill()
    }

    @objc public func clear() {
        rows = []
        partial = nil
        unseen = 0
        rerender()
        showPill()
    }

    @objc private func find() {
        window?.makeFirstResponder(textView)
        let item = NSMenuItem()
        item.tag = NSTextFinder.Action.showFindInterface.rawValue
        textView.performFindPanelAction(item)
    }

    @objc private func reset() { onReset?() }

    public func send(_ text: String, ending: Bool = true) {
        guard !text.isEmpty else { return }
        remember(text)
        historyAt = nil
        onSend?(text, ending)
    }

    public func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)), #selector(NSResponder.insertNewlineIgnoringFieldEditor(_:)):
            let withEnding = selector == #selector(NSResponder.insertNewline(_:))
            let line = field.stringValue
            field.stringValue = ""
            send(line, ending: withEnding)
            return true
        case #selector(NSResponder.moveUp(_:)):
            guard !history.isEmpty else { return true }
            if historyAt == nil { draft = field.stringValue }
            historyAt = max(0, (historyAt ?? history.count) - 1)
            field.stringValue = history[historyAt!]
            textView.moveToEndOfDocument(nil)
            return true
        case #selector(NSResponder.moveDown(_:)):
            guard let at = historyAt else { return true }
            if at + 1 < history.count {
                historyAt = at + 1
                field.stringValue = history[at + 1]
            } else {
                historyAt = nil
                field.stringValue = draft
            }
            textView.moveToEndOfDocument(nil)
            return true
        default:
            return false
        }
    }

    public func type(_ text: String, ending: Bool = true) {
        field.stringValue = text
        let line = field.stringValue
        field.stringValue = ""
        send(line, ending: ending)
    }
}

final class SerialTextView: NSTextView {
    var onFocus: (() -> Void)?
    var contextMenu: (() -> NSMenu?)?

    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result { onFocus?() }
        return result
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        contextMenu?() ?? super.menu(for: event)
    }
}
