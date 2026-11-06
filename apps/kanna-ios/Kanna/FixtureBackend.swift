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

    private static func task(_ name: String, _ status: String, desk: String? = nil, machine: String? = nil, agent: String = "claude", ago: Double = 0, branch: String? = nil, after: String? = nil, message: String? = nil) -> ShelfTask {
        ShelfTask(id: name, name: name, status: status, since: Date().timeIntervalSince1970 - ago * 60, desk: desk, machine: machine, agent: agent, branch: branch, after: after.map { [Named(id: $0, name: $0)] } ?? [], message: message)
    }

    private var shelf = ShelfPage(tasks: [
        task("Keep terminal output", "working", desk: "studio", machine: "dev-1", ago: 12, branch: "terminal-output", message: "Keep each terminal's scrollback through a daemon restart, up to the 10,000 line limit."),
        task("Review build output", "needs", desk: "studio", machine: "tests-1", agent: "codex", ago: 40),
        task("Restore task tabs", "idle", desk: "mbp", machine: "mbp", ago: 120, message: "Bring a task's tabs back in their order after a relaunch."),
        task("Fix the page_comments pinch", "idle", desk: "mbp", machine: "mbp", ago: 180, message: "A pinch over a comment shouldn't zoom the page's comments away."),
        task("Shelf trailing edge", "working", desk: "mbp", machine: "mbp", ago: 1, message: "The Shelf's last row is cut off at some widths."),
        task("Polish project search", "working", machine: "studio", ago: 4, branch: "project-search", message: "Rank a project's exact name above a fuzzy match."),
        task("Agent prompts setting", "ready", machine: "dev-1", after: "Task events", message: "Let a project set the prompts its agents start with."),
        task("Pane titles in tooltips", "stale", machine: "dev-1", after: "Restore task tabs", message: "Show a pane's whole title in its tab's tooltip."),
        task("Document reconnect behavior", "stopped", machine: "dev-1", ago: 1500, message: "Write down what the app does while the daemon restarts."),
        task("Native diff viewer", "waiting", message: "Show a task's branch diff in a tab, side by side, with comments like doc markup."),
        task("Check tab restoration", "blocked", agent: "codex", after: "Restore task tabs", message: "Reopen a project after a crash and check every tab comes back in its place, with its scroll and zoom."),
        task("Schedule history page", "waiting", message: "A page with every schedule's firings and replies, and an export of them."),
    ], desks: [Desk(machine: "studio", open: true), Desk(machine: "mbp", open: false)], done: ["Schedules tab", "Task events", "Stats headings", "Fix pinch anchor", "CLI split by command"])

    private var threads: [String: [ThreadLine]] = [
        "Keep terminal output": [
            ThreadLine(at: Date().timeIntervalSince1970 - 2400, from: "planner", you: false, text: "The scrollback test on main changed. Merge it before you touch vt_buffer.rs."),
            ThreadLine(at: Date().timeIntervalSince1970 - 1560, from: "Keep terminal output", you: false, text: "Merged, and the test passes. Moving on to the restore path."),
        ],
    ]

    private var words: [String: [Said]] = [
        "Review build output": [Said(at: Date().timeIntervalSince1970 - 2400, asking: true, text: "Keep the old colours as a fallback, or drop them and restyle the crash page here too?")],
    ]

    func tasks(repo: String) async throws -> ShelfPage {
        repo == "kanna" ? shelf : ShelfPage(tasks: [], desks: shelf.desks, done: [])
    }

    func task(_ id: String) async throws -> TaskPage {
        guard let task = shelf.tasks.first(where: { $0.id == id }) else { throw Refused(code: "not_found", message: "no task \(id)") }
        return TaskPage(task: task, thread: threads[id] ?? [], said: words[id] ?? [])
    }

    private func changed() {
        onNotice?(.tasks(repo: "kanna"))
    }

    private func set(_ id: String, _ apply: (inout ShelfTask) -> Void) {
        guard let index = shelf.tasks.firstIndex(where: { $0.id == id }) else { return }
        apply(&shelf.tasks[index])
        changed()
    }

    private func status(of id: String) -> String { shelf.tasks.first { $0.id == id }?.status ?? "" }

    func tell(task: String, id: String, text: String) async throws {
        let unstarted = ["waiting", "blocked"].contains(status(of: task))
        threads[task, default: []].append(ThreadLine(at: Date().timeIntervalSince1970, from: "you", you: true, text: text, client: id))
        guard !unstarted else { return }
        Task {
            await wait(0.6)
            set(task) { $0.status = "working"; $0.since = Date().timeIntervalSince1970 }
            await wait(0.4)
            if let index = threads[task]?.lastIndex(where: { $0.you }) { threads[task]?[index].read = Date().timeIntervalSince1970 }
            changed()
            await wait(1)
            words[task, default: []].append(Said(at: Date().timeIntervalSince1970, asking: false, text: "Got it. Carrying on with that."))
            changed()
        }
    }

    func pick(task: String, desk: String, room: String?) async throws {
        await wait(0.9)
        if let room { set(room) { $0.desk = nil } }
        guard shelf.tasks.filter({ $0.desk == desk }).count < 3 else { throw Refused(code: "task_limit", message: "\(desk)'s Desk is full") }
        threads[task] = threads[task]?.map { var line = $0; line.read = line.read ?? Date().timeIntervalSince1970; return line }
        set(task) { t in
            t.desk = desk
            t.machine = t.machine ?? desk
            if t.status == "waiting" || t.status == "stopped" {
                t.status = "working"
                t.since = Date().timeIntervalSince1970
            }
        }
    }

    func close() {}

    func forget() { paired = nil }

    func link() async -> Link { Link(path: .lan, milliseconds: 9) }

    func highlight(file: String, text: String) -> [TextSpan] { Rust.highlight(file: file, text: text) }
}
