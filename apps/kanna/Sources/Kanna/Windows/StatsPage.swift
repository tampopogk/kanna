import AppKit
import KannaWindows

struct StatsRow: Equatable {
    let label: String
    let value: Double?
    let note: String
    var detail = ""
    var figure = ""
    var open: JSON?
}

struct StatsPoint: Equatable {
    let day: Int
    let value: Double
    let label: String
}

struct StatsColumn: Equatable {
    let title: String
    let right: Bool
    let fill: Bool
}

struct StatsCell: Equatable {
    let text: String
    let bar: Double?
    let swatch: Bool
    let mono: Bool
    let tone: String?
}

struct StatsTableRow: Equatable {
    let series: Int?
    let pipeline: String
    let dim: Bool
    let cells: [StatsCell]
}

struct StatsRunPoint: Equatable {
    let at: Double
    let value: Double
    let series: Int?
    let pipeline: String
    let label: String
}

struct StatsChart: Equatable {
    let kind: String
    var columns: [StatsColumn] = []
    var table: [StatsTableRow] = []
    var runPoints: [StatsRunPoint] = []
    var medians: [StatsRunPoint] = []
    var from = 0.0
    var to = 0.0
    var ticks: [String] = []
    let values: [Double?]
    let max: Double?
    let rows: [StatsRow]
    let points: [StatsPoint]
    let line: [Double?]
    let made: [Double?]
    let done: [Double?]
    let what: String
    let empty: String?

    init(_ json: JSON?) {
        let numbers = { (key: String) in (json?[key]?.array ?? []).map { $0.number } }
        kind = json?["kind"]?.string ?? ""
        values = numbers("values")
        max = json?["max"]?.number
        rows = (json?["rows"]?.array ?? []).map {
            StatsRow(label: $0["label"]?.string ?? "", value: $0["value"]?.number, note: $0["note"]?.string ?? "", detail: $0["detail"]?.string ?? "", figure: $0["figure"]?.string ?? "",
                     open: $0["open"].flatMap { $0 == .null ? nil : $0 })
        }
        points = (json?["points"]?.array ?? []).map { StatsPoint(day: Int($0["day"]?.number ?? 0), value: $0["value"]?.number ?? 0, label: $0["label"]?.string ?? "") }
        line = numbers("line")
        made = numbers("made")
        done = numbers("done")
        what = json?["what"]?.string ?? ""
        empty = json?["empty"]?.string
        columns = (json?["columns"]?.array ?? []).map { StatsColumn(title: $0["title"]?.string ?? "", right: $0["right"]?.bool == true, fill: $0["fill"]?.bool == true) }
        table = (json?["rows"]?.array ?? []).filter { $0["cells"] != nil }.map { row in
            StatsTableRow(series: row["series"]?.number.map { Int($0) }, pipeline: row["pipeline"]?.string ?? "", dim: row["dim"]?.bool == true, cells: (row["cells"]?.array ?? []).map {
                StatsCell(text: $0["text"]?.string ?? "", bar: $0["bar"]?.number, swatch: $0["swatch"]?.bool == true, mono: $0["mono"]?.bool == true, tone: $0["tone"]?.string)
            })
        }
        let run = { (item: JSON) in StatsRunPoint(at: item["at"]?.number ?? 0, value: item["value"]?.number ?? 0, series: item["series"]?.number.map { Int($0) }, pipeline: item["pipeline"]?.string ?? "", label: item["label"]?.string ?? "") }
        runPoints = kind == "runs" ? (json?["points"]?.array ?? []).map(run) : []
        medians = (json?["medians"]?.array ?? []).map(run)
        from = json?["from"]?.number ?? 0
        to = json?["to"]?.number ?? 0
        ticks = (json?["ticks"]?.array ?? []).compactMap(\.string)
    }
}

struct StatsTile: Equatable {
    let id: String
    let label: String
    let value: Double?
    let unit: String?
    let note: String
    let span: Int
    let chart: StatsChart
    var tip: String?

    var figure: String { StatsFormat.figure(value, unit: unit) }
}

struct StatsSection: Equatable {
    let id: String
    let title: String
    let note: String
    let empty: String?
    let placeholder: Bool
    let tiles: [StatsTile]
    var filters: [String] = []
    var hasFilters = false
    var legend: [StatsRunPoint] = []
    var allRuns: StatsChart?
}

struct StatsReport: Equatable {
    let project: String
    let days: [String]
    let status: String?
    private(set) var sections: [StatsSection]

    func keeping(_ shown: StatsReport?) -> StatsReport {
        guard sections.isEmpty, let shown, !shown.sections.isEmpty else { return self }
        var kept = self
        kept.sections = shown.sections
        return kept
    }

