import AppKit

public struct CodeTourStep: Equatable, Sendable {
    public let at: Int
    public let of: Int

    public init(at: Int, of: Int) {
        self.at = at
        self.of = max(of, at)
    }

    public var last: Bool { at >= of }
}

public struct CodeComment: Equatable, Sendable {
    public let first: Int
    public let last: Int
    public let text: String
    public let tour: CodeTourStep?

    public init(first: Int, last: Int, text: String, tour: CodeTourStep? = nil) {
        self.first = first
        self.last = max(last, first)
        self.text = text
        self.tour = tour
    }

    func contains(_ line: Int) -> Bool { line >= first && line <= last }
}

@MainActor
final class CodeCommentBox: NSView {
    static let widest: CGFloat = 620
    static let padding = NSSize(width: 11, height: 7)
    static let margin: CGFloat = 6

    private let heading = NSTextField(labelWithString: "")
    private let body = NSTextField(wrappingLabelWithString: "")
    let footer: CodeTourFooter?

    init(comment: CodeComment, onStep: @escaping (String) -> Void) {
        footer = comment.tour.map { CodeTourFooter(step: $0, onStep: onStep) }
        super.init(frame: .zero)
        wantsLayer = true
        heading.font = .systemFont(ofSize: 11)
        heading.stringValue = comment.last > comment.first ? "lines \(comment.first)–\(comment.last)" : "line \(comment.first)"
        body.font = .systemFont(ofSize: 12.5)
        body.stringValue = comment.text
        body.isSelectable = true
        for label in [heading, body] {
            label.translatesAutoresizingMaskIntoConstraints = true
            addSubview(label)
        }
        if let footer { addSubview(footer) }
        paint()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }

    var text: String { body.stringValue }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }

    private func paint() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = CodeTheme.card.cgColor
            layer?.borderColor = CodeTheme.line.cgColor
        }
        layer?.borderWidth = 1
        layer?.cornerRadius = 6
        heading.textColor = CodeTheme.muted
        body.textColor = CodeTheme.text
        needsDisplay = true
    }

    func height(width: CGFloat) -> CGFloat {
        let inner = width - Self.padding.width * 2 - 3
        let top = heading.intrinsicContentSize.height
        body.preferredMaxLayoutWidth = inner
        let text = body.attributedStringValue.boundingRect(with: NSSize(width: inner, height: .greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading])
        return ceil(Self.padding.height * 2 + top + 2 + text.height + (footer == nil ? 0 : CodeTourFooter.height))
    }

    func place(at origin: NSPoint, width: CGFloat) {
        let height = height(width: width)
        frame = NSRect(x: origin.x, y: origin.y, width: width, height: height)
        let inner = width - Self.padding.width * 2 - 3
        let top = heading.intrinsicContentSize.height
        let foot = footer == nil ? 0 : CodeTourFooter.height
        heading.frame = NSRect(x: Self.padding.width + 3, y: Self.padding.height, width: inner, height: top)
        body.frame = NSRect(x: Self.padding.width + 3, y: Self.padding.height + top + 2, width: inner, height: height - Self.padding.height * 2 - top - 2 - foot)
        footer?.frame = NSRect(x: Self.padding.width + 3, y: height - Self.padding.height - foot, width: inner, height: foot)
        footer?.layout()
    }

    override func draw(_ dirtyRect: NSRect) {
        CodeTheme.tintEdge.setFill()
        NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: 3, height: bounds.height), xRadius: 1.5, yRadius: 1.5).fill()
    }
}

@MainActor
final class CodeTourFooter: NSView {
    static let height: CGFloat = 36

    let step: CodeTourStep
    let place = NSTextField(labelWithString: "")
    let back: CodeTourButton
    let next: CodeTourButton

    init(step: CodeTourStep, onStep: @escaping (String) -> Void) {
        self.step = step
        back = CodeTourButton("‹ Back", primary: false) { onStep("back") }
        next = CodeTourButton(step.last ? "Done" : "Next ›", primary: true) { onStep("next") }
        super.init(frame: .zero)
        let label = NSMutableAttributedString(string: "\(step.at) of \(step.of)", attributes: [.font: NSFont.systemFont(ofSize: 11, weight: .semibold), .foregroundColor: CodeTheme.text])
        label.append(NSAttributedString(string: " · tour", attributes: [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: CodeTheme.muted]))
        place.attributedStringValue = label
        back.enabled = step.at > 1
        for view in [place, back, next] as [NSView] { addSubview(view) }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }

    override func layout() {
        super.layout()
        let row: CGFloat = 22, top = Self.height - row
        next.frame = NSRect(x: bounds.width - next.width, y: top, width: next.width, height: row)
        back.frame = NSRect(x: next.frame.minX - 6 - back.width, y: top, width: back.width, height: row)
        place.sizeToFit()
        place.frame.origin = NSPoint(x: 0, y: top + ((row - place.frame.height) / 2).rounded())
    }

    override func draw(_ dirtyRect: NSRect) {
        CodeTheme.line.setFill()
        NSRect(x: 0, y: 7, width: bounds.width, height: 1).fill()
    }
}

@MainActor
final class CodeTourButton: NSView {
    let title: String
    let primary: Bool
    var enabled = true { didSet { needsDisplay = true } }
    private let press: () -> Void

    init(_ title: String, primary: Bool, press: @escaping () -> Void) {
        self.title = title
        self.primary = primary
        self.press = press
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { true }

    private var attributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.systemFont(ofSize: 11.5), .foregroundColor: primary ? NSColor.white : CodeTheme.text]
    }

    var width: CGFloat { ceil((title as NSString).size(withAttributes: attributes).width) + 20 }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 5, yRadius: 5)
        if primary {
            CodeTheme.tintEdge.setFill()
            shape.fill()
        } else {
            CodeTheme.line.setStroke()
            shape.stroke()
        }
        let size = (title as NSString).size(withAttributes: attributes)
        NSGraphicsContext.current?.saveGraphicsState()
        if !enabled { NSGraphicsContext.current?.cgContext.setAlpha(0.35) }
        (title as NSString).draw(at: NSPoint(x: ((bounds.width - size.width) / 2).rounded(), y: ((bounds.height - size.height) / 2).rounded()), withAttributes: attributes)
        NSGraphicsContext.current?.restoreGraphicsState()
    }

    override func mouseDown(with event: NSEvent) {
        guard enabled else { return }
        press()
    }

    func click() {
        guard enabled else { return }
        press()
    }
}
