import Foundation

public struct PortTunnelURL: Equatable, Sendable {
    public let remote: URL
    public let port: Int

    public init?(_ string: String) {
        guard let url = URL(string: string), let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme),
              let host = url.host(percentEncoded: false)?.lowercased(), Self.loopback(host) else { return nil }
        remote = url
        port = url.port ?? (scheme == "https" ? 443 : 80)
    }

    public static func loopback(_ host: String) -> Bool {
        host == "localhost" || host.hasSuffix(".localhost") || host == "::1" || host == "[::1]" || host.hasPrefix("127.") || host == "0.0.0.0"
    }

    public var address: String { "\(remote.host(percentEncoded: false) ?? "localhost"):\(port)" }

    public func local(_ local: Int) -> URL {
        var parts = URLComponents(url: remote, resolvingAgainstBaseURL: false) ?? URLComponents()
        parts.host = "127.0.0.1"
        parts.port = local
        return parts.url ?? remote
    }

    public func remote(_ url: URL, local: Int) -> URL {
        guard url.host(percentEncoded: false) == "127.0.0.1", url.port == local,
              var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return url }
        let original = URLComponents(url: remote, resolvingAgainstBaseURL: false)
        parts.host = original?.host
        parts.port = original?.port
        return parts.url ?? url
    }
}
