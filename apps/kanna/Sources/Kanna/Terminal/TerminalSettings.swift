import AppKit
import GhosttyKit

struct TerminalSettings: Equatable {
    var foreground: String
    var background: String
    var cursor: String?
    var palette: [String]
    var dark: Bool
    var cursorStyle: String
    var cursorBlink: Bool
    var graphemes: Bool
    var bell: UInt32
    var notifications: Bool
    var opacity: Double

    init(_ config: ghostty_config_t, dark: Bool, cursor: String?) {
        func color(_ key: String) -> String? {
            var value = ghostty_config_color_s()
            guard ghostty_config_get(config, &value, key, UInt(key.utf8.count)) else { return nil }
            return Self.hex(value)
        }
        func text(_ key: String) -> String? {
            var value: UnsafePointer<CChar>?
            guard ghostty_config_get(config, &value, key, UInt(key.utf8.count)), let value else { return nil }
            return String(cString: value)
        }
        func flag(_ key: String) -> Bool? {
            var value = false
            return ghostty_config_get(config, &value, key, UInt(key.utf8.count)) ? value : nil
        }
        var palette = ghostty_config_palette_s()
        let hasPalette = ghostty_config_get(config, &palette, "palette", UInt("palette".utf8.count))
        self.palette = hasPalette ? withUnsafeBytes(of: palette.colors) { raw in
            raw.bindMemory(to: ghostty_config_color_s.self).map(Self.hex)
        } : []
        foreground = color("foreground") ?? "#ffffff"
        background = color("background") ?? "#000000"
        self.cursor = cursor
        self.dark = dark
        cursorStyle = text("cursor-style") ?? "block"
        cursorBlink = flag("cursor-style-blink") ?? true
        graphemes = text("grapheme-width-method") != "legacy"
        var bell: UInt32 = 0
        self.bell = ghostty_config_get(config, &bell, "bell-features", UInt("bell-features".utf8.count)) ? bell : 0b1100
        notifications = flag("desktop-notifications") ?? true
        var opacity = 1.0
        self.opacity = ghostty_config_get(config, &opacity, "background-opacity", UInt("background-opacity".utf8.count)) ? opacity : 1
    }

    static func hex(_ color: ghostty_config_color_s) -> String {
        String(format: "#%02x%02x%02x", color.r, color.g, color.b)
    }

    var objectFields: [String: JSON] {
        var fields: [String: JSON] = [
            "foreground": .string(foreground), "background": .string(background), "scheme": .string(dark ? "dark" : "light"),
            "cursorStyle": .string(cursorStyle), "cursorBlink": .bool(cursorBlink), "graphemes": .bool(graphemes),
        ]
        if palette.count == 256 { fields["palette"] = .array(palette.map { .string($0) }) }
        if let cursor { fields["cursor"] = .string(cursor) }
        return fields
    }

    @MainActor static var appearanceIsDark: Bool {
        NSApp.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    }
}
