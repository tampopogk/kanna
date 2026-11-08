import AppKit
import KannaDocs
import KannaLinks
import WebKit

@MainActor
final class WebPaneView: NSView, WKNavigationDelegate, NSTextFieldDelegate {
    let webView: FocusReportingWebView
    var onFocus: (() -> Void)? { didSet { webView.onFocus = onFocus } }
    var onNavigate: ((String) -> Void)?
    var onURLChange: ((String) -> Void)?
    var onTitleChange: ((String) -> Void)?
    private var titled: NSKeyValueObservation?
    private(set) var url: URL?
    var waitingFor: String?
    var unmap: (URL) -> URL = { $0 }
    private let notice = NSTextField(wrappingLabelWithString: "")
    private var editor: NSTextField?
    private var retry: Timer?
    private let relay: PageMarkupRelay
    private(set) var markup: PageMarkupView?
    var agent: String? { didSet { markup?.agent = agent } }
    var agentStatus: String? { didSet { markup?.agentStatus = agentStatus } }
    var onSend: DocSend? { didSet { markup?.onSend = onSend } }
    var onMarkup: ((PageMarkupView?) -> Void)?
    var onComments: ((NSView) -> Void)?
    var agentName: ((String) -> String)? { didSet { markup?.agentName = agentName } }
    private let bar = DocBar()
    private var barShown = false

    var markable: Bool { didSet { if markable != oldValue { markUp(webView.url ?? url) } } }
    private(set) var loads = 0

    private let files: MachineFiles?
    var pagePath: @MainActor (URL?) -> String? = { PageMarkupView.page(for: $0) }

    init(url: String, markup: Bool = false, store: WKWebsiteDataStore = .default(), files: MachineFiles? = nil) {
        markable = markup
        self.files = files
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = store
        if let files { configuration.setURLSchemeHandler(files, forURLScheme: MachineFileURL.scheme) }
        relay = PageMarkupView.install(on: configuration)
        webView = FocusReportingWebView(frame: .zero, configuration: configuration)
        super.init(frame: .zero)
        webView.navigationDelegate = self
        titled = webView.observe(\.title, options: [.new]) { [weak self] view, _ in
            MainActor.assumeIsolated { self?.onTitleChange?(view.title ?? "") }
        }
        addSubview(webView)
        bar.comments.isHidden = false
        bar.comments.onPress = { [weak self] in
            guard let self else { return }
            onComments?(bar.comments)
        }
        notice.alignment = .center
        notice.font = .systemFont(ofSize: 13)
        notice.textColor = .secondaryLabelColor
        notice.isHidden = true
        addSubview(notice)
        load(url)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func load(_ string: String) {
        retry?.invalidate()
        notice.isHidden = true
        guard let url = URL(string: string) else {
            show("Not a URL: \(string)")
            return
        }
        self.url = url
        markUp(url)
        webView.load(URLRequest(url: url))
    }

    private func markUp(_ url: URL?) {
        let local = pagePath(url)
        showBar(markable ? nil : local)
        let page = markable ? local : nil
        guard page != markup?.path else { return }
        markup?.stop()
        markup?.removeFromSuperview()
        markup = nil
        if let page {
            let view = PageMarkupView(path: page, webView: webView)
            view.agent = agent
            view.agentStatus = agentStatus
            view.onSend = onSend
            view.agentName = agentName
            addSubview(view, positioned: .below, relativeTo: notice)
            markup = view
        } else {
            addSubview(webView, positioned: .below, relativeTo: notice)
        }
        relay.target = markup
        needsLayout = true
        onMarkup?(markup)
    }

    private func showBar(_ page: String?) {
        barShown = page != nil
        if let page {
            bar.path.stringValue = (page as NSString).lastPathComponent
            if bar.superview == nil { addSubview(bar) }
        } else {
            bar.removeFromSuperview()
        }
        needsLayout = true
    }

    var commentsButtonShown: Bool { barShown }
    var barText: [String] { barShown ? [bar.path.stringValue, bar.comments.label.stringValue] : markup?.barText ?? [] }
    func pressComments() { bar.comments.onPress?() }

    var zoom: CGFloat { webView.magnification }

    func zoom(by step: Int) {
        let steps = Self.zoomSteps, now = webView.magnification
        let target = step == 0 ? 1 : step > 0 ? (steps.first { $0 > now + 0.001 } ?? steps.last!) : (steps.last { $0 < now - 0.001 } ?? steps.first!)
        webView.zoom(to: target, around: NSPoint(x: webView.bounds.midX, y: webView.bounds.midY))
    }

    static let zoomSteps: [CGFloat] = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3]

    func stop() {
        retry?.invalidate()
        markup?.stop()
        webView.stopLoading()
    }

    override func layout() {
        super.layout()
        let barHeight: CGFloat = barShown ? 34 : 0
        bar.frame = NSRect(x: 0, y: bounds.height - barHeight, width: bounds.width, height: barHeight)
        if let markup { markup.frame = bounds } else { webView.frame = NSRect(x: 0, y: 0, width: bounds.width, height: max(0, bounds.height - barHeight)) }
        let width = min(bounds.width - 40, 420)
        let size = notice.sizeThatFits(NSSize(width: width, height: .greatestFiniteMagnitude))
        notice.frame = NSRect(x: (bounds.width - width) / 2, y: (bounds.height - size.height) / 2, width: width, height: size.height)
        editor?.frame = NSRect(x: 8, y: bounds.height - 34, width: bounds.width - 16, height: 26)
    }

