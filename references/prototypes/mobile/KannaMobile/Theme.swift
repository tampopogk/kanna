import SwiftUI
import UIKit

private func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> UIColor {
    UIColor(red: CGFloat((hex >> 16) & 255) / 255, green: CGFloat((hex >> 8) & 255) / 255, blue: CGFloat(hex & 255) / 255, alpha: alpha)
}

private func tone(_ dark: UIColor, _ light: UIColor) -> UIColor {
    UIColor { $0.userInterfaceStyle == .dark ? dark : light }
}

enum U {
    static let text = tone(rgb(0xe6e8eb), rgb(0x1d2026))
    static let muted = tone(rgb(0x8b93a0), rgb(0x6b7280))
    static let num = tone(rgb(0x4b5260), rgb(0xb4bac4))
    static let keyword = tone(rgb(0xc792ea), rgb(0x8839c4))
    static let function = tone(rgb(0x7aa2ff), rgb(0x2f63d8))
    static let type = tone(rgb(0xe5c07b), rgb(0xa15c00))
    static let string = tone(rgb(0xa5d6a7), rgb(0x2d7d3a))
}

enum K {
    static let bg = Color(tone(rgb(0x14161a), rgb(0xffffff)))
    static let panel = Color(tone(rgb(0x1b1e24), rgb(0xf4f5f7)))
    static let panel2 = Color(tone(rgb(0x22262d), rgb(0xeaecf0)))
    static let line = Color(tone(rgb(0x2c313a), rgb(0xd8dce2)))
    static let text = Color(U.text)
    static let muted = Color(U.muted)
    static let accent = Color(tone(rgb(0x7aa2ff), rgb(0x2f63d8)))
    static let ok = Color(tone(rgb(0x5fcf93), rgb(0x1f9d5c)))
    static let warn = Color(tone(rgb(0xf0b35a), rgb(0xb7791f)))
    static let bad = Color(tone(rgb(0xef6b6b), rgb(0xcf3f3f)))
    static let machine = Color(tone(rgb(0xb48cff), rgb(0x7c4dcc)))
    static let term = Color(tone(rgb(0x16181c), rgb(0xfbfbfc)))
    static let tint = Color(tone(rgb(0x7aa2ff, 0.18), rgb(0x2f63d8, 0.14)))
    static let okTint = Color(tone(rgb(0x5fcf93, 0.13), rgb(0x1f9d5c, 0.10)))
    static let badTint = Color(tone(rgb(0xef6b6b, 0.13), rgb(0xcf3f3f, 0.09)))
    static let warnTint = Color(tone(rgb(0xf0b35a, 0.13), rgb(0xb7791f, 0.11)))
    static let card = Color(tone(rgb(0x1f232a), rgb(0xffffff)))
    static let onAccent = Color(tone(rgb(0x0d0e10), rgb(0xffffff)))
    static let series = Color(tone(rgb(0x3987e5), rgb(0x2a78d6)))
    static let wash = Color(tone(rgb(0x3987e5, 0.12), rgb(0x2a78d6, 0.10)))
    static let axis = Color(tone(rgb(0x3a404a), rgb(0xc9cdd4)))
    static let hatch = Color(tone(rgb(0xf0b35a, 0.3), rgb(0xb7791f, 0.24)))
    static let m0 = Color(tone(rgb(0x1f232a), rgb(0xf4f5f7)))
    static let m1 = Color(tone(rgb(0x232c40), rgb(0xe4ebfb)))
    static let m2 = Color(tone(rgb(0x2c3f66), rgb(0xc6d5f6)))
    static let m3 = Color(tone(rgb(0x3c5a96), rgb(0x93b0ee)))
    static let m4 = Color(tone(rgb(0x4a72cc), rgb(0x2f63d8)))
    static let mt = Color(tone(rgb(0xe6e8eb), rgb(0x1d2026)))
    static let mt4 = Color.white
    static let mm = Color(tone(rgb(0xe6e8eb, 0.68), rgb(0x1d2026, 0.66)))
    static let mm4 = Color(rgb(0xffffff, 0.8))
}

extension Font {
    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }
}

func rich(_ source: String, code: CGFloat = 13) -> AttributedString {
    var out = (try? AttributedString(markdown: source, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(source)
    for run in out.runs where run.inlinePresentationIntent?.contains(.code) == true {
        out[run.range].font = .mono(code)
        out[run.range].foregroundColor = K.accent
    }
    return out
}

struct Hatch: View {
    var body: some View {
        Canvas { context, size in
            var path = Path()
            var x: CGFloat = -size.height
            while x < size.width {
                path.move(to: CGPoint(x: x, y: size.height))
                path.addLine(to: CGPoint(x: x + size.height, y: 0))
                x += 11
            }
            context.stroke(path, with: .color(K.hatch), lineWidth: 2)
        }
        .allowsHitTesting(false)
    }
}
