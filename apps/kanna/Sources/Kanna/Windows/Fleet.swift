import AppKit

extension AppController {
    func offline(_ machine: String?) -> Bool {
        guard let machine else { return false }
        return links?.remotes[machine]?.attached != true
    }

    func pathLabel(_ machine: String?) -> String {
        guard let machine else { return "This Mac" }
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
        let since = lastSeen[machine].map { " \(ShelfTask.ago($0.timeIntervalSince1970))" } ?? ""
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
        let cores = capabilities["cores"]?.number.map { "\(Int($0)) cores" }
        return [system, chip, cores].compactMap { $0 }.joined(separator: " · ")
    }

    var fleetMachines: [FleetMachine] {
        guard let links else { return [] }
        let local = Self.platform(links.local.daemonInfo?["capabilities"]).map { "This Mac · \($0)" } ?? "This Mac"
        var rows = [FleetMachine(name: links.name ?? "This Mac", detail: local, path: "This Mac", note: "Its own tasks and planners",
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
            case "Offline":
                let failure = links.failure(of: name)
                if failure?["code"]?.string == "other_protocol" {
                    note = "Speaks another protocol: it updates itself"
                } else {
                    note = lastSeen[name].map { "Last seen \(ShelfTask.ago($0.timeIntervalSince1970))" } ?? "Not reached since Kanna started"
                }
                explanation = "Kanna can't reach \(name) now\(failure?["message"]?.string.map { ": \($0)" } ?? ""). It keeps trying, and \(name)'s tasks keep their places on the Shelf."
            case "Direct · internet":
                note = rtt ?? "No relay in use"
                explanation = "Connected directly over the internet\(remote.map { " (\($0))" } ?? ""), with no relay carrying the traffic."
            default:
                note = rtt.map { "No relay in use · \($0)" } ?? "No relay in use"
                explanation = "Connected directly on your local network\(remote.map { " (\($0))" } ?? "")."
            }
            rows.append(FleetMachine(name: name, detail: detail, path: path, note: note, explanation: explanation))
        }
        return rows
    }
}
