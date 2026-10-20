import Darwin
import Foundation

final class DaemonConnection: @unchecked Sendable {
    struct Delivery: Sendable {
        let generation: Int
        let size: Int
    }

    let path: String
    private let hello: @MainActor @Sendable () -> JSON
    private let connected: @MainActor @Sendable () -> Void
    private let received: @MainActor @Sendable (JSON, Delivery) -> Void
    private let disconnected: @MainActor @Sendable () -> Void
    private let unreachable: @Sendable () -> Void
    private let writes = DispatchQueue(label: "kanna.daemon.write")
    private let lock = NSCondition()
    private var fd: Int32 = -1
    private var stopped = false
    private var generation = 0
    private var inFlight = 0
    private var retryDelay = DaemonConnection.reconnectDelay

    private static let inFlightLimit = 8 << 20
    private static let reconnectDelay: TimeInterval = 0.5
    private static let refusedDelayLimit: TimeInterval = 30

    init(path: String, hello: @escaping @MainActor @Sendable () -> JSON, connected: @escaping @MainActor @Sendable () -> Void,
         received: @escaping @MainActor @Sendable (JSON, Delivery) -> Void, disconnected: @escaping @MainActor @Sendable () -> Void,
         unreachable: @escaping @Sendable () -> Void = {}) {
        self.path = path
        self.hello = hello
        self.connected = connected
        self.received = received
        self.disconnected = disconnected
        self.unreachable = unreachable
    }

    func start() {
        let thread = Thread { [self] in run() }
        thread.name = "kanna.daemon.read"
        thread.start()
    }

    func stop() {
        lock.withLock {
            stopped = true
            if fd >= 0 { shutdown(fd, SHUT_RDWR) }
            lock.broadcast()
        }
    }

    func send(_ message: JSON, sent: (@Sendable () -> Void)? = nil) { send(message, generation: nil, sent: sent) }

    func send(_ message: JSON, generation: Int?, sent: (@Sendable () -> Void)? = nil) {
        var encoded = message.encoded()
        encoded.append(UInt8(ascii: "\n"))
        let line = encoded
        writes.async { [self] in
            let fd = lock.withLock { generation == nil || generation == self.generation ? self.fd : -1 }
            if fd >= 0 { _ = Self.writeAll(fd, line) }
            sent?()
        }
    }

    func consumed(_ delivery: Delivery) {
        lock.withLock {
            guard delivery.generation == generation else { return }
            inFlight -= delivery.size
            lock.broadcast()
        }
    }

    func accepted() {
        lock.withLock { retryDelay = Self.reconnectDelay }
    }

    func refused() {
        lock.withLock { retryDelay = min(max(retryDelay * 2, 1), Self.refusedDelayLimit) }
    }

    private func run() {
        while !lock.withLock({ stopped }) {
            guard let socket = Self.connect(path) else {
                unreachable()
                Thread.sleep(forTimeInterval: 0.2)
                continue
            }
            let hello = hello
            var line = DispatchQueue.main.sync { MainActor.assumeIsolated { hello() } }.encoded()
            line.append(UInt8(ascii: "\n"))
            guard Self.writeAll(socket, line) else {
                close(socket)
                Thread.sleep(forTimeInterval: 0.2)
                continue
            }
            let current = lock.withLock {
                fd = socket
                generation += 1
                inFlight = 0
                return generation
            }
            let connected = connected
            DispatchQueue.main.async { MainActor.assumeIsolated { connected() } }
            readLines(socket, generation: current)
            writes.sync { lock.withLock { fd = -1 } }
            close(socket)
            let disconnected = disconnected
            DispatchQueue.main.async { MainActor.assumeIsolated { disconnected() } }
            sleepUntilRetry()
        }
    }

    private func sleepUntilRetry() {
        lock.withLock {
            let until = Date(timeIntervalSinceNow: retryDelay)
            while !stopped && lock.wait(until: until) {}
        }
    }

    private func readLines(_ socket: Int32, generation: Int) {
        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 65536)
        while true {
            let count = read(socket, &chunk, chunk.count)
            if count < 0 && errno == EINTR { continue }
            if count <= 0 { return }
            buffer.append(chunk, count: count)
            while let newline = buffer.firstIndex(of: UInt8(ascii: "\n")) {
                let line = Data(buffer[buffer.startIndex..<newline])
                buffer.removeSubrange(buffer.startIndex...newline)
                guard let message = try? JSON.parse(line) else {
                    FileHandle.standardError.write(Data("kanna: the daemon sent a line that isn't JSON\n".utf8))
                    continue
                }
                let delivery = Delivery(generation: generation, size: line.count)
                guard waitForRoom(delivery) else { return }
                let received = received
                DispatchQueue.main.async { MainActor.assumeIsolated { received(message, delivery) } }
            }
        }
    }

    private func waitForRoom(_ delivery: Delivery) -> Bool {
        lock.withLock {
            while !stopped && inFlight > 0 && inFlight + delivery.size > Self.inFlightLimit { lock.wait() }
            inFlight += delivery.size
            return !stopped
        }
    }

    private static func connect(_ path: String) -> Int32? {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return nil }
        var address = SocketServer.address(path)
        let ok = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        } == 0
        guard ok else {
            close(fd)
            return nil
        }
        var noSigPipe: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))
        return fd
    }

    private static func writeAll(_ fd: Int32, _ data: Data) -> Bool {
        data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let written = write(fd, raw.baseAddress! + offset, raw.count - offset)
                if written < 0 {
                    if errno == EINTR { continue }
                    return false
                }
                offset += written
            }
            return true
        }
    }
}
