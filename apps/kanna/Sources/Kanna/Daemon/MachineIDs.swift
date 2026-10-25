import Foundation

enum MachineIDs {
    static let kinds: Set<Substring> = ["task", "ws", "win", "tab", "term", "diff", "file", "page", "ios"]
    static let fields: Set<String> = ["id", "pane", "terminal", "workspace", "tab", "window", "key", "focused", "zoomed", "shows", "planner", "content", "view", "held", "by", "task", "panes", "windows", "tasks", "about", "lead"]

    static func isID(_ text: Substring) -> Bool {
        guard let dash = text.firstIndex(of: "-"), kinds.contains(text[..<dash]) else { return false }
        let rest = text[text.index(after: dash)...]
        return !rest.isEmpty && rest.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber) }
    }

    static func machine(of id: String) -> String? {
        guard let at = id.lastIndex(of: "@"), isID(id[..<at]) else { return nil }
        return String(id[id.index(after: at)...])
    }

    static func bare(_ id: String) -> String {
        guard let at = id.lastIndex(of: "@"), isID(id[..<at]) else { return id }
        return String(id[..<at])
    }

    static func qualified(_ json: JSON, on machine: String) -> JSON {
        rewrite(json, machine, inside: false)
    }

    private static func rewrite(_ json: JSON, _ machine: String, inside: Bool) -> JSON {
        switch json {
        case .string(let text) where inside && isID(Substring(text)): return .string("\(text)@\(machine)")
        case .array(let items): return .array(items.map { rewrite($0, machine, inside: inside) })
        case .object(let object): return .object(Dictionary(uniqueKeysWithValues: object.map { ($0.key, rewrite($0.value, machine, inside: fields.contains($0.key))) }))
        default: return json
        }
    }

    static func local(_ json: JSON, on machine: String) -> JSON {
        switch json {
        case .string(let text) where self.machine(of: text) == machine: return .string(bare(text))
        case .array(let items): return .array(items.map { local($0, on: machine) })
        case .object(let object): return .object(object.mapValues { local($0, on: machine) })
        default: return json
        }
    }

    static func machine(in json: JSON) -> String? {
        switch json {
        case .string(let text): return machine(of: text)
        case .array(let items): return items.lazy.compactMap { machine(in: $0) }.first
        case .object(let object): return object.keys.sorted().lazy.compactMap { object[$0].flatMap { machine(in: $0) } }.first
        default: return nil
        }
    }

    static func location(_ path: String, on machine: String?) -> String {
        machine.map { "\($0):\(path)" } ?? path
    }

    static func split(location: String) -> (machine: String?, path: String) {
        guard !location.hasPrefix("/"), let colon = location.firstIndex(of: ":") else { return (nil, location) }
        return (String(location[..<colon]), String(location[location.index(after: colon)...]))
    }
}
