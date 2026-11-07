import SwiftUI
import UIKit

struct LCG {
    var x: Double
    mutating func next() -> Double {
        x = (x * 1103515245 + 12345).truncatingRemainder(dividingBy: 2147483648)
        return x / 2147483648
    }
}

func series(_ seed: Double, _ base: Double, _ swing: Double, _ up: Double = 0) -> [Double] {
    var r = LCG(x: seed)
    return (0..<30).map { i in max(0, base + (r.next() - 0.5) * swing + Double(i) * up) }
}

private func day(_ index: Int) -> String {
    var parts = DateComponents()
    parts.year = 2026
    parts.month = 10
    parts.day = 30
    let end = Calendar.current.date(from: parts) ?? Date()
    let date = Calendar.current.date(byAdding: .day, value: index - 29, to: end) ?? end
    return date.formatted(.dateTime.month(.abbreviated).day())
}

struct Metric: Identifiable {
    var id: String { name }
    let name: String
    let value: String
    var small = ""
    let data: [Double]
    var bars = false
    var wide = false
    var zero = false
    var format: (Double) -> String = { String(Int($0.rounded())) }
    var unit = ""
    var extra: ((Int) -> String)?
}

struct StatsSection: Identifiable {
    var id: String { title }
    let title: String
    let note: String
    let metrics: [Metric]
}

enum StatsData {
    static func sections(seed s: Double, kanna: Bool) -> [StatsSection] {
        let v = { (t: String, d: [Double]) in kanna ? t : String(Int((d.last ?? 0).rounded())) }
        let grade = series(7 + s, 66, 5, 0.34)
        let misses = series(9 + s, 5, 3, 0.05).map { $0.rounded() }
        let answers = series(21 + s, 3, 5, 0.03)
        let merges = series(11 + s, 7, 12).map { $0.rounded() }
        let commits = merges.enumerated().map { $0.element * 3 + Double(($0.offset * 7) % 9) }
        let open = series(13 + s, 20, 4, 0.14).map { $0.rounded() }
        let made = series(17 + s, 5, 7).map { $0.rounded() }
        let done = series(19 + s, 6, 8).map { $0.rounded() }
        let lead = series(15 + s, 4.4, 2.4, -0.03)
        return [
            StatsSection(title: "Quiz", note: "what you know of the code", metrics: [
                Metric(name: "Right, 30 days", value: kanna ? "72.4%" : "\(Int(grade.last ?? 0))%", data: grade, wide: true, format: { String(format: "%.1f%%", $0) }),
                Metric(name: "Misses waiting", value: v("6", misses), data: misses, zero: true, unit: "waiting"),
                Metric(name: "Answers", value: v("85", answers), data: answers, unit: "answers"),
            ]),
            StatsSection(title: "Main", note: "what landed", metrics: [
                Metric(name: "Commits", value: v("38", commits), data: commits, bars: true, unit: "commits"),
                Metric(name: "Merges", value: v("9", merges), data: merges, bars: true, unit: "merges"),
            ]),
            StatsSection(title: "Tasks", note: "from the Desk's and the Shelf's events", metrics: [
                Metric(name: "Open", value: v("23", open), data: open, wide: true, unit: "open", extra: { "\(Int(made[$0])) made · \(Int(done[$0])) done" }),
                Metric(name: "Made", value: v("7", made), data: made, bars: true, unit: "made"),
                Metric(name: "Done", value: v("9", done), data: done, bars: true, unit: "done"),
                Metric(name: "Made to done", value: "3h 33m", small: "7-day median", data: lead, wide: true, format: { String(format: "%.1fh", $0) }),
            ]),
            StatsSection(title: "Code quality", note: "how good the code is", metrics: [
                Metric(name: "Verbosity", value: "18.8%", data: series(23 + s, 21, 2, -0.1), format: { String(format: "%.1f%%", $0) }),
                Metric(name: "Erosion", value: "0.34", data: series(25 + s, 0.4, 0.04, -0.002), format: { String(format: "%.2f", $0) }),
            ]),
        ]
    }
}

struct Spark: View {
    var data: [Double]
    var zero = false
    var marker: Int?

    private func points(_ size: CGSize) -> [CGPoint] {
        let low = zero ? 0 : (data.min() ?? 0)
        let high = data.max() ?? 1
        let range = high - low == 0 ? 1 : high - low
        return data.enumerated().map { i, y in
            CGPoint(x: CGFloat(i) / CGFloat(data.count - 1) * size.width, y: size.height - 3 - CGFloat((y - low) / range) * (size.height - 8))
        }
    }

    var body: some View {
        Canvas { context, size in
            let pts = points(size)
            var line = Path()
            line.addLines(pts)
            var area = line
            area.addLine(to: CGPoint(x: size.width, y: size.height))
            area.addLine(to: CGPoint(x: 0, y: size.height))
            area.closeSubpath()
            context.fill(area, with: .color(K.wash))
            var axis = Path()
            axis.move(to: CGPoint(x: 0, y: size.height - 0.5))
            axis.addLine(to: CGPoint(x: size.width, y: size.height - 0.5))
            context.stroke(axis, with: .color(K.axis), lineWidth: 1)
            context.stroke(line, with: .color(K.series), style: StrokeStyle(lineWidth: 2, lineJoin: .round))
            if let marker {
                let p = pts[marker]
                var rule = Path()
                rule.move(to: CGPoint(x: p.x, y: 0))
                rule.addLine(to: CGPoint(x: p.x, y: size.height))
                context.stroke(rule, with: .color(K.muted), lineWidth: 1)
                context.fill(Path(ellipseIn: CGRect(x: p.x - 4, y: p.y - 4, width: 8, height: 8)), with: .color(K.series))
            }
        }
    }
}

