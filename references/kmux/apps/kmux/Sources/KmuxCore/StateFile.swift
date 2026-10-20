import Darwin
import Foundation

/// Where an instance keeps its saved layout (docs/kmux-spec.md §3.6), and
/// reading and writing it safely. Restoring re-runs terminal commands, so
/// kmux only reads a file it wrote itself: a regular file (not a link) owned
/// by this user, readable by no one else, in a folder no one else can write to.
public struct StateFile: Sendable {
    public let path: String

    public init(path: String) { self.path = path }

    /// Next to the instance's socket: `state-default.json` (`state-work.json`, …)
    /// in `~/Library/Application Support/kmux/`, or, for a socket elsewhere
    /// (`$KMUX_SOCKET`), the socket's path with `.state.json` for `.sock`, so
    /// tests on private sockets keep private state.
    public static func path(for instance: Instance) -> String {
        if instance.socketPath == Instance.socketPath(for: instance.name) {
            return (Instance.directory as NSString).appendingPathComponent("state-\(instance.name).json")
        }
        let socket = instance.socketPath
        return (socket.hasSuffix(".sock") ? String(socket.dropLast(5)) : socket) + ".state.json"
    }

    /// Where the app keeps what is only its own (docs/presentation-layers.md):
    /// where its windows are and how large markdown panes are drawn.
    /// `app-default.json` beside `state-default.json`, or the socket's path
    /// with `.app.json`.
    public static func appPath(for instance: Instance) -> String {
        if instance.socketPath == Instance.socketPath(for: instance.name) {
            return (Instance.directory as NSString).appendingPathComponent("app-\(instance.name).json")
        }
        let socket = instance.socketPath
        return (socket.hasSuffix(".sock") ? String(socket.dropLast(5)) : socket) + ".app.json"
    }

    public enum Contents: Equatable {
        /// No saved layout.
        case none
        case state(JSON)
        /// There is a file, but kmux won't restore it; the reason says why.
        case refused(String)
    }

    /// Larger than any real layout; a bigger file is not one kmux wrote.
    static let maxSize = 16 << 20

    public func load() -> Contents {
        var info = stat()
        guard lstat(path, &info) == 0 else {
            return errno == ENOENT ? .none : .refused("can't read \(path): \(String(cString: strerror(errno)))")
        }
        if let problem = Self.unsafe(info, regularFile: true) { return .refused("\(path) \(problem)") }
        if let problem = Self.unsafeFolder((path as NSString).deletingLastPathComponent) { return .refused(problem) }
        guard info.st_size <= Self.maxSize else { return .refused("\(path) is too large") }
        let fd = open(path, O_RDONLY | O_NOFOLLOW)
        guard fd >= 0 else { return .refused("can't read \(path): \(String(cString: strerror(errno)))") }
        let data = FileHandle(fileDescriptor: fd, closeOnDealloc: true).readDataToEndOfFile()
        guard let state = try? JSON.parse(data) else { return .refused("\(path) is not valid JSON") }
        return .state(state)
    }

    /// Why a file with `info` must not be trusted, or nil.
    static func unsafe(_ info: stat, regularFile: Bool) -> String? {
        let type = info.st_mode & S_IFMT
        if regularFile, type != S_IFREG { return "is not a regular file" }
        if info.st_uid != getuid() { return "belongs to another user" }
        if info.st_mode & 0o077 != 0 { return "can be read or written by other users (mode \(String(info.st_mode & 0o777, radix: 8)))" }
        return nil
    }

    /// Why files in `folder` can't be trusted (someone else could have put
    /// them there), or nil. A sticky folder such as /tmp is fine: others can't
    /// replace a file this user owns there.
    static func unsafeFolder(_ folder: String) -> String? {
        var info = stat()
        guard stat(folder, &info) == 0 else { return "can't check \(folder)" }
        let sticky = info.st_mode & S_ISVTX != 0
        if info.st_uid != getuid(), !(info.st_uid == 0 && sticky) { return "\(folder) belongs to another user" }
        if info.st_mode & 0o022 != 0, !sticky { return "\(folder) can be written by other users" }
        return nil
    }

    /// Writes `state` atomically: to a temporary file beside it (mode 0600),
    /// flushed to disk, then renamed over the old one.
    public func write(_ state: JSON) throws {
        let folder = (path as NSString).deletingLastPathComponent
        try FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        let data = try encoder.encode(state)
        let temporary = "\(path).tmp-\(getpid())"
        unlink(temporary)
        let fd = open(temporary, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)
        guard fd >= 0 else { throw Self.failure("can't write \(temporary)") }
        let written = data.withUnsafeBytes { buffer -> Bool in
            var offset = 0
            while offset < buffer.count {
                let n = Darwin.write(fd, buffer.baseAddress! + offset, buffer.count - offset)
                if n < 0 { if errno == EINTR { continue }; return false }
                offset += n
            }
            return true
        }
        let synced = fsync(fd) == 0
        close(fd)
        guard written, synced, rename(temporary, path) == 0 else {
            let error = Self.failure("can't save \(path)")
            unlink(temporary)
            throw error
        }
    }

    /// Moves a file kmux won't restore out of the way, keeping it for a
    /// person to look at: `state-default.json.bad`. Returns where it went.
    @discardableResult
    public func setAside() -> String? {
        let aside = path + ".bad"
        return rename(path, aside) == 0 ? aside : nil
    }

    private static func failure(_ message: String) -> KmuxError {
        KmuxError("io_error", "\(message): \(String(cString: strerror(errno)))")
    }
}
