import AppKit

public struct SerialRow: Equatable, Sendable {
    public let n: Int
    public let t: Double
    public let dir: String
    public let text: String
    public let level: String?

    public init(n: Int, t: Double, dir: String, text: String, level: String? = nil) {
        self.n = n
        self.t = t
        self.dir = dir
        self.text = text
        self.level = level
    }
}

public struct SerialState: Equatable, Sendable {
    public var state = "connected"
    public var cmd: String?
    public var why: String?
    public var path = ""
    public var name = ""
    public var title = ""
    public var product: String?
    public var device: String?
    public var baud = 115_200
    public var lineEnding = "lf"
    public var rx = 0
    public var format: String?
    public var log = ""

    public init() {}
}

enum SerialTheme {
    static func isDark(_ appearance: NSAppearance) -> Bool { appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }

    static func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> NSColor {
        NSColor(srgbRed: CGFloat(hex >> 16 & 0xff) / 255, green: CGFloat(hex >> 8 & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
    }

    static func color(dark: UInt32, light: UInt32, alpha: CGFloat = 1) -> NSColor {
        NSColor(name: nil) { isDark($0) ? rgb(dark, alpha) : rgb(light, alpha) }
    }

    static let background = color(dark: 0x16181c, light: 0xfbfbfc)
    static let bar = color(dark: 0x1b1e24, light: 0xf4f5f7)
    static let line = color(dark: 0x2c313a, light: 0xd8dce2)
    static let text = color(dark: 0xe6e8eb, light: 0x1d2026)
    static let muted = color(dark: 0x8b93a0, light: 0x6b7280)
    static let faint = color(dark: 0x4b5260, light: 0xb4bac4)
    static let error = color(dark: 0xff6b6b, light: 0xd63c3c)
    static let errorTint = color(dark: 0xff6b6b, light: 0xd63c3c, alpha: 0.08)
    static let warning = color(dark: 0xe5c07b, light: 0xa15c00)
    static let info = color(dark: 0x8fd19e, light: 0x2d7d3a)
    static let debug = color(dark: 0x7f8896, light: 0x8a919c)
    static let sent = color(dark: 0x7aa2ff, light: 0x2f63d8)
    static let ok = color(dark: 0x4cb782, light: 0x2d9d5f)
    static let ansi: [Int: NSColor] = [
        30: color(dark: 0x8b93a0, light: 0x6b7280), 31: color(dark: 0xff6b6b, light: 0xc62828), 32: color(dark: 0x8fd19e, light: 0x2d7d3a), 33: color(dark: 0xe5c07b, light: 0xa15c00),
        34: color(dark: 0x7aa2ff, light: 0x2f63d8), 35: color(dark: 0xc792ea, light: 0x8839c4), 36: color(dark: 0x56b6c2, light: 0x0e7490), 37: color(dark: 0xe6e8eb, light: 0x1d2026),
    ]
}

public enum SerialRender {
    public static let fontSize: CGFloat = 12

    static func font(_ size: CGFloat, bold: Bool = false) -> NSFont {
        NSFont.monospacedSystemFont(ofSize: size, weight: bold ? .bold : .regular)
    }

    public static func clock(_ ms: Double) -> String {
        let date = Date(timeIntervalSince1970: ms / 1000)
        let parts = Calendar.current.dateComponents([.hour, .minute, .second], from: date)
        let millis = Int(ms.truncatingRemainder(dividingBy: 1000))
        return String(format: "%02d:%02d:%02d.%03d", parts.hour ?? 0, parts.minute ?? 0, parts.second ?? 0, millis)
    }

    public static func levelColor(_ level: String?) -> NSColor? {
        switch level {
        case "E": SerialTheme.error
        case "W": SerialTheme.warning
        case "I": SerialTheme.info
        case "D", "V": SerialTheme.debug
        default: nil
        }
    }

    public static func strip(_ text: String) -> String {
        text.replacingOccurrences(of: "\u{1b}(\\[[0-?]*[ -/]*[@-~]|[@-Z\\\\-_])", with: "", options: .regularExpression)
    }

    public static func row(_ row: SerialRow, time: Bool, size: CGFloat = fontSize) -> NSAttributedString {
        let out = NSMutableAttributedString()
        let base: [NSAttributedString.Key: Any] = [.font: font(size), .foregroundColor: SerialTheme.text]
        if time { out.append(NSAttributedString(string: clock(row.t) + "  ", attributes: base.merging([.foregroundColor: SerialTheme.faint]) { $1 })) }
        switch row.dir {
        case "#":
            let words = row.text.split(separator: " ", maxSplits: 1).map(String.init)
            let bad = row.text.hasPrefix("unplugged")
            out.append(NSAttributedString(string: "── ", attributes: base.merging([.foregroundColor: SerialTheme.faint]) { $1 }))
            out.append(NSAttributedString(string: words.first ?? "", attributes: base.merging([.font: font(size, bold: true), .foregroundColor: bad ? SerialTheme.error : SerialTheme.text]) { $1 }))
            if words.count > 1 { out.append(NSAttributedString(string: " " + words[1], attributes: base.merging([.foregroundColor: SerialTheme.muted]) { $1 })) }
            out.append(NSAttributedString(string: " ──", attributes: base.merging([.foregroundColor: SerialTheme.faint]) { $1 }))
        case ">":
            out.append(NSAttributedString(string: "→ ", attributes: base.merging([.foregroundColor: SerialTheme.muted]) { $1 }))
            out.append(escapes(row.text, base.merging([.foregroundColor: SerialTheme.sent]) { $1 }, size: size))
        default:
            if let color = levelColor(row.level) {
                let mark = row.level == "E" ? SerialTheme.error : row.level == "W" ? SerialTheme.warning : NSColor.clear
                out.append(NSAttributedString(string: "▎", attributes: base.merging([.foregroundColor: mark]) { $1 }))
                var attributes = base.merging([.foregroundColor: color]) { $1 }
                if row.level == "E" { attributes[.backgroundColor] = SerialTheme.errorTint }
                out.append(escapes(strip(row.text), attributes, size: size))
            } else {
                out.append(NSAttributedString(string: " ", attributes: base))
                out.append(ansi(row.text, base, size: size))
            }
        }
        return out
    }

    public static func ansi(_ text: String, _ base: [NSAttributedString.Key: Any], size: CGFloat = fontSize) -> NSAttributedString {
        let out = NSMutableAttributedString()
        var attributes = base
        var rest = Substring(text)
        while let escape = rest.firstIndex(of: "\u{1b}") {
            out.append(escapes(String(rest[..<escape]), attributes, size: size))
            let after = rest[rest.index(after: escape)...]
            guard after.first == "[", let end = after.dropFirst().firstIndex(where: { ("@"..."~").contains($0) }) else {
                rest = after.dropFirst(after.isEmpty ? 0 : 1)
                continue
            }
            if after[end] == "m" {
                let codes = after[after.index(after: after.startIndex)..<end].split(separator: ";", omittingEmptySubsequences: false).map { Int($0) ?? 0 }
                for code in codes.isEmpty ? [0] : codes {
                    switch code {
                    case 0: attributes = base
                    case 1: attributes[.font] = font(size, bold: true)
                    case 22: attributes[.font] = font(size)
                    case 39: attributes[.foregroundColor] = base[.foregroundColor]
                    case 30...37: attributes[.foregroundColor] = SerialTheme.ansi[code]
                    case 90...97: attributes[.foregroundColor] = SerialTheme.ansi[code - 60]
                    default: break
                    }
                }
            }
            rest = rest[rest.index(after: end)...]
        }
        out.append(escapes(String(rest), attributes, size: size))
        return out
    }

    static func escapes(_ text: String, _ attributes: [NSAttributedString.Key: Any], size: CGFloat) -> NSAttributedString {
        let out = NSMutableAttributedString()
        var rest = Substring(text)
        while let slash = rest.firstIndex(of: "\\") {
            let after = rest[rest.index(after: slash)...]
            if after.first == "\\" {
                out.append(NSAttributedString(string: String(rest[..<slash]) + "\\", attributes: attributes))
                rest = after.dropFirst()
                continue
            }
            if after.first == "x", after.count >= 3, UInt8(after.dropFirst().prefix(2), radix: 16) != nil {
                out.append(NSAttributedString(string: String(rest[..<slash]), attributes: attributes))
                out.append(NSAttributedString(string: "\\x" + after.dropFirst().prefix(2), attributes: attributes.merging([.foregroundColor: SerialTheme.faint]) { $1 }))
                rest = after.dropFirst(3)
                continue
            }
            out.append(NSAttributedString(string: String(rest[...slash]), attributes: attributes))
            rest = after
        }
        out.append(NSAttributedString(string: String(rest), attributes: attributes))
        return out
    }

    public static func bytes(_ text: String) -> [UInt8] {
        let utf8 = Array(text.utf8)
        var out: [UInt8] = []
        out.reserveCapacity(utf8.count)
        var i = 0
        while i < utf8.count {
            if utf8[i] == UInt8(ascii: "\\"), i + 1 < utf8.count {
                if utf8[i + 1] == UInt8(ascii: "\\") {
                    out.append(UInt8(ascii: "\\"))
                    i += 2
                    continue
                }
                if utf8[i + 1] == UInt8(ascii: "x"), i + 3 < utf8.count, let byte = UInt8(String(decoding: utf8[i + 2...i + 3], as: UTF8.self), radix: 16) {
                    out.append(byte)
                    i += 4
                    continue
                }
            }
            out.append(utf8[i])
            i += 1
        }
        return out
    }

    public static func hexLines(_ rows: [SerialRow], limit: Int = 64 * 1024) -> [String] {
        var data: [UInt8] = []
        for row in rows where row.dir == "<" {
            data += bytes(row.text)
            data.append(0x0a)
        }
        let start = max(0, (data.count - limit + 15) / 16 * 16)
        var lines: [String] = []
        var at = start
        while at < data.count {
            let chunk = data[at..<min(at + 16, data.count)]
            let hex = chunk.map { String(format: "%02x", $0) }.joined(separator: " ").padding(toLength: 47, withPad: " ", startingAt: 0)
            let ascii = String(chunk.map { (32..<127).contains($0) ? Character(UnicodeScalar($0)) : "·" })
            lines.append(String(format: "%08x", at) + "  " + hex + "  " + ascii)
            at += 16
        }
        return lines
    }

    public static func hex(_ rows: [SerialRow], size: CGFloat = fontSize) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for line in hexLines(rows) {
            let offset = String(line.prefix(8)), rest = line.dropFirst(10)
            out.append(NSAttributedString(string: offset + "  ", attributes: [.font: font(size), .foregroundColor: SerialTheme.faint]))
            out.append(NSAttributedString(string: String(rest.prefix(47)) + "  ", attributes: [.font: font(size), .foregroundColor: SerialTheme.text]))
            out.append(NSAttributedString(string: String(rest.dropFirst(49)) + "\n", attributes: [.font: font(size), .foregroundColor: SerialTheme.muted]))
        }
        return out
    }
}