struct Bars: View {
    var data: [Double]
    var marker: Int?

    var body: some View {
        Canvas { context, size in
            let high = max(data.max() ?? 1, 1)
            let step = size.width / CGFloat(data.count)
            for (i, y) in data.enumerated() {
                let h = CGFloat(y / high) * (size.height - 2)
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

struct Tile: View {
    var metric: Metric
    var preset: Int?
    @Environment(Store.self) private var store
    @State private var at: Int?

    private var shown: Int? { at ?? preset }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(metric.name).font(.system(size: 12)).foregroundStyle(K.muted)
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(metric.value).font(.system(size: 22, weight: .semibold))
                if !metric.small.isEmpty { Text(metric.small).font(.system(size: 12)).foregroundStyle(K.muted) }
            }
            GeometryReader { geo in
                Group {
                    if metric.bars { Bars(data: metric.data, marker: shown) } else { Spark(data: metric.data, zero: metric.zero, marker: shown) }
                }
                .contentShape(Rectangle())
                .gesture(
                    LongPressGesture(minimumDuration: 0.12)
                        .sequenced(before: DragGesture(minimumDistance: 0, coordinateSpace: .local))
                        .onChanged { value in
                            switch value {
                            case .second(true, let drag?):
                                let x = min(max(drag.location.x, 0), geo.size.width - 1)
                                let i = Int(x / geo.size.width * CGFloat(metric.data.count))
                                if i != at { UISelectionFeedbackGenerator().selectionChanged() }
                                at = i
                            case .second(true, nil), .first(true):
                                if at == nil {
                                    at = metric.data.count - 1
                                    UISelectionFeedbackGenerator().selectionChanged()
                                }
                            default: break
                            }
                        }
                        .onEnded { _ in at = nil }
                )
            }
            .frame(height: metric.wide ? 72 : 44)
        }
        .overlay(alignment: metric.wide ? .topTrailing : .top) {
            if let i = shown {
                Readout(metric: metric, index: i).allowsHitTesting(false)
            }
        }
        .zIndex(shown == nil ? 0 : 1)
        .accessibilityIdentifier("tile-" + metric.name)
    }
}

struct Readout: View {
    var metric: Metric
    var index: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            (Text(metric.format(metric.data[index])).fontWeight(.semibold) + Text(" \(metric.unit)\(metric.unit == "open" ? " at the end of" : " on") \(day(index))").foregroundColor(K.muted))
            if let extra = metric.extra { Text(extra(index)).foregroundStyle(K.text) }
        }
        .font(.system(size: 12))
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(K.card, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(K.line))
        .shadow(color: .black.opacity(0.28), radius: 8, y: 6)
    }
}

struct StatsView: View {
    @Environment(Store.self) private var store

    var body: some View {
        let sections = StatsData.sections(seed: store.repo.seed, kanna: store.repo.id == "kanna")
        VStack(spacing: 0) {
            OfflineBanner(detail: "These are the numbers as they were at \(store.asOf).")
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(sections) { section in
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Text(section.title).font(.system(size: 14, weight: .bold))
                                Text(section.note).font(.system(size: 12)).foregroundStyle(K.muted)
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(K.panel2, in: RoundedRectangle(cornerRadius: 6))
                            .padding(.horizontal, 16)
                            .padding(.top, 14)
                            .id(section.title)
                            Grid(horizontalSpacing: 16, verticalSpacing: 12) {
                                ForEach(rows(section.metrics), id: \.first!.id) { row in
                                    GridRow {
                                        ForEach(row) { metric in
                                            Tile(metric: metric, preset: metric.name == "Open" ? store.scrub : nil)
                                                .gridCellColumns(metric.wide ? 2 : 1)
                                        }
                                    }
                                }
                            }
                            .padding(.horizontal, 16)
                            .padding(.top, 10)
                        }
                    }
                    .padding(.bottom, 16)
                }
                .task {
                    try? await Task.sleep(for: .milliseconds(150))
                    if let target = store.scrollStats { proxy.scrollTo(target, anchor: .top) }
                }
            }
        }
        .background(K.bg)
        .offlineSub(store, "Last 30 days")
    }

    private func rows(_ metrics: [Metric]) -> [[Metric]] {
        var out: [[Metric]] = []
        var cur: [Metric] = []
        for m in metrics {
            if m.wide {
                if !cur.isEmpty { out.append(cur); cur = [] }
                out.append([m])
            } else {
                cur.append(m)
                if cur.count == 2 { out.append(cur); cur = [] }
            }
        }
        if !cur.isEmpty { out.append(cur) }
        return out
    }
}
