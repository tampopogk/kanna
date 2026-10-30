import AppKit

enum DiffAside: CaseIterable {
    case files

    static let textColumns: CGFloat = 110

    var width: CGFloat {
        switch self {
        case .files: DiffFileList.width + 1
        }
    }

    static func columnWidth(_ font: NSFont) -> CGFloat { font.advancement(forGlyph: font.glyph(withName: "M")).width }

    static func shown(_ wanted: [DiffAside], width: CGFloat, font: NSFont) -> [DiffAside] {
        let text = textColumns * columnWidth(font)
        var shown = wanted
        while !shown.isEmpty, width - shown.map(\.width).reduce(0, +) < text { shown.removeFirst() }
        return shown
    }
}
