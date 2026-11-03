import AppKit

@MainActor
enum MachineAlert {
    static func confirm(_ args: JSON) async -> Bool {
        let name = args["name"]?.string ?? "a machine"
        let key = args["key"]?.string ?? ""
        guard !key.isEmpty else { return false }
        let addrs = (args["addrs"]?.array ?? []).compactMap(\.string)
        let hosted = args["hosted"]?.bool ?? false
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = hosted ? "Add \(name) (Kanna Cloud) to Your Machines?" : "Add \(name) to Your Machines?"
        var details = hosted
            ? ["\(name) is a VM Kanna Cloud made for you, enrolling with its one-time token. Once added, it can run tasks and commands on your machines, and they on it. Its operator can see what runs there, and it can never add or revoke other machines."]
            : ["\(name) asked to join with the code you typed. Once added, it can run tasks and commands on your machines, and they on it."]
        details.append("Key: \(key)")
        if !addrs.isEmpty {
            details.append("Addresses: \(addrs.joined(separator: ", "))")
        }
        details.append(hosted ? "Add it only if you just ran kanna cloud provision." : "Add it only if you just ran kanna machine pair on it.")
        alert.informativeText = details.joined(separator: "\n\n")
        alert.addButton(withTitle: "Add Machine")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        return await withCheckedContinuation { continuation in
            if let window = NSApp.keyWindow ?? NSApp.mainWindow {
                alert.beginSheetModal(for: window) { response in continuation.resume(returning: response == .alertFirstButtonReturn) }
            } else {
                continuation.resume(returning: alert.runModal() == .alertFirstButtonReturn)
            }
        }
    }
}