    init?(_ json: JSON) {
        guard let sections = json["sections"]?.array else { return nil }
        project = json["project"]?.string ?? ""
        status = json["status"]?.string
        days = (json["days"]?.array ?? []).compactMap(\.string)
        self.sections = sections.map { section in
            StatsSection(id: section["id"]?.string ?? "", title: section["title"]?.string ?? "", note: section["note"]?.string ?? "", empty: section["empty"]?.string,
                         placeholder: section["placeholder"]?.bool == true,
                         tiles: (section["tiles"]?.array ?? []).map { tile in
                             StatsTile(id: tile["id"]?.string ?? "", label: tile["label"]?.string ?? "", value: tile["value"]?.number, unit: tile["unit"]?.string,
                                       note: tile["note"]?.string ?? "", span: Int(tile["span"]?.number ?? 2), chart: StatsChart(tile["chart"]), tip: tile["tip"]?.string)
                         },
                         filters: (section["filters"]?["pipelines"]?.array ?? []).compactMap(\.string), hasFilters: section["filters"] != nil,
                         legend: (section["legend"]?.array ?? []).map { StatsRunPoint(at: 0, value: 0, series: $0["series"]?.number.map { Int($0) }, pipeline: $0["pipeline"]?.string ?? "", label: $0["label"]?.string ?? "") },
                         allRuns: section["allRuns"].map { StatsChart($0) })
        }
    }
}

enum StatsInk {
    private static func dynamic(_ light: UInt32, _ dark: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            let hex = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: 1)
        }
    }

    static let series = dynamic(0x2a78d6, 0x3987e5)
    static let context = dynamic(0xb9bec7, 0x5b616b)
    static let grid = dynamic(0xeceef1, 0x262a31)
    static let axis = dynamic(0xc9cdd4, 0x3a404a)
    static let made = dynamic(0x9aa1ab, 0x6f7783)
    static let done = dynamic(0x2f9a68, 0x3f9a6e)
    static let second = dynamic(0xeb6834, 0xd95926)
    static let third = dynamic(0x1baf7a, 0x199e70)
    static let panel = dynamic(0xf4f5f7, 0x1b1e24)
    static let line = dynamic(0xd8dce2, 0x2c313a)
    static let landed = dynamic(0x1f9d5c, 0x5fcf93)
    static let going = dynamic(0x2f63d8, 0x7aa2ff)

    static func of(series: Int?) -> NSColor {
        switch series {
        case 0: return StatsInk.series
        case 1: return second
        case 2: return third
        default: return context
        }
    }
}

@MainActor
final class StatsPage: NSView {
    private let scroller = NSScrollView()
    private let document = StatsDocument()
    private let tip = StatsTip()
    private(set) var report: StatsReport?
    private var location = ""
    private(set) var tileFrames: [String: NSRect] = [:]
    private(set) var bandFrames: [String: NSRect] = [:]
    var onOpen: ((JSON) -> Void)?
    var onDays: (() -> Void)?
    private(set) var lists: [String: StatsList] = [:]
    private(set) var chips: [String: StatsChip] = [:]
    private(set) var runDays = 30
    private(set) var hiddenPipelines: Set<String> = []
    private(set) var tableView = false
    private var tables: [String: StatsTable] = [:]
    private var statusLine: NSTextField?
    private(set) var loadings = 0

    var shownStatus: String? { statusLine.flatMap { $0.isHidden ? nil : $0.stringValue } }

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

    func show(_ report: StatsReport?, project: String) {
        guard report != self.report || project != location else { return }
        let sameNumbers = project == location && report?.days == self.report?.days && report?.sections == self.report?.sections
        self.report = report
        location = project
        if sameNumbers, let statusLine {
            statusLine.stringValue = report?.status ?? ""
            statusLine.isHidden = report?.status == nil
        } else {
            lay()
        }
    }

    override func resizeSubviews(withOldSize oldSize: NSSize) {
        super.resizeSubviews(withOldSize: oldSize)
        if oldSize.width != bounds.width { lay() }
    }

    @discardableResult
    private func label(_ text: String, size: CGFloat = 12, weight: NSFont.Weight = .regular, color: NSColor = .labelColor, wraps: Double? = nil) -> NSTextField {
        let field = wraps == nil ? NSTextField(labelWithString: text) : NSTextField(wrappingLabelWithString: text)
        field.font = .systemFont(ofSize: size, weight: weight)
        field.textColor = color
        if let wraps {
            field.preferredMaxLayoutWidth = wraps
            field.frame.size = field.sizeThatFits(NSSize(width: wraps, height: .greatestFiniteMagnitude))
        } else {
            field.lineBreakMode = .byTruncatingTail
            field.sizeToFit()
        }
        document.addSubview(field)
        return field
    }

