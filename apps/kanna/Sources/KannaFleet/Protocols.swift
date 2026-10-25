public enum Protocols {
    public static let mine: [Int] = [1, 1]

    public static func common(_ mine: [Int], _ theirs: [Int]) -> Int? {
        guard mine.count == 2, theirs.count == 2 else { return nil }
        let top = min(mine[1], theirs[1])
        return top >= max(mine[0], theirs[0]) ? top : nil
    }

    public static func otherProtocol(machine: String, protocols: [Int]?, version: String?, commit: String?, mine: [Int] = mine) -> String {
        let theirs = protocols.flatMap { $0.count == 2 && $0[0] <= $0[1] ? $0 : nil } ?? [1, 1]
        var runs = "an older Kanna"
        if let version {
            runs = "Kanna \(version)"
            if let commit { runs += " (\(short(commit)))" }
        }
        let fix = theirs[1] < mine[0] ? "it's older: kanna fleet update \(machine)" : "this Kanna is older: update it, or run kanna fleet update on \(machine)"
        return "\(machine) runs \(runs), protocol \(span(theirs)), and this Kanna speaks \(span(mine)); \(fix)"
    }

    static func short(_ commit: String) -> String {
        String(commit.prefix(8)) + (commit.hasSuffix("-dirty") ? "-dirty" : "")
    }

    static func span(_ p: [Int]) -> String {
        p[0] == p[1] ? "\(p[0])" : "\(p[0])–\(p[1])"
    }
}
