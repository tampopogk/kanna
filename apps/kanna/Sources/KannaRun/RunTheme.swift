import AppKit
import KannaCode

enum RunTheme {
    static let background = CodeTheme.background
    static let text = CodeTheme.text
    static let muted = CodeTheme.muted
    static let line = CodeTheme.line
    static let panel = color(dark: (0x1b1e24, 1), light: (0xf4f5f7, 1))
    static let panel2 = color(dark: (0x22262d, 1), light: (0xeaecf0, 1))
    static let term = color(dark: (0x16181c, 1), light: (0xfbfbfc, 1))
    static let accent = color(dark: (0x7aa2ff, 1), light: (0x2f63d8, 1))
    static let tint = color(dark: (0x7aa2ff, 0.18), light: (0x2f63d8, 0.14))
    static let ok = color(dark: (0x5fcf93, 1), light: (0x1f9d5c, 1))
    static let bad = color(dark: (0xef6b6b, 1), light: (0xcf3f3f, 1))
    static let warn = color(dark: (0xf0b35a, 1), light: (0xb7791f, 1))
    static let warnFill = color(dark: (0xf0b35a, 0.10), light: (0xb7791f, 0.08))
    static let rowLine = color(dark: (0x2c313a, 0.6), light: (0xd8dce2, 0.6))

    static func sans(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont { .systemFont(ofSize: size, weight: weight) }
    static func mono(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont { .monospacedSystemFont(ofSize: size, weight: weight) }

    static func state(_ state: String?) -> NSColor? {
        switch state {
        case "pass"?: return ok
        case "fail"?: return bad
        case "now"?: return accent
        case "wait"?: return warn
        default: return nil
        }
    }

    static func verdict(_ said: String) -> NSColor {
        switch said {
        case "pass", "yes", "skipped": return ok
        case "fail", "no", "stopped", "timed out": return bad
        case "running": return accent
        case "waiting": return warn
        default: return RunTheme.text
        }
    }

    private static func color(dark: (UInt32, CGFloat), light: (UInt32, CGFloat)) -> NSColor {
        NSColor(name: nil) { appearance in
            let (value, alpha) = CodeTheme.isDark(appearance) ? dark : light
            return NSColor(srgbRed: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255, blue: CGFloat(value & 0xff) / 255, alpha: alpha)
        }
    }
}
