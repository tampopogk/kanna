import AppKit
import CoreImage
import KannaMachines

@MainActor
final class PhonePairing {
    private let panel: NSPanel
    private let image: NSImageView
    private(set) static var shown: PhonePairing?

    var attached: Bool { panel.sheetParent != nil || panel.isVisible }

    var scanned: String? {
        guard let data = image.image?.tiffRepresentation, let picture = CIImage(data: data),
              let detector = CIDetector(ofType: CIDetectorTypeQRCode, context: nil) else { return nil }
        return (detector.features(in: picture).first as? CIQRCodeFeature)?.messageString
    }

    init(code: String) {
        let side: CGFloat = 320
        panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: side + 80, height: side + 130), styleMask: [.titled], backing: .buffered, defer: false)
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
        let title = NSTextField(labelWithString: "Scan this in Kanna on your phone")
        title.font = .systemFont(ofSize: 15, weight: .semibold)
        image = NSImageView(image: Self.qr(code, side: side))
        image.imageScaling = .scaleNone
        let note = NSTextField(labelWithString: "It works once, for 5 minutes.")
        note.textColor = .secondaryLabelColor
        let cancel = NSButton(title: "Cancel", target: nil, action: nil)
        cancel.keyEquivalent = "\u{1b}"
        for view in [title, image, note, cancel] { stack.addArrangedSubview(view) }
        panel.contentView = stack
        cancel.target = self
        cancel.action = #selector(cancelled)
    }

    static func qr(_ text: String, side: CGFloat) -> NSImage {
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return NSImage() }
        filter.setValue(Data(text.utf8), forKey: "inputMessage")
        filter.setValue("L", forKey: "inputCorrectionLevel")
        guard let code = filter.outputImage else { return NSImage() }
        let scale = max(1, floor(side / code.extent.width))
        let rep = NSCIImageRep(ciImage: code.transformed(by: CGAffineTransform(scaleX: scale, y: scale)))
        let image = NSImage(size: rep.size)
        image.addRepresentation(rep)
        return image
    }

    static func failed(_ message: String, over window: NSWindow?) {
        let alert = NSAlert()
        alert.messageText = "The phone wasn't added"
        alert.informativeText = message
        if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
    }

    func show(over window: NSWindow?) {
        Self.shown = self
        if let window { window.beginSheet(panel) } else { panel.makeKeyAndOrderFront(nil) }
    }

    func close() {
        panel.sheetParent?.endSheet(panel)
        panel.orderOut(nil)
        if Self.shown === self { Self.shown = nil }
    }

    @objc func cancelled() { close() }
}

extension AppController {
    func offline(_ machine: String?) -> Bool {
        guard let machine else { return false }
        return links?.remotes[machine]?.attached != true
    }

    func incompatible(_ machine: String) -> Bool {
        offline(machine) && links?.failure(of: machine)?["code"]?.string == "other_protocol"
    }

    func pathLabel(_ machine: String?) -> String {
        guard let machine else { return "This Mac" }
        if incompatible(machine) { return "Incompatible" }
        guard !offline(machine), let path = links?.path(of: machine) else { return "Offline" }
        switch (path["path"]?.string, path["lan"]?.bool) {
        case ("relay"?, _): return "Relayed"
        case ("direct"?, true?): return "Direct · LAN"
        case ("direct"?, _): return "Direct · internet"
        default: return "Direct"
        }
    }

    func machineName(_ machine: String?) -> String {
        machine ?? links?.name ?? "this Mac"
    }

    func placeText(_ id: String) -> String {
        let machine = MachineIDs.machine(of: id)
        return "\(machineName(machine)) · \(pathLabel(machine))"
    }

    func staleText(_ id: String?) -> String? {
        guard let machine = id.flatMap(MachineIDs.machine(of:)), offline(machine) else { return nil }
        let since = lastSeen[machine].map { " \(DeskTask.ago($0.timeIntervalSince1970))" } ?? ""
        return "Last received\(since) · input is unavailable · reconnecting"
    }

    func seen(_ machine: String?) {
        guard let machine else { return }
        lastSeen[machine] = Date()
    }

    private static func platform(_ capabilities: JSON?) -> String? {
        guard let capabilities, let os = capabilities["os"]?.string else { return nil }
        let arch = capabilities["arch"]?.string
        let system = os == "macos" ? "macOS" : os == "linux" ? "Linux" : os
        let chip = os == "macos" && arch == "aarch64" ? "Apple Silicon" : arch
        return [system, chip].compactMap { $0 }.joined(separator: " · ")
    }

