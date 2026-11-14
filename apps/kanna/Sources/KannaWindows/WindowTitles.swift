import Foundation

public enum WindowTitles {
    public static let separator = " — "

    public static func projectNames(_ folders: [String], home: String) -> [String: String] {
        var names = Dictionary(uniqueKeysWithValues: Set(folders).map { ($0, Name($0, home: home)) })
        while true {
            var grew = false
            for (_, same) in Dictionary(grouping: names.keys, by: { names[$0]?.text ?? "" }) where same.count > 1 {
                let machines = Set(same.map { names[$0]?.machine })
                for folder in same {
                    if machines.count > 1, names[folder]?.machine != nil, names[folder]?.showsMachine == false {
                        names[folder]?.showsMachine = true
                        grew = true
                    } else if machines.count == 1, names[folder]?.grow() == true {
                        grew = true
                    }
                }
            }
            if !grew { break }
        }
        return names.mapValues(\.text)
    }

    public static func title(_ parts: String?...) -> String {
        parts.compactMap { $0 }.joined(separator: separator)
    }

    struct Name {
        let machine: String?
        let parts: [String]
        var depth = 1
        var showsMachine = false

        init(_ location: String, home: String) {
            let (machine, path) = Self.split(location)
            self.machine = machine
            if machine == nil, path == home {
                parts = ["~"]
            } else if machine == nil, path.hasPrefix(home + "/") {
                parts = ["~"] + path.dropFirst(home.count + 1).split(separator: "/").map(String.init)
            } else {
                let split = path.split(separator: "/").map(String.init)
                parts = split.isEmpty ? ["/"] : split
            }
        }

        var text: String {
            let path = parts.suffix(depth).joined(separator: "/")
            return showsMachine ? "\(machine ?? ""):\(path)" : path
        }

        mutating func grow() -> Bool {
            guard depth < parts.count else { return false }
            depth += 1
            return true
        }

        static func split(_ location: String) -> (machine: String?, path: String) {
            guard !location.hasPrefix("/"), let colon = location.firstIndex(of: ":") else { return (nil, location) }
            return (String(location[..<colon]), String(location[location.index(after: colon)...]))
        }
    }
}
