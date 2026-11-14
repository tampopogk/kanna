import AppKit
import GhosttyKit

struct Shortcut: Equatable {
    let key: String
    let modifiers: NSEvent.ModifierFlags

    static func cmd(_ key: String) -> Shortcut { Shortcut(key: key, modifiers: .command) }
    static func shiftCmd(_ key: String) -> Shortcut { Shortcut(key: key, modifiers: [.command, .shift]) }

    init?(_ trigger: ghostty_input_trigger_s) {
        let key: String?
        switch trigger.tag {
        case GHOSTTY_TRIGGER_UNICODE: key = UnicodeScalar(trigger.key.unicode).map { String(Character($0)).lowercased() }
        case GHOSTTY_TRIGGER_PHYSICAL: key = Self.physical[trigger.key.physical]
        default: key = nil
        }
        guard let key, !key.isEmpty else { return nil }
        var modifiers: NSEvent.ModifierFlags = []
        let mods = trigger.mods.rawValue
        if mods & GHOSTTY_MODS_SHIFT.rawValue != 0 { modifiers.insert(.shift) }
        if mods & GHOSTTY_MODS_CTRL.rawValue != 0 { modifiers.insert(.control) }
        if mods & GHOSTTY_MODS_ALT.rawValue != 0 { modifiers.insert(.option) }
        if mods & GHOSTTY_MODS_SUPER.rawValue != 0 { modifiers.insert(.command) }
        self.init(key: key, modifiers: modifiers)
    }

    init(key: String, modifiers: NSEvent.ModifierFlags) {
        self.key = key
        self.modifiers = modifiers
    }

    var label: String {
        (modifiers.contains(.control) ? "⌃" : "") + (modifiers.contains(.option) ? "⌥" : "") + (modifiers.contains(.shift) ? "⇧" : "")
            + (modifiers.contains(.command) ? "⌘" : "") + key.replacingOccurrences(of: "\r", with: "↩").replacingOccurrences(of: "\u{7f}", with: "⌫")
    }

    private static let physical: [ghostty_input_key_e: String] = {
        var table: [ghostty_input_key_e: String] = [
            GHOSTTY_KEY_BACKQUOTE: "`", GHOSTTY_KEY_BACKSLASH: "\\", GHOSTTY_KEY_BRACKET_LEFT: "[", GHOSTTY_KEY_BRACKET_RIGHT: "]",
            GHOSTTY_KEY_COMMA: ",", GHOSTTY_KEY_EQUAL: "=", GHOSTTY_KEY_MINUS: "-", GHOSTTY_KEY_PERIOD: ".", GHOSTTY_KEY_QUOTE: "'",
            GHOSTTY_KEY_SEMICOLON: ";", GHOSTTY_KEY_SLASH: "/", GHOSTTY_KEY_ENTER: "\r",
            GHOSTTY_KEY_ARROW_UP: String(UnicodeScalar(NSUpArrowFunctionKey)!), GHOSTTY_KEY_ARROW_DOWN: String(UnicodeScalar(NSDownArrowFunctionKey)!),
            GHOSTTY_KEY_ARROW_LEFT: String(UnicodeScalar(NSLeftArrowFunctionKey)!), GHOSTTY_KEY_ARROW_RIGHT: String(UnicodeScalar(NSRightArrowFunctionKey)!),
        ]
        for (offset, letter) in "abcdefghijklmnopqrstuvwxyz".enumerated() {
            table[ghostty_input_key_e(rawValue: GHOSTTY_KEY_A.rawValue + UInt32(offset))] = String(letter)
        }
        for digit in 0...9 { table[ghostty_input_key_e(rawValue: GHOSTTY_KEY_DIGIT_0.rawValue + UInt32(digit))] = String(digit) }
        return table
    }()
}

extension GhosttyRuntime {
    func shortcut(for action: String) -> Shortcut? { Shortcut(trigger(for: action)) }
}
