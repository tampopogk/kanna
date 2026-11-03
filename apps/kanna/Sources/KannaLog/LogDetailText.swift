import AppKit

@MainActor
enum LogDetailText {
    static func make(_ detail: CommitDetail, row: LogRow?, labels: [LogTask], fold: (Int, Bool)?, now: Int, zoom: CGFloat) -> NSAttributedString {
        let z = max(zoom, 0.8)
        let out = NSMutableAttributedString()
        func add(_ text: String, _ font: NSFont, _ color: NSColor = LogTheme.text, link: String? = nil, spacing: CGFloat = 0) {
            var attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color]
            if let link { attributes[.link] = URL(string: link) }
            if spacing > 0 {
                let style = NSMutableParagraphStyle()
                style.paragraphSpacing = spacing
                style.lineHeightMultiple = 1.15
                attributes[.paragraphStyle] = style
            }
            out.append(NSAttributedString(string: text, attributes: attributes))
        }
        let sans = LogTheme.sans(size: 12 * z), small = LogTheme.sans(size: 11.5 * z), mono = LogTheme.mono(size: 11.5 * z)
        let lines = detail.message.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
        let subject = lines.first.map(String.init) ?? ""
        let body = lines.count > 1 ? String(lines[1]).trimmingCharacters(in: .whitespacesAndNewlines) : ""
        add(subject + "\n", LogTheme.sans(size: 13.5 * z, weight: .semibold), spacing: 10 * z)
        add("Commit   ", small, LogTheme.muted)
        add(String(detail.hash.prefix(8)), mono)
        add("   ", small)
        add("Copy hash", small, LogTheme.accent, link: "kanna-log-copy:")
        add("\n", small)
        add("Author   ", small, LogTheme.muted)
        add(detail.author + "\n", sans)
        add("Date      ", small, LogTheme.muted)
        let date = DateFormatter.localizedString(from: Date(timeIntervalSince1970: TimeInterval(detail.time)), dateStyle: .medium, timeStyle: .short)
        add("\(date) · \(LogAge.text(detail.time, now: now))\n", sans)
        add(detail.parents.count > 1 ? "Parents  " : "Parent    ", small, LogTheme.muted)
        if detail.parents.isEmpty { add("none", sans, LogTheme.muted) }
        for parent in detail.parents {
            add(String(parent.prefix(8)), mono, LogTheme.accent, link: "kanna-log-commit:\(parent)")
            add("  ", small)
        }
        add("\n", small, spacing: 8 * z)
        for label in labels {
            add("● ", small, label.merged ? LogTheme.ok : LogTheme.accent)
            add(label.name, sans)
            if label.merged {
                add("  ✓ merged", small, LogTheme.ok)
            } else {
                add("  \(label.place == "desk" ? "Desk" : label.place == "shelf" ? "Shelf" : "Done") · \(label.ahead) ahead", small, LogTheme.muted)
                if label.behind > 0 { add(" · \(label.behind) behind", small, LogTheme.bad) }
            }
            add("  \(label.branch)\n", mono, LogTheme.muted)
        }
        let refs = (row?.refs ?? []).filter { ref in !labels.contains { $0.branch == ref } }
        if !refs.isEmpty { add(refs.joined(separator: "  ") + "\n", mono, LogTheme.muted) }
        if let fold, fold.0 > 0 {
            let (count, open) = fold
            add("Merges \(count) commits, \(open ? "shown below" : "folded into this row") · ", small, LogTheme.muted)
            add(open ? "Fold (←)" : "Unfold (→)", small, LogTheme.accent, link: "kanna-log-fold:\(detail.hash)")
            add("\n", small)
        }
        add("\n", LogTheme.sans(size: 6 * z))
        if !body.isEmpty { add(body + "\n\n", LogTheme.sans(size: 12 * z), spacing: 4 * z) }
        let total = detail.files.reduce(0) { $0 + ($1.added ?? 0) + ($1.removed ?? 0) }
        add("\(detail.files.count) file\(detail.files.count == 1 ? "" : "s")\(detail.parents.count > 1 ? " (against its first parent)" : "") · \(total) lines\n", small, LogTheme.muted, spacing: 4 * z)
        for file in detail.files {
            add(file.status + "  ", LogTheme.mono(size: 11 * z, weight: .semibold), LogTheme.status(file.status))
            add(file.path, mono)
            if let added = file.added, let removed = file.removed {
                add("  +\(added)", mono, LogTheme.ok)
                add(" −\(removed)", mono, LogTheme.bad)
            } else {
                add("  binary", mono, LogTheme.muted)
            }
            add("\n", mono)
        }
        add("\n", small)
        add("Open its diff (↩)", small, LogTheme.accent, link: "kanna-log-diff:")
        return out
    }
}
