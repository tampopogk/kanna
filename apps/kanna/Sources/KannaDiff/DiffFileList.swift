import AppKit
import KannaCode

@MainActor
final class DiffFileList: NSView {
    enum Entry: Equatable {
        case folder(String)
        case file(Int)
    }

    var onPick: ((Int) -> Void)?
    private(set) var entries: [Entry] = []
    private var files: [DiffFile] = []
    var selected: Int? { didSet { if selected != oldValue { needsDisplay = true } } }
    var fresh: Set<Int> = [] { didSet { needsDisplay = true } }
    static let rowHeight: CGFloat = 22
    nonisolated static let width: CGFloat = 230

    override var isFlipped: Bool { true }

    func show(_ files: [DiffFile]) {
        self.files = files
        entries = []
        var order: [String] = []
        var groups: [String: [Int]] = [:]
        for (index, file) in files.enumerated() {
            let slash = (file.path as NSString).range(of: "/", options: .backwards)
            let here = slash.location == NSNotFound ? "" : (file.path as NSString).substring(to: slash.location + 1)
            if groups[here] == nil { order.append(here) }
            groups[here, default: []].append(index)
        }
        for folder in order {
            if !folder.isEmpty { entries.append(.folder(folder)) }
            entries += groups[folder, default: []].map(Entry.file)
        }
        frame.size.height = max(superview?.bounds.height ?? 0, CGFloat(entries.count) * Self.rowHeight + 12)
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        DiffTheme.background.setFill()
        dirtyRect.fill()
        let mono = NSFont.monospacedSystemFont(ofSize: 10.5, weight: .regular)
        let sans = NSFont.systemFont(ofSize: 12)
        for (index, entry) in entries.enumerated() {
            let row = NSRect(x: 0, y: 6 + CGFloat(index) * Self.rowHeight, width: bounds.width, height: Self.rowHeight)
            guard row.intersects(dirtyRect) else { continue }
            switch entry {
            case .folder(let folder):
                let text = NSAttributedString(string: folder, attributes: [.font: mono, .foregroundColor: DiffTheme.muted])
                text.draw(with: NSRect(x: 12, y: row.minY + 7, width: row.width - 20, height: 14), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            case .file(let number):
                let file = files[number]
                if number == selected { DiffTheme.tint.setFill(); row.fill() }
                if fresh.contains(number) { DiffTheme.addedWord.setFill(); row.fill(using: .sourceOver) }
                let status = NSAttributedString(string: file.status, attributes: [.font: NSFont.monospacedSystemFont(ofSize: 10.5, weight: .semibold), .foregroundColor: DiffTheme.status(file.status)])
                status.draw(at: NSPoint(x: 12, y: row.minY + 4))
                let counts = NSMutableAttributedString()
                if file.added > 0 { counts.append(NSAttributedString(string: "+\(file.added)", attributes: [.font: mono, .foregroundColor: DiffTheme.ok])) }
                if file.added > 0 && file.removed > 0 { counts.append(NSAttributedString(string: " ", attributes: [.font: mono])) }
                if file.removed > 0 { counts.append(NSAttributedString(string: "−\(file.removed)", attributes: [.font: mono, .foregroundColor: DiffTheme.bad])) }
                let countsWidth = counts.size().width
                counts.draw(at: NSPoint(x: row.maxX - 10 - countsWidth, y: row.minY + 4))
                var attributes: [NSAttributedString.Key: Any] = [.font: sans, .foregroundColor: file.status == "D" ? DiffTheme.muted : DiffTheme.text]
                if file.status == "D" { attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
                let name = NSAttributedString(string: (file.path as NSString).lastPathComponent, attributes: attributes)
                name.draw(with: NSRect(x: 29, y: row.minY + 3, width: max(0, row.width - 29 - countsWidth - 16), height: 16), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            }
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let index = Int((point.y - 6) / Self.rowHeight)
        guard index >= 0, index < entries.count, case .file(let number) = entries[index] else { return }
        onPick?(number)
    }

    func entryRect(file number: Int) -> NSRect? {
        guard let index = entries.firstIndex(of: .file(number)) else { return nil }
        return NSRect(x: 0, y: 6 + CGFloat(index) * Self.rowHeight, width: bounds.width, height: Self.rowHeight)
    }
}

@MainActor
final class DiffStickyHeader: NSView {
    var onClick: ((Bool) -> Void)?
    var file: DiffFile? { didSet { needsDisplay = true } }
    var gutterWidth: CGFloat = 0
    var font = CodeTheme.font(size: CodeTheme.fontSize)

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        DiffTheme.panel.setFill()
        bounds.fill()
        DiffTheme.line.setFill()
        NSRect(x: 0, y: bounds.maxY - 1, width: bounds.width, height: 1).fill()
        guard let file else { return }
        let status = NSAttributedString(string: file.status, attributes: [.font: NSFont.monospacedSystemFont(ofSize: font.pointSize - 1, weight: .semibold), .foregroundColor: DiffTheme.status(file.status)])
        let y = (bounds.height - status.size().height) / 2
        status.draw(at: NSPoint(x: gutterWidth - status.size().width - 8, y: y))
        let slash = (file.path as NSString).range(of: "/", options: .backwards)
        let folder = slash.location == NSNotFound ? "" : (file.path as NSString).substring(to: slash.location + 1)
        let text = NSMutableAttributedString(string: folder, attributes: [.font: font, .foregroundColor: DiffTheme.muted])
        text.append(NSAttributedString(string: (file.path as NSString).substring(from: folder.utf16.count), attributes: [.font: font, .foregroundColor: DiffTheme.text]))
        text.append(NSAttributedString(string: "  ", attributes: [.font: font]))
        if file.added > 0 { text.append(NSAttributedString(string: "+\(file.added)", attributes: [.font: font, .foregroundColor: DiffTheme.ok])) }
        if file.removed > 0 { text.append(NSAttributedString(string: (file.added > 0 ? " " : "") + "−\(file.removed)", attributes: [.font: font, .foregroundColor: DiffTheme.bad])) }
        text.draw(at: NSPoint(x: gutterWidth, y: (bounds.height - text.size().height) / 2))
        let shadow = NSGradient(starting: NSColor.black.withAlphaComponent(0.12), ending: .clear)
        shadow?.draw(in: NSRect(x: 0, y: bounds.maxY, width: bounds.width, height: 3), angle: 90)
    }

    override func mouseDown(with event: NSEvent) {
        onClick?(event.modifierFlags.contains(.option))
    }
}
