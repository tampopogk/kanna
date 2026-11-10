public enum Protocols {
    public static let mine = 5

    public static func otherProtocol(machine: String, protocol theirs: Int?, version: String?, commit: String?, mine: Int = mine) -> (title: String, detail: String) {
        var runs = "an older Kanna"
        if let version {
            runs = version
            if let commit { runs += " (\(short(commit)))" }
        }
        let speaks = theirs.map { "protocol \($0)" } ?? "an older protocol"
        let facts = "It runs \(runs), \(speaks); this Kanna speaks \(mine)."
        if (theirs ?? 0) < mine {
            return ("\(machine) runs an older Kanna", "Update Kanna on \(machine). \(facts)")
        }
        return ("\(machine) runs a newer Kanna", "Update this Kanna. \(facts)")
    }

    static func short(_ commit: String) -> String {
        String(commit.prefix(8)) + (commit.hasSuffix("-dirty") ? "-dirty" : "")
    }
}
