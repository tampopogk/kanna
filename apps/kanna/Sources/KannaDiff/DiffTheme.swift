import AppKit
import KannaCode

enum DiffTheme {
    static let background = CodeTheme.background
    static let text = CodeTheme.text
    static let muted = CodeTheme.muted
    static let number = CodeTheme.number
    static let line = CodeTheme.line
    static let panel = color(dark: (0x1b1e24, 1), light: (0xf4f5f7, 1))
    static let fold = color(dark: (0x191c21, 1), light: (0xf6f7f9, 1))
    static let accent = color(dark: (0x7aa2ff, 1), light: (0x2f63d8, 1))
    static let tint = color(dark: (0x7aa2ff, 0.18), light: (0x2f63d8, 0.14))
    static let ok = color(dark: (0x5fcf93, 1), light: (0x1f9d5c, 1))
    static let bad = color(dark: (0xef6b6b, 1), light: (0xcf3f3f, 1))
    static let warn = color(dark: (0xf0b35a, 1), light: (0xb7791f, 1))
    static let addedLine = color(dark: (0x5fcf93, 0.11), light: (0x1f9d5c, 0.09))
    static let addedWord = color(dark: (0x5fcf93, 0.32), light: (0x1f9d5c, 0.26))
    static let addedGutter = color(dark: (0x5fcf93, 0.16), light: (0x1f9d5c, 0.14))
    static let removedLine = color(dark: (0xef6b6b, 0.11), light: (0xcf3f3f, 0.08))
    static let removedWord = color(dark: (0xef6b6b, 0.34), light: (0xcf3f3f, 0.24))
    static let removedGutter = color(dark: (0xef6b6b, 0.16), light: (0xcf3f3f, 0.13))

    static func status(_ letter: String) -> NSColor {
        switch letter {
        case "A": return ok
        case "D": return bad
        case "R": return accent
        default: return warn
        }
    }

    static func sans(size: CGFloat) -> NSFont { CodeTheme.sans(size: size - 1) }

    private static func color(dark: (UInt32, CGFloat), light: (UInt32, CGFloat)) -> NSColor {
        NSColor(name: nil) { appearance in
            let (value, alpha) = CodeTheme.isDark(appearance) ? dark : light
            return NSColor(srgbRed: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255, blue: CGFloat(value & 0xff) / 255, alpha: alpha)
        }
    }
}
