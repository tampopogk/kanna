import Foundation
import Virtualization

struct TartConfig: Decodable {
    let ecid: String
    let hardwareModel: String
}

let args = CommandLine.arguments
if args.count == 3, args[1] == "new-identity" {
    let url = URL(fileURLWithPath: args[2]).appendingPathComponent("config.json")
    var json = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
    json["ecid"] = VZMacMachineIdentifier().dataRepresentation.base64EncodedString()
    try JSONSerialization.data(withJSONObject: json, options: [.prettyPrinted, .sortedKeys]).write(to: url)
    exit(0)
}
guard args.count == 5, let cpus = Int(args[3]), let memoryGiB = UInt64(args[4]) else {
    FileHandle.standardError.write("usage: vmrun VM_DIR SOCKET_DIR CPUS MEMORY_GIB\n".data(using: .utf8)!)
    exit(64)
}
let vmDir = URL(fileURLWithPath: args[1])
let socketDir = URL(fileURLWithPath: args[2])
let agentPort: UInt32 = 7000
let egressPort: UInt32 = 7001

func log(_ line: String) {
    FileHandle.standardError.write((ISO8601DateFormatter().string(from: Date()) + " " + line + "\n").data(using: .utf8)!)
}

func splice(_ a: Int32, _ b: Int32, onClose: @escaping () -> Void) {
    let group = DispatchGroup()
    for (from, to) in [(a, b), (b, a)] {
        group.enter()
        Thread.detachNewThread {
            var buffer = [UInt8](repeating: 0, count: 65536)
            while true {
                let n = read(from, &buffer, buffer.count)
                if n <= 0 { break }
                var offset = 0
                while offset < n {
                    let w = buffer.withUnsafeBytes { write(to, $0.baseAddress! + offset, n - offset) }
                    if w <= 0 { break }
                    offset += w
                }
                if offset < n { break }
            }
            shutdown(to, SHUT_WR)
            group.leave()
        }
    }
    group.notify(queue: .global()) { onClose() }
}

func unixAddress(_ path: String) -> sockaddr_un {
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    withUnsafeMutableBytes(of: &address.sun_path) { raw in
        let bytes = Array(path.utf8)
        precondition(bytes.count < raw.count, "socket path too long")
        raw.copyBytes(from: bytes)
    }
    return address
}

func listenUnix(_ path: String) -> Int32 {
    unlink(path)
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    var address = unixAddress(path)
    let bound = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
    }
    precondition(bound == 0, "bind \(path) failed: \(errno)")
    chmod(path, 0o600)
    precondition(listen(fd, 16) == 0)
    return fd
}

func connectUnix(_ path: String) -> Int32? {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    var address = unixAddress(path)
    let connected = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
    }
    if connected != 0 { close(fd); return nil }
    return fd
}

let configData = try Data(contentsOf: vmDir.appendingPathComponent("config.json"))
let tart = try JSONDecoder().decode(TartConfig.self, from: configData)
guard let hardwareData = Data(base64Encoded: tart.hardwareModel),
      let hardware = VZMacHardwareModel(dataRepresentation: hardwareData), hardware.isSupported,
      let ecidData = Data(base64Encoded: tart.ecid),
      let machine = VZMacMachineIdentifier(dataRepresentation: ecidData) else {
    log("unsupported or malformed hardware model / machine identifier")
    exit(65)
}

let platform = VZMacPlatformConfiguration()
platform.hardwareModel = hardware
platform.machineIdentifier = machine
platform.auxiliaryStorage = VZMacAuxiliaryStorage(url: vmDir.appendingPathComponent("nvram.bin"))

let graphics = VZMacGraphicsDeviceConfiguration()
graphics.displays = [VZMacGraphicsDisplayConfiguration(widthInPixels: 1440, heightInPixels: 900, pixelsPerInch: 80)]

let disk = try VZDiskImageStorageDeviceAttachment(url: vmDir.appendingPathComponent("disk.img"), readOnly: false,
                                                  cachingMode: .automatic, synchronizationMode: .full)

