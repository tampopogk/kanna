import AppKit
import KannaCode
import KannaOverview

@MainActor
public final class DiffPaneView: NSView {
    public var onFocus: (() -> Void)?
    public var onZoom: (() -> Void)?
    public var onView: ((String) -> Void)?
    public var onOpen: ((String, Int) -> Void)?
    public var onShowFiles: (([String]) -> Void)?
    public var contextMenu: (() -> NSMenu?)? { didSet { columns.forEach { $0.textView.contextMenu = contextMenu } } }
    public var keyView: NSView { columns[0].textView }
    public private(set) var result: DiffResult?
    public private(set) var document = DiffDocument(result: nil, view: .unified)
    public private(set) var view: DiffView
    public private(set) var updating = false
    public private(set) var serial = 0
    public var zoom: CGFloat { fontSize / CodeTheme.fontSize }
    public var current: Int? { columns[0].textView.current }
    public var fresh: Set<Int> { columns[0].textView.fresh }

    let header = DiffHeader()
    let fileList = DiffFileList()
    private let fileScroll = NSScrollView()
    let sticky = DiffStickyHeader()
    public let overview = OverviewView()
    public private(set) var overviewShown = false
    public private(set) var overviewWanted = false
    public private(set) var listShown = false
    private var overviewStale = true
    private var overviewQueued = false
    private var settingObserver: NSObjectProtocol?
    private var columns: [DiffColumn] = []
    private var fontSize = CodeTheme.fontSize
    private var shown: Set<String> = []
    private var closed: Set<String> = []
    private var observers: [NSObjectProtocol] = []
    private var syncing = false
    private var highlighted: [String: [Int: [(NSRange, String)]]] = [:]
    private var highlightTask: Task<Void, Never>?
    private let message = NSTextField(labelWithString: "")

    public init(view: String?, zoom: CGFloat = 1) {
        self.view = DiffView(rawValue: view ?? "") ?? .unified
        fontSize = Self.clamp((CodeTheme.fontSize * zoom * 2).rounded() / 2)
        super.init(frame: .zero)
        addSubview(header)
        header.onView = { [weak self] view in self?.switchView(to: view) }
        message.textColor = DiffTheme.muted
        message.alignment = .center
        message.font = DiffTheme.sans(size: 14)
        message.isHidden = true
        addSubview(message)
        fileScroll.documentView = fileList
        fileScroll.hasVerticalScroller = true
        fileScroll.autohidesScrollers = true
        fileScroll.drawsBackground = true
        fileScroll.backgroundColor = DiffTheme.background
        addSubview(fileScroll)
        fileList.onPick = { [weak self] file in self?.go(toFile: file) }
        sticky.onClick = { [weak self] all in self?.foldTop(all: all) }
        overview.background = DiffTheme.background
        overview.edge = DiffTheme.line
        overview.onScroll = { [weak self] top in self?.scrollOverview(to: top) }
        overview.isHidden = true
        addSubview(overview)
        settingObserver = NotificationCenter.default.addObserver(forName: OverviewSetting.changed, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.needsLayout = true }
        }
        makeColumns()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    public func stop() {
        highlightTask?.cancel()
        observers.forEach(NotificationCenter.default.removeObserver)
        observers = []
        settingObserver.map(NotificationCenter.default.removeObserver)
        settingObserver = nil
    }

    static func clamp(_ size: CGFloat) -> CGFloat { min(max(size, 9), 32) }