    private func lay() {
        document.subviews.forEach { $0.removeFromSuperview() }
        tileFrames = [:]
        bandFrames = [:]
        lists = [:]
        chips = [:]
        tables = [:]
        let left: CGFloat = 32, gap: CGFloat = 22
        let width = max(280, min(980, scroller.contentSize.width - left * 2))
        var y: CGFloat = 28
        let title = label("Stats", size: 22, weight: .bold)
        title.frame.origin = NSPoint(x: left, y: y)
        y += title.frame.height + 4
        let sub = label("\(location) · main · last 30 days", size: 13, color: .secondaryLabelColor)
        sub.frame = NSRect(x: left, y: y, width: width, height: sub.frame.height)
        y += sub.frame.height
        let status = label(report?.status ?? "", color: .secondaryLabelColor)
        status.frame = NSRect(x: left, y: y + 6, width: width, height: status.frame.height)
        status.isHidden = report?.status == nil
        statusLine = status
        guard let report, !report.sections.isEmpty else {
            loadings += 1
            y += 26
            label("Loading…", color: .secondaryLabelColor).frame.origin = NSPoint(x: left, y: y)
            finish(y + 40)
            return
        }
        for section in report.sections {
            y += 40
            let band = StatsBand(frame: NSRect(x: left, y: y, width: width, height: 0))
            document.addSubview(band)
            let inset: CGFloat = 12
            let head = label(section.title, size: 15, weight: .bold)
            head.frame.origin = NSPoint(x: left + inset, y: y + 7)
            let note = label(section.note, color: .secondaryLabelColor)
            let noteX = head.frame.maxX + 10
            note.frame = NSRect(x: noteX, y: y + 7 + head.frame.height - note.frame.height - 1, width: max(0, left + width - inset - noteX), height: note.frame.height)
            band.frame.size.height = head.frame.height + 14
            bandFrames[section.id] = band.frame
            y = band.frame.maxY + 14
            if section.hasFilters { y = layFilters(section, at: NSPoint(x: left, y: y), width: width) }
            if section.tiles.isEmpty {
                if section.placeholder {
                    let box = StatsPlaceholder(frame: NSRect(x: left, y: y, width: width, height: 0))
                    let text = label(section.empty ?? "", color: .secondaryLabelColor, wraps: width - 40)
                    box.frame.size.height = text.frame.height + 36
                    document.addSubview(box, positioned: .below, relativeTo: text)
                    text.frame.origin = NSPoint(x: left + 20, y: y + 18)
                    y = box.frame.maxY
                } else {
                    let text = label(section.empty ?? "", color: .secondaryLabelColor, wraps: min(width, 520))
                    text.frame.origin = NSPoint(x: left, y: y)
                    y = text.frame.maxY
                }
                continue
            }
            let tiles = tableView ? section.tiles.compactMap { tile -> StatsTile? in
                guard tile.chart.kind == "runs" else { return tile }
                guard tile.id == "run-land", let all = section.allRuns else { return nil }
                return StatsTile(id: "all-runs", label: "Run by run", value: nil, unit: nil, note: "", span: 6, chart: all)
            } : section.tiles
            let places = StatsFormat.places(tiles.map(\.span), width: width, gap: gap)
            var rowTop = y, rowBottom = y, row = 0, legendDrawn = false
            for (tile, place) in zip(tiles, places) {
                if place.row != row {
                    row = place.row
                    rowTop = rowBottom + 14
                }
                if tile.chart.kind == "runs", !legendDrawn {
                    legendDrawn = true
                    rowTop = layLegend(section.legend, at: NSPoint(x: left, y: rowTop), width: width)
                    rowBottom = rowTop
                }
                let bottom = layTile(tile, days: report.days, at: NSPoint(x: left + place.x, y: rowTop), width: place.width, hero: section.id == "quiz")
                tileFrames[tile.id] = NSRect(x: left + place.x, y: rowTop, width: place.width, height: bottom - rowTop)
                rowBottom = max(rowBottom, bottom)
            }
            y = rowBottom
        }
        finish(y + 40)
    }

    private func finish(_ height: CGFloat) {
        tip.isHidden = true
        document.addSubview(tip)
        document.frame = NSRect(x: 0, y: 0, width: scroller.contentSize.width, height: max(height, scroller.contentSize.height))
    }

    private func layFilters(_ section: StatsSection, at origin: NSPoint, width: CGFloat) -> CGFloat {
        var x = origin.x
        func word(_ text: String) {
            let field = label(text, color: .secondaryLabelColor)
            field.frame.origin = NSPoint(x: x, y: origin.y + 4)
            x = field.frame.maxX + 8
        }
        func chip(_ title: String, on: Bool, click: @escaping () -> Void) {
            let view = StatsChip(title: title, on: on)
            view.onClick = click
            view.frame.origin = NSPoint(x: x, y: origin.y)
            document.addSubview(view)
            chips[title] = view
            x = view.frame.maxX + 6
        }
        word("Show")
        for (title, days) in [("30 days", 30), ("90 days", 90), ("all", 0)] {
            chip(title, on: runDays == days) { [weak self] in
                guard let self else { return }
                runDays = days
                lay()
                onDays?()
            }
        }
        x += 10
        if !section.filters.isEmpty { word("Pipelines") }
        for name in section.filters {
            chip(name, on: !hiddenPipelines.contains(name)) { [weak self] in
                guard let self else { return }
                if hiddenPipelines.contains(name) { hiddenPipelines.remove(name) } else { hiddenPipelines.insert(name) }
                lay()
            }
        }
        return origin.y + 36
    }

    private func layLegend(_ legend: [StatsRunPoint], at origin: NSPoint, width: CGFloat) -> CGFloat {
        let head = label("RUN BY RUN", size: 11, color: .secondaryLabelColor)
        head.frame.origin = NSPoint(x: origin.x, y: origin.y + 6)
        var x = origin.x
        let y = head.frame.maxY + 6
        for item in legend where !hiddenPipelines.contains(item.pipeline) {
            let swatch = StatsSwatch(frame: NSRect(x: x, y: y + 3, width: 9, height: 9))
            swatch.color = StatsInk.of(series: item.series)
            document.addSubview(swatch)
            let name = label(item.label, size: 11.5, color: .secondaryLabelColor)
            name.frame.origin = NSPoint(x: x + 14, y: y)
            x = name.frame.maxX + 14
        }
        return y + 24
    }

