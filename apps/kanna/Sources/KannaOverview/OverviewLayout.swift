import AppKit

public struct OverviewGeometry: Equatable, Sendable {
    public var viewport: OverviewViewport
    public var scale: CGFloat
    public var height: CGFloat

    public init(viewport: OverviewViewport, scale: CGFloat, height: CGFloat) {
        self.viewport = viewport
        self.scale = scale
        self.height = height
    }

    public var maxTop: CGFloat { max(0, viewport.documentHeight - viewport.height) }
    public var thumbHeight: CGFloat { viewport.documentHeight * scale }
    public var offset: CGFloat {
        guard maxTop > 0 else { return 0 }
        return max(0, thumbHeight - height) * min(max(viewport.top / maxTop, 0), 1)
    }
    public var boxY: CGFloat { viewport.top * scale - offset }
    public var boxHeight: CGFloat { viewport.height * scale }

    public func clamp(_ top: CGFloat) -> CGFloat { min(max(top, 0), maxTop) }

    public func boxContains(_ y: CGFloat) -> Bool { y >= boxY && y <= boxY + boxHeight }

    public func top(clickingThumbAt y: CGFloat) -> CGFloat {
        guard scale > 0 else { return viewport.top }
        return clamp((y + offset) / scale - viewport.height / 2)
    }

    public var dragRate: CGFloat {
        let track = min(thumbHeight, height) - boxHeight
        return track > 0 ? maxTop / track : 0
    }

    public var marksScale: CGFloat { height / max(viewport.documentHeight, viewport.height, 1) }

    public func top(clickingMarksAt y: CGFloat) -> CGFloat { clamp(y / marksScale - viewport.height / 2) }

    public var marksDragRate: CGFloat { 1 / marksScale }

    public func top(dragFrom start: CGFloat, by delta: CGFloat, rate: CGFloat) -> CGFloat { clamp(start + delta * rate) }
}

public struct OverviewPanel: Equatable, Sendable {
    public var name: String
    public var width: CGFloat
    public var columns: CGFloat

    public init(name: String, width: CGFloat, columns: CGFloat) {
        self.name = name
        self.width = width
        self.columns = columns
    }

    public static let overview = OverviewPanel(name: "overview", width: OverviewMetrics.width, columns: OverviewMetrics.columns)
}

public enum OverviewLayout {
    public static func columnWidth(_ font: NSFont) -> CGFloat { font.advancement(forGlyph: font.glyph(withName: "M")).width }

    public static func wanted(enabled: Bool, viewport: OverviewViewport) -> Bool { enabled && !viewport.fits }

    public static func shown(_ wanted: [OverviewPanel], width: CGFloat, columnWidth: CGFloat) -> [OverviewPanel] {
        var shown = wanted
        func beside() -> CGFloat { width - shown.map(\.width).reduce(0, +) }
        while !shown.isEmpty, shown.contains(where: { beside() < $0.columns * columnWidth }) { shown.removeFirst() }
        return shown
    }
}

@MainActor
public enum OverviewSetting {
    public static let changed = Notification.Name("KannaOverviewSettingChanged")

    public static var enabled = true {
        didSet { if enabled != oldValue { NotificationCenter.default.post(name: changed, object: nil) } }
    }
}
