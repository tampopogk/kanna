import AppKit
import KannaCode

enum LogTheme {
    static let background = CodeTheme.background
    static let text = CodeTheme.text
    static let muted = CodeTheme.muted
    static let line = CodeTheme.line
    static let panel = color(dark: (0x1b1e24, 1), light: (0xf4f5f7, 1))
    static let accent = color(dark: (0x7aa2ff, 1), light: (0x2f63d8, 1))
    static let tint = color(dark: (0x7aa2ff, 0.16), light: (0x2f63d8, 0.11))
    static let hover = color(dark: (0xffffff, 0.03), light: (0x000000, 0.025))
    static let hit = color(dark: (0xf0b35a, 0.22), light: (0xd69628, 0.22))
    static let fresh = color(dark: (0x5fcf93, 0.28), light: (0x1f9d5c, 0.2))
    static let ok = color(dark: (0x5fcf93, 1), light: (0x1f9d5c, 1))
    static let bad = color(dark: (0xef6b6b, 1), light: (0xcf3f3f, 1))
    static let warn = color(dark: (0xf0b35a, 1), light: (0xb7791f, 1))
    static let lanes: [NSColor] = [
        color(dark: (0x7aa2ff, 1), light: (0x2f63d8, 1)), color(dark: (0x5fcf93, 1), light: (0x1f9d5c, 1)),
        color(dark: (0xf0b35a, 1), light: (0xc47f12, 1)), color(dark: (0xc792ea, 1), light: (0x8839c4, 1)),
        color(dark: (0x56b6c2, 1), light: (0x0e7490, 1)), color(dark: (0xef8a6b, 1), light: (0xd0552f, 1)),
        color(dark: (0xe5c07b, 1), light: (0x9a7a10, 1)), color(dark: (0xff8fb8, 1), light: (0xc8327a, 1)),
    ]

    static func lane(_ index: Int) -> NSColor { lanes[((index % lanes.count) + lanes.count) % lanes.count] }

    static func sans(size: CGFloat, weight: NSFont.Weight = .regular) -> NSFont { NSFont.systemFont(ofSize: size, weight: weight) }
    static func mono(size: CGFloat, weight: NSFont.Weight = .regular) -> NSFont { NSFont.monospacedSystemFont(ofSize: size, weight: weight) }

    static func status(_ letter: String) -> NSColor {
        switch letter {
        case "A": return ok
        case "D": return bad
        case "R", "C": return accent
        default: return warn
        }
    }

    static func color(dark: (UInt32, CGFloat), light: (UInt32, CGFloat)) -> NSColor {
        NSColor(name: nil) { appearance in
            let (value, alpha) = CodeTheme.isDark(appearance) ? dark : light
            return NSColor(srgbRed: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255, blue: CGFloat(value & 0xff) / 255, alpha: alpha)
        }
    }
}