    var phoneNames: [String] {
        (links?.phones ?? []).filter { $0["revoked"]?.bool != true }.compactMap { $0["name"]?.string }.sorted()
    }

    func pairPhone(over window: NSWindow?) {
        request("phone", ["action": "offer"]) { [weak self] offer in
            guard let self, offer["ok"]?.bool == true, let code = offer["code"]?.string, let key = offer["key"]?.string,
                  let body = try? JSONSerialization.data(withJSONObject: ["code": code, "key": key, "addrs": (offer["addrs"]?.array ?? []).compactMap(\.string)]) else {
                PhonePairing.failed(offer["error"]?["message"]?.string ?? "Kanna couldn't make a code.", over: window)
                return
            }
            let sheet = PhonePairing(code: String(decoding: body, as: UTF8.self))
            sheet.show(over: window)
            request("phone", ["action": "wait"]) { [weak self] reply in
                let waiting = PhonePairing.shown === sheet
                sheet.close()
                if waiting, reply["ok"]?.bool != true {
                    PhonePairing.failed(reply["error"]?["message"]?.string ?? "The phone wasn't added.", over: window)
                }
                self?.links?.refresh()
            }
        }
    }

    func revokePhone(_ name: String, over window: NSWindow?) {
        let alert = NSAlert()
        alert.messageText = "Revoke \(name)?"
        alert.informativeText = "It can no longer reach your machines. To use it again, pair it again."
        alert.addButton(withTitle: "Revoke")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        request("machine", ["action": "revoke", "host": .string(name)]) { [weak self] _ in self?.links?.refresh() }
    }

    var machineRows: [MachineRow] {
        guard let links else { return [] }
        let local = Self.platform(links.local.daemonInfo?["capabilities"]).map { "This Mac · \($0)" } ?? "This Mac"
        var rows = [MachineRow(name: links.name ?? "This Mac", detail: local, path: "This Mac", note: "Its own tasks and planners",
                                 explanation: "Kanna.app's own machine: the app talks to its daemon on this Mac's socket.")]
        for peer in links.peers.sorted(by: { ($0["name"]?.string ?? "") < ($1["name"]?.string ?? "") }) {
            guard let name = peer["name"]?.string, peer["trusted"]?.bool == true, peer["revoked"]?.bool != true else { continue }
            let link = links.remotes[name]
            let path = pathLabel(name)
            let route = links.path(of: name)
            let rtt = route["rtt_ms"]?.number.map { String(format: "%.0f ms round trip", $0) }
            let remote = route["remote"]?.string
            let detail = Self.platform(link?.daemonInfo?["capabilities"]) ?? "Not reached yet"
            let note: String
            let explanation: String
            switch path {
            case "Relayed":
                note = "Via your relay · encrypted"
                explanation = "\(name) can't be reached directly now, so its encrypted traffic goes through your relay. It moves to a direct path by itself as soon as one opens."
            case "Incompatible":
                let refusal = link?.refusal
                let message = Protocols.otherProtocol(machine: name, protocol: refusal?["protocol"]?.number.map(Int.init), version: refusal?["version"]?.string, commit: refusal?["commit"]?.string)
                note = message.title
                explanation = "\(message.detail) Until then, \(name)'s tasks keep their places on the Desk."
            case "Offline":
                let failure = links.failure(of: name)
                note = lastSeen[name].map { "Last seen \(DeskTask.ago($0.timeIntervalSince1970))" } ?? "Not reached since Kanna started"
                explanation = "Kanna can't reach \(name) now\(failure?["message"]?.string.map { ": \($0)" } ?? ""). It keeps trying, and \(name)'s tasks keep their places on the Desk."
            case "Direct · internet":
                note = rtt ?? "No relay in use"
                explanation = "Connected directly over the internet\(remote.map { " (\($0))" } ?? ""), with no relay carrying the traffic."
            default:
                note = rtt.map { "No relay in use · \($0)" } ?? "No relay in use"
                explanation = "Connected directly on your local network\(remote.map { " (\($0))" } ?? "")."
            }
            rows.append(MachineRow(name: name, detail: detail, path: path, note: note, explanation: explanation))
        }
        return rows
    }
}
