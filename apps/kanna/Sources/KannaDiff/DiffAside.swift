import AppKit
import KannaOverview

enum DiffAside: CaseIterable {
    case files
    case overview

    static let textColumns: CGFloat = 110

    var width: CGFloat { panel.width }

    var panel: OverviewPanel {
        switch self {
        case .files: OverviewPanel(name: "files", width: DiffFileList.width + 1, columns: Self.textColumns)
        case .overview: .overview
        }
    }

    static func columnWidth(_ font: NSFont) -> CGFloat { OverviewLayout.columnWidth(font) }

    static func shown(_ wanted: [DiffAside], width: CGFloat, font: NSFont) -> [DiffAside] {
        let panels = OverviewLayout.shown(wanted.map(\.panel), width: width, columnWidth: columnWidth(font))
        return wanted.filter { aside in panels.contains(aside.panel) }
    }
}
