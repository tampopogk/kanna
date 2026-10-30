import AppKit
import GhosttyKit

@MainActor
enum Snapshot {
    private typealias CreateImage = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?

    private static let createImage: CreateImage? = {
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return nil }
        return unsafeBitCast(symbol, to: CreateImage.self)
    }()

    static func capture(_ window: NSWindow, terminals: [TerminalSurfaceView]) async throws -> CGImage {
        window.displayIfNeeded()
        let hidden = terminals.filter(\.hiddenFromGhostty).map { ($0, $0.presented) }
        for (terminal, _) in hidden { terminal.showToGhostty() }
        defer { for (terminal, _) in hidden { terminal.updateVisibility() } }
        let deadline = ContinuousClock.now + .seconds(1)
        while hidden.contains(where: { $0.0.presented == $0.1 }), ContinuousClock.now < deadline {
            try? await Task.sleep(for: .milliseconds(5))
        }
        CATransaction.flush()
        guard let image = createImage?(.null, CGWindowListOption.optionIncludingWindow.rawValue, UInt32(window.windowNumber), CGWindowImageOption([.boundsIgnoreFraming, .bestResolution]).rawValue)?.takeRetainedValue() else {
            throw KannaError("unavailable", "window capture unavailable")
        }
        return image
    }

    static func crop(_ image: CGImage, to view: NSView, in window: NSWindow) -> CGImage? {
        let scale = CGFloat(image.width) / window.frame.width
        let inWindow = view.convert(view.bounds, to: nil)
        let rect = CGRect(x: inWindow.minX * scale, y: (window.frame.height - inWindow.maxY) * scale, width: inWindow.width * scale, height: inWindow.height * scale)
        return image.cropping(to: rect.integral)
    }

    static func png(_ image: CGImage) throws -> Data {
        guard let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else {
            throw KannaError("bad_request", "could not encode the screenshot")
        }
        return png
    }

    static func run(_ window: NSWindow, id: String, panes: [(id: String, view: NSView)], tabBar: NSView? = nil, path: String?, marker: String?) async throws -> [String: JSON] {
        let image = try await capture(window, terminals: panes.compactMap { $0.view as? TerminalSurfaceView })
        guard let data = image.dataProvider?.data else { throw KannaError("unavailable", "window capture unavailable") }
        var out: [String: JSON] = ["window": .string(id), "width": .number(Double(image.width)), "height": .number(Double(image.height))]
        if let path {
            let url = try outputURL(path)
            try write(png(image), to: url, replacing: false)
            out["path"] = .string(url.path)
        }
        let scale = CGFloat(image.width) / window.frame.width
        let reader = Pixels(bytes: [UInt8](data as Data), image: image)
        func measure(_ view: NSView) -> (rect: CGRect, background: RGB, fields: [String: JSON]) {
            let inWindow = view.convert(view.bounds, to: nil)
            let rect = CGRect(x: inWindow.minX * scale, y: (window.frame.height - inWindow.maxY) * scale, width: inWindow.width * scale, height: inWindow.height * scale)
            let background = reader.dominant(in: rect)
            var total = 0, inked = 0
            reader.forEach(in: rect) {
                total += 1
                if $0.distance(to: background) > 60 { inked += 1 }
            }
            return (rect, background, ["background": .string(background.description), "width": .number(Double(rect.width.rounded())), "ink": .number(total == 0 ? 0 : (Double(inked) / Double(total) * 10000).rounded() / 10000)])
        }
        out["panes"] = .array(panes.map { id, view in
            let (rect, background, fields) = measure(view)
            var result = fields
            result["id"] = .string(id)
            if let marker, let terminal = view as? TerminalSurfaceView, let surface = terminal.surface {
                result["marker"] = markerCheck(marker, terminal, surface, rect: rect, scale: scale, reader: reader, background: background)
            }
            return .object(result)
        })
        if let tabBar { out["tabBar"] = .object(measure(tabBar).fields.merging(["shown": .bool(!tabBar.isHidden)]) { $1 }) }
        return out
    }

    private static var outputDirectory: URL {
        if let dir = ProcessInfo.processInfo.environment["KANNA_STATE_DIR"], !dir.isEmpty { return URL(fileURLWithPath: dir, isDirectory: true) }
        return URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
    }

    static func outputURL(_ name: String) throws -> URL {
        guard !name.isEmpty, name != ".", name != "..", !name.contains("/"), !name.contains("\0") else {
            throw KannaError("bad_request", "path must be a file name; snapshots are saved in \(outputDirectory.path)")
        }
        return outputDirectory.appendingPathComponent(name, isDirectory: false)
    }

    static func write(_ data: Data, to url: URL, replacing: Bool) throws {
        let fd = open(url.path, O_WRONLY | O_CREAT | O_NOFOLLOW | O_CLOEXEC | (replacing ? O_TRUNC : O_EXCL), 0o600)
        guard fd >= 0 else {
            if errno == EEXIST { throw KannaError("exists", "\(url.path) already exists (--force replaces it)") }
            throw KannaError("bad_request", "can't create \(url.path): \(String(cString: strerror(errno)))")
        }
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        do {
            try handle.write(contentsOf: data)
        } catch {
            unlink(url.path)
            throw KannaError("bad_request", "can't write \(url.path): \(error.localizedDescription)")
        }
    }
}

