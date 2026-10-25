import AppKit

@MainActor
final class PaneView: NSView {
    let id: String
    let type: String
    private(set) var content: NSView
    private let notice = NSTextField(labelWithString: "")
    private let spinner = Spinner()
    private let outline = Outline()
    let grip = PaneGrip(frame: NSRect(origin: .zero, size: PaneGrip.size))

    init(id: String, type: String, content: NSView) {
        self.id = id
        self.type = type
        self.content = content
        super.init(frame: .zero)
        addSubview(content)

        notice.font = .systemFont(ofSize: 13, weight: .medium)
        notice.textColor = .white
        notice.alignment = .center
        notice.wantsLayer = true
        notice.drawsBackground = true
        notice.backgroundColor = NSColor.black.withAlphaComponent(0.65)
        notice.isHidden = true
        addSubview(notice)

        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        addSubview(spinner)

        outline.wantsLayer = true
        outline.layer?.borderWidth = 2
        outline.paint()
        outline.isHidden = true
        addSubview(outline)

        addSubview(grip)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var terminal: TerminalSurfaceView? { content as? TerminalSurfaceView }

    func replace(_ content: NSView) {
        self.content.removeFromSuperview()
        self.content = content
        content.frame = bounds
        addSubview(content, positioned: .below, relativeTo: notice)
    }

    var waiting = false {
        didSet { waiting && message == nil ? spinner.startAnimation(nil) : spinner.stopAnimation(nil) }
    }

    var focused = false { didSet { outline.isHidden = !focused } }

    var message: String? {
        didSet {
            notice.stringValue = message.map { "  \($0)  " } ?? ""
            notice.isHidden = message == nil
            if message != nil { spinner.stopAnimation(nil) }
            needsLayout = true
        }
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        content.frame = bounds
    }

    override func layout() {
        super.layout()
        content.frame = bounds
        outline.frame = bounds
        notice.sizeToFit()
        notice.frame.origin = NSPoint(x: (bounds.width - notice.frame.width) / 2, y: 12)
        spinner.frame = NSRect(x: ((bounds.width - 16) / 2).rounded(), y: ((bounds.height - 16) / 2).rounded(), width: 16, height: 16)
        grip.frame.origin = NSPoint(x: ((bounds.width - PaneGrip.size.width) / 2).rounded(), y: bounds.height - PaneGrip.size.height)
    }
}

private final class Spinner: NSProgressIndicator {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

private final class Outline: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        paint()
    }
    func paint() { layer?.borderColor = resolved(NSColor.controlAccentColor.withAlphaComponent(0.8)) }
}
