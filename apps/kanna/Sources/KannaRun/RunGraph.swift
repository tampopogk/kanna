import CoreGraphics

public struct PlacedNode: Equatable, Sendable {
    public var node: RunNode
    public var frame: CGRect
}

public struct PlacedLabel: Equatable, Sendable {
    public var text: String
    public var at: CGPoint
    public var trailing: Bool
}

public struct PlacedEdge: Equatable, Sendable {
    public var edge: RunEdge
    public var points: [CGPoint]
    public var label: PlacedLabel?
}

public struct RunGraph: Equatable, Sendable {
    public static let height: CGFloat = 34
    public static let gap: CGFloat = 22
    public static let rowGap: CGFloat = 58
    public static let lane: CGFloat = 14

    public var nodes: [PlacedNode] = []
    public var edges: [PlacedEdge] = []
    public var size: CGSize = .zero

    public func node(_ id: String) -> PlacedNode? { nodes.first { $0.node.id == id } }

    public static func lay(_ nodes: [RunNode], _ edges: [RunEdge], width: (RunNode) -> CGFloat) -> RunGraph {
        guard !nodes.isEmpty else { return RunGraph() }
        let columns = (nodes.map(\.col).max() ?? 0) + 1
        var widths = [CGFloat](repeating: 56, count: columns)
        for node in nodes { widths[node.col] = max(widths[node.col], width(node)) }
        let over = edges.filter { $0.route == "over" }.count
        let top: CGFloat = 24 + CGFloat(over) * lane + (over > 0 ? 16 : 0)
        var lefts: [CGFloat] = []
        var x: CGFloat = 8
        for column in 0..<columns {
            lefts.append(x)
            x += widths[column] + gap
        }
        let rowTop = { (row: Int) in top + CGFloat(row) * (height + rowGap) }
        let placed: [PlacedNode] = nodes.map { PlacedNode(node: $0, frame: CGRect(x: lefts[$0.col], y: rowTop($0.row), width: max(56, width($0)), height: height)) }
        let frames = Dictionary(placed.map { ($0.node.id, $0.frame) }, uniquingKeysWith: { a, _ in a })
        var out: [PlacedEdge] = []
        var downs: [String: Int] = [:]
        for edge in edges {
            guard let a = frames[edge.from], let b = frames[edge.to] else { continue }
            switch edge.route {
            case "next":
                out.append(PlacedEdge(edge: edge, points: [CGPoint(x: a.maxX, y: a.midY), CGPoint(x: b.minX - 3, y: b.midY)], label: label(edge, CGPoint(x: (a.maxX + b.minX) / 2, y: a.midY - 6), trailing: false)))
            case "down":
                let sx = a.midX, tx = b.midX + (CGFloat(edge.lane) - CGFloat(edge.lanes - 1) / 2) * 16
                if abs(sx - tx) < 1 {
                    out.append(PlacedEdge(edge: edge, points: [CGPoint(x: sx, y: a.maxY), CGPoint(x: tx, y: b.minY - 3)], label: label(edge, CGPoint(x: sx + 6, y: a.maxY + 15), trailing: false)))
                } else {
                    let rank = downs[edge.to, default: 0]
                    downs[edge.to] = rank + 1
                    let level = a.maxY + 32 + CGFloat(rank) * lane
                    let left = sx < tx
                    out.append(PlacedEdge(edge: edge, points: [CGPoint(x: sx, y: a.maxY), CGPoint(x: sx, y: level), CGPoint(x: tx, y: level), CGPoint(x: tx, y: b.minY - 3)],
                                          label: label(edge, CGPoint(x: left ? sx - 12 : sx + 6, y: a.maxY + 15), trailing: left)))
                }
            case "up":
                let leftward = b.midX <= a.midX
                let sx = leftward ? a.minX : a.maxX
                let tx = b.midX - 6
                out.append(PlacedEdge(edge: edge, points: [CGPoint(x: sx, y: a.midY), CGPoint(x: tx, y: a.midY), CGPoint(x: tx, y: b.maxY + 3)],
                                      label: label(edge, CGPoint(x: tx - 8, y: a.midY + 4), trailing: true)))
            default:
                let level = top - 26 - CGFloat(edge.lane) * lane
                out.append(PlacedEdge(edge: edge, points: [CGPoint(x: a.midX, y: a.minY), CGPoint(x: a.midX, y: level), CGPoint(x: b.midX, y: level), CGPoint(x: b.midX, y: b.minY - 3)],
                                      label: label(edge, CGPoint(x: (a.midX + b.midX) / 2, y: level - 4), trailing: false)))
            }
        }
        let right = placed.map(\.frame.maxX).max() ?? 0
        let bottom = placed.map(\.frame.maxY).max() ?? 0
        let lowest = out.flatMap(\.points).map(\.y).max() ?? 0
        return RunGraph(nodes: placed, edges: out, size: CGSize(width: right + 8, height: max(bottom, lowest) + 10))
    }

    private static func label(_ edge: RunEdge, _ at: CGPoint, trailing: Bool) -> PlacedLabel? {
        edge.label.isEmpty ? nil : PlacedLabel(text: edge.label, at: at, trailing: trailing)
    }
}