@MainActor
private func markerCheck(_ marker: String, _ terminal: TerminalSurfaceView, _ surface: ghostty_surface_t, rect: CGRect, scale: CGFloat,
                         reader: Pixels, background: RGB) -> JSON {
    let size = ghostty_surface_size(surface)
    let text = terminal.viewportText()
    let marker = marker != "*" ? marker : text.split(separator: "\n").lazy.compactMap { $0.split(separator: " ").first.map(String.init) }.first ?? ""
    guard !marker.isEmpty, let (row, column) = locate(marker, in: text, columns: Int(size.columns)) else { return ["found": false] }
    let backing = terminal.window?.backingScaleFactor ?? 2
    let cell = CGSize(width: CGFloat(size.cell_width_px) * scale / backing, height: CGFloat(size.cell_height_px) * scale / backing)
    let ghosttyDefaultWindowPaddingPoints: CGFloat = 2
    let pad = ghosttyDefaultWindowPaddingPoints * scale
    var glyphs = 0, inked = 0
    for (offset, character) in marker.enumerated() where !character.isWhitespace {
        glyphs += 1
        let cellRect = CGRect(x: rect.minX + pad + CGFloat(column + offset) * cell.width, y: rect.minY + pad + CGFloat(row) * cell.height,
                              width: cell.width, height: cell.height).insetBy(dx: 1, dy: 1)
        var count = 0
        reader.forEach(in: cellRect) { if $0.distance(to: background) > 60 { count += 1 } }
        if count >= 4 { inked += 1 }
    }
    return ["found": true, "text": .string(marker), "row": .number(Double(row)), "column": .number(Double(column)), "glyphs": .number(Double(glyphs)), "inked": .number(Double(inked))]
}

private func locate(_ marker: String, in text: String, columns: Int) -> (Int, Int)? {
    let columns = max(1, columns)
    var row = 0
    for line in text.components(separatedBy: "\n") {
        if let range = line.range(of: marker) {
            let offset = line.distance(from: line.startIndex, to: range.lowerBound)
            if offset % columns + marker.count <= columns { return (row + offset / columns, offset % columns) }
        }
        row += max(1, (line.count + columns - 1) / columns)
    }
    return nil
}

private struct RGB: Hashable, CustomStringConvertible {
    let r: Int, g: Int, b: Int
    func distance(to other: RGB) -> Int { abs(r - other.r) + abs(g - other.g) + abs(b - other.b) }
    var description: String { String(format: "#%02x%02x%02x", r, g, b) }
}

private struct Pixels {
    let bytes: [UInt8]
    let image: CGImage
    let bgra: Bool

    init(bytes: [UInt8], image: CGImage) {
        self.bytes = bytes
        self.image = image
        bgra = image.bitmapInfo.intersection(.byteOrderMask) == .byteOrder32Little
    }

    func pixel(_ x: Int, _ y: Int) -> RGB {
        let p = y * image.bytesPerRow + x * (image.bitsPerPixel / 8)
        guard p >= 0, p + 2 < bytes.count else { return RGB(r: 0, g: 0, b: 0) }
        return bgra ? RGB(r: Int(bytes[p + 2]), g: Int(bytes[p + 1]), b: Int(bytes[p])) : RGB(r: Int(bytes[p]), g: Int(bytes[p + 1]), b: Int(bytes[p + 2]))
    }

    func forEach(in rect: CGRect, _ body: (RGB) -> Void) {
        let r = rect.integral.intersection(CGRect(x: 0, y: 0, width: image.width, height: image.height))
        guard !r.isNull, r.width > 0, r.height > 0 else { return }
        for y in Int(r.minY)..<Int(r.maxY) { for x in Int(r.minX)..<Int(r.maxX) { body(pixel(x, y)) } }
    }

    func dominant(in rect: CGRect) -> RGB {
        var counts: [RGB: Int] = [:]
        let r = rect.integral.intersection(CGRect(x: 0, y: 0, width: image.width, height: image.height))
        guard !r.isNull else { return RGB(r: 0, g: 0, b: 0) }
        let step = max(1, Int(min(r.width, r.height) / 60))
        for y in stride(from: Int(r.minY), to: Int(r.maxY), by: step) {
            for x in stride(from: Int(r.minX), to: Int(r.maxX), by: step) { counts[pixel(x, y), default: 0] += 1 }
        }
        return counts.max { $0.value < $1.value }?.key ?? RGB(r: 0, g: 0, b: 0)
    }
}
