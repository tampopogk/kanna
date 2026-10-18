import Darwin
import Foundation

final class DaemonStarter: @unchecked Sendable {
    let socketPath: String
    private let lock = NSLock()
    private var everConnected = false
    private var unreachableSince: Date?
    private var lastStart: Date?

    private static let graceAfterLoss: TimeInterval = 2
    private static let startInterval: TimeInterval = 10
    private static let passedThrough: Set<String> = ["KANNA_STATE_DIR", "KANNA_DEBUG", "KANNA_AGENTS_HOME", "KANNA_NO_STATE", "KANNA_FRESH"]

    init(socketPath: String) {
        self.socketPath = socketPath
    }

    static var helpers: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers") }

    func connected() {
        lock.withLock {
            everConnected = true
            unreachableSince = nil
        }
    }

    func replacing() {
        lock.withLock { everConnected = false }
    }

    func unreachable() {
        let now = Date()
        let due = lock.withLock { () -> Bool in
            let since = unreachableSince ?? now
            unreachableSince = since
            if everConnected && now.timeIntervalSince(since) < Self.graceAfterLoss { return false }
            if let lastStart, now.timeIntervalSince(lastStart) < Self.startInterval { return false }
            lastStart = now
            return true
        }
        if due { start() }
    }

    private var stateDir: String {
        if let dir = ProcessInfo.processInfo.environment["KANNA_STATE_DIR"], !dir.isEmpty { return dir }
        return (socketPath as NSString).deletingLastPathComponent
    }

    private func start() {
        let daemon = Self.helpers.appendingPathComponent("kanna-daemon").path
        let keeper = Self.helpers.appendingPathComponent("kanna-keeper").path
        guard FileManager.default.isExecutableFile(atPath: daemon) else {
            report("no daemon at \(daemon) to start")
            return
        }
        do {
            try FileManager.default.createDirectory(atPath: stateDir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        } catch {
            report("can't make \(stateDir): \(error.localizedDescription)")
            return
        }
        var environment = ProcessInfo.processInfo.environment.filter { name, _ in
            !(name.hasPrefix("KANNA_") || name.hasPrefix("KMUX_")) || Self.passedThrough.contains(name)
        }
        environment["KANNA_SOCKET"] = socketPath
        environment["KANNA_KEEPER"] = keeper
        if CommandLine.arguments.contains("--fresh") { environment["KANNA_FRESH"] = "1" }
        let log = (stateDir as NSString).appendingPathComponent("daemon.log")
        switch spawn(daemon, environment: environment, log: log) {
        case .success(let pid):
            report("started the daemon (pid \(pid)) on \(socketPath)")
            DispatchQueue.global(qos: .utility).async {
                var status: Int32 = 0
                while waitpid(pid, &status, 0) < 0 && errno == EINTR {}
            }
        case .failure(let error):
            report("can't start the daemon \(daemon): \(String(cString: strerror(error.code)))")
        }
    }

    private struct SpawnError: Error { let code: Int32 }

    private func spawn(_ path: String, environment: [String: String], log: String) -> Result<pid_t, SpawnError> {
        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID | POSIX_SPAWN_CLOEXEC_DEFAULT | POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK))
        var all = sigset_t()
        sigfillset(&all)
        posix_spawnattr_setsigdefault(&attributes, &all)
        var none = sigset_t()
        sigemptyset(&none)
        posix_spawnattr_setsigmask(&attributes, &none)

        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 1, log, O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW, 0o600)
        posix_spawn_file_actions_adddup2(&actions, 1, 2)

        let argv: [UnsafeMutablePointer<CChar>?] = [strdup(path), nil]
        let envp: [UnsafeMutablePointer<CChar>?] = environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer {
            argv.forEach { free($0) }
            envp.forEach { free($0) }
        }
        var pid: pid_t = 0
        let result = posix_spawn(&pid, path, &actions, &attributes, argv, envp)
        return result == 0 ? .success(pid) : .failure(SpawnError(code: result))
    }

    private func report(_ message: String) {
        FileHandle.standardError.write(Data("kanna: \(message)\n".utf8))
    }
}
