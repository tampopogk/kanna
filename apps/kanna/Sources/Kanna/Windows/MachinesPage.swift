import AppKit

struct MachineRow: Equatable {
    let name: String
    let detail: String
    let path: String
    let note: String
    let explanation: String

    var tone: NSColor {
        switch path {
        case "Relayed": return .systemOrange
        case "Offline", "Incompatible": return .systemRed
        default: return .secondaryLabelColor
        }
    }
}

@MainActor
final class MachinesPage: NSView {
    private let scroller = NSScrollView()
    private let document = MachinesDocument()
    private var shown: [MachineRow]?
    private var phones: [String] = []
    var pair: (() -> Void)?
    var revoke: ((String) -> Void)?
    private(set) var badges: [String: NSRect] = [:]

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        scroller.drawsBackground = false
        scroller.hasVerticalScroller = true
        scroller.autoresizingMask = [.width, .height]
        scroller.frame = bounds
        scroller.documentView = document
        addSubview(scroller)
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    private func paint() { layer?.backgroundColor = resolved(.textBackgroundColor) }

    var machines: [MachineRow] { shown ?? [] }

    func show(_ machines: [MachineRow], phones: [String] = []) {
        guard machines != shown || phones != self.phones else { return }
        shown = machines
        self.phones = phones
        lay()
    }

    @objc private func pairPressed() { pair?() }

    @objc private func revokePressed(_ sender: NSButton) {
        guard phones.indices.contains(sender.tag) else { return }
        revoke?(phones[sender.tag])
    }

    override func resizeSubviews(withOldSize oldSize: NSSize) {
        super.resizeSubviews(withOldSize: oldSize)
        if oldSize.width != bounds.width { lay() }
    }

    private func label(_ text: String, size: CGFloat = 12, weight: NSFont.Weight = .regular, color: NSColor = .labelColor) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = .systemFont(ofSize: size, weight: weight)
        field.textColor = color
        field.lineBreakMode = .byTruncatingTail
        field.sizeToFit()
        document.addSubview(field)
        return field
    }

    private func lay() {
        document.subviews.forEach { $0.removeFromSuperview() }
        badges = [:]
        let left: CGFloat = 32, width = max(320, min(800, scroller.contentSize.width - left * 2))
        let column = left + (width * 0.56).rounded()
        var y: CGFloat = 28
        let title = label("Machines", size: 22, weight: .bold)
        title.frame.origin = NSPoint(x: left, y: y)
        y += title.frame.height + 4
        let sub = label("Your machines · all projects", size: 13, color: .secondaryLabelColor)
        sub.frame.origin = NSPoint(x: left, y: y)
        let pairButton = NSButton(title: "Pair a Phone", target: self, action: #selector(pairPressed))
        pairButton.sizeToFit()
        pairButton.frame.origin = NSPoint(x: left + width - pairButton.frame.width, y: y - 2)
        document.addSubview(pairButton)
        y += sub.frame.height + 26
        for (text, x) in [("MACHINE", left), ("PATH FROM THIS MAC", column)] {
            let head = label(text, size: 11, weight: .medium, color: .secondaryLabelColor)
            head.frame.origin = NSPoint(x: x, y: y)
        }
        y += 22
        for machine in shown ?? [] {
            document.addSubview(MachinesRule(frame: NSRect(x: left, y: y, width: width, height: 1)))
            y += 14
            let name = label(machine.name, size: 13, weight: .semibold)
            name.frame.origin = NSPoint(x: left, y: y)
            let detail = label(machine.detail, color: .secondaryLabelColor)
            detail.frame = NSRect(x: left, y: y + 22, width: column - left - 16, height: detail.frame.height)
            let badge = MachinesBadge(text: machine.path, tone: machine.tone, explanation: machine.explanation)
            badge.frame.origin = NSPoint(x: column, y: y - 2)
            document.addSubview(badge)
            badges[machine.name] = badge.frame
            let note = label(machine.note, color: .secondaryLabelColor)
            note.frame = NSRect(x: column, y: y + 26, width: left + width - column, height: note.frame.height)
            y += 56
        }
        document.addSubview(MachinesRule(frame: NSRect(x: left, y: y, width: width, height: 1)))
        if !phones.isEmpty {
            y += 24
            let head = label("PHONES", size: 11, weight: .medium, color: .secondaryLabelColor)
            head.frame.origin = NSPoint(x: left, y: y)
            y += 22
            for (index, phone) in phones.enumerated() {
                document.addSubview(MachinesRule(frame: NSRect(x: left, y: y, width: width, height: 1)))
                y += 12
                let name = label(phone, size: 13, weight: .semibold)
                name.frame.origin = NSPoint(x: left, y: y + 2)
                let button = NSButton(title: "Revoke", target: self, action: #selector(revokePressed(_:)))
                button.tag = index
                button.sizeToFit()
                button.frame.origin = NSPoint(x: left + width - button.frame.width, y: y)
                document.addSubview(button)
                y += 36
            }
            document.addSubview(MachinesRule(frame: NSRect(x: left, y: y, width: width, height: 1)))
        }
        y += 24
        let footer = label("Connections change automatically. Tasks stay where they run when you bring them to a Desk.", color: .secondaryLabelColor)
        footer.frame = NSRect(x: left, y: y, width: width, height: footer.frame.height)
        y += footer.frame.height + 32
        document.frame = NSRect(x: 0, y: 0, width: scroller.contentSize.width, height: max(y, scroller.contentSize.height))
    }
}

private final class MachinesDocument: NSView {
    override var isFlipped: Bool { true }
}

private final class MachinesRule: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        bounds.fill()
    }
}

@MainActor
private final class MachinesBadge: NSView {
    private let text: NSTextField
    private let tone: NSColor

    override var acceptsFirstResponder: Bool { true }

    init(text title: String, tone: NSColor, explanation: String) {
        text = NSTextField(labelWithString: title)
        self.tone = tone
        text.font = .systemFont(ofSize: 11, weight: .medium)
        text.textColor = tone
        text.sizeToFit()
        super.init(frame: NSRect(x: 0, y: 0, width: text.frame.width + 14, height: text.frame.height + 6))
        text.frame.origin = NSPoint(x: 7, y: 3)
        addSubview(text)
        toolTip = explanation
        setAccessibilityLabel("\(title): \(explanation)")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draw(_ dirtyRect: NSRect) {
        tone.withAlphaComponent(0.5).setStroke()
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 4, yRadius: 4)
        outline.lineWidth = 1
        outline.stroke()
    }
}
