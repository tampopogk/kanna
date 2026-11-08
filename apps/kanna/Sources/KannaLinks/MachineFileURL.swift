import Foundation
import UniformTypeIdentifiers

public enum MachineFileURL {
    public static let scheme = "kanna-file"
    static let host = "files"

    public static func page(_ string: String) -> URL? {
        guard let url = URL(string: string), url.isFileURL, url.path.hasPrefix("/") else { return nil }
        var parts = URLComponents()
        parts.scheme = scheme
        parts.host = host
        parts.path = url.path
        parts.query = url.query
        parts.fragment = url.fragment
        return parts.url
    }

    public static func path(of url: URL?) -> String? {
        guard let url, url.scheme?.lowercased() == scheme, url.host(percentEncoded: false) == host else { return nil }
        let path = url.path(percentEncoded: false)
        return path.hasPrefix("/") ? path : nil
    }

    public static func file(_ url: URL) -> URL {
        guard let path = path(of: url) else { return url }
        var parts = URLComponents()
        parts.scheme = "file"
        parts.host = ""
        parts.path = path
        parts.query = url.query
        parts.fragment = url.fragment
        return parts.url ?? url
    }

    public static func parts(_ path: String) -> [Substring]? {
        guard path.hasPrefix("/") else { return nil }
        var out: [Substring] = []
        for part in path.split(separator: "/", omittingEmptySubsequences: true) {
            switch part {
            case ".": continue
            case "..": if out.popLast() == nil { return nil }
            default: out.append(part)
            }
        }
        return out
    }

    public static func folder(of page: String) -> [Substring]? {
        guard let parts = parts(page), !parts.isEmpty else { return nil }
        return Array(parts.dropLast())
    }

    public static func inside(_ path: String, _ folder: [Substring]) -> Bool {
        guard let parts = parts(path), parts.count >= folder.count else { return false }
        return zip(parts, folder).allSatisfy { Array($0.utf8) == Array($1.utf8) }
    }

    public static func mime(of path: String) -> String {
        let ext = (path as NSString).pathExtension.lowercased()
        switch ext {
        case "html", "htm": return "text/html"
        case "js", "mjs": return "text/javascript"
        case "css": return "text/css"
        case "json", "map": return "application/json"
        case "svg": return "image/svg+xml"
        case "wasm": return "application/wasm"
        default: return UTType(filenameExtension: ext)?.preferredMIMEType ?? "application/octet-stream"
        }
    }
}