    private func layPipelineTile(_ tile: StatsTile, at origin: NSPoint, width: CGFloat) -> CGFloat {
        var y = origin.y
        if tile.chart.kind == "runs" {
            let card = StatsCard(frame: NSRect(x: origin.x, y: y, width: width, height: 196))
            document.addSubview(card)
            let name = label(tile.label, weight: .semibold)
            name.frame.origin = NSPoint(x: origin.x + 12, y: y + 10)
            let note = label(tile.note, size: 11, color: .secondaryLabelColor, wraps: width - 24)
            note.frame.origin = NSPoint(x: origin.x + 12, y: name.frame.maxY + 1)
            let chart = StatsRunChart(tile: tile, hidden: hiddenPipelines)
            chart.frame = NSRect(x: origin.x + 8, y: note.frame.maxY + 6, width: width - 16, height: card.frame.maxY - note.frame.maxY - 12)
            chart.onHover = { [weak self, weak chart] text, point in
                guard let self, let chart else { return }
                self.hover(text, at: chart.convert(point, to: self.document))
            }
            document.addSubview(chart)
            return card.frame.maxY
        }
        if !tile.label.isEmpty {
            let name = label(tile.label.uppercased(), size: 11, color: .secondaryLabelColor)
            name.frame.origin = NSPoint(x: origin.x, y: y + 6)
            y = name.frame.maxY + 6
        }
        let table = StatsTable(chart: tile.chart, hidden: hiddenPipelines, width: width)
        tables[tile.id] = table
        let holder = NSScrollView(frame: NSRect(x: origin.x, y: y, width: width, height: table.frame.height))
        holder.drawsBackground = false
        holder.hasHorizontalScroller = table.frame.width > width
        holder.autohidesScrollers = true
        holder.verticalScrollElasticity = .none
        holder.documentView = table
        document.addSubview(holder)
        y = holder.frame.maxY
        if !tile.note.isEmpty {
            let note = label(tile.note, size: 11.5, color: .secondaryLabelColor, wraps: width)
            note.frame.origin = NSPoint(x: origin.x, y: y + 8)
            y = note.frame.maxY
        }
        if tile.id == "pipelines" {
            let link = StatsChip(title: tableView ? "Charts" : "Table view", on: false, plain: true)
            link.onClick = { [weak self] in
                guard let self else { return }
                tableView.toggle()
                lay()
            }
            link.frame.origin = NSPoint(x: origin.x, y: y + 4)
            document.addSubview(link)
            chips["Table view"] = link
            let cli = label("kanna stats pipelines --json", size: 11.5, color: .secondaryLabelColor)
            cli.frame.origin = NSPoint(x: link.frame.maxX + 10, y: y + 7)
            y = link.frame.maxY
        }
        return y
    }

    private func layTile(_ tile: StatsTile, days: [String], at origin: NSPoint, width: CGFloat, hero: Bool) -> CGFloat {
        if ["table", "runs"].contains(tile.chart.kind) { return layPipelineTile(tile, at: origin, width: width) }
        var y = origin.y
        let name = label(tile.label, color: .secondaryLabelColor)
        name.frame = NSRect(x: origin.x, y: y, width: width, height: name.frame.height)
        name.toolTip = tile.tip
        y += name.frame.height + 2
        if tile.chart.kind == "list" {
            if tile.chart.rows.isEmpty {
                let note = label(tile.chart.empty ?? "", color: .secondaryLabelColor, wraps: width)
                note.frame.origin = NSPoint(x: origin.x, y: y + 6)
                return note.frame.maxY
            }
            let rows = StatsList(rows: tile.chart.rows, width: width, what: tile.chart.what)
            rows.onOpen = { [weak self] open in self?.onOpen?(open) }
            lists[tile.id] = rows
            rows.frame.origin = NSPoint(x: origin.x, y: y + 6)
            document.addSubview(rows)
            return rows.frame.maxY
        }
        if tile.chart.kind == "ranked" {
            let rows = StatsRanked(rows: tile.chart.rows, width: width)
            rows.frame.origin = NSPoint(x: origin.x, y: y + 6)
            document.addSubview(rows)
            return rows.frame.maxY
        }
        let big = hero && tile.chart.kind == "line"
        let value = label(tile.figure, size: big ? 32 : 24, weight: .semibold)
        value.frame.origin = NSPoint(x: origin.x, y: y)
        value.toolTip = tile.tip
        value.setAccessibilityLabel("\(tile.label): \(tile.figure)\(tile.note.isEmpty ? "" : ", \(tile.note)")")
        if !tile.note.isEmpty {
            let note = label(tile.note, color: .secondaryLabelColor)
            let x = origin.x + value.frame.width + 6
            note.frame = NSRect(x: x, y: value.frame.maxY - note.frame.height - (big ? 6 : 4), width: max(0, origin.x + width - x), height: note.frame.height)
        }
        y = value.frame.maxY + 6
        let chart = StatsChartView(tile: tile, days: days)
        let flow = tile.chart.kind == "flow"
        chart.frame = NSRect(x: origin.x, y: y, width: width, height: flow ? 150 : big ? 120 : 84)
        chart.onHover = { [weak self, weak chart] text, point in
            guard let self, let chart else { return }
            self.hover(text, at: chart.convert(point, to: self.document))
        }
        document.addSubview(chart)
        guard flow else { return chart.frame.maxY }
        let legend = label("", size: 11, color: .secondaryLabelColor)
        legend.attributedStringValue = StatsChartView.legend([(StatsInk.series, tile.chart.what), (StatsInk.made, "made"), (StatsInk.done, "done")])
        legend.sizeToFit()
        legend.frame.origin = NSPoint(x: origin.x, y: chart.frame.maxY + 4)
        return legend.frame.maxY
    }

