import Foundation

@MainActor
final class FixtureBackend: Backend {
    var onNotice: ((Notice) -> Void)?
    var onLost: (() -> Void)?
    private(set) var paired: Paired?
    private var busy: Bool
    private var chats: [String: [Item]] = [:]
    private var statuses: [String: String]
    let phoneName = "iphone-fix1"

    private static let machine = Paired(name: "iphone-fix1", machine: "studio", key: "fixture", addrs: [])

    init(paired: Bool, planner: String) {
        self.paired = paired ? Self.machine : nil
        busy = planner == "busy"
        statuses = ["kanna": planner == "busy" ? "elsewhere" : "idle", "kmux": "not_running"]
    }

    func pair(scanned: String) async throws -> Paired {
        try await Task.sleep(for: .seconds(4))
        paired = Self.machine
        return Self.machine
    }

    func connect() async throws -> Hello {
        Hello(name: "studio", key: "fixture", addrs: [], projects: ["kanna", "kmux"].map { Project(repo: $0, name: $0, planner: "studio", status: statuses[$0] ?? "idle", since: Date().timeIntervalSince1970) })
    }

    func read(repo: String, after: Int) async throws -> Page {
        Page(items: (chats[repo] ?? []).filter { $0.seq > after }, status: statuses[repo] ?? "idle", since: Date().timeIntervalSince1970)
    }

    private func put(_ repo: String, _ kind: String, _ doc: Doc, client: String? = nil) -> Item {
        let item = Item(seq: (chats[repo]?.count ?? 0) + 1, at: Date().timeIntervalSince1970, kind: kind, client: client, doc: doc)
        chats[repo, default: []].append(item)
        return item
    }

    private func say(_ repo: String, _ kind: String, _ doc: Doc) {
        onNotice?(.chat(repo: repo, item: put(repo, kind, doc)))
    }

    private func status(_ repo: String, _ value: String) {
        statuses[repo] = value
        onNotice?(.planner(repo: repo, status: value, since: Date().timeIntervalSince1970))
    }

    private func markRead(_ repo: String) {
        guard let index = chats[repo]?.lastIndex(where: { $0.kind == "me" }) else { return }
        chats[repo]?[index].read = Date().timeIntervalSince1970
        if let item = chats[repo]?[index] { onNotice?(.chat(repo: repo, item: item)) }
    }

    private func wait(_ seconds: Double) async {
        try? await Task.sleep(for: .seconds(seconds))
    }

    private static let code = Doc(file: "crates/kanna-daemon/src/agents.rs", line: 160, commit: "270bf73", hot: [161, 161], lines: ["fn agent_for(name: &str) -> Option<Agent> {", "    let kind = kinds().get(name)?;", "    Some(Agent::new(kind, \"codex\"))", "}"])

    func send(repo: String, id: String, text: String) async throws -> Item {
        let item = put(repo, "me", Doc(text: text), client: id)
        Task { await run(repo, text) }
        return item
    }

    private func run(_ repo: String, _ text: String) async {
        await wait(0.5)
        if repo == "kmux" {
            say(repo, "sys", Doc(text: "No planner was running for kmux, so one is starting on studio"))
            status(repo, "starting")
            await wait(6)
            markRead(repo)
            status(repo, "working")
            await wait(1)
            say(repo, "say", Doc(text: "Three merges on main since yesterday."))
            status(repo, "idle")
        } else if busy {
            await wait(2)
            busy = false
            markRead(repo)
            status(repo, "working")
            await wait(1)
            say(repo, "say", Doc(text: "Kanna adds codex through the agent table."))
            say(repo, "show", Self.code)
            status(repo, "idle")
        } else {
            markRead(repo)
            status(repo, "working")
            await wait(1)
            say(repo, "say", Doc(text: "Schedule history page is out of date on the Shelf."))
            say(repo, "ask", Doc(text: "What should happen to it?", choices: [Choice(label: "Cut it down", why: "Keep the list, drop the page"), Choice(label: "Leave it", why: "Do it later")]))
            status(repo, "asks")
        }
    }

    func choose(repo: String, id: String, seq: Int, choice: String) async throws -> Item {
        if let index = chats[repo]?.firstIndex(where: { $0.seq == seq }) {
            chats[repo]?[index].doc.chosen = choice
            if let asked = chats[repo]?[index] { onNotice?(.chat(repo: repo, item: asked)) }
        }
        let item = put(repo, "me", Doc(text: choice, choice: seq), client: id)
        Task {
            status(repo, "working")
            await wait(1)
            say(repo, "say", Doc(text: "Schedule export is now the task on the Shelf."))
            status(repo, "idle")
        }
        return item
    }

    func close() {}

    func forget() { paired = nil }

    func link() async -> Link { Link(path: .lan, milliseconds: 9) }

    func highlight(file: String, text: String) -> [TextSpan] { Rust.highlight(file: file, text: text) }
}