    private func show(_ message: String) {
        notice.stringValue = message
        notice.isHidden = false
        needsLayout = true
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        loads += 1
        notice.isHidden = true
        if let current = webView.url, current != url {
            url = current
            onURLChange?(current.absoluteString)
        }
        markUp(webView.url)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    private func failed(_ error: Error) {
        let error = error as NSError
        if error.domain == NSURLErrorDomain, error.code == NSURLErrorCancelled { return }
        guard let url else { return }
        let refused = error.domain == NSURLErrorDomain && [NSURLErrorCannotConnectToHost, NSURLErrorNetworkConnectionLost, NSURLErrorTimedOut].contains(error.code)
        let lost = waitingFor != nil && error.domain == NSURLErrorDomain && [NSURLErrorZeroByteResource, NSURLErrorBadServerResponse].contains(error.code)
        if refused || lost, ["localhost", "127.0.0.1", "::1"].contains(url.host ?? "") {
            show("Waiting for \(waitingFor ?? "\(url.host ?? "")\(url.port.map { ":\($0)" } ?? "")")…\nKanna keeps retrying until the server responds.")
            retry = Timer.scheduledTimer(withTimeInterval: 1, repeats: false) { [weak self] _ in
                MainActor.assumeIsolated { if let self, let url = self.url { self.webView.load(URLRequest(url: url)) } }
            }
        } else {
            show("Couldn't load \(url.absoluteString)\n\(error.localizedDescription)")
        }
    }

    func editURL() {
        guard editor == nil else {
            window?.makeFirstResponder(editor)
            return
        }
        let field = NSTextField(string: url.map(unmap)?.absoluteString ?? "")
        field.font = .systemFont(ofSize: 13)
        field.bezelStyle = .roundedBezel
        field.delegate = self
        addSubview(field)
        editor = field
        needsLayout = true
        layoutSubtreeIfNeeded()
        window?.makeFirstResponder(field)
        field.currentEditor()?.selectAll(nil)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            submit(control.stringValue)
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            closeEditor()
            return true
        default:
            return false
        }
    }

    var editing: String? { editor?.stringValue }

    var shownNotice: String? { notice.isHidden ? nil : notice.stringValue }

    func submit(_ text: String) {
        let text = text.trimmingCharacters(in: .whitespaces)
        closeEditor()
        if !text.isEmpty { onNavigate?(text) }
    }

    func controlTextDidEndEditing(_ notification: Notification) { closeEditor() }

    private func closeEditor() {
        guard let field = editor else { return }
        editor = nil
        field.removeFromSuperview()
        window?.makeFirstResponder(webView)
    }
}

final class FocusReportingWebView: WKWebView {
    var onFocus: (() -> Void)?
    var contextMenu: (() -> NSMenu?)?
    private var pinch: Pinch?

    override func willOpenMenu(_ menu: NSMenu, with event: NSEvent) {
        super.willOpenMenu(menu, with: event)
        window?.makeFirstResponder(self)
        guard let extra = contextMenu?() else { return }
        menu.addItem(.separator())
        for item in extra.items {
            extra.removeItem(item)
            menu.addItem(item)
        }
    }

    func zoom(to target: CGFloat, around point: NSPoint) {
        let shift = magnification / target - 1
        setMagnification(target, centeredAt: NSPoint(x: (point.x * shift).rounded(), y: (point.y * shift).rounded()))
    }

    override func magnify(with event: NSEvent) {
        if event.phase == .began || pinch == nil { pinch = startPinch(at: convert(event.locationInWindow, from: nil)) }
        if let pinch {
            let steps = WebPaneView.zoomSteps
            let target = min(max(magnification * (1 + event.magnification), steps.first!), steps.last!)
            if target != magnification { zoom(to: target, around: pinch.point, holding: pinch.anchor) }
        }
        if event.phase != .began, event.phase != .changed { pinch = nil }
    }

    private func startPinch(at point: NSPoint) -> Pinch {
        let pinch = Pinch(point: point), scale = magnification
        evaluateJavaScript("[visualViewport.pageLeft, visualViewport.pageTop]", in: nil, in: .defaultClient) { result in
            guard case .success(let value) = result, let page = value as? [Double], page.count == 2 else { return }
            pinch.anchor = NSPoint(x: page[0] + point.x / scale, y: page[1] + point.y / scale)
        }
        return pinch
    }

    private func zoom(to target: CGFloat, around point: NSPoint, holding anchor: NSPoint?) {
        zoom(to: target, around: point)
        guard let anchor else { return }
        let left = anchor.x - point.x / target, top = anchor.y - point.y / target
        evaluateJavaScript("scrollTo({left: \(left), top: \(top), behavior: 'instant'})", in: nil, in: .defaultClient, completionHandler: nil)
    }

    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result { onFocus?() }
        return result
    }
}

@MainActor
private final class Pinch {
    let point: NSPoint
    var anchor: NSPoint?

    init(point: NSPoint) { self.point = point }
}
