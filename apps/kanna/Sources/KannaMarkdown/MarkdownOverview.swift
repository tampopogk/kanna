import AppKit
import KannaDocs
import KannaOverview

struct MarkdownPictureKey: Equatable {
    var revision: Int
    var width: CGFloat
    var dark: Bool
    var comments: [Int]

    static func comments(_ marks: [DocMark]) -> [Int] {
        marks.flatMap { mark -> [Int] in
            let kind: Int = switch mark.kind {
            case .open: 0
            case .resolved: 1
            case .active: 2
            }
            return [mark.range.location, mark.range.length, kind]
        }
    }
}

@MainActor
enum MarkdownOverviewLayout {
    static let gutter: CGFloat = 20
    static let margin: CGFloat = 28
    static let padding: CGFloat = 5
    static let comments = OverviewPanel(name: "comments", width: ThreadPanel.width, columns: 0, room: ThreadPanel.width)

    static var digit: CGFloat { ("0" as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: MarkdownTheme.bodySize)]).width }

    static func inset(_ width: CGFloat) -> CGFloat { max(margin, (width - (MarkdownTheme.maxTextWidth + padding * 2)) / 2) }

    static func column(_ width: CGFloat) -> CGFloat { max(0, width - inset(width) * 2 - padding * 2) }

    static func scale(column: CGFloat) -> CGFloat { OverviewMetrics.thumbWidth / max(1, column + gutter) }

    static func wanted(enabled: Bool, contentHeight: CGFloat, paneHeight: CGFloat) -> Bool {
        enabled && contentHeight > paneHeight + 0.5
    }

    static func shown(overview: Bool, comments wantsComments: Bool, forced: Bool, width: CGFloat, digit: CGFloat) -> (overview: Bool, comments: Bool) {
        let pinned = wantsComments && forced
        let wanted = (overview ? [OverviewPanel.overview] : []) + (wantsComments && !forced ? [comments] : [])
        let shown = OverviewLayout.shown(wanted, width: width - (pinned ? comments.width : 0), columnWidth: digit) { column($0) }
        return (shown.contains(.overview), pinned || shown.contains(comments))
    }
}

@MainActor
enum MarkdownOverviewMarks {
    static func changes(in text: NSAttributedString, place: (NSRange) -> NSRect) -> [OverviewMark] {
        var out: [OverviewMark] = []
        text.enumerateAttribute(.kannaChange, in: NSRange(location: 0, length: text.length)) { value, range, _ in
            guard let kind = (value as? String).flatMap(ChangeKind.init) else { return }
            let rect = place(range)
            guard !rect.isEmpty else { return }
            switch kind {
            case .added: out.append(OverviewMark(y: rect.minY, height: rect.height, kind: .added, color: MarkdownTheme.addedBar))
            case .removed: out.append(OverviewMark(y: rect.minY, height: rect.height, kind: .removed, color: MarkdownTheme.removedText))
            case .changed: out.append(OverviewMark(y: rect.minY, height: rect.height, kind: .changed, color: MarkdownTheme.changedBar))
            }
        }
        return out
    }

    static func ranges(_ ranges: [NSRange], kind: OverviewMarkKind, color: NSColor, place: (NSRange) -> NSRect) -> [OverviewMark] {
        ranges.compactMap { range in
            let rect = place(range)
            return rect.isEmpty ? nil : OverviewMark(y: rect.minY, height: rect.height, kind: kind, color: color)
        }
    }
}

@MainActor
extension MarkdownLayoutManager {
    func lineRect(for range: NSRange) -> NSRect {
        guard let storage = textStorage, range.location < storage.length else { return .zero }
        let clipped = NSRange(location: range.location, length: max(1, min(range.length, storage.length - range.location)))
        let glyphs = glyphRange(forCharacterRange: clipped, actualCharacterRange: nil)
        var rect = NSRect.null
        enumerateLineFragments(forGlyphRange: glyphs) { _, used, _, _, _ in rect = rect.union(used) }
        return rect.isNull ? .zero : rect
    }