    private func makeColumns() {
        let first = columns.first?.textView.window?.firstResponder === columns.first?.textView
        columns.forEach { $0.scrollView.removeFromSuperview() }
        observers.forEach(NotificationCenter.default.removeObserver)
        observers = []
        columns = view == .unified ? [DiffColumn(numbers: .both)] : [DiffColumn(numbers: .old), DiffColumn(numbers: .new)]
        for (index, column) in columns.enumerated() {
            addSubview(column.scrollView)
            column.textView.onFocus = { [weak self] in self?.onFocus?() }
            column.textView.onKey = { [weak self] key in self?.press(key) ?? false }
            column.textView.onClickRow = { [weak self] row in self?.setCurrent(row, scroll: false) }
            column.textView.onMarks = { [weak self] in self?.columns.forEach { $0.gutter.needsDisplay = true } }
            column.textView.onMatches = { [weak self] in self?.updateOverviewMarks() }
            column.textView.contextMenu = contextMenu
            column.textView.onHeader = { [weak self] row, all in self?.foldRow(row, all: all) }
            column.textView.onLink = { [weak self] link in self?.follow(link: link) }
            column.scrollView.onTile = { [weak self] in self?.needsLayout = true }
            observers.append(NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: column.scrollView.contentView, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.follow(index)
                    self?.columns[index].gutter.needsDisplay = true
                    self?.updateSticky()
                    if index == 0 { self?.updateOverviewViewport() }
                }
            })
        }
        if first { window?.makeFirstResponder(columns[0].textView) }
        overview.scrollTarget = columns[0].scrollView
        addSubview(sticky, positioned: .above, relativeTo: nil)
        addSubview(header, positioned: .above, relativeTo: nil)
    }

    public var debugColumns: [[String: Double]] {
        columns.map { column in
            let clip = column.scrollView.contentView
            return ["x": clip.bounds.minX, "y": clip.bounds.minY, "inset_left": clip.contentInsets.left, "inset_top": clip.contentInsets.top,
                    "gutter": column.gutter.ruleThickness, "clip_x": clip.frame.minX, "frame_x": column.scrollView.frame.minX, "text_x": column.textView.frame.minX]
        }
    }

    public var selectedFile: String? { fileList.selected.flatMap { result?.files[safe: $0]?.path } }
    public var stickyFile: String? { sticky.isHidden ? nil : sticky.file?.path }

    private func go(toFile file: Int) {
        guard let index = document.rows.firstIndex(where: { $0.file == file && $0.kind == .header }) else { return }
        scroll(toRow: index, top: true)
        let first = navigable.first { $0 > index && document.rows[$0].file == file }
        if let first { setCurrent(first, scroll: false) } else { fileList.selected = file }
    }

    private func foldRow(_ index: Int, all: Bool) {
        guard let file = document.rows[safe: index].flatMap({ result?.files[safe: $0.file] }) else { return }
        toggle(file: file.path, all: all)
        if let header = document.rows.firstIndex(where: { $0.kind == .header && result?.files[safe: $0.file]?.path == file.path }) { scroll(toRow: header, top: true) }
    }

    private func foldTop(all: Bool) {
        guard let top = topRow, let row = document.rows[safe: top] else { return }
        if let header = document.rows.firstIndex(where: { $0.kind == .header && $0.file == row.file }) { foldRow(header, all: all) }
    }

    private func follow(link: String) {
        guard link.hasPrefix("kanna-diff-show:"), let number = Int(link.dropFirst("kanna-diff-show:".count)), let file = result?.files[safe: number] else { return }
        showFolded(file.path)
    }

    func updateSticky() {
        guard let column = columns.first, let top = topRow, let row = document.rows[safe: top], let file = result?.files[safe: row.file] else {
            sticky.isHidden = true
            return
        }
        let height = column.textView.rect(ofRow: top)?.height ?? 18
        let clip = column.scrollView.contentView
        let headerRect = column.textView.rect(ofRow: document.rows.firstIndex { $0.kind == .header && $0.file == row.file } ?? top)
        let headerVisible = headerRect.map { $0.minY >= clip.bounds.minY + clip.contentInsets.top - 1 } ?? false
        sticky.isHidden = headerVisible
        sticky.file = file
        sticky.font = CodeTheme.font(size: fontSize)
        sticky.gutterWidth = column.gutter.ruleThickness
        let left = columns.first!.scrollView.frame.minX
        let right = columns.last!.scrollView.frame.maxX
        let edge = convert(clip.frame, from: column.scrollView).maxY - clip.contentInsets.top
        sticky.frame = NSRect(x: left, y: edge - height, width: right - left, height: height)
    }

    private func follow(_ index: Int) {
        guard !syncing, columns.count > 1 else { return }
        syncing = true
        let y = columns[index].scrollView.contentView.bounds.minY
        for (other, column) in columns.enumerated() where other != index {
            column.scrollView.contentView.scroll(to: NSPoint(x: column.scrollView.contentView.bounds.minX, y: y))
            column.scrollView.reflectScrolledClipView(column.scrollView.contentView)
        }
        syncing = false
    }

    public override func layout() {
        super.layout()
        let headerHeight: CGFloat = 34
        header.frame = NSRect(x: 0, y: bounds.height - headerHeight, width: bounds.width, height: headerHeight)
        var body = NSRect(x: 0, y: 0, width: bounds.width, height: bounds.height - headerHeight)
        needsDisplay = true
        placeAsides(height: body.height)
        fileScroll.isHidden = !listShown
        if listShown {
            fileScroll.frame = NSRect(x: 0, y: 0, width: DiffFileList.width, height: body.height)
            fileList.frame.size.width = DiffFileList.width
            fileList.show(result?.files ?? [])
            body.origin.x = DiffFileList.width + 1
            body.size.width -= DiffFileList.width + 1
        }
        if overviewShown { body.size.width -= OverviewMetrics.width }
        overview.frame = NSRect(x: bounds.width - OverviewMetrics.width, y: 0, width: OverviewMetrics.width, height: body.height)
        overview.isHidden = !overviewShown
        let width = (body.width / CGFloat(columns.count)).rounded(.down)
        for (index, column) in columns.enumerated() {
            let x = body.minX + CGFloat(index) * width
            column.scrollView.frame = NSRect(x: x, y: 0, width: index == columns.count - 1 ? body.maxX - x : width - 1, height: body.height)
            column.scrollView.hasVerticalScroller = !overviewShown
            column.size(minimumHeight: body.height / 2, font: CodeTheme.font(size: fontSize))
        }
        message.frame = NSRect(x: body.minX + 20, y: body.height / 2 - 40, width: body.width - 40, height: 60)
        updateSticky()
        if overviewShown, overviewStale { updateOverviewContent() }
        updateOverviewViewport()
    }

    private func placeAsides(height: CGFloat) {
        let hasFiles = (result?.files.count ?? 0) > 0
        var viewport = overviewViewport
        viewport.height = height - columns[0].scrollView.contentView.contentInsets.top
        overviewWanted = hasFiles && OverviewLayout.wanted(enabled: OverviewSetting.enabled, viewport: viewport)
        let shown = DiffAside.shown((hasFiles ? [.files] : []) + (overviewWanted ? [.overview] : []), width: bounds.width, font: CodeTheme.font(size: fontSize))
        listShown = shown.contains(.files)
        overviewShown = shown.contains(.overview)
    }

    private var overviewViewport: OverviewViewport {
        let column = columns[0]
        let clip = column.scrollView.contentView
        return OverviewViewport(documentHeight: column.textView.frame.height, contentHeight: column.usedHeight, top: clip.bounds.minY + clip.contentInsets.top,
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
        overviewStale = false
        var lineHeight: CGFloat = 0
        let sides: [OverviewText] = columns.map { column in
            let text = column.textView
            let string = NSAttributedString(attributedString: text.textStorage ?? NSTextStorage())
            let length = string.length
            var lines: [OverviewLine] = []
            lines.reserveCapacity(text.rows.count)
            for (index, row) in text.rows.enumerated() {
                guard let rect = text.rect(ofRow: index) else { continue }
                if row.kind == .context || row.kind == .added || row.kind == .removed { lineHeight = rect.height }
                let start = min(text.start(ofRow: index), length)
                let end = index + 1 < text.rows.count ? max(start, min(text.start(ofRow: index + 1) - 1, length)) : length
                lines.append(OverviewLine(y: rect.minY, height: rect.height, range: NSRange(location: start, length: end - start), background: Self.overviewBackground(row.kind)))
            }
            return OverviewText(string: string, lines: lines)
        }
        if lineHeight == 0 { lineHeight = NSLayoutManager().defaultLineHeight(for: CodeTheme.font(size: fontSize)) }
        overview.content = .text(sides, lineHeight: lineHeight)
        updateOverviewMarks()
        updateOverviewViewport()
    }

    static func overviewBackground(_ kind: DiffRowKind) -> NSColor? {
        switch kind {
        case .added: DiffTheme.addedLine
        case .removed: DiffTheme.removedLine
        case .header: DiffTheme.panel
        case .fold, .note, .blank: DiffTheme.fold
        case .context: nil
        }
    }

    private func updateOverviewMarks() {
        guard overviewShown, let text = columns.first?.textView else { return }
        var marks: [OverviewMark] = []
        let count = document.rows.count
        var index = 0
        while index < count {
            let kinds = Set(document.columns.map { $0[index].kind })
            guard kinds.contains(.added) || kinds.contains(.removed) else { index += 1; continue }
            var end = index
            var all = kinds
            while end + 1 < count {
                let next = Set(document.columns.map { $0[end + 1].kind })
                guard next.contains(.added) || next.contains(.removed) else { break }
                all.formUnion(next)
                end += 1
            }
            if let first = text.rect(ofRow: index), let last = text.rect(ofRow: end) {
                let both = all.contains(.added) && all.contains(.removed)
                marks.append(OverviewMark(y: first.minY, height: last.maxY - first.minY, kind: both ? .changed : all.contains(.added) ? .added : .removed,
                                          color: both ? DiffTheme.warn : all.contains(.added) ? DiffTheme.ok : DiffTheme.bad))
            }
            index = end + 1
        }
        for column in columns {
            var last = -1
            for match in column.textView.matches {
                let row = column.textView.row(at: match.location)
                guard row != last, let rect = column.textView.rect(ofRow: row) else { continue }
                last = row
                marks.append(OverviewMark(y: rect.minY, height: rect.height, kind: .hit, color: CodeTheme.hit))
            }
        }
        overview.marks = OverviewMark.runs(marks)
    }

    private func scrollOverview(to top: CGFloat) {
        let clip = columns[0].scrollView.contentView
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: top - clip.contentInsets.top))
        columns[0].scrollView.reflectScrolledClipView(clip)
    }

    public override func draw(_ dirtyRect: NSRect) {
        DiffTheme.line.setFill()
        if columns.count > 1 {
            let x = columns[0].scrollView.frame.maxX
            NSRect(x: x, y: 0, width: 1, height: bounds.height - 34).fill()
        }
        if listShown { NSRect(x: DiffFileList.width, y: 0, width: 1, height: bounds.height - 34).fill() }
    }

    public func show(_ data: Data, serial: Int) {
        guard let result = DiffResult.decode(data) else { return }
        show(result, serial: serial)
    }

    public func show(_ result: DiffResult, serial: Int) {
        let old = document
        let oldFiles = self.result?.files ?? []
        let anchor = current.flatMap { old.rows[safe: $0] }.map { (oldFiles[safe: $0.file]?.path ?? "", $0) }
        let top = topRow.flatMap { old.rows[safe: $0] }.map { (oldFiles[safe: $0.file]?.path ?? "", $0) }
        self.result = result
        self.serial = serial
        setUpdating(false)
        let keep = Set(result.files.map(\.path))
        highlighted = highlighted.filter { keep.contains($0.key) }
        highlighted = highlighted.filter { path, _ in result.files.first { $0.path == path } == oldFiles.first { $0.path == path } }
        rebuild()
        let fresh = old.rows.isEmpty ? [] : changedRows(from: old, files: oldFiles)
        columns.forEach { $0.textView.fresh = fresh }
        fileList.fresh = Set(fresh.compactMap { document.rows[safe: $0]?.file })
        if let (path, row) = anchor, let index = find(path, row) { columns.forEach { $0.textView.current = index } }
        if let (path, row) = top, let index = find(path, row) { scroll(toRow: index, top: true) }
        highlight()
    }

    private func find(_ path: String, _ row: DiffRow) -> Int? {
        document.rows.firstIndex { self.path(of: $0) == path && $0.kind == row.kind && $0.old == row.old && $0.new == row.new }
    }

    public func setUpdating(_ on: Bool) {
        updating = on
        header.updating = on
    }

    private func path(of row: DiffRow) -> String { result?.files[safe: row.file]?.path ?? "" }

    private func changedRows(from old: DiffDocument, files: [DiffFile]) -> Set<Int> {
        let before = Set(old.rows.filter(\.isChange).map { "\(files[safe: $0.file]?.path ?? "")\u{0}\($0.kind)\u{0}\($0.text)" })
        var out = Set<Int>()
        for (index, row) in document.rows.enumerated() where row.isChange {
            if !before.contains("\(path(of: row))\u{0}\(row.kind)\u{0}\(row.text)") { out.insert(index) }
        }
        return out
    }

    private func rebuild() {
        guard let result else { return }
        document = DiffDocument(result: result, view: view, closed: closed)
        header.show(result, view: view)
        message.isHidden = result.error == nil && !result.files.isEmpty
        message.stringValue = result.error ?? "No changes\n\(result.command)"
        message.textColor = result.error == nil ? DiffTheme.muted : DiffTheme.bad
        for (index, column) in columns.enumerated() {
            column.show(document.columns[index], files: result.files, text: text(for: document.columns[index], files: result.files), size: fontSize)
        }
        overviewStale = true
        needsLayout = true
        layoutSubtreeIfNeeded()
    }

    private func text(for rows: [DiffRow], files: [DiffFile]) -> NSAttributedString {
        let font = CodeTheme.font(size: fontSize)
        let out = NSMutableAttributedString()
        let plain: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: DiffTheme.text]
        let muted: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: DiffTheme.muted]
        for (index, row) in rows.enumerated() {
            let text = Self.oneLine(row.text)
            switch row.kind {
            case .header:
                let file = files[safe: row.file]
                let slash = (text as NSString).range(of: "/", options: .backwards)
                let folder = slash.location == NSNotFound ? "" : (text as NSString).substring(to: slash.location + 1)
                out.append(NSAttributedString(string: folder, attributes: muted))
                out.append(NSAttributedString(string: (text as NSString).substring(from: folder.utf16.count), attributes: plain))
                if let file {
                    out.append(NSAttributedString(string: "  ", attributes: plain))
                    if file.added > 0 { out.append(NSAttributedString(string: "+\(file.added)", attributes: [.font: font, .foregroundColor: DiffTheme.ok])) }
                    if file.added > 0 && file.removed > 0 { out.append(NSAttributedString(string: " ", attributes: plain)) }
                    if file.removed > 0 { out.append(NSAttributedString(string: "−\(file.removed)", attributes: [.font: font, .foregroundColor: DiffTheme.bad])) }
                    if file.untracked == true { out.append(NSAttributedString(string: "  untracked", attributes: muted)) }
                }
            case .fold:
                out.append(NSAttributedString(string: text, attributes: muted))
                if !row.scope.isEmpty { out.append(NSAttributedString(string: "  " + row.scope, attributes: [.font: font, .foregroundColor: DiffTheme.muted.withAlphaComponent(0.8)])) }
            case .note:
                let link = " Show the diff"
                if text.hasSuffix(link) {
                    out.append(NSAttributedString(string: String(text.dropLast(link.count)) + " ", attributes: muted))
                    out.append(NSAttributedString(string: String(link.dropFirst()), attributes: [.font: font, .foregroundColor: DiffTheme.accent, .link: "kanna-diff-show:\(row.file)"]))
                } else {
                    out.append(NSAttributedString(string: text, attributes: muted))
                }
            default:
                out.append(NSAttributedString(string: text, attributes: plain))
            }
            if index < rows.count - 1 { out.append(NSAttributedString(string: "\n", attributes: plain)) }
        }
        return out
    }

    nonisolated static func oneLine(_ text: String) -> String {
        String(String.UnicodeScalarView(text.unicodeScalars.map { scalar in
            switch scalar {
            case "\r", "\u{0B}", "\u{0C}", "\u{85}", "\u{2028}", "\u{2029}": return "\u{2424}"
            default: return scalar
            }
        }))
    }

    private func highlight() {
        highlightTask?.cancel()
        guard let result else { return }
        let files = result.files
        let missing = files.enumerated().filter { highlighted[$0.element.path] == nil && $0.element.fold == nil && $0.element.binary != true }
        guard !missing.isEmpty else { applyHighlights(); return }
        highlightTask = Task { [weak self] in
            for (_, file) in missing {
                if Task.isCancelled { return }
                let found = await Task.detached { Self.spans(for: file) }.value
                guard let self, !Task.isCancelled else { return }
                self.highlighted[file.path] = found
                self.applyHighlights(only: file.path)
            }
        }
    }

    nonisolated static func spans(for file: DiffFile) -> [Int: [(NSRange, String)]] {
        let first = file.hunks.first?.lines.first?.text ?? ""
        guard let language = CodeSyntax.language(for: file.path, firstLine: first) else { return [:] }
        var out: [Int: [(NSRange, String)]] = [:]
        var index = 0
        for hunk in file.hunks {
            for side in [DiffLineKind.removed, .added] {
                var text = ""
                var owners: [(Int, Int)] = []
                for (offset, line) in hunk.lines.enumerated() where line.kind == .context || line.kind == side {
                    owners.append((index + offset, (text as NSString).length))
                    text += line.text + "\n"
                }
                let spans = CodeSyntax.highlight(text, language: language).spans
                for span in spans {
                    for (owner, start) in owners {
                        let length = (hunk.lines[owner - index].text as NSString).length
                        let range = NSIntersectionRange(span.range, NSRange(location: start, length: length))
                        guard range.length > 0 else { continue }
                        if side == .added && hunk.lines[owner - index].kind == .context { continue }
                        out[owner, default: []].append((NSRange(location: range.location - start, length: range.length), span.capture))
                    }
                }
            }
            index += hunk.lines.count
        }
        return out
    }

    private func applyHighlights(only path: String? = nil) {
        guard let result else { return }
        for (column, rows) in zip(columns, document.columns) {
            guard let storage = column.textView.textStorage else { continue }
            storage.beginEditing()
            for (index, row) in rows.enumerated() {
                guard let at = row.at, let file = result.files[safe: row.file], path == nil || file.path == path, let found = highlighted[file.path]?[at] else { continue }
                let start = column.textView.start(ofRow: index)
                for (range, capture) in found {
                    guard let color = CodeTheme.color(capture: capture) else { continue }
                    let target = NSRange(location: start + range.location, length: range.length)
                    guard NSMaxRange(target) <= storage.length else { continue }
                    storage.addAttribute(.foregroundColor, value: color, range: target)
                    if capture.hasPrefix("comment") { storage.addAttribute(.font, value: CodeTheme.italic(size: fontSize), range: target) }
                }
            }
            storage.endEditing()
        }
        overviewChanged()
    }

    private func switchView(to next: DiffView) {
        guard next != view else { return }
        let top = topRow.flatMap { document.rows[safe: $0] }
        let currentRow = current.flatMap { document.rows[safe: $0] }
        view = next
        makeColumns()
        rebuild()
        applyHighlights()
        if let currentRow, let index = locate(currentRow) { columns.forEach { $0.textView.current = index } }
        if let top, let index = locate(top) { scroll(toRow: index, top: true) }
        onView?(next.rawValue)
    }

    private func locate(_ row: DiffRow) -> Int? {
        for column in document.columns {
            if let index = column.firstIndex(where: { $0.file == row.file && $0.kind == row.kind && $0.old == row.old && $0.new == row.new && $0.text == row.text }) { return index }
        }
        return nil
    }

    public func setView(_ name: String) {
        if let next = DiffView(rawValue: name) { switchView(to: next) }
    }

    var topRow: Int? {
        let column = columns[0]
        return column.textView.visibleRows(in: column.textView.visibleRect)?.lowerBound
    }

    func scroll(toRow index: Int, top: Bool) {
        guard let rect = columns[0].textView.rect(ofRow: index) else { return }
        let clip = columns[0].scrollView.contentView
        let y: CGFloat
        if top {
            y = rect.minY
        } else if rect.minY < clip.bounds.minY + clip.contentInsets.top + rect.height * 2 {
            y = rect.minY - rect.height * 2
        } else if rect.maxY > clip.bounds.maxY - rect.height * 3 {
            y = rect.maxY - clip.bounds.height + rect.height * 3
        } else {
            return
        }
        clip.scroll(to: NSPoint(x: -clip.contentInsets.left, y: max(-clip.contentInsets.top, y - clip.contentInsets.top)))
        columns[0].scrollView.reflectScrolledClipView(clip)
    }

    var navigable: [Int] {
        (0..<document.rows.count).filter { index in document.columns.contains { [.context, .added, .removed].contains($0[index].kind) } }
    }

    func setCurrent(_ index: Int, scroll: Bool = true) {
        guard index >= 0, index < document.rows.count else { return }
        columns.forEach { $0.textView.current = index }
        fileList.selected = document.rows[index].file
        if let rect = fileList.entryRect(file: document.rows[index].file) { fileList.scrollToVisible(rect) }
        if scroll { self.scroll(toRow: index, top: false) }
    }

    private func isChange(_ index: Int) -> Bool { document.columns.contains { $0[index].isChange } }

    public func press(_ key: String) -> Bool {
        let rows = navigable
        guard !rows.isEmpty else { return false }
        let at = current.flatMap { rows.firstIndex(of: $0) }
        let starts = rows.enumerated().filter { offset, index in isChange(index) && !(offset > 0 && isChange(rows[offset - 1]) && rows[offset - 1] == index - 1) }.map(\.element)
        let files = rows.enumerated().filter { offset, index in offset == 0 || document.rows[rows[offset - 1]].file != document.rows[index].file }.map(\.element)
        let here = current ?? -1
        func go(_ list: [Int], forward: Bool) {
            let target = forward ? list.first { $0 > here } : list.last { $0 < here }
            if let target { setCurrent(target) }
        }
        switch key {
        case "j", "\u{F701}": setCurrent(at.map { rows[min($0 + 1, rows.count - 1)] } ?? rows[0])
        case "k", "\u{F700}": setCurrent(at.map { rows[max($0 - 1, 0)] } ?? rows[0])
        case "n": go(starts, forward: true)
        case "p": go(starts, forward: false)
        case "]": go(files, forward: true)
        case "[": go(files, forward: false)
        case "g": setCurrent(rows[0])
        case "G": setCurrent(rows[rows.count - 1])
        case "s": switchView(to: view == .unified ? .split : .unified)
        case "\r":
            guard let index = current, let result else { return true }
            let row = document.columns.last![index].kind == .blank ? document.columns[0][index] : document.columns.last![index]
            if row.kind != .removed, let line = row.new, let file = result.files[safe: row.file] { onOpen?((result.top as NSString).appendingPathComponent(file.path), line) }
            else { NSSound.beep() }
        default: return false
        }
        return true
    }

    public func zoom(by step: Int) {
        let target = step == 0 ? CodeTheme.fontSize : Self.clamp(fontSize + CGFloat(step))
        guard target != fontSize else { return }
        fontSize = target
        rebuild()
        applyHighlights()
        onZoom?()
    }

    public func toggle(file path: String, all: Bool) {
        guard let result else { return }
        if all {
            let everyClosed = result.files.allSatisfy { closed.contains($0.path) }
            closed = everyClosed ? [] : Set(result.files.map(\.path))
        } else if closed.contains(path) {
            closed.remove(path)
        } else {
            closed.insert(path)
        }
        rebuild()
        applyHighlights()
    }

    public func showFolded(_ path: String) {
        shown.insert(path)
        setUpdating(true)
        onShowFiles?(Array(shown).sorted())
    }

    public var debugRows: [[String: Any]] {
        document.columns.last!.indices.map { index in
            let rows = document.columns.map { $0[index] }
            let row = rows.count > 1 && rows[1].kind == .blank ? rows[0] : rows.last!
            var out: [String: Any] = ["kind": row.kind.rawValue, "text": row.text]
            if let old = row.old { out["old"] = old }
            if let new = row.new { out["new"] = new }
            if rows.count > 1 { out["left"] = rows[0].kind.rawValue; out["right"] = rows[1].kind.rawValue }
            return out
        }
    }

    public func colors(atRow index: Int) -> [String] {
        guard let storage = columns.last?.textView.textStorage, index < document.rows.count else { return [] }
        let start = columns.last!.textView.start(ofRow: index)
        let length = (document.columns.last![index].text as NSString).length
        var out: [String] = []
        storage.enumerateAttribute(.foregroundColor, in: NSRange(location: start, length: min(length, storage.length - start))) { value, _, _ in
            if let color = value as? NSColor, color != DiffTheme.text { out.append(color.description) }
        }
        return out
    }

    public var highlightedFiles: Int { highlighted.count }
}