    private func hover(_ text: NSAttributedString?, at point: NSPoint) {
        guard let text else { return tip.isHidden = true }
        tip.show(text)
        var x = point.x + 12
        if x + tip.frame.width > document.bounds.width - 8 { x = point.x - tip.frame.width - 12 }
        tip.frame.origin = NSPoint(x: max(4, x), y: point.y + 14)
        tip.isHidden = false
    }

    var shownTiles: [JSON] {
        (report?.sections ?? []).flatMap { section in
            section.tiles.map { tile -> JSON in
                ["section": .string(section.id), "id": .string(tile.id), "label": .string(tile.label), "figure": .string(["ranked", "list"].contains(tile.chart.kind) ? "" : tile.figure), "note": .string(tile.note),
                 "rows": .array(tile.chart.kind == "list" ? tile.chart.rows.map { .string($0.label) } : []), "tip": tile.tip.map { .string($0) } ?? .null, "kind": .string(tile.chart.kind),
                 "table": .array((tables[tile.id]?.shown ?? []).map { row in .array(row.cells.map { .string($0.text) }) }), "dots": .number(Double(tile.chart.runPoints.filter { !hiddenPipelines.contains($0.pipeline) }.count)),
                 "frame": tileFrames[tile.id].map { ["x": .number($0.minX), "y": .number($0.minY), "width": .number($0.width), "height": .number($0.height)] } ?? .null]
            }
        }
    }

    func scrollToPipelines() {
        guard let chip = chips["30 days"] else { return }
        scroller.contentView.scroll(to: NSPoint(x: 0, y: max(0, chip.frame.minY - 50)))
        scroller.reflectScrolledClipView(scroller.contentView)
    }

    var shownSections: [JSON] {
        (report?.sections ?? []).map { ["id": .string($0.id), "title": .string($0.title), "empty": $0.empty.map { .string($0) } ?? .null, "placeholder": .bool($0.placeholder), "tiles": .number(Double($0.tiles.count)), "chips": .array(chips.keys.sorted().map { .string($0) }), "on": .array(chips.filter { $0.value.on }.keys.sorted().map { .string($0) }), "tableView": .bool(tableView), "band": bandFrames[$0.id].map { ["x": .number($0.minX), "y": .number($0.minY), "width": .number($0.width), "height": .number($0.height)] } ?? .null] }
    }
}

private final class StatsDocument: NSView {
    override var isFlipped: Bool { true }
}

private final class StatsBand: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.quaternaryLabelColor.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 6, yRadius: 6).fill()
    }
}

private final class StatsPlaceholder: NSView {
    override func draw(_ dirtyRect: NSRect) {
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 8, yRadius: 8)
        outline.setLineDash([4, 3], count: 2, phase: 0)
        outline.lineWidth = 1
        NSColor.separatorColor.setStroke()
        outline.stroke()
    }
}

@MainActor
private final class StatsTip: NSView {
    private let text = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.borderWidth = 1
        text.maximumNumberOfLines = 0
        addSubview(text)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        layer?.backgroundColor = resolved(.windowBackgroundColor)
        layer?.borderColor = resolved(.separatorColor)
    }

    func show(_ shown: NSAttributedString) {
        text.attributedStringValue = shown
        text.sizeToFit()
        text.frame.origin = NSPoint(x: 8, y: 5)
        frame.size = NSSize(width: text.frame.width + 16, height: text.frame.height + 10)
        needsDisplay = true
    }
}

@MainActor
private final class StatsRanked: NSView {
    override var isFlipped: Bool { true }

    init(rows: [StatsRow], width: CGFloat) {
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: CGFloat(rows.count) * 24))
        let name: CGFloat = min(150, width * 0.4), figures: CGFloat = 86
        for (i, row) in rows.enumerated() {
            let y = CGFloat(i) * 24
            let label = NSTextField(labelWithString: row.label)
            label.font = .systemFont(ofSize: 12)
            label.lineBreakMode = .byTruncatingTail
            label.frame = NSRect(x: 0, y: y + 3, width: name - 10, height: 17)
            addSubview(label)
            let bar = StatsBar(share: (row.value ?? 0) / 100)
            bar.frame = NSRect(x: name, y: y + 8, width: max(0, width - name - figures - 10), height: 8)
            addSubview(bar)
            let figure = NSTextField(labelWithString: "\(StatsFormat.figure(row.value, unit: "%")) · \(row.note)")
            figure.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
            figure.textColor = .secondaryLabelColor
            figure.alignment = .right
            figure.frame = NSRect(x: width - figures, y: y + 3, width: figures, height: 17)
            addSubview(figure)
            let tip = "\(row.label): \(row.note) right, \(StatsFormat.figure(row.value, unit: "%"))"
            [label, bar, figure].forEach { $0.toolTip = tip }
            label.setAccessibilityLabel(tip)
        }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
}

