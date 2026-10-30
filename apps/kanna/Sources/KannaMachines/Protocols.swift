public enum Protocols {
    public static let mine: [Int] = [2, 2]

    public static func common(_ mine: [Int], _ theirs: [Int]) -> Int? {
        guard mine.count == 2, theirs.count == 2 else { return nil }
        let top = min(mine[1], theirs[1])
        return top >= max(mine[0], theirs[0]) ? top : nil
    }

    public static func otherProtocol(machine: String, protocols: [Int]?, version: String?, commit: String?, mine: [Int] = mine) -> (title: String, detail: String) {
        let theirs = protocols.flatMap { $0.count == 2 && $0[0] <= $0[1] ? $0 : nil } ?? [1, 1]
        var runs = "an older Kanna"
        if let version {
            runs = version
            if let commit { runs += " (\(short(commit)))" }
        }
        let facts = "It runs \(runs), protocol \(span(theirs)); this Kanna speaks \(span(mine))."
        if theirs[1] < mine[0] {
            return ("\(machine) runs an older Kanna", "kanna machine update \(machine) updates it. \(facts)")
        }
        return ("\(machine) runs a newer Kanna", "Update this Kanna, or run kanna machine update on \(machine). \(facts)")
    }

    static func short(_ commit: String) -> String {
        String(commit.prefix(8)) + (commit.hasSuffix("-dirty") ? "-dirty" : "")
    }

    static func span(_ p: [Int]) -> String {
        p[0] == p[1] ? "\(p[0])" : "\(p[0])–\(p[1])"
    }
}
