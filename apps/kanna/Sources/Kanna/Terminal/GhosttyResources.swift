import AppKit
import GhosttyKit
import os

enum GhosttyResources {
    static let ghosttyBundleID = "com.mitchellh.ghostty"
    private static let log = Logger(subsystem: "dev.kanna.app", category: "ghostty-config")

    static let bundled: String? = {
        guard let resources = Bundle.main.resourceURL else { return nil }
        let terminfo = resources.appendingPathComponent("terminfo/78/xterm-ghostty").path
        return FileManager.default.fileExists(atPath: terminfo) ? resources.appendingPathComponent("ghostty").path : nil
    }()

    static let installed: String? = {
        let env = ProcessInfo.processInfo.environment["KANNA_GHOSTTY_APP"]
        if env == "none" { return nil }
        guard let app = env.map({ URL(fileURLWithPath: $0) }) ?? NSWorkspace.shared.urlForApplication(withBundleIdentifier: ghosttyBundleID) else { return nil }
        let dir = app.appendingPathComponent("Contents/Resources/ghostty")
        var isDir: ObjCBool = false
        return FileManager.default.fileExists(atPath: dir.appendingPathComponent("themes").path, isDirectory: &isDir) && isDir.boolValue ? dir.path : nil
    }()

    static var themes: String? { installed ?? bundled }

    static func initialize(_ argc: Int32, _ argv: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> Bool {
        let inherited = ProcessInfo.processInfo.environment["GHOSTTY_RESOURCES_DIR"]
        if let themes { setenv("GHOSTTY_RESOURCES_DIR", themes, 1) }
        let ok = ghostty_init(UInt(argc), argv) == GHOSTTY_SUCCESS
        if let restore = bundled ?? inherited { setenv("GHOSTTY_RESOURCES_DIR", restore, 1) } else { unsetenv("GHOSTTY_RESOURCES_DIR") }
        return ok
    }

    static func report(_ config: ghostty_config_t) {
        let count = ghostty_config_diagnostics_count(config)
        guard count > 0 else { return }
        var lines = (0..<count).compactMap { index -> String? in
            ghostty_config_get_diagnostic(config, index).message.map { "Ghostty config: " + String(cString: $0) }
        }
        if installed == nil, lines.contains(where: { $0.contains("theme \"") && $0.contains("not found") }) {
            let shipped = bundled.flatMap { try? FileManager.default.contentsOfDirectory(atPath: $0 + "/themes") }?.sorted() ?? []
            lines.append("Ghostty config: Kanna ships only these themes: \(shipped.joined(separator: ", ")); install Ghostty to use all of its themes, or put the theme's file in ~/.config/ghostty/themes")
        }
        for line in lines {
            log.error("\(line, privacy: .public)")
            FileHandle.standardError.write(Data("kanna: \(line)\n".utf8))
        }
    }
}