@MainActor
final class StatsList: NSView {
    static let height: CGFloat = 38
    let rows: [StatsRow]
    var onOpen: ((JSON) -> Void)?
    private var hovered: Int?

    override var isFlipped: Bool { true }

    init(rows: [StatsRow], width: CGFloat, what: String) {
        self.rows = rows
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: CGFloat(rows.count) * Self.height))
        let top = rows.compactMap(\.value).max() ?? 0
        let figures: CGFloat = 92
        for (i, row) in rows.enumerated() {
            let y = CGFloat(i) * Self.height
            let name = NSTextField(labelWithString: row.label)
            name.font = .systemFont(ofSize: 12, weight: .medium)
            name.lineBreakMode = .byTruncatingTail
            name.frame = NSRect(x: 0, y: y + 2, width: width - figures - 8, height: 17)
            let figure = NSTextField(labelWithString: row.figure)
            figure.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
            figure.alignment = .right
            figure.frame = NSRect(x: width - figures, y: y + 2, width: figures, height: 17)
            let under = [row.detail, row.note].filter { !$0.isEmpty }.joined(separator: " · ")
            let detail = NSTextField(labelWithString: under)
            detail.font = .systemFont(ofSize: 11)
            detail.textColor = .secondaryLabelColor
            detail.lineBreakMode = .byTruncatingMiddle
            detail.frame = NSRect(x: 0, y: y + 19, width: width - 8, height: 15)
            let bar = StatsBar(share: top > 0 ? (row.value ?? 0) / top : 0)
            bar.frame = NSRect(x: width - figures, y: y + 22, width: figures, height: 6)
            for view in [name, figure, detail, bar] as [NSView] {
                view.toolTip = "\(row.label): \(row.figure), \(what)\(row.open == nil ? "" : ". Click to open it.")"
                addSubview(view)
            }
            detail.frame.size.width = width - figures - 14
            name.setAccessibilityLabel("\(row.label), \(row.figure), \(under)")
        }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func center(of row: Int) -> NSPoint? {
        guard rows.indices.contains(row) else { return nil }
        let spot = NSRect(x: 0, y: CGFloat(row) * Self.height, width: bounds.width, height: Self.height)
        scrollToVisible(spot)
        return convert(NSPoint(x: spot.midX, y: spot.midY), to: nil)
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        frame.contains(point) ? self : nil
    }

    private func index(_ event: NSEvent) -> Int? {
        let point = convert(event.locationInWindow, from: nil)
        let i = Int(point.y / Self.height)
        return bounds.contains(point) && rows.indices.contains(i) ? i : nil
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .cursorUpdate], owner: self))
    }

    override func mouseMoved(with event: NSEvent) {
        let i = index(event).flatMap { rows[$0].open == nil ? nil : $0 }
        guard i != hovered else { return }
        hovered = i
        needsDisplay = true
    }

    override func mouseExited(with event: NSEvent) {
        hovered = nil
        needsDisplay = true
    }

    override func cursorUpdate(with event: NSEvent) {
        (index(event).flatMap { rows[$0].open } == nil ? NSCursor.arrow : NSCursor.pointingHand).set()
    }

    override func mouseDown(with event: NSEvent) {
        guard let i = index(event), rows[i].open != nil else { return }
        while let next = window?.nextEvent(matching: [.leftMouseUp, .leftMouseDragged]) {
            guard next.type == .leftMouseUp else { continue }
            if index(next) == i, let open = rows[i].open { onOpen?(open) }
            return
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let hovered else { return }
        NSColor.selectedContentBackgroundColor.withAlphaComponent(0.12).setFill()
        NSBezierPath(roundedRect: NSRect(x: -6, y: CGFloat(hovered) * Self.height - 1, width: bounds.width + 12, height: Self.height - 2), xRadius: 5, yRadius: 5).fill()
    }
}

private final class StatsBar: NSView {
    let share: Double

    init(share: Double) {
        self.share = share
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draw(_ dirtyRect: NSRect) {
        let length = bounds.width * CGFloat(min(max(share, 0), 1))
        guard length > 0 else { return }
        StatsInk.series.setFill()
        let r = min(4, length / 2, bounds.height / 2)
        let path = NSBezierPath()
        path.move(to: NSPoint(x: 0, y: 0))
        path.line(to: NSPoint(x: length - r, y: 0))
        path.appendArc(withCenter: NSPoint(x: length - r, y: r), radius: r, startAngle: 270, endAngle: 360)
        path.line(to: NSPoint(x: length, y: bounds.height - r))
        path.appendArc(withCenter: NSPoint(x: length - r, y: bounds.height - r), radius: r, startAngle: 0, endAngle: 90)
        path.line(to: NSPoint(x: 0, y: bounds.height))
        path.close()
        path.fill()
    }
}

@MainActor
final class StatsChartView: NSView {
    let tile: StatsTile
    let days: [String]
    var onHover: ((NSAttributedString?, NSPoint) -> Void)?
    private var hovered: Int?
    private let gutter: CGFloat = 34, under: CGFloat = 18, top: CGFloat = 4

