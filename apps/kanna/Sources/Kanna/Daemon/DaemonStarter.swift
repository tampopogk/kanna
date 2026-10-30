import Darwin
import Foundation

final class DaemonStarter: @unchecked Sendable {
    let socketPath: String
    private let lock = NSLock()
    private var listener: Int32 = -1
    private var isAttached = false
    private var lastStart: Date?
    private var scheduled = false
    private var quit = false

    private static let startInterval: TimeInterval = 10
    private static let passedThrough: Set<String> = ["KANNA_STATE_DIR", "KANNA_DEBUG", "KANNA_AGENTS_HOME", "KANNA_NO_STATE", "KANNA_FRESH", "KANNA_E2E_OWNER"]
    private static let listenerDescriptor: Int32 = 3

    init(socketPath: String) {
        self.socketPath = socketPath
    }

    static var helpers: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers") }
    private static var daemon: String { helpers.appendingPathComponent("kanna-daemon").path }

    func attached() {
        lock.withLock { isAttached = true }
    }

    func keep(_ fd: Int32) {
        lock.withLock {
            if listener >= 0 && listener != fd { close(listener) }
            listener = fd
        }
    }

    func quitting() {
        lock.withLock { quit = true }
    }

    func lost() {
        lock.withLock { isAttached = false }
        startWhenDue()
    }

    func unreachable() {
        lock.withLock {
            isAttached = false
            if listener >= 0 {
                close(listener)
                listener = -1
            }
        }
        if !lock.withLock({ quit }), FileManager.default.isExecutableFile(atPath: Self.daemon) { _ = socketListener() }
        startWhenDue()
    }

    private func startWhenDue() {
        let now = Date()
        let wait = lock.withLock { () -> TimeInterval? in
            if isAttached || quit { return nil }
            if let lastStart, now.timeIntervalSince(lastStart) < Self.startInterval {
                if scheduled { return nil }
                scheduled = true
                return Self.startInterval - now.timeIntervalSince(lastStart)
            }
            return 0
        }
        guard let wait else { return }
        if wait > 0 {
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + wait) { [self] in
                lock.withLock { scheduled = false }
                startWhenDue()
            }
            return
        }
        start()
    }

    private func socketListener() -> Int32? {
        if let held = lock.withLock({ listener >= 0 ? listener : nil }) { return held }
        if SocketServer.isListening(socketPath) { return nil }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            report("can't make a socket for \(socketPath): \(String(cString: strerror(errno)))")
            return nil
        }
        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
        unlink(socketPath)
        var address = SocketServer.address(socketPath)
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        } == 0
        guard bound, chmod(socketPath, 0o600) == 0, listen(fd, SOMAXCONN) == 0 else {
            report("can't listen on \(socketPath): \(String(cString: strerror(errno)))")
            close(fd)
            return nil
        }
        keep(fd)
        return fd
    }

    private var stateDir: String {
        if let dir = ProcessInfo.processInfo.environment["KANNA_STATE_DIR"], !dir.isEmpty { return dir }
        return (socketPath as NSString).deletingLastPathComponent
    }

    private func start() {
        let daemon = Self.daemon
        let keeper = Self.helpers.appendingPathComponent("kanna-keeper").path
        guard FileManager.default.isExecutableFile(atPath: daemon) else {
            report("no daemon at \(daemon) to start")
            return
        }
        do {
            for folder in [stateDir, (socketPath as NSString).deletingLastPathComponent] {
                try FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            }
        } catch {
            report("can't make \(stateDir): \(error.localizedDescription)")
            return
        }
        guard let listener = socketListener() else { return }
        lock.withLock { lastStart = Date() }
        var environment = ProcessInfo.processInfo.environment.filter { name, _ in
            !(name.hasPrefix("KANNA_") || name.hasPrefix("KMUX_")) || Self.passedThrough.contains(name)
        }
        environment["KANNA_SOCKET"] = socketPath
        environment["KANNA_KEEPER"] = keeper
        if CommandLine.arguments.contains("--fresh") { environment["KANNA_FRESH"] = "1" }
        environment["KANNA_LISTEN_FD"] = String(Self.listenerDescriptor)
        let log = (stateDir as NSString).appendingPathComponent("daemon.log")
        switch spawn(daemon, environment: environment, log: log, listener: listener) {
        case .success(let pid):
            report("started the daemon (pid \(pid)) on \(socketPath)")
            DispatchQueue.global(qos: .utility).async { [self] in
                var status: Int32 = 0
                while waitpid(pid, &status, 0) < 0 && errno == EINTR {}
                startWhenDue()
            }
        case .failure(let error):
            report("can't start the daemon \(daemon): \(String(cString: strerror(error.code)))")
        }
    }

    private struct SpawnError: Error { let code: Int32 }

    private func spawn(_ path: String, environment: [String: String], log: String, listener: Int32) -> Result<pid_t, SpawnError> {
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
        if listener == Self.listenerDescriptor {
            posix_spawn_file_actions_addinherit_np(&actions, listener)
        } else {
            posix_spawn_file_actions_adddup2(&actions, listener, Self.listenerDescriptor)
        }

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
