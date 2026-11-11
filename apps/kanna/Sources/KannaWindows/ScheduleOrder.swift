import Foundation

public enum ScheduleOrder {
    public static func groups<T>(_ items: [T], repo: (T) -> String?, here: String?) -> [(repo: String?, items: [T])] {
        var kept: [String?: [T]] = [:]
        for item in items { kept[repo(item), default: []].append(item) }
        let name = { (path: String) in (path as NSString).lastPathComponent.lowercased() }
        let keys = kept.keys.sorted { a, b in
            switch (a, b) {
            case (nil, _): return false
            case (_, nil): return true
            case (let a?, let b?):
                if (a == here) != (b == here) { return a == here }
                return name(a) != name(b) ? name(a) < name(b) : a < b
            }
        }
        return keys.map { (repo: $0, items: kept[$0] ?? []) }
    }
}