    override var isFlipped: Bool { true }

    init(tile: StatsTile, days: [String]) {
        self.tile = tile
        self.days = days
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
        let values = tile.chart.kind == "dots" ? tile.chart.line : tile.chart.values
        setAccessibilityLabel("\(tile.label), last \(days.count) days: \(values.map { StatsFormat.figure($0, unit: tile.unit) }.joined(separator: ", "))")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self))
    }

    private var count: Int { max(1, days.count) }
    private var plot: NSRect { NSRect(x: gutter, y: top, width: max(1, bounds.width - gutter - 2), height: max(1, bounds.height - top - under)) }
    private func x(_ day: Int) -> CGFloat { plot.minX + (CGFloat(day) + 0.5) * plot.width / CGFloat(count) }

    private var scale: Double {
        if let max = tile.chart.max { return max }
        if tile.chart.kind == "dots" {
            let all = tile.chart.points.map(\.value)
            return StatsFormat.niceDuration(StatsFormat.percentile(all, 0.9) ?? 3600)
        }
        let top = tile.chart.values.compactMap { $0 }.max() ?? 0
        return tile.unit == "s" ? StatsFormat.niceDuration(top) : StatsFormat.nice(top)
    }

    private var strip: CGFloat { tile.chart.kind == "flow" ? 34 : 0 }
    private var baseline: CGFloat { plot.maxY - (strip > 0 ? strip + 6 : 0) }

    private func y(_ value: Double) -> CGFloat { baseline - CGFloat(min(value, scale) / scale) * (baseline - plot.minY) }

    private func tick(_ value: Double) -> String { tile.unit == "s" ? StatsFormat.duration(value) : tile.unit == "h" ? StatsFormat.hours(value) : tile.unit == "%" ? "\(Int(value))%" : "\(Int(value))" }

    override func draw(_ dirtyRect: NSRect) {
        let plot = self.plot
        StatsInk.grid.setFill()
        NSRect(x: plot.minX, y: plot.minY, width: plot.width, height: 1).fill()
        StatsInk.axis.setFill()
        NSRect(x: plot.minX, y: plot.maxY, width: plot.width, height: 1).fill()
        let small: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .regular), .foregroundColor: NSColor.secondaryLabelColor]
        func text(_ string: String, _ point: NSPoint, align: CGFloat) {
            let size = (string as NSString).size(withAttributes: small)
            (string as NSString).draw(at: NSPoint(x: point.x - size.width * align, y: point.y - size.height / 2), withAttributes: small)
        }
        text(tick(scale), NSPoint(x: plot.minX - 6, y: plot.minY), align: 1)
        text("0", NSPoint(x: plot.minX - 6, y: baseline), align: 1)
        if strip > 0 {
            StatsInk.axis.setFill()
            NSRect(x: plot.minX, y: baseline, width: plot.width, height: 1).fill()
        }
        if days.count > 1 {
            for (day, align) in [(0, 0.0), (days.count / 2, 0.5), (days.count - 1, 1.0)] {
                text(StatsFormat.day(days[day], isToday: day == days.count - 1), NSPoint(x: x(day), y: bounds.height - 7), align: align)
            }
        }
        switch tile.chart.kind {
        case "columns": columns()
        case "line": line(tile.chart.values, wash: true)
        case "flow":
            bars()
            line(tile.chart.values, wash: true)
        case "dots": dots()
        default: break
        }
        if let hovered {
            NSColor.secondaryLabelColor.setFill()
            NSRect(x: x(hovered) - 0.5, y: plot.minY, width: 1, height: plot.height).fill()
        }
    }

    private func columns() {
        let band = plot.width / CGFloat(count)
        let width = max(2, min(24, band - 2))
        StatsInk.series.setFill()
        for (day, value) in tile.chart.values.enumerated() {
            guard let value, value > 0 else { continue }
            let top = y(value), left = x(day) - width / 2
            let r = min(2, width / 2, plot.maxY - top)
            let path = NSBezierPath()
            path.move(to: NSPoint(x: left, y: plot.maxY))
            path.line(to: NSPoint(x: left, y: top + r))
            path.appendArc(withCenter: NSPoint(x: left + r, y: top + r), radius: r, startAngle: 180, endAngle: 270)
            path.line(to: NSPoint(x: left + width - r, y: top))
            path.appendArc(withCenter: NSPoint(x: left + width - r, y: top + r), radius: r, startAngle: 270, endAngle: 360)
            path.line(to: NSPoint(x: left + width, y: plot.maxY))
            path.close()
            path.fill()
        }
    }

    private func bars() {
        let top = StatsFormat.nice((tile.chart.made + tile.chart.done).compactMap { $0 }.max() ?? 0)
        let width = max(1.5, min(8, plot.width / CGFloat(count) / 2 - 1.5))
        for (values, ink, side) in [(tile.chart.made, StatsInk.made, -1.0), (tile.chart.done, StatsInk.done, 1.0)] {
            ink.setFill()
            for (day, value) in values.enumerated() {
                guard let value, value > 0 else { continue }
                let height = CGFloat(value / top) * strip
                let left = side < 0 ? x(day) - width - 0.5 : x(day) + 0.5
                NSBezierPath(roundedRect: NSRect(x: left, y: plot.maxY - height, width: width, height: height), xRadius: 1, yRadius: 1).fill()
            }
        }
    }

    static func legend(_ keys: [(NSColor, String)]) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for (i, (ink, words)) in keys.enumerated() {
            out.append(NSAttributedString(string: (i == 0 ? "" : "     ") + "━ ", attributes: [.foregroundColor: ink, .font: NSFont.systemFont(ofSize: 11, weight: .bold)]))
            out.append(NSAttributedString(string: words, attributes: [.foregroundColor: NSColor.secondaryLabelColor, .font: NSFont.systemFont(ofSize: 11)]))
        }
        return out
    }

    private func line(_ values: [Double?], wash: Bool) {
        var runs: [[NSPoint]] = [], run: [NSPoint] = []
        for (day, value) in values.enumerated() {
            if let value { run.append(NSPoint(x: x(day), y: y(value))) } else if !run.isEmpty {
                runs.append(run)
                run = []
            }
        }
        if !run.isEmpty { runs.append(run) }
        for run in runs {
            if wash, let first = run.first, let last = run.last {
                let area = NSBezierPath()
                area.move(to: NSPoint(x: first.x, y: baseline))
                run.forEach { area.line(to: $0) }
                area.line(to: NSPoint(x: last.x, y: baseline))
                area.close()
                StatsInk.series.withAlphaComponent(0.12).setFill()
                area.fill()
            }
            let path = NSBezierPath()
            path.lineWidth = 2
            path.lineJoinStyle = .round
            path.lineCapStyle = .round
            path.move(to: run[0])
            run.dropFirst().forEach { path.line(to: $0) }
            StatsInk.series.setStroke()
            if run.count > 1 { path.stroke() }
        }
        if wash, let last = values.last ?? nil {
            let dot = NSBezierPath(ovalIn: NSRect(x: x(values.count - 1) - 4, y: y(last) - 4, width: 8, height: 8))
            NSColor.textBackgroundColor.setStroke()
            dot.lineWidth = 2
            StatsInk.series.setFill()
            dot.fill()
            dot.stroke()
        }
    }

    private func dots() {
        StatsInk.context.setFill()
        StatsInk.context.setStroke()
        for point in tile.chart.points {
            let cx = x(point.day)
            if point.value > scale {
                let caret = NSBezierPath()
                caret.lineWidth = 1.5
                caret.move(to: NSPoint(x: cx - 3, y: plot.minY + 4))
                caret.line(to: NSPoint(x: cx, y: plot.minY))
                caret.line(to: NSPoint(x: cx + 3, y: plot.minY + 4))
                caret.stroke()
            } else {
                NSBezierPath(ovalIn: NSRect(x: cx - 3, y: y(point.value) - 3, width: 6, height: 6)).fill()
            }
        }
        line(tile.chart.line, wash: false)
    }

    override func mouseMoved(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let day = min(count - 1, max(0, Int((point.x - plot.minX) / (plot.width / CGFloat(count)))))
        if day != hovered {
            hovered = day
            needsDisplay = true
        }
        onHover?(readout(day), point)
    }

    override func mouseExited(with event: NSEvent) {
        hovered = nil
        needsDisplay = true
        onHover?(nil, .zero)
    }

    func readout(_ day: Int) -> NSAttributedString {
        let muted: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.secondaryLabelColor]
        let strong: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 13, weight: .semibold), .foregroundColor: NSColor.labelColor]
        let out = NSMutableAttributedString(string: day < days.count ? StatsFormat.day(days[day], isToday: day == days.count - 1, long: true) : "", attributes: muted)
        out.append(NSAttributedString(string: "\n"))
        let values = tile.chart.kind == "dots" ? tile.chart.line : tile.chart.values
        let value = day < values.count ? values[day] : nil
        out.append(NSAttributedString(string: StatsFormat.figure(value, unit: tile.unit), attributes: strong))
        out.append(NSAttributedString(string: " \(tile.chart.what)", attributes: muted))
        if tile.chart.kind == "dots" {
            let done = tile.chart.points.filter { $0.day == day }
            let lines = done.isEmpty ? ["No task done"] : done.map { "\($0.label) · \(StatsFormat.duration($0.value))" }
            out.append(NSAttributedString(string: "\n" + lines.joined(separator: "\n"), attributes: muted))
        }
        if tile.chart.kind == "flow" {
            let at = { (values: [Double?]) in StatsFormat.figure(day < values.count ? values[day] : nil, unit: nil) }
            out.append(NSAttributedString(string: "\n\(at(tile.chart.made)) made · \(at(tile.chart.done)) done", attributes: muted))
        }
        return out
    }
}
