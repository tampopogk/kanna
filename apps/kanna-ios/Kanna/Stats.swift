import SwiftUI
import UIKit

enum StatsFormat {
    static func duration(_ seconds: Double) -> String {
        let minutes = Int((seconds / 60).rounded())
        if minutes < 60 { return "\(minutes)m" }
        if minutes < 48 * 60 { return minutes % 60 == 0 ? "\(minutes / 60)h" : "\(minutes / 60)h \(minutes % 60)m" }
        return "\(Int((Double(minutes) / 1440).rounded()))d"
    }

    static func figure(_ value: Double?, unit: String?) -> String {
        guard let value else { return "—" }
        switch unit {
        case "%": return value == value.rounded() ? "\(Int(value))%" : String(format: "%.1f%%", value)
        case "s": return duration(value)
        default: return "\(Int(value.rounded()))"
        }
    }

    private static let parser: DateFormatter = {
        let format = DateFormatter()
        format.locale = Locale(identifier: "en_US_POSIX")
        format.dateFormat = "yyyy-MM-dd"
        return format
    }()

    private static let short: DateFormatter = {
        let format = DateFormatter()
        format.locale = Locale(identifier: "en_US")
        format.dateFormat = "MMM d"
        return format
    }()

    static func day(_ iso: String) -> String {
        parser.date(from: iso).map(short.string(from:)) ?? iso
    }
}

extension StatsChart {
    var series: [Double?] { (kind == "dots" ? line : values) ?? [] }
    var bars: Bool { kind == "columns" }
    var drawn: Bool { ["line", "columns", "flow", "dots"].contains(kind) && !series.isEmpty }
    var listed: Bool { ["ranked", "list"].contains(kind) }
}

extension StatsTile {
    var wide: Bool { (span ?? 2) >= 4 || chart?.listed == true }
}

struct Spark: View {
    var data: [Double?]
    var top: Double?
    var zero = false
    var marker: Int?

    var body: some View {
        Canvas { context, size in
            let known = data.compactMap { $0 }
            let low = zero ? 0 : (known.min() ?? 0)
            let high = top ?? known.max() ?? 1
            let range = high - low == 0 ? 1 : high - low
            let place = { (i: Int, y: Double) in
                CGPoint(x: CGFloat(i) / CGFloat(max(data.count - 1, 1)) * size.width, y: size.height - 3 - CGFloat((y - low) / range) * (size.height - 8))
            }
            let points = data.enumerated().compactMap { i, y in y.map { place(i, $0) } }
            var axis = Path()
            axis.move(to: CGPoint(x: 0, y: size.height - 0.5))
            axis.addLine(to: CGPoint(x: size.width, y: size.height - 0.5))
            if let first = points.first, let last = points.last {
                var line = Path()
                line.addLines(points)
                var area = line
                area.addLine(to: CGPoint(x: last.x, y: size.height))
                area.addLine(to: CGPoint(x: first.x, y: size.height))
                area.closeSubpath()
                context.fill(area, with: .color(K.wash))
                context.stroke(line, with: .color(K.series), style: StrokeStyle(lineWidth: 2, lineJoin: .round))
            }
            context.stroke(axis, with: .color(K.axis), lineWidth: 1)
            if let marker, marker < data.count {
                let x = CGFloat(marker) / CGFloat(max(data.count - 1, 1)) * size.width
                var rule = Path()
                rule.move(to: CGPoint(x: x, y: 0))
                rule.addLine(to: CGPoint(x: x, y: size.height))
                context.stroke(rule, with: .color(K.muted), lineWidth: 1)
                if let y = data[marker] {
                    let p = place(marker, y)
                    context.fill(Path(ellipseIn: CGRect(x: p.x - 4, y: p.y - 4, width: 8, height: 8)), with: .color(K.series))
                }
            }
        }
    }
}

struct Bars: View {
    var data: [Double?]
    var marker: Int?

    var body: some View {
        Canvas { context, size in
            let high = max(data.compactMap { $0 }.max() ?? 1, 1)
            let step = size.width / CGFloat(max(data.count, 1))
            for (i, y) in data.enumerated() {
                let h = CGFloat((y ?? 0) / high) * (size.height - 2)
                let rect = CGRect(x: CGFloat(i) * step, y: size.height - h, width: max(step - 1.2, 1), height: h)
                context.fill(Path(roundedRect: rect, cornerRadius: 0.8), with: .color(K.series.opacity(marker == nil || marker == i ? 1 : 0.45)))
            }
            var axis = Path()
            axis.move(to: CGPoint(x: 0, y: size.height - 0.5))
            axis.addLine(to: CGPoint(x: size.width, y: size.height - 0.5))
            context.stroke(axis, with: .color(K.axis), lineWidth: 1)
        }
    }
}

struct RankedRows: View {
    var chart: StatsChart

    var body: some View {
        let rows = chart.rows ?? []
        let high = chart.max ?? max(rows.compactMap(\.value).max() ?? 1, 1)
        VStack(alignment: .leading, spacing: 7) {
            if rows.isEmpty, let empty = chart.empty {
                Text(empty).font(.system(size: 13)).foregroundStyle(K.muted)
            }
            ForEach(Array(rows.prefix(5).enumerated()), id: \.offset) { _, row in
                VStack(alignment: .leading, spacing: 2) {
                    HStack(alignment: .firstTextBaseline) {
                        Text(row.label).font(.system(size: 13)).lineLimit(1).truncationMode(.middle)
                        Spacer(minLength: 8)
                        Text(row.figure ?? row.note ?? "").font(.system(size: 12)).foregroundStyle(K.muted).lineLimit(1)
                    }
                    if chart.kind == "ranked" {
                        GeometryReader { geo in
                            Capsule().fill(K.panel2).overlay(alignment: .leading) {
                                Capsule().fill(K.series).frame(width: geo.size.width * CGFloat((row.value ?? 0) / max(high, 1)))
                            }
                        }
                        .frame(height: 5)
                    }
                }
            }
        }
        .padding(.top, 6)
    }
}

