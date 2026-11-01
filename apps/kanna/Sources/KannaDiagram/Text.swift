import AppKit
import CoreText

enum LabelText {
    static func lineHeight(size: CGFloat, htmlLike: Bool) -> CGFloat { size * (htmlLike ? 1.5 : 1.1) }

    static func font(size: CGFloat, bold: Bool = false, italic: Bool = false, monospaced: Bool = false) -> CTFont {
        var font: NSFont = monospaced
            ? .monospacedSystemFont(ofSize: size, weight: bold ? .semibold : .regular)
            : .systemFont(ofSize: size, weight: bold ? .semibold : .regular)
        if italic { font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask) }
        return font as CTFont
    }

    static func plainLines(_ label: String) -> [String] {
        var text = label.replacingOccurrences(of: "<br\\s*/?>", with: "\n", options: [.regularExpression, .caseInsensitive])
        text = text.replacingOccurrences(of: "<[^>]+>", with: "", options: .regularExpression)
        text = text.replacingOccurrences(of: "\\*\\*|__|`", with: "", options: .regularExpression)
        return decodeEntities(text).components(separatedBy: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
    }

    static let namedEntities = ["lt": "<", "gt": ">", "quot": "\"", "apos": "'", "nbsp": "\u{A0}", "amp": "&", "hash": "#", "semi": ";",
                                "colon": ":", "comma": ",", "lpar": "(", "rpar": ")", "rarr": "→", "larr": "←", "harr": "↔", "uarr": "↑",
                                "darr": "↓", "rArr": "⇒", "lArr": "⇐", "hellip": "…", "mdash": "—", "ndash": "–", "middot": "·",
                                "bull": "•", "times": "×", "check": "✓", "hearts": "♥", "copy": "©", "deg": "°", "plusmn": "±"]

    static func decodeEntities(_ label: String) -> String {
        let text = label.replacingOccurrences(of: "\u{FB02}\u{B0}\u{B0}", with: "&#").replacingOccurrences(of: "\u{FB02}\u{B0}", with: "&")
            .replacingOccurrences(of: "\u{B6}\u{DF}", with: ";")
        return text.replacing(/&(#[0-9]+|#[xX][0-9a-fA-F]+|[A-Za-z][A-Za-z0-9]*);/) { match in
            let name = String(match.1)
            if name.hasPrefix("#") {
                let hex = name.dropFirst().first.map { $0 == "x" || $0 == "X" } ?? false
                let code = UInt32(name.dropFirst(hex ? 2 : 1), radix: hex ? 16 : 10)
                return code.flatMap(Unicode.Scalar.init).map { String(Character($0)) } ?? String(match.0)
            }
            return namedEntities[name] ?? String(match.0)
        }
    }

    static func width(_ line: String, font: CTFont) -> CGFloat {
        guard !line.isEmpty else { return 0 }
        let attributed = NSAttributedString(string: line, attributes: [.font: font])
        return CGFloat(CTLineGetTypographicBounds(CTLineCreateWithAttributedString(attributed), nil, nil, nil))
    }

    static func wrap(_ label: String, font: CTFont, maxWidth: CGFloat?) -> [String] {
        let lines = plainLines(label)
        guard let maxWidth, maxWidth > 0 else { return lines }
        return lines.flatMap { line -> [String] in
            guard width(line, font: font) > maxWidth else { return [line] }
            var out: [String] = []
            var current = ""
            for word in line.split(separator: " ", omittingEmptySubsequences: true).map(String.init) {
                let candidate = current.isEmpty ? word : current + " " + word
                if current.isEmpty || width(candidate, font: font) <= maxWidth {
                    current = candidate
                } else {
                    out.append(current)
                    current = word
                }
            }
            if !current.isEmpty { out.append(current) }
            return out.isEmpty ? [""] : out
        }
    }

    struct Size { var width: CGFloat; var height: CGFloat; var lines: Int }

    static func measure(_ label: String, size: CGFloat, bold: Bool, italic: Bool, monospaced: Bool, maxWidth: CGFloat?, htmlLike: Bool) -> Size {
        let font = font(size: size, bold: bold, italic: italic, monospaced: monospaced)
        let lines = wrap(label, font: font, maxWidth: maxWidth)
        let width = lines.map { Self.width($0, font: font) }.max() ?? 0
        return Size(width: ceil(width), height: CGFloat(lines.count) * lineHeight(size: size, htmlLike: htmlLike), lines: lines.count)
    }

    @discardableResult
    static func draw(_ label: String, in rect: CGRect, size: CGFloat, color: CGColor, bold: Bool = false, italic: Bool = false,
                     htmlLike: Bool = true, alignment: NSTextAlignment = .center, context: CGContext) -> Bool {
        var size = size
        var font = font(size: size, bold: bold, italic: italic)
        var lines = wrap(label, font: font, maxWidth: rect.width + 0.5)
        var shrunk = false
        func fits() -> Bool {
            let height = CGFloat(lines.count) * lineHeight(size: size, htmlLike: htmlLike)
            return height <= rect.height + 1 && lines.allSatisfy { width($0, font: font) <= rect.width + 1 }
        }
        while !fits() && size > 4 {
            size *= 0.92
            shrunk = true
            font = Self.font(size: size, bold: bold, italic: italic)
            lines = wrap(label, font: font, maxWidth: rect.width + 0.5)
        }
        let step = lineHeight(size: size, htmlLike: htmlLike)
        var top = rect.midY - CGFloat(lines.count) * step / 2
        let ascent = CTFontGetAscent(font), descent = CTFontGetDescent(font)
        for line in lines {
            let attributed = NSAttributedString(string: line, attributes: [.font: font, NSAttributedString.Key(kCTForegroundColorAttributeName as String): color])
            let ctLine = CTLineCreateWithAttributedString(attributed)
            let lineWidth = width(line, font: font)
            let x = alignment == .left ? rect.minX : alignment == .right ? rect.maxX - lineWidth : rect.midX - lineWidth / 2
            let baseline = top + step / 2 + (ascent - descent) / 2
            context.saveGState()
            context.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
            context.textPosition = CGPoint(x: x, y: baseline)
            CTLineDraw(ctLine, context)
            context.restoreGState()
            top += step
        }
        return shrunk
    }
}