extension DiffLineKind {
    var rowKind: DiffRowKind {
        switch self {
        case .context: return .context
        case .added: return .added
        case .removed: return .removed
        }
    }
}

@MainActor
final class DiffHeader: NSView {
    var onView: ((DiffView) -> Void)?
    var updating = false { didSet { busy.isHidden = !updating } }
    private let command = NSTextField(labelWithString: "")
    private let counts = NSTextField(labelWithString: "")
    private let busy = NSTextField(labelWithString: "updating")
    private let switcher = NSSegmentedControl(labels: ["Unified", "Split"], trackingMode: .selectOne, target: nil, action: nil)

    override init(frame: NSRect) {
        super.init(frame: frame)
        command.font = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .medium)
        command.textColor = DiffTheme.text
        command.lineBreakMode = .byTruncatingTail
        counts.font = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
        busy.font = DiffTheme.sans(size: 12)
        busy.textColor = DiffTheme.muted
        busy.isHidden = true
        switcher.controlSize = .small
        switcher.target = self
        switcher.action = #selector(picked)
        [command, counts, busy, switcher].forEach(addSubview)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }

    func show(_ result: DiffResult, view: DiffView) {
        command.stringValue = result.command
        let text = NSMutableAttributedString(string: "\(result.files.count) file\(result.files.count == 1 ? "" : "s") · ", attributes: [.foregroundColor: DiffTheme.muted])
        text.append(NSAttributedString(string: "+\(result.added)", attributes: [.foregroundColor: DiffTheme.ok]))
        text.append(NSAttributedString(string: " −\(result.removed)", attributes: [.foregroundColor: DiffTheme.bad]))
        text.addAttribute(.font, value: NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular), range: NSRange(location: 0, length: text.length))
        counts.attributedStringValue = text
        switcher.selectedSegment = view == .unified ? 0 : 1
        needsLayout = true
    }

    @objc private func picked() { onView?(switcher.selectedSegment == 0 ? .unified : .split) }

    override func layout() {
        super.layout()
        let height = bounds.height
        switcher.sizeToFit()
        switcher.frame.origin = NSPoint(x: bounds.width - switcher.frame.width - 10, y: (height - switcher.frame.height) / 2)
        busy.sizeToFit()
        busy.frame.origin = NSPoint(x: switcher.frame.minX - busy.frame.width - 12, y: (height - busy.frame.height) / 2)
        command.sizeToFit()
        counts.sizeToFit()
        let room = max(40, busy.frame.minX - 12 - counts.frame.width - 10 - 12)
        command.frame = NSRect(x: 12, y: (height - command.frame.height) / 2, width: min(command.frame.width, room), height: command.frame.height)
        counts.frame.origin = NSPoint(x: command.frame.maxX + 10, y: (height - counts.frame.height) / 2)
    }

    override func draw(_ dirtyRect: NSRect) {
        DiffTheme.background.setFill()
        bounds.fill()
        DiffTheme.line.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }
}
