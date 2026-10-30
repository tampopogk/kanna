import AppKit
import KannaMerman

public enum Diagram {
    public static func layout(source: String) throws -> DiagramLayout {
        let log = MeasureLog()
        let json = try layoutJSON(source: source, log: log)
        guard let meta = json["meta"] as? [String: Any], let semantic = json["semantic"] as? [String: Any],
              let layout = json["layout"] as? [String: Any] else { throw DiagramError(message: "merman returned no layout") }
        let type = meta["diagram_type"] as? String ?? "unknown"
        let scene: DiagramScene?
        switch json["scene"] as? String {
        case "flowchart": scene = Flowchart.scene(semantic: semantic, layout: layout["FlowchartV2"] as? [String: Any] ?? [:])
        case "sequence": scene = Sequence.scene(semantic: semantic, layout: layout["SequenceDiagram"] as? [String: Any] ?? [:])
        case "state": scene = State.scene(semantic: semantic, layout: layout["StateDiagramV2"] as? [String: Any] ?? [:])
        case "class": scene = Class.scene(semantic: semantic, layout: layout["ClassDiagramV2"] as? [String: Any] ?? [:])
        case "gitGraph": scene = GitGraph.scene(semantic: semantic, layout: layout["GitGraphDiagram"] as? [String: Any] ?? [:])
        default: scene = nil
        }
        return DiagramLayout(type: type, scene: scene.map { scene in
            var scene = log.apply(to: scene)
            scene.fit()
            return scene
        })
    }

    static func layoutJSON(source: String, log: MeasureLog = MeasureLog()) throws -> [String: Any] {
        let bytes = Array(source.utf8)
        let context = Unmanaged.passUnretained(log).toOpaque()
        guard let raw = withExtendedLifetime(log, {
            bytes.withUnsafeBufferPointer { buffer in
                buffer.withMemoryRebound(to: CChar.self) { kanna_merman_layout($0.baseAddress, $0.count, measure, context) }
            }
        }) else { throw DiagramError(message: "merman returned nothing") }
        defer { kanna_merman_free(raw) }
        guard let object = try? JSONSerialization.jsonObject(with: Data(bytes: raw, count: strlen(raw))) as? [String: Any] else {
            throw DiagramError(message: "merman returned invalid JSON")
        }
        if let error = object["error"] as? String { throw DiagramError(message: error) }
        return object
    }
}

final class MeasureLog {
    private var styles: [String: (size: CGFloat, htmlLike: Bool)] = [:]

    func record(_ text: String, size: CGFloat, htmlLike: Bool) {
        let key = text.trimmingCharacters(in: .whitespaces)
        if styles[key] == nil { styles[key] = (size, htmlLike) }
    }

    func apply(to scene: DiagramScene) -> DiagramScene {
        var scene = scene
        scene.items = scene.items.map { item in
            guard case .text(var label) = item, !label.fixedSize else { return item }
            let lines = LabelText.plainLines(label.text)
            let key = [label.text.trimmingCharacters(in: .whitespaces), lines.joined(separator: " ")] + lines
            if let style = key.lazy.compactMap({ self.styles[$0] }).first {
                label.size = style.size
                label.htmlLike = style.htmlLike
            }
            return .text(label)
        }
        return scene
    }
}

public struct DiagramLayout {
    public let type: String
    public let scene: DiagramScene?
}

public struct DiagramError: Error, CustomStringConvertible {
    public let message: String
    public var description: String { message }
    public init(message: String) { self.message = message }
}

private let measure: kanna_merman_measure_fn = { request, out, context in
    guard let request = request?.pointee, let out else { return 0 }
    func string(_ pointer: UnsafePointer<CChar>?, _ length: Int) -> String {
        guard let pointer, length > 0 else { return "" }
        return pointer.withMemoryRebound(to: UInt8.self, capacity: length) { String(decoding: UnsafeBufferPointer(start: $0, count: length), as: UTF8.self) }
    }
    let text = string(request.text, request.text_len)
    let family = string(request.font_family, request.font_family_len).lowercased()
    let size = CGFloat(request.font_size > 0 ? request.font_size : 16)
    let htmlLike = request.html_like != 0
    let monospaced = family.contains("mono") || family.contains("courier")
    let maxWidth = request.max_width > 0 ? CGFloat(request.max_width) : nil
    if let context, Int(request.kind) == Int(KANNA_MEASURE_METRICS) {
        Unmanaged<MeasureLog>.fromOpaque(context).takeUnretainedValue().record(text, size: size, htmlLike: htmlLike)
    }
    let measured = LabelText.measure(text, size: size, bold: request.bold != 0, italic: request.italic != 0, monospaced: monospaced,
                                     maxWidth: maxWidth, htmlLike: htmlLike)
    switch Int(request.kind) {
    case Int(KANNA_MEASURE_METRICS), Int(KANNA_MEASURE_WRAPPED_RAW):
        out.pointee.width = Double(measured.width)
        out.pointee.height = Double(measured.height)
        out.pointee.line_count = UInt32(measured.lines)
        if Int(request.kind) == Int(KANNA_MEASURE_WRAPPED_RAW) {
            out.pointee.raw_width = Double(LabelText.measure(text, size: size, bold: request.bold != 0, italic: request.italic != 0,
                                                             monospaced: monospaced, maxWidth: nil, htmlLike: htmlLike).width)
        }
    case Int(KANNA_MEASURE_WIDTH):
        out.pointee.length = Double(LabelText.measure(text, size: size, bold: request.bold != 0, italic: request.italic != 0,
                                                      monospaced: monospaced, maxWidth: nil, htmlLike: htmlLike).width)
    case Int(KANNA_MEASURE_HEIGHT):
        out.pointee.length = Double(measured.height)
    case Int(KANNA_MEASURE_EXTENTS):
        let width = LabelText.measure(text, size: size, bold: request.bold != 0, italic: request.italic != 0,
                                      monospaced: monospaced, maxWidth: nil, htmlLike: htmlLike).width
        out.pointee.left = Double(width) / 2
        out.pointee.right = Double(width) / 2
    default:
        return 0
    }
    return 1
}
