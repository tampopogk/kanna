import AppKit

@MainActor
enum FleetAlert {
    static func confirm(_ args: JSON) async -> Bool {
        let name = args["name"]?.string ?? "a machine"
        let key = args["key"]?.string ?? ""
        let addrs = (args["addrs"]?.array ?? []).compactMap(\.string)
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Add \(name) to Your Fleet?"
        var details = ["\(name) asked to join with the code you typed. Once added, it can run tasks and commands on your machines, and they on it."]
        details.append("Key: \(key)")
        if !addrs.isEmpty {
            details.append("Addresses: \(addrs.joined(separator: ", "))")
        }
        details.append("Add it only if you just ran kanna fleet pair on it.")
        alert.informativeText = details.joined(separator: "\n\n")
        alert.addButton(withTitle: "Add \(name)")
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
