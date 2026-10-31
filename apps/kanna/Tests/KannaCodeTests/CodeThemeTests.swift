import AppKit
import Testing
@testable import KannaCode

struct CodeThemeTests {
    let missing = unsafeBitCast(0, to: NSFont.self)

    @Test func aFontAppKitCannotMakeFallsBackToOneOfTheSameSize() {
        for monospaced in [true, false] {
            let key = CodeTheme.FontKey(monospaced: monospaced, size: 13.25, weight: NSFont.Weight.semibold.rawValue)
            let font = CodeTheme.cached(key) { _ in missing }
            #expect(unsafeBitCast(font, to: NSFont?.self) != nil)
            #expect(font.pointSize == 13.25)
            let text = NSAttributedString(string: "M", attributes: [.font: font, .foregroundColor: CodeTheme.text])
            #expect(text.size().width > 0)
        }
    }

    @Test func aFallbackIsNotKeptSoTheNextAskGetsTheSystemFont() {
        let key = CodeTheme.FontKey(monospaced: true, size: 14.75, weight: NSFont.Weight.regular.rawValue)
        #expect(CodeTheme.cached(key) { _ in missing }.fontName == "Menlo-Regular")
        #expect(CodeTheme.font(size: 14.75) == NSFont.monospacedSystemFont(ofSize: 14.75, weight: .regular))
    }

    @Test func fontsAreMadeOncePerSizeAndWeight() {
        #expect(CodeTheme.font(size: 11.5, weight: .semibold) === CodeTheme.font(size: 11.5, weight: .semibold))
        #expect(CodeTheme.sans(size: 11.5) === CodeTheme.sans(size: 11.5))
        #expect(CodeTheme.font(size: 11.5) !== CodeTheme.font(size: 11.5, weight: .semibold))
        #expect(CodeTheme.font(size: 11.5) !== CodeTheme.sans(size: 11.5))
    }
}