let config = VZVirtualMachineConfiguration()
config.platform = platform
config.bootLoader = VZMacOSBootLoader()
config.cpuCount = cpus
config.memorySize = memoryGiB * 1024 * 1024 * 1024
config.graphicsDevices = [graphics]
config.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: disk)]
config.keyboards = [VZUSBKeyboardConfiguration()]
config.pointingDevices = [VZUSBScreenCoordinatePointingDeviceConfiguration()]
config.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
config.socketDevices = [VZVirtioSocketDeviceConfiguration()]
var pair: [Int32] = [0, 0]
precondition(socketpair(AF_UNIX, SOCK_DGRAM, 0, &pair) == 0)
let nullNic = VZVirtioNetworkDeviceConfiguration()
nullNic.macAddress = VZMACAddress.randomLocallyAdministered()
nullNic.attachment = VZFileHandleNetworkDeviceAttachment(fileHandle: FileHandle(fileDescriptor: pair[0], closeOnDealloc: false))
let sink = pair[1]
var droppedFrames = 0
var droppedBytes = 0
Thread.detachNewThread {
    var buffer = [UInt8](repeating: 0, count: 65536)
    while true {
        let n = read(sink, &buffer, buffer.count)
        if n <= 0 { break }
        droppedFrames += 1
        droppedBytes += n
    }
}
config.networkDevices = [nullNic]
config.directorySharingDevices = []
config.audioDevices = []
config.serialPorts = []
config.consoleDevices = []
config.memoryBalloonDevices = []
try config.validate()

let summary: [String: Any] = [
    "cpus": cpus, "memory_gib": memoryGiB,
    "network_devices": config.networkDevices.count,
    "network": "one virtio NIC on a host socketpair; the host reads and discards every frame",
    "directory_sharing_devices": config.directorySharingDevices.count,
    "audio_devices": config.audioDevices.count,
    "serial_ports": config.serialPorts.count,
    "console_devices": config.consoleDevices.count,
    "socket_devices": config.socketDevices.count,
    "storage_devices": config.storageDevices.count,
    "usb_controllers": config.usbControllers.count,
    "graphics": "1440x900 headless, no view",
    "vsock_host_listen_ports": [egressPort],
    "vsock_guest_connect_ports": [agentPort],
]
try JSONSerialization.data(withJSONObject: summary, options: [.prettyPrinted, .sortedKeys])
    .write(to: socketDir.appendingPathComponent("vm-config.json"))

final class EgressListener: NSObject, VZVirtioSocketListenerDelegate {
    func listener(_ listener: VZVirtioSocketListener, shouldAcceptNewConnection connection: VZVirtioSocketConnection,
                  from socketDevice: VZVirtioSocketDevice) -> Bool {
        let guestFd = dup(connection.fileDescriptor)
        guard let hostFd = connectUnix(socketDir.appendingPathComponent("egress.sock").path) else {
            close(guestFd)
            return false
        }
        splice(guestFd, hostFd) { close(guestFd); close(hostFd) }
        return true
    }
}

let stats = DispatchSource.makeTimerSource(queue: .main)
stats.schedule(deadline: .now() + 60, repeating: 60)
stats.setEventHandler { log("null nic dropped \(droppedFrames) frames, \(droppedBytes) bytes") }
stats.resume()

final class Delegate: NSObject, VZVirtualMachineDelegate {
    func guestDidStop(_ virtualMachine: VZVirtualMachine) { log("guest stopped; null nic dropped \(droppedFrames) frames"); exit(0) }
    func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: Error) {
        log("guest error: \(error)"); exit(70)
    }
}

let vm = VZVirtualMachine(configuration: config)
let delegate = Delegate()
let egress = EgressListener()
vm.delegate = delegate

signal(SIGTERM, SIG_IGN)
let term = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
term.setEventHandler {
    log("SIGTERM: requesting guest stop")
    if vm.canRequestStop { try? vm.requestStop() }
    DispatchQueue.main.asyncAfter(deadline: .now() + 60) {
        vm.stop { _ in exit(0) }
    }
}
term.resume()
signal(SIGINT, SIG_IGN)
let kill = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
kill.setEventHandler { vm.stop { _ in exit(0) } }
kill.resume()

vm.start { result in
    switch result {
    case .failure(let error):
        log("start failed: \(error)")
        exit(71)
    case .success:
        log("guest started")
        let device = vm.socketDevices.first as! VZVirtioSocketDevice
        let listener = VZVirtioSocketListener()
        listener.delegate = egress
        device.setSocketListener(listener, forPort: egressPort)
        let agentSock = listenUnix(socketDir.appendingPathComponent("agent.sock").path)
        Thread.detachNewThread {
            while true {
                let client = accept(agentSock, nil, nil)
                if client < 0 { continue }
                DispatchQueue.main.async {
                    device.connect(toPort: agentPort) { result in
                        switch result {
                        case .success(let connection):
                            let guestFd = dup(connection.fileDescriptor)
                            splice(client, guestFd) { close(client); close(guestFd); _ = connection }
                        case .failure(let error):
                            log("agent connect failed: \(error)")
                            close(client)
                        }
                    }
                }
            }
        }
    }
}
RunLoop.main.run()
