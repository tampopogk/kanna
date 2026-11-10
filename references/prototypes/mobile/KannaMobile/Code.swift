import SwiftUI
import UIKit

enum Highlight {
    static let keywords: Set<String> = ["fn", "let", "match", "true", "false", "if", "return", "self", "pub", "const", "Some", "None", "Ok", "Err", "move"]
    static let types: Set<String> = ["String", "Option", "Vec", "Command"]

    static func attributed(_ source: String, size: CGFloat, wrap: Bool) -> NSAttributedString {
        let font = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
        let indent = source.prefix { $0 == " " }.count
        let text = wrap ? String(source.dropFirst(indent)) : source
        let out = NSMutableAttributedString(string: text, attributes: [.font: font, .foregroundColor: U.text])
        let chars = Array(text)
        var i = 0
        func color(_ from: Int, _ to: Int, _ c: UIColor) {
            let start = String(chars[0..<from]).utf16.count
            let length = String(chars[from..<to]).utf16.count
            out.addAttribute(.foregroundColor, value: c, range: NSRange(location: start, length: length))
        }
        while i < chars.count {
            let c = chars[i]
            if c == "\"" {
                var j = i + 1
                while j < chars.count, chars[j] != "\"" { j += 1 }
                let end = min(j + 1, chars.count)
                color(i, end, U.string)
                i = end
            } else if c.isLetter || c == "_" {
                var j = i
                while j < chars.count, chars[j].isLetter || chars[j].isNumber || chars[j] == "_" { j += 1 }
                let word = String(chars[i..<j])
                if keywords.contains(word) { color(i, j, U.keyword) }
                else if types.contains(word) { color(i, j, U.type) }
                else if j < chars.count, chars[j] == "(", word.first?.isLowercase == true { color(i, j, U.function) }
                i = j
            } else {
                i += 1
            }
        }
        if wrap {
            let width = ("0" as NSString).size(withAttributes: [.font: font]).width
            let style = NSMutableParagraphStyle()
            style.firstLineHeadIndent = CGFloat(indent) * width
            style.headIndent = CGFloat(indent + 2) * width
            style.lineBreakMode = .byWordWrapping
            style.minimumLineHeight = size * 1.44
            style.maximumLineHeight = size * 1.44
            out.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: out.length))
        }
        return out
    }
}

struct CodeText: UIViewRepresentable {
    var text: String
    var size: CGFloat
    var wrap: Bool

    func makeUIView(context: Context) -> UILabel {
        let label = UILabel()
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return label
    }

    func updateUIView(_ label: UILabel, context: Context) {
        label.numberOfLines = wrap ? 0 : 1
        label.attributedText = Highlight.attributed(text, size: size, wrap: wrap)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: UILabel, context: Context) -> CGSize? {
        if !wrap { return uiView.intrinsicContentSize }
        let width = proposal.width ?? 300
        let fit = uiView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude))
        return CGSize(width: width, height: fit.height)
    }
}

struct CodeRow: View {
    var line: CodeLine
    var size: CGFloat
    var wrap: Bool
    var strong: CGFloat = 2
    var selected = false

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            Text(String(line.n))
                .font(.mono(size))
                .foregroundStyle(line.hot || selected ? K.muted : Color(U.num))
                .frame(width: size * 3.04, height: size * 1.44, alignment: .bottomTrailing)
                .padding(.trailing, 9)
            CodeText(text: line.text, size: size, wrap: wrap)
                .padding(.trailing, 8)
        }
        .padding(.vertical, 0.5)
        .frame(maxWidth: wrap ? .infinity : nil, alignment: .leading)
        .background(line.hot || selected ? K.tint : .clear)
        .overlay(alignment: .leading) {
            if line.hot || selected { K.accent.frame(width: strong) }
        }
    }
}

struct CodeCard: View {
    var snippet: Snippet
    @Environment(Store.self) private var store
    @State private var full: FullCode?

    private var lines: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(snippet.lines) { CodeRow(line: $0, size: 12.5, wrap: store.wrapByDefault) }
        }
    }

    var body: some View {
        Button { store.fullLine = nil; full = FullCode(snippet: snippet) } label: {
            VStack(spacing: 0) {
                HStack(spacing: 6) {
                    (Text(snippet.folder) + Text(snippet.base).bold().foregroundColor(K.text) + Text(":\(snippet.first)"))
                        .font(.mono(12))
                        .foregroundStyle(K.muted)
                        .lineLimit(1)
                        .truncationMode(.head)
                    Spacer(minLength: 0)
                    Image(systemName: "arrow.up.left.and.arrow.down.right").font(.system(size: 12, weight: .medium)).foregroundStyle(K.muted)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(K.panel)
                .overlay(alignment: .bottom) { K.line.frame(height: 1) }
                Group {
                    if store.wrapByDefault { lines } else { ScrollView(.horizontal, showsIndicators: false) { lines } }
                }
                .padding(.vertical, 6)
            }
            .background(K.term)
            .clipShape(RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(K.line))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("code-card")
        .fullScreenCover(item: $full) { FullCodeView(snippet: $0) }
    }
}

struct FullCodeView: View {
    var snippet: FullCode
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var wrap = true
    @State private var size: CGFloat = 14
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        let s = snippet.snippet
        VStack(spacing: 0) {
            HStack {
                Button { dismiss() } label: {
                    Image(systemName: "xmark").font(.system(size: 17, weight: .medium)).foregroundStyle(K.accent).frame(width: 44, height: 36)
                }
                .accessibilityIdentifier("close-code")
                Spacer()
                Text("\(s.base):\(s.first)").font(.mono(13, .semibold))
                Spacer()
                Button { wrap.toggle() } label: {
                    Text("Wrap").font(.system(size: 12, weight: .medium)).foregroundStyle(wrap ? K.accent : K.muted)
                        .padding(.horizontal, 9).padding(.vertical, 3)
                        .background(wrap ? K.tint : K.panel, in: Capsule())
                }
                .buttonStyle(.plain)
                .frame(width: 60, alignment: .trailing)
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 6)
            .overlay(alignment: .bottom) { K.line.frame(height: 1) }
            ScrollView(wrap ? .vertical : [.vertical, .horizontal]) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(s.lines) { line in
                        Button { store.fullLine = store.fullLine == line.n ? nil : line.n } label: {
                            CodeRow(line: CodeLine(line.n, line.text), size: size * pinch, wrap: wrap, strong: 3, selected: store.fullLine == line.n)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.top, 8)
            }
            .gesture(MagnifyGesture().updating($pinch) { value, state, _ in state = value.magnification }.onEnded { size = min(26, max(10, size * $0.magnification)) })
            Text("\(s.file), on main at \(s.commit)")
                .font(.system(size: 13))
                .foregroundStyle(K.muted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            Composer(placeholder: store.fullLine.map { "Ask about line \($0)" } ?? "Ask about this code") { text in
                let line = store.fullLine.map { " (line \($0))" } ?? ""
                dismiss()
                store.send(text + line)
            }
        }
        .background(K.bg)
        .onAppear { wrap = store.wrapByDefault }
    }
}