struct Tile: View {
    var tile: StatsTile
    var days: [String]
    @State private var at: Int?

    private var chart: StatsChart? { tile.chart }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(tile.label).font(.system(size: 12)).foregroundStyle(K.muted).lineLimit(1)
            if tile.value != nil || chart?.listed != true {
                Text(StatsFormat.figure(tile.value, unit: tile.unit)).font(.system(size: 22, weight: .semibold))
            }
            if let note = tile.note, !note.isEmpty {
                Text(note).font(.system(size: 12)).foregroundStyle(K.muted).lineLimit(1)
            }
            if let chart, chart.drawn { drawing(chart) }
            if let chart, chart.listed { RankedRows(chart: chart) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: tile.wide ? .topTrailing : .top) {
            if let at, let chart { Readout(tile: tile, chart: chart, index: at, days: days).allowsHitTesting(false) }
        }
        .zIndex(at == nil ? 0 : 1)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("tile-" + tile.id)
    }

    @ViewBuilder private func drawing(_ chart: StatsChart) -> some View {
        GeometryReader { geo in
            Group {
                if chart.bars {
                    Bars(data: chart.series, marker: at)
                } else {
                    Spark(data: chart.series, top: chart.max, zero: chart.kind == "flow" || chart.max != nil, marker: at)
                }
            }
            .contentShape(Rectangle())
            .gesture(
                LongPressGesture(minimumDuration: 0.12)
                    .sequenced(before: DragGesture(minimumDistance: 0, coordinateSpace: .local))
                    .onChanged { value in
                        let count = chart.series.count
                        switch value {
                        case .second(true, let drag?):
                            let x = min(max(drag.location.x, 0), geo.size.width - 1)
                            let i = Int(x / geo.size.width * CGFloat(count))
                            if i != at { UISelectionFeedbackGenerator().selectionChanged() }
                            at = i
                        case .second(true, nil), .first(true):
                            if at == nil {
                                at = count - 1
                                UISelectionFeedbackGenerator().selectionChanged()
                            }
                        default: break
                        }
                    }
                    .onEnded { _ in at = nil }
            )
        }
        .frame(height: tile.wide ? 72 : 44)
        .padding(.top, 4)
    }
}

struct Readout: View {
    var tile: StatsTile
    var chart: StatsChart
    var index: Int
    var days: [String]

    private var value: Double? { index < chart.series.count ? chart.series[index] : nil }
    private var day: String { index < days.count ? StatsFormat.day(days[index]) : "" }

    private var extra: String? {
        guard chart.kind == "flow", let made = chart.made, let done = chart.done, index < made.count, index < done.count else { return nil }
        return "\(Int(made[index] ?? 0)) made · \(Int(done[index] ?? 0)) done"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            (Text(StatsFormat.figure(value, unit: tile.unit)).fontWeight(.semibold) + Text(" \(chart.what ?? "") · \(day)").foregroundColor(K.muted))
            if let extra { Text(extra).foregroundStyle(K.text) }
        }
        .font(.system(size: 12))
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(K.card, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(K.line))
        .shadow(color: .black.opacity(0.28), radius: 8, y: 6)
        .accessibilityIdentifier("readout")
    }
}

struct StatsView: View {
    @Environment(Store.self) private var store

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner(detail: "These are the numbers as they were at \(store.asOf).")
            ScrollView {
                VStack(spacing: 0) {
                    if store.statsPage == nil {
                        Text(store.offline ? "No numbers yet: Stats reads from \(store.repo.plannerMachine)." : "Asking \(store.repo.plannerMachine)…").font(.system(size: 14)).foregroundStyle(K.muted).padding(.top, 40)
                    }
                    ForEach(store.statsPage?.sections ?? []) { section in
                        head(section)
                        if let tiles = section.tiles, !tiles.isEmpty {
                            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 12) {
                                ForEach(rows(tiles), id: \.first!.id) { row in
                                    GridRow {
                                        ForEach(row) { tile in
                                            Tile(tile: tile, days: store.statsPage?.days ?? []).gridCellColumns(tile.wide ? 2 : 1)
                                        }
                                        if row.count == 1, !row[0].wide { Color.clear.gridCellUnsizedAxes([.horizontal, .vertical]) }
                                    }
                                }
                            }
                            .padding(.horizontal, 16)
                            .padding(.top, 10)
                        } else if let words = section.empty ?? section.placeholder {
                            Text(words).font(.system(size: 13)).foregroundStyle(K.muted).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 16).padding(.top, 10)
                        }
                    }
                }
                .padding(.bottom, 16)
            }
        }
        .background(K.bg)
        .offlineSub(store, "Last 30 days")
    }

    private func head(_ section: StatsSection) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(section.title).font(.system(size: 14, weight: .bold))
            Text(section.note ?? "").font(.system(size: 12)).foregroundStyle(K.muted).lineLimit(1)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(K.panel2, in: RoundedRectangle(cornerRadius: 6))
        .padding(.horizontal, 16)
        .padding(.top, 14)
    }

    private func rows(_ tiles: [StatsTile]) -> [[StatsTile]] {
        var out: [[StatsTile]] = []
        var row: [StatsTile] = []
        for tile in tiles {
            if tile.wide {
                if !row.isEmpty { out.append(row); row = [] }
                out.append([tile])
            } else {
                row.append(tile)
                if row.count == 2 { out.append(row); row = [] }
            }
        }
        if !row.isEmpty { out.append(row) }
        return out
    }
}
