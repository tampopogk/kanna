import AppKit

@MainActor
enum SyntheticKey {
    private static let codes: [String: CGKeyCode] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14,
        "r": 15, "y": 16, "t": 17, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46,
        "]": 30, "[": 33, "=": 24, "-": 27, "0": 29, "`": 50, "return": 36, "escape": 53, "space": 49, "tab": 48, "delete": 51, "home": 115, "end": 119, "left": 123, "right": 124, "down": 125, "up": 126,
    ]

    static func event(_ name: String, window: NSWindow?) -> NSEvent? {
        var parts = name.lowercased().split(separator: "+").map(String.init)
        guard let key = parts.popLast(), let code = codes[key == "enter" ? "return" : key] ?? (key.count == 1 ? keys[Character(key)].flatMap({ $0.shift ? nil : $0.code }) : nil) else { return nil }
        var flags: CGEventFlags = []
        for part in parts {
            switch part {
            case "cmd", "super": flags.insert(.maskCommand)
            case "shift": flags.insert(.maskShift)
            case "opt", "alt", "option": flags.insert(.maskAlternate)
            case "ctrl", "control": flags.insert(.maskControl)
            default: return nil
            }
        }
        guard let cgEvent = CGEvent(keyboardEventSource: CGEventSource(stateID: .hidSystemState), virtualKey: code, keyDown: true) else { return nil }
        cgEvent.flags = flags
        guard let event = NSEvent(cgEvent: cgEvent) else { return nil }
        return NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: event.modifierFlags, timestamp: event.timestamp,
                                windowNumber: window?.windowNumber ?? 0, context: nil, characters: event.characters ?? "",
                                charactersIgnoringModifiers: event.charactersIgnoringModifiers ?? "", isARepeat: false, keyCode: event.keyCode)
    }

    private static let layout: [(CGKeyCode, Character, Character)] = [
        (0, "a", "A"), (1, "s", "S"), (2, "d", "D"), (3, "f", "F"), (4, "h", "H"), (5, "g", "G"), (6, "z", "Z"), (7, "x", "X"),
        (8, "c", "C"), (9, "v", "V"), (11, "b", "B"), (12, "q", "Q"), (13, "w", "W"), (14, "e", "E"), (15, "r", "R"),
        (16, "y", "Y"), (17, "t", "T"), (18, "1", "!"), (19, "2", "@"), (20, "3", "#"), (21, "4", "$"), (22, "6", "^"),
        (23, "5", "%"), (24, "=", "+"), (25, "9", "("), (26, "7", "&"), (27, "-", "_"), (28, "8", "*"), (29, "0", ")"),
        (30, "]", "}"), (31, "o", "O"), (32, "u", "U"), (33, "[", "{"), (34, "i", "I"), (35, "p", "P"), (37, "l", "L"),
        (38, "j", "J"), (39, "'", "\""), (40, "k", "K"), (41, ";", ":"), (42, "\\", "|"), (43, ",", "<"), (44, "/", "?"),
        (45, "n", "N"), (46, "m", "M"), (47, ".", ">"), (49, " ", " "), (50, "`", "~"),
    ]
    private static let keys: [Character: (code: CGKeyCode, shift: Bool, unshifted: Character)] = {
        var keys: [Character: (CGKeyCode, Bool, Character)] = [:]
        for (code, plain, shifted) in layout {
            keys[shifted] = (code, true, plain)
            keys[plain] = (code, false, plain)
        }
        return keys
    }()

    static func typing(_ text: String, window: NSWindow) -> [NSEvent] {
        let time = ProcessInfo.processInfo.systemUptime
        return text.flatMap { character -> [NSEvent] in
            let code: CGKeyCode, flags: NSEvent.ModifierFlags, characters: String, unshifted: String
            switch character {
            case "\n", "\r", "\r\n": (code, flags, characters, unshifted) = (36, [], "\r", "\r")
            case "\t": (code, flags, characters, unshifted) = (48, [], "\t", "\t")
            default:
                let key = keys[character]
                code = key?.code ?? 0xFFFF
                flags = key?.shift == true ? .shift : []
                characters = String(character)
                unshifted = key.map { String($0.unshifted) } ?? characters
            }
            return [NSEvent.EventType.keyDown, .keyUp].compactMap {
                NSEvent.keyEvent(with: $0, location: .zero, modifierFlags: flags, timestamp: time, windowNumber: window.windowNumber, context: nil,
                                 characters: characters, charactersIgnoringModifiers: unshifted, isARepeat: false, keyCode: code)
            }
        }
    }
}