    func picture(origin: NSPoint, contentHeight: CGFloat, backing: CGFloat, appearance: NSAppearance) -> (CGImage, CGFloat)? {
        guard let container = textContainers.first, let storage = textStorage, storage.length > 0 else { return nil }
        ensureLayout(for: container)
        let column = container.size.width - container.lineFragmentPadding * 2
        let scale = MarkdownOverviewLayout.scale(column: column)
        let left = origin.x + container.lineFragmentPadding - MarkdownOverviewLayout.gutter
        let points = NSSize(width: OverviewMetrics.thumbWidth, height: max(1, contentHeight * scale))
        let density = min(backing, 16384 / points.height)
        let pixels = (width: Int((points.width * density).rounded(.up)), height: Int((points.height * density).rounded(.up)))
        guard pixels.width > 0, pixels.height > 0,
              let context = CGContext(data: nil, width: pixels.width, height: pixels.height, bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        context.translateBy(x: 0, y: CGFloat(pixels.height))
        context.scaleBy(x: density * scale, y: -density * scale)
        context.translateBy(x: -left, y: 0)
        let previous = NSGraphicsContext.current
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
        appearance.performAsCurrentDrawingAppearance {
            let page = NSRect(x: left, y: 0, width: column + MarkdownOverviewLayout.gutter, height: contentHeight)
            MarkdownTheme.background.setFill()
            page.fill()
            let glyphs = glyphRange(for: container)
            drawChangeBars(in: page, origin: origin)
            drawBackground(forGlyphRange: glyphs, at: origin)
            drawGlyphs(forGlyphRange: glyphs, at: origin)
        }
        NSGraphicsContext.current = previous
        return context.makeImage().map { ($0, scale) }
    }
}

extension MarkdownPaneView {
    var overviewViewport: OverviewViewport {
        let clip = scrollView.contentView
        var content: CGFloat = 0
        if let container = textView.textContainer, (textView.textStorage?.length ?? 0) > 0 {
            content = layoutManager.usedRect(for: container).height + textView.textContainerInset.height * 2
        }
        return OverviewViewport(documentHeight: textView.frame.height, contentHeight: content, top: clip.bounds.minY, height: clip.bounds.height)
    }

    func sides(comments: Bool? = nil, forced: Bool? = nil) -> (overview: Bool, comments: Bool) {
        if let container = textView.textContainer { layoutManager.ensureLayout(for: container) }
        let wanted = (textView.textStorage?.length ?? 0) > 0
            && MarkdownOverviewLayout.wanted(enabled: OverviewSetting.enabled, contentHeight: overviewViewport.contentHeight, paneHeight: textHeight)
        overviewWanted = wanted
        return MarkdownOverviewLayout.shown(overview: wanted, comments: markup && (comments ?? showThreads), forced: forced ?? forcedThreads,
                                            width: bounds.width, digit: MarkdownOverviewLayout.digit)
    }

    func setUpOverview() {
        overview.background = MarkdownTheme.background
        overview.edge = MarkdownTheme.line
        overview.scrollTarget = scrollView
        overview.onScroll = { [weak self] top in self?.scrollOverview(to: top) }
        overview.isHidden = true
        addSubview(overview)
        scrollView.contentView.postsBoundsChangedNotifications = true
        overviewObservers = [
            NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: scrollView.contentView, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.updateOverviewViewport() }
            },
            NotificationCenter.default.addObserver(forName: OverviewSetting.changed, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.needsLayout = true }
            },
        ]
        textView.onMatches = { [weak self] in self?.updateOverviewMarks() }
    }

    func stopOverview() {
        overviewObservers.forEach(NotificationCenter.default.removeObserver)
        overviewObservers = []
    }

    var pictureKey: MarkdownPictureKey {
        MarkdownPictureKey(revision: revision, width: textView.frame.width, dark: MarkdownTheme.isDark(effectiveAppearance), comments: MarkdownPictureKey.comments(layoutManager.marks))
    }

    func overviewChanged() {
        guard !overviewQueued else { return }
        overviewQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            overviewQueued = false
            updatePicture()
        }
    }

    func updatePicture() {
        guard overviewShown else { return }
        let key = pictureKey
        guard key != drawnKey else { return }
        drawnKey = key
        let backing = window?.backingScaleFactor ?? 2
        if let (image, scale) = layoutManager.picture(origin: textView.textContainerOrigin, contentHeight: overviewViewport.contentHeight, backing: backing, appearance: effectiveAppearance) {
            overview.content = .image(image, scale: scale)
        } else {
            overview.content = .empty
        }
        pictures += 1
        let origin = textView.textContainerOrigin
        let place: (NSRange) -> NSRect = { [layoutManager] range in layoutManager.lineRect(for: range).offsetBy(dx: origin.x, dy: origin.y) }
        var marks: [OverviewMark] = []
        if let storage = textView.textStorage, diffBase != nil { marks += MarkdownOverviewMarks.changes(in: storage, place: place) }
        marks += MarkdownOverviewMarks.ranges(record.threads.filter(\.isOpen).compactMap { anchors[$0.id] }, kind: .comment, color: DocTheme.accent, place: place)
        pictureMarks = marks
        updateOverviewMarks()
        updateOverviewViewport()
    }

    func updateOverviewMarks() {
        guard overviewShown else { return }
        let origin = textView.textContainerOrigin
        let hits = MarkdownOverviewMarks.ranges(textView.matches, kind: .hit, color: MarkdownTheme.hit) { [layoutManager] range in
            layoutManager.lineRect(for: range).offsetBy(dx: origin.x, dy: origin.y)
        }
        overview.marks = OverviewMark.runs(pictureMarks + hits)
    }

    func updateOverviewViewport() {
        guard overviewShown else { return }
        overview.viewport = overviewViewport
    }

    func scrollOverview(to top: CGFloat) {
        let clip = scrollView.contentView
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: top))
        scrollView.reflectScrolledClipView(clip)
    }
}
