import AppKit

struct AgentChoice: Equatable {
    let pane: String
    let name: String
    let detail: String
}

extension AppController {
    func turnOnComments(_ pane: String, under button: NSView?) {
        request("markup", ["pane": .string(pane), "route": true]) { [weak self] answer in
            guard let self, answer["error"]?["code"]?.string == "no_agent" else { return }
            askForAgent(pane, under: button)
        }
    }

    var runningAgents: [AgentChoice] {
        var out: [AgentChoice] = []
        let running = { [agentStatuses] (pane: String) in agentStatuses.statuses[pane].map { $0 != "exited" } ?? false }
        for entry in deskEntries {
            guard let pane = entry["task"]?.string, running(pane) else { continue }
            out.append(AgentChoice(pane: pane, name: taskTitle(pane, name: entry["name"]?.string), detail: "Desk · \(ShelfTask.program(entry["agent"]?.string) ?? "agent")"))
        }
        for entry in shelf where entry["running"]?.bool == true {
            guard let pane = entry["held"]?.string, running(pane), !out.contains(where: { $0.pane == pane }) else { continue }
            out.append(AgentChoice(pane: pane, name: entry["name"]?.string ?? taskTitle(pane, name: nil), detail: "Shelf · \(ShelfTask.program(entry["agent"]?.string) ?? "agent")"))
        }
        for ws in published["workspaces"]?.array ?? [] where ws["where"]?["planner"] != nil {
            guard let pane = ws["dock"]?["pane"]?.string, running(pane) else { continue }
            out.append(AgentChoice(pane: pane, name: "Planner", detail: projectName([pane])))
        }
        return out
    }

    func agentName(_ pane: String) -> String {
        if let entry = deskEntries.first(where: { $0["task"]?.string == pane }) { return taskTitle(pane, name: entry["name"]?.string) }
        if let entry = shelf.first(where: { $0["held"]?.string == pane }), let name = entry["name"]?.string { return name }
        if published["workspaces"]?.array?.contains(where: { $0["where"]?["planner"] != nil && $0["dock"]?["pane"]?.string == pane }) == true { return "Planner" }
        return pane
    }

    func askForAgent(_ pane: String, under button: NSView?) {
        let menu = NSMenu()
        menu.autoenablesItems = false
        let header = NSMenuItem(title: "Send comments to", action: nil, keyEquivalent: "")
        header.isEnabled = false
        menu.addItem(header)
        let choices = runningAgents
        if choices.isEmpty {
            let none = NSMenuItem(title: "No agent is running", action: nil, keyEquivalent: "")
            none.isEnabled = false
            menu.addItem(none)
        }
        for (index, choice) in choices.enumerated() {
            if index > 0, choice.name == "Planner", choices[index - 1].name != "Planner" { menu.addItem(.separator()) }
            let item = CommentsAgentItem(choice: choice) { [weak self] in
                self?.request("markup", ["pane": .string(pane), "agent": .string(choice.pane)])
            }
            menu.addItem(item)
        }
        if debugCommands {
            agentMenu = (pane, choices)
            return
        }
        if let button {
            menu.popUp(positioning: nil, at: NSPoint(x: 0, y: button.isFlipped ? button.bounds.maxY + 4 : -4), in: button)
        } else if let window = NSApp.keyWindow, let view = window.contentView {
            menu.popUp(positioning: nil, at: view.convert(window.mouseLocationOutsideOfEventStream, from: nil), in: view)
        }
    }
}

final class CommentsAgentItem: NSMenuItem {
    private let pick: () -> Void

    init(choice: AgentChoice, pick: @escaping () -> Void) {
        self.pick = pick
        super.init(title: choice.name, action: #selector(picked), keyEquivalent: "")
        target = self
        let title = NSMutableAttributedString(string: choice.name, attributes: [.font: NSFont.menuFont(ofSize: 0)])
        title.append(NSAttributedString(string: "   \(choice.detail)", attributes: [.font: NSFont.menuFont(ofSize: 0), .foregroundColor: NSColor.secondaryLabelColor]))
        attributedTitle = title
    }

    required init(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    @objc private func picked() { pick() }
}
