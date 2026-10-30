import AppKit

public enum CodeTheme {
    public static let background = color(dark: 0x14161a, light: 0xffffff)
    public static let text = color(dark: 0xe6e8eb, light: 0x1d2026)
    public static let number = color(dark: 0x4b5260, light: 0xb4bac4)
    public static let currentNumber = color(dark: 0xc9ccd3, light: 0x1d2026)
    public static let currentLine = color(dark: 0x1b1e24, light: 0xf4f5f7)
    public static let flash = color(dark: 0x2a3a5c, light: 0xd6e2fa)
    public static let changed = color(dark: 0x5fcf93, light: 0x1f9d5c)
    public static let hit = color(dark: 0xe2c541, light: 0xc9a400)
    public static let guide = color(dark: 0x2a2f37, light: 0xe4e7eb)
    public static let line = color(dark: 0x2c313a, light: 0xd8dce2)
    public static let tint = color(dark: 0x1e2433, light: 0xeef3fc)
    public static let tintEdge = color(dark: 0x7aa2ff, light: 0x2f63d8)
    public static let card = color(dark: 0x1f232a, light: 0xffffff)
    public static let muted = color(dark: 0x8b93a0, light: 0x6b7280)
    public static let dim = NSColor(name: nil) { appearance in rgb(isDark(appearance) ? 0x14161a : 0xffffff).withAlphaComponent(0.6) }

    public static let keyword = color(dark: 0xc792ea, light: 0x8839c4)
    public static let function = color(dark: 0x7aa2ff, light: 0x2f63d8)
    public static let type = color(dark: 0xe5c07b, light: 0xa15c00)
    public static let string = color(dark: 0xa5d6a7, light: 0x2d7d3a)
    public static let constant = color(dark: 0xf78c6c, light: 0xc2410c)
    public static let comment = color(dark: 0x6b7280, light: 0x8a919c)
    public static let macro = color(dark: 0x56b6c2, light: 0x0e7490)
    public static let attribute = color(dark: 0x8b93a0, light: 0x6b7280)
    public static let property = color(dark: 0xc9d1e0, light: 0x374151)
    public static let operatorColor = color(dark: 0x89ddff, light: 0x0369a1)
    public static let punctuation = color(dark: 0x8b93a0, light: 0x6b7280)

    public static func color(capture: String) -> NSColor? {
        switch capture {
        case "function.macro", "constant.macro", "keyword.directive": return macro
        case "constructor": return type
        case "string.escape", "escape", "character", "boolean", "number", "number.float": return constant
        case "tag.attribute", "label": return attribute
        case "tag.delimiter": return punctuation
        case "variable.builtin", "tag": return keyword
        case "variable.member": return property
        default: break
        }
        switch capture.split(separator: ".").first ?? "" {
        case "keyword": return keyword
        case "function": return function
        case "type": return type
        case "string": return string
        case "constant": return constant
        case "comment": return comment
        case "attribute": return attribute
        case "property": return property
        case "operator": return operatorColor
        case "punctuation": return punctuation
        default: return nil
        }
    }

    public static let fontSize: CGFloat = 12.5
    public static let gutterPadding: CGFloat = 14

    public static func font(size: CGFloat) -> NSFont { NSFont.monospacedSystemFont(ofSize: size, weight: .regular) }
    public static func italic(size: CGFloat) -> NSFont { NSFontManager.shared.convert(font(size: size), toHaveTrait: .italicFontMask) }

    public static func isDark(_ appearance: NSAppearance) -> Bool { appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }

    private static func color(dark: UInt32, light: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in rgb(isDark(appearance) ? dark : light) }
    }

    private static func rgb(_ value: UInt32) -> NSColor {
        NSColor(srgbRed: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255,
                blue: CGFloat(value & 0xff) / 255, alpha: 1)
    }
}
