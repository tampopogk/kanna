import AppKit

public struct OverviewLine: @unchecked Sendable {
    public var y: CGFloat
    public var height: CGFloat
    public var range: NSRange
    public var background: NSColor?

    public init(y: CGFloat, height: CGFloat, range: NSRange, background: NSColor? = nil) {
        self.y = y
        self.height = height
        self.range = range
        self.background = background
    }
}

public struct OverviewText: @unchecked Sendable {
    public var string: NSAttributedString
    public var lines: [OverviewLine]

    public init(string: NSAttributedString, lines: [OverviewLine]) {
        self.string = string
        self.lines = lines
    }
}

public enum OverviewContent: @unchecked Sendable {
    case empty
    case text([OverviewText], lineHeight: CGFloat)
    case image(CGImage, scale: CGFloat)

    public var scale: CGFloat {
        switch self {
        case .empty: return 0
        case .text(_, let lineHeight): return lineHeight > 0 ? OverviewMetrics.lineHeight / lineHeight : 0
        case .image(_, let scale): return scale
        }
    }
}

public enum OverviewMarkKind: String, Equatable, Sendable, CaseIterable {
    case added
    case removed
    case changed
    case comment
    case hit

    public var lane: Int {
        switch self {
        case .added, .removed, .changed: return 0
        case .comment, .hit: return 1
        }
    }
}

public struct OverviewMark: Equatable, @unchecked Sendable {
    public var y: CGFloat
    public var height: CGFloat
    public var kind: OverviewMarkKind
    public var color: NSColor

    public init(y: CGFloat, height: CGFloat, kind: OverviewMarkKind, color: NSColor) {
        self.y = y
        self.height = height
        self.kind = kind
        self.color = color
    }

    public static func runs(_ marks: [OverviewMark]) -> [OverviewMark] {
        var out: [OverviewMark] = []
        for mark in marks.sorted(by: { ($0.kind.rawValue, $0.y) < ($1.kind.rawValue, $1.y) }) {
            if let last = out.last, last.kind == mark.kind, last.color == mark.color, mark.y <= last.y + last.height + 0.5 {
                out[out.count - 1].height = max(last.height, mark.y + mark.height - last.y)
            } else {
                out.append(mark)
            }
        }
        return out
    }
}

public struct OverviewViewport: Equatable, Sendable {
    public var documentHeight: CGFloat
    public var contentHeight: CGFloat
    public var top: CGFloat
    public var height: CGFloat

    public init(documentHeight: CGFloat, contentHeight: CGFloat, top: CGFloat, height: CGFloat) {
        self.documentHeight = documentHeight
        self.contentHeight = contentHeight
        self.top = top
        self.height = height
    }

    public var fits: Bool { contentHeight <= height + 0.5 }
}

public enum OverviewMetrics {
    public static let lineHeight: CGFloat = 2
    public static let thumbWidth: CGFloat = 84
    public static let marksWidth: CGFloat = 12
    public static let width: CGFloat = 1 + thumbWidth + 1 + marksWidth
    public static let columns: CGFloat = 80
}
