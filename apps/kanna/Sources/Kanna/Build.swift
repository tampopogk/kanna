import AppKit
import CryptoKit

enum KannaBuild {
    static let channels = ["production": "kanna-prod", "staging": "kanna-staging", "dev": "kanna-dev"]

    private static var info: [String: Any] { Bundle.main.infoDictionary ?? [:] }

    static var channel: String { info["KannaChannel"] as? String ?? "dev" }

    static var folder: String { channels[channel] ?? "kanna-dev" }

    static var defaultInstance: String { info["KannaInstance"] as? String ?? "default" }

    static var name: String { info["CFBundleName"] as? String ?? "Kanna" }

    static var commit: String { info["KannaCommit"] as? String ?? "unknown" }

    static let daemonBuild: String? = buildID(of: DaemonStarter.helpers.appendingPathComponent("kanna-daemon"))

    static func buildID(of binary: URL) -> String? {
        guard let handle = try? FileHandle(forReadingFrom: binary) else { return nil }
        defer { try? handle.close() }
        var hasher = SHA256()
        while let chunk = try? handle.read(upToCount: 1 << 16), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    static func instanceFolder(_ name: String) -> String {
        name == "default" ? folder : "\(folder)-\(name)"
    }

    static func instance(ofFolder folder: String) -> String {
        for prefix in channels.values where folder.hasPrefix("\(prefix)-") {
            return String(folder.dropFirst(prefix.count + 1))
        }
        return "default"
    }

    @MainActor static func badgeIcon() {
        let label: String
        let color: NSColor
        switch channel {
        case "staging": (label, color) = ("STAGING", .systemPurple)
        case "dev": (label, color) = ("DEV", .systemOrange)
        default: return
        }
        guard let icon = NSApp.applicationIconImage?.copy() as? NSImage else { return }
        let size = icon.size
        let badged = NSImage(size: size, flipped: false) { rect in
            icon.draw(in: rect)
            let band = NSRect(x: rect.width * 0.1, y: rect.height * 0.08, width: rect.width * 0.8, height: rect.height * 0.22)
            color.setFill()
            NSBezierPath(roundedRect: band, xRadius: band.height * 0.3, yRadius: band.height * 0.3).fill()
            let font = NSFont.systemFont(ofSize: band.height * 0.62, weight: .heavy)
            let text = NSAttributedString(string: label, attributes: [.font: font, .foregroundColor: NSColor.white])
            let textSize = text.size()
            text.draw(at: NSPoint(x: band.midX - textSize.width / 2, y: band.midY - textSize.height / 2))
            return true
        }
        NSApp.applicationIconImage = badged
    }
}
