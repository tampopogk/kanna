import Foundation

@MainActor
final class FixtureBackend: Backend {
    var onNotice: ((Notice) -> Void)?
    var onLost: (() -> Void)?
    private(set) var paired: Paired?
    private var busy: Bool
    private var chats: [String: [Item]] = [:]
    private var statuses: [String: String]
    private var linked = "studio"
    private var planners = ["kanna": "studio", "kmux": "dev-1"]
    private var ups = ["studio": true, "mbp": true, "dev-1": true, "tests-1": false, "pi": true]
    let phoneName = "iphone-fix1"
    var down = false
    private let writerSeconds: Double?
    private let emptyQuiz: Bool
    private let writerFails: Bool
    private var unseen: Set<String> = []
    private let slowDial = UserDefaults.standard.bool(forKey: "slowDial")
    private var missed = false
    private var written = 0
    private var paths: [LinkPath]

    private static let machine = Paired(name: "iphone-fix1", machine: "studio", key: "fixture", addrs: [])

    init(paired: Bool, planner: String, paths: String? = nil) {
        self.paired = paired ? Self.machine : nil
        self.paths = (paths ?? "").split(separator: ",").compactMap { ["lan": LinkPath.lan, "net": .net, "relay": .relay][String($0)] }
        self.paired?.known = Self.names.map(Self.reach)
        busy = planner == "busy"
        statuses = ["kanna": planner == "busy" ? "elsewhere" : "idle", "kmux": "not_running"]
        let defaults = UserDefaults.standard
        writerSeconds = defaults.bool(forKey: "slowWriter") ? nil : defaults.bool(forKey: "writerFails") ? 8 : 3
        emptyQuiz = defaults.bool(forKey: "emptyQuiz")
        writerFails = defaults.bool(forKey: "writerFails")
        if !emptyQuiz { quizOpen = [Self.today] }
        if planner == "away" {
            ups["studio"] = false
            ups["tests-1"] = true
            if defaults.bool(forKey: "plannerWakes") { unseen = ["studio"] }
        }
    }

    func pair(scanned: String) async throws -> Paired {
        try await Task.sleep(for: .seconds(4))
        paired = Self.machine
        return Self.machine
    }

    private static let names = ["studio", "mbp", "dev-1", "tests-1", "pi"]

    private static func reach(_ name: String) -> Reach { Reach(name: name, key: "key-\(name)", addrs: []) }

    private func helloNow() -> Hello {
        let projects = ["kanna", "kmux"].map { repo -> Project in
            let planner = planners[repo] ?? "studio"
            return Project(repo: repo, name: repo, planner: planner, plannerKey: "key-\(planner)", status: planner == linked ? statuses[repo] ?? "idle" : nil, since: Date().timeIntervalSince1970)
        }
        return Hello(name: linked, key: "key-\(linked)", addrs: [], projects: projects, machines: Self.names.map(Self.reach))
    }

    func connect(to machine: Reach?) async throws -> Hello {
        if down, slowDial { try? await Task.sleep(for: .seconds(4)) }
        if down { throw Refused(code: "unreachable", message: "down") }
        let name = machine?.name ?? "studio"
        guard ups[name] == true else { throw Refused(code: "unreachable", message: "Can't reach \(name)") }
        linked = name
        paired?.known = helloNow().machines
        return helloNow()
    }

    func hello() async throws -> Hello { helloNow() }

    func reaches(_ machine: Reach) async -> Bool {
        if unseen.contains(machine.name) { ups[machine.name] = true }
        return !down && ups[machine.name] == true
    }

    func machines(repo: String) async throws -> MachinesPage {
        let has: Set<String> = repo == "kanna" ? ["studio", "mbp", "dev-1", "tests-1"] : ["studio", "dev-1"]
        func one(_ name: String, os: String, arch: String, cores: Int, ms: Double, path: String, lan: Bool) -> MachineInfo {
            let up = ups[name] == true && !unseen.contains(name)
            let me = name == linked
            return MachineInfo(name: name, key: "key-\(name)", me: me, os: os, arch: arch, cores: cores, version: "4.2.0", has: has.contains(name), planner: planners[repo] == name, up: up, path: up && !me ? path : nil, lan: lan, ms: up && !me ? ms : nil, since: up ? Date().timeIntervalSince1970 - 3600 : nil, seen: Date().timeIntervalSince1970 - 2400)
        }
        return MachinesPage(from: linked, machines: [
            one("studio", os: "macos", arch: "aarch64", cores: 16, ms: 4, path: "direct", lan: true),
            one("mbp", os: "macos", arch: "aarch64", cores: 12, ms: 9, path: "direct", lan: true),
            one("dev-1", os: "linux", arch: "x86_64", cores: 32, ms: 184, path: "relay", lan: false),
            one("tests-1", os: "linux", arch: "x86_64", cores: 8, ms: 0, path: "direct", lan: false),
            one("pi", os: "linux", arch: "aarch64", cores: 4, ms: 7, path: "direct", lan: true),
        ])
    }

    func move(repo: String, machine: String) async throws -> Moved {
        await wait(0.3)
        guard ups[machine] == true else { throw Refused(code: "unreachable", message: "\(machine) didn't answer") }
        planners[repo] = machine
        statuses[repo] = "starting"
        let here = linked
        linked = machine
        _ = put(repo, "sys", Doc(text: "No planner was running for \(repo), so one is starting on \(machine)"))
        linked = here
        Task {
            await wait(1)
            status(repo, "idle")
        }
        return Moved(planner: machine, plannerKey: "key-\(machine)")
    }

    private static func days() -> [String] {
        let format = DateFormatter()
        format.dateFormat = "yyyy-MM-dd"
        return (0..<30).map { format.string(from: Date(timeIntervalSinceNow: Double($0 - 29) * 86400)) }
    }

    private static func wave(_ base: Double, _ swing: Double) -> [Double?] {
        (0..<30).map { base + swing * sin(Double($0) / 2.5) + Double($0) * 0.1 }
    }

    func stats(repo: String) async throws -> StatsPage {
        StatsPage(days: Self.days(), sections: [
            StatsSection(id: "quiz", title: "Quiz", note: "what you know of the code", tiles: [
                StatsTile(id: "right", label: "Answers right, last 30 days", value: 72.4, unit: "%", note: "of 85 answers", span: 3, chart: StatsChart(kind: "line", values: Self.wave(70, 5), max: 100, what: "of the answers in the 30 days before")),
                StatsTile(id: "misses", label: "Misses to come back", value: 6, note: "", span: 3, chart: StatsChart(kind: "line", values: Self.wave(6, 2), what: "misses to come back")),
                StatsTile(id: "kinds", label: "By kind", note: "", span: 3, chart: StatsChart(kind: "ranked", max: 100, rows: [StatsRow(label: "why", value: 80, note: "8/10"), StatsRow(label: "bug", value: 50, note: "1/2")])),
            ]),
            StatsSection(id: "main", title: "Main", note: "what landed", tiles: [
                StatsTile(id: "commits", label: "Commits", value: 38, note: "today · 616 in 30 days", span: 2, chart: StatsChart(kind: "columns", values: Self.wave(20, 15), what: "commits")),
                StatsTile(id: "landed", label: "Tasks landed", value: 9, note: "today · 153 in 30 days", span: 2, chart: StatsChart(kind: "columns", values: Self.wave(6, 5), what: "merges into main")),
            ]),
            StatsSection(id: "tasks", title: "Tasks", note: "from the Desk's and the Shelf's events", tiles: [
                StatsTile(id: "open", label: "Open", value: 23, note: "+4 in 30 days", span: 6, chart: StatsChart(kind: "flow", values: Self.wave(20, 3), made: Self.wave(5, 3), done: Self.wave(4, 3), what: "open at the end of the day")),
                StatsTile(id: "lead", label: "Made to done", value: 12780, unit: "s", note: "median of 40", span: 4, chart: StatsChart(kind: "dots", what: "7-day median", points: [StatsPoint(day: 29, value: 12000, label: "Schedules tab")], line: Self.wave(13000, 3000))),
            ]),
            StatsSection(id: "code", title: "Code quality", note: "how good the code is", tiles: [
                StatsTile(id: "erosion", label: "Erosion", value: 48.5, unit: "%", note: "down 1.0 points in 29 days", span: 2, chart: StatsChart(kind: "line", values: Self.wave(50, 2), max: 100, what: "of complexity")),
                StatsTile(id: "later", label: "A number from a later Kanna", value: 7, note: "a chart this phone can't draw", span: 2, chart: StatsChart(kind: "heatmap")),
                StatsTile(id: "worst", label: "Worst functions", note: "", span: 3, chart: StatsChart(kind: "list", rows: [StatsRow(label: "Contents::start", value: 665, note: "164 lines", detail: "Contents.swift:169", figure: "CC 52")])),
            ]),
            StatsSection(id: "later", title: "Coming", empty: "Nothing here yet."),
        ])
    }

    func read(repo: String, after: Int) async throws -> Page {
        Page(items: (chats[repo + linked] ?? []).filter { $0.seq > after }, status: statuses[repo] ?? "idle", since: Date().timeIntervalSince1970)
    }

    private func put(_ repo: String, _ kind: String, _ doc: Doc, client: String? = nil) -> Item {
        let item = Item(seq: (chats[repo + linked]?.count ?? 0) + 1, at: Date().timeIntervalSince1970, kind: kind, client: client, doc: doc)
        chats[repo + linked, default: []].append(item)
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
        guard let index = chats[repo + linked]?.lastIndex(where: { $0.kind == "me" }) else { return }
        chats[repo + linked]?[index].read = Date().timeIntervalSince1970
        if let item = chats[repo + linked]?[index] { onNotice?(.chat(repo: repo, item: item)) }
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
            say(repo, "sys", Doc(text: "No planner was running for kmux, so one is starting on \(linked)"))
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
        if let index = chats[repo + linked]?.firstIndex(where: { $0.seq == seq }) {
            chats[repo + linked]?[index].doc.chosen = choice
            if let asked = chats[repo + linked]?[index] { onNotice?(.chat(repo: repo, item: asked)) }
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

    private var spoken: [String: [Item]] = [:]

    private func speak(_ task: String, _ kind: String, _ doc: Doc) {
        spoken[task, default: []].append(Item(seq: (spoken[task]?.count ?? 0) + 1, at: Date().timeIntervalSince1970, kind: kind, doc: doc))
    }

    func tasks(repo: String) async throws -> ShelfPage {
        guard planners[repo] == linked else { throw Refused(code: "unknown_repo", message: "not here") }
        return repo == "kanna" ? shelf : ShelfPage(tasks: [], desks: shelf.desks, done: [])
    }

    func task(_ id: String) async throws -> TaskPage {
        guard let task = shelf.tasks.first(where: { $0.id == id }) else { throw Refused(code: "not_found", message: "no task \(id)") }
        return TaskPage(task: task, thread: threads[id] ?? [], said: words[id] ?? [], items: spoken[id] ?? [])
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
            if text.contains("show me") {
                speak(task, "say", Doc(text: "It's **here**, in `build_view.swift`:"))
                speak(task, "show", Doc(file: "apps/build_view.swift", line: 12, commit: "abcdef1234", hot: [13, 13], lines: ["let old = palette.legacy", "let new = palette.status"]))
                speak(task, "ask", Doc(text: "Which do I keep?", choices: [Choice(label: "The new one", why: "Restyle the crash page"), Choice(label: "Both")]))
            } else {
                words[task, default: []].append(Said(at: Date().timeIntervalSince1970, asking: false, text: "Got it. Carrying on with that."))
            }
            changed()
        }
    }

    func choose(task: String, id: String, seq: Int, choice: String) async throws {
        if let index = spoken[task]?.firstIndex(where: { $0.seq == seq }) { spoken[task]?[index].doc.chosen = choice }
        changed()
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

    private static func question(_ kind: String, _ source: String, _ text: String, _ file: String, _ line: Int, _ topic: String, _ subsystem: String, answer: Int, why: String, choices: [(String, String)]? = nil, lines: [PickLine]? = nil, drawings: [String]? = nil, code: Doc? = nil, again: String? = nil) -> Question {
        Question(kind: kind, file: file, line: line, text: text, why: why, topic: topic, source: source, choices: choices?.map { Choice(label: $0.0, why: $0.1) }, lines: lines, drawings: drawings, answer: answer, again: again, subsystem: subsystem, code: code)
    }

    private static let mergeCode = Doc(file: "crates/kanna-daemon/src/agents.rs", line: 514, commit: "ad2317c", hot: [515, 515], lines: ["fn merge(mine: &str, base: &str, theirs: &str) -> Option<Vec<u8>> {", "    let out = Command::new(\"git\").args([\"merge-file\", \"-p\", \"-q\"]);", "}"])

    private static let mergeBack = question("where", "ad2317c", "A new Kanna ships a skill you've changed. What runs `git merge-file` on it?", "crates/kanna-daemon/src/agents.rs", 514, "Merging your skill edits, again", "Daemon", answer: 2, why: "The daemon. It keeps each file as it installed it, so it has a base to merge against.", choices: [("The `kanna` CLI", "On the next command"), ("The agent", "On its first turn"), ("The daemon", "With the installed copy as the base"), ("Kanna.app", "When it relaunches")], code: mergeCode, again: "a3")

    private static let today = QuizRound(round: "r1", title: "Next round", says: "about today's changes", written: 0, head: "ad2317c", questions: [
        question("drawing", "docs/kanna-spec.md", "Which drawing shows how a terminal's bytes reach Kanna.app?", "docs/kanna-spec.md", 141, "Who reads a terminal", "Daemon", answer: 1, why: "The keeper only holds the pty so it outlives a restart. The daemon has the master, passed over `SCM_RIGHTS`, and reads the bytes on their way to the app.", drawings: ["flowchart LR\n  A[Kanna.app] --> B[pty]\n  C[daemon] --> A", "flowchart LR\n  P[pty] --> D[daemon]\n  K[keeper] -.-> P\n  D --> A[Kanna.app]", "flowchart LR\n  A[Kanna.app] --> K[keeper]\n  K --> P[pty]\n  P --> D[daemon]"]),
        question("behaviour", "6dae46b", "When a Codex task starts, how does Kanna run `codex`?", "crates/kanna-daemon/src/agents.rs", 160, "Codex runs with --no-daemon", "Daemon", answer: 0, why: "Codex's shared server runs every session's hooks with the environment of whatever started it, so the hook would see the wrong pane.", choices: [("With `--no-daemon`", "Its session runs in the task's own terminal"), ("In Codex's shared background server", "One session per task"), ("As `codex exec`", "With the first message as its prompt"), ("With `resume`", "And the task's session ID, as Claude does")]),
        question("where", "ad2317c", "You edited an installed skill, and Kanna ships a new version. Where are the two merged?", "crates/kanna-daemon/src/agents.rs", 514, "Merging your skill edits", "Daemon", answer: 1, why: "The daemon keeps each file as it installed it and runs `git merge-file` on yours, that base and the new one. Only a conflict brings in your editor.", choices: [("In the `kanna` CLI", "When you next run a command"), ("In the daemon", "With `git merge-file`"), ("In the app", "In a merge view you approve"), ("In the agent", "On its first turn after the update")], code: mergeCode),
        question("why", "docs/specs/code-tour.md", "A comment on code snaps to the whole function. Who works out its lines?", "docs/specs/code-tour.md", 115, "The app snaps comments", "The app", answer: 0, why: "Tree-sitter is compiled into the app only: it costs about 18 MB, so the daemon doesn't link it too.", choices: [("The app", "The daemon asks over the app link"), ("The daemon", "It reads the file anyway"), ("The agent", "It names the range itself"), ("The CLI", "Before it sends the comment")]),
        question("bug", "ad2317c", "This is `merge` from today's agent setup, with one bug put in. Which line?", "crates/kanna-daemon/src/agents.rs", 526, "git merge-file's exit code", "Daemon", answer: 3, why: "`git merge-file` exits 0 when clean and with the number of conflicts otherwise, so a merge only counts at `Some(0)`.", lines: [PickLine(n: 521, text: "let out = match files {"), PickLine(n: 522, text: "[Ok(..), ..] => Command::new(\"git\")"), PickLine(n: 525, text: "let _ = fs::remove_dir_all(&dir);"), PickLine(n: 526, text: "out.filter(|o| o.status.code() != Some(1)).map(|o| o.stdout)")], code: Doc(file: "crates/kanna-daemon/src/agents.rs", line: 526, commit: "ad2317c", hot: [526, 526], lines: ["    out.filter(|o| o.status.code() == Some(0)).map(|o| o.stdout)"])),
    ], answers: [])

    private static func machines(_ id: String, about: String?, title: String, back: Bool) -> QuizRound {
        let ask = { (text: String, topic: String, line: Int, answer: Int, why: String, choices: [(String, String)]) in
            question("behaviour", "docs/specs/machines.md", text, "docs/specs/machines.md", line, topic, "Machines", answer: answer, why: why, choices: choices)
        }
        var questions = [
            ask("When does a machine use a relay?", "When the relay's used", 30, 0, "LAN first: machines connect directly whenever they can.", [("Only when it's away or the LAN fails", "Direct first, whenever it can"), ("Always, for its first packet", "Then it upgrades to direct"), ("For control messages", "Terminals go direct"), ("Whenever two machines differ in version", "To translate between them")]),
            ask("What can your relay do with what it carries?", "What a relay can see", 33, 2, "End to end: a relay carries only ciphertext and can't add a device.", [("Read task names", "They're in the clear for routing"), ("Add a device", "If you've signed in to it"), ("Only pass ciphertext on", "It can't read it or add a device"), ("Hold messages while a peer is away", "For a day")]),
            ask("What does signing a new peer into the log take?", "Signing a peer", 89, 0, "Signing a peer takes a gesture in the app.", [("A gesture in the app", "On a machine already in the log"), ("`kanna machine pair` alone", "The code is proof enough"), ("A signature from every peer", "So all of them agree"), ("Nothing: any peer can add one", "Over its link")]),
            ask("A peer is revoked, then a later entry adds it back. Which wins?", "Revocation wins", 89, 1, "A revocation beats any later add.", [("The add", "It's the newest entry"), ("The revocation", "It beats any later add"), ("Whichever most peers have seen", "By majority"), ("Neither: the log stops", "Until you choose")]),
            ask("A name comes back with another key. What happens?", "A name and its key", 89, 1, "A name that comes back with another key is refused.", [("It takes over the name", "The log keeps the newest key"), ("It's refused", "A name stays with its key"), ("It joins as `name-2`", "Beside the old one"), ("The app asks you", "On every machine")]),
        ]
        if back { questions[0] = mergeBack }
        return QuizRound(round: id, title: title, about: about, says: about.map { "about \($0)" } ?? "about how machines link", written: 0, head: "ad2317c", questions: questions, answers: [])
    }

    private static func tile(_ name: String, _ lines: Int, _ mastery: Int?, moved: Int = 0, paths: [String], was: Int? = nil, notes: String = "", misses: [TileMiss] = []) -> SubsystemTile {
        let level = mastery.map { $0 >= 80 ? 4 : $0 >= 60 ? 3 : $0 >= 40 ? 2 : 1 } ?? 0
        let commits = (0..<min(moved, 3)).map { MovedCommit(hash: "abc123\($0)", at: Date().timeIntervalSince1970 - Double($0 + 1) * 86400, subject: "A change in \(name) \($0 + 1)") }
        return SubsystemTile(name: name, paths: paths, lines: lines, mastery: mastery, level: level, answers: mastery == nil ? 0 : 9, asked: mastery == nil ? nil : Date().timeIntervalSince1970 - 86400 * 3, was: was, wasAt: was.map { _ in Date().timeIntervalSince1970 - 86400 * 12 }, moved: moved, slipping: moved >= 10 && (mastery ?? 100) < 50, commits: commits, misses: misses, notes: notes)
    }

    private var tiles: [SubsystemTile] = [
        tile("Daemon", 2500, 72, moved: 12, paths: ["crates/kanna-daemon/**"], was: 75, notes: "You know the task lifecycle well, and slip on what restarts adopt."),
        tile("The app", 1700, 88, paths: ["apps/kanna/**"], was: 86, notes: "Strong on panes and tabs."),
        tile("Machines", 1100, 24, moved: 41, paths: ["crates/kanna-net/**", "docs/specs/machine*.md"], was: 58, notes: "You know who owns what here, but slip on what happens when a link drops.", misses: [TileMiss(answer: "a4", text: "What a drop leaves behind", file: "crates/kanna-net/src/link.rs:88")]),
        tile("CLI", 600, 91, paths: ["crates/kanna/**"], was: 90),
        tile("Keeper", 280, 85, paths: ["crates/kanna-keeper/**"], was: 85),
        tile("Terminal", 560, 55, paths: ["crates/kanna-mux/**"], was: 61, notes: "Unsure of the flow control on a terminal's output."),
        tile("Cloud", 560, nil, paths: ["docs/specs/kanna-cloud.md"]),
        tile("Pipelines", 560, 31, moved: 19, paths: ["crates/kanna-pipe/**"], was: 44, notes: "Last asked three weeks ago."),
        tile("Build", 560, 47, moved: 8, paths: ["scripts/**"], was: 50),
        tile("Skills", 560, 66, paths: [".agents/kanna-skills/**"], was: 66),
    ]

    private var quizOpen: [QuizRound] = []
    private var quizWriting: Writing?
    private var quizFailed: String?

    func quizMap(repo: String) async throws -> MapPage {
        if down { throw Refused(code: "unreachable", message: "down") }
        return MapPage(subsystems: repo == "kanna" ? tiles : [])
    }

    func quizRounds(repo: String) async throws -> RoundsPage {
        if down { throw Refused(code: "unreachable", message: "down") }
        return RoundsPage(rounds: repo == "kanna" ? quizOpen : [], writing: quizWriting, queued: [], failed: quizFailed)
    }

    private func write(_ repo: String, about: String?) {
        guard quizWriting == nil else { return }
        quizWriting = Writing(since: Date().timeIntervalSince1970, about: about)
        onNotice?(.quiz(repo: repo))
        guard let seconds = writerSeconds else { return }
        Task {
            await wait(seconds)
            quizWriting = nil
            if writerFails, about == nil {
                quizFailed = "The quiz writer finished without writing a round"
                onNotice?(.quiz(repo: repo))
                return
            }
            written += 1
            let id = "r\(written + 1)"
            quizOpen.append(about == nil && written > 1 || about == nil && emptyQuiz ? Self.today.withID(id) : Self.machines(id, about: about, title: about ?? "Machine links", back: missed && about == nil))
            if let about { say(repo, "round", Doc(round: id, title: about, about: about)) }
            onNotice?(.quiz(repo: repo))
        }
    }

    func quizAnswer(round: String, n: Int, pick: Int, again: String?) async throws {
        if down { throw Refused(code: "unreachable", message: "down") }
        guard let index = quizOpen.firstIndex(where: { $0.round == round }) else { return }
        guard !quizOpen[index].answers.contains(where: { $0.n == n }) else { return }
        quizOpen[index].answers.append(Pick(n: n, pick: pick))
        let question = quizOpen[index].questions[n]
        let right = pick == question.answer
        if !right {
            missed = true
            if let waiting = quizOpen.firstIndex(where: { $0.taken == nil && $0.about == nil }) { quizOpen[waiting].questions[0] = Self.mergeBack }
        }
        if let tile = tiles.firstIndex(where: { $0.name == question.subsystem }) {
            let was = tiles[tile].mastery ?? 0
            tiles[tile].mastery = min(100, max(0, was + (right ? 2 : -1)))
        }
        if quizOpen[index].taken == nil {
            quizOpen[index].taken = Date().timeIntervalSince1970
            if !quizOpen.contains(where: { $0.taken == nil && $0.about == nil }) { write("kanna", about: nil) }
        }
        if quizOpen[index].answers.count == quizOpen[index].questions.count { quizOpen.remove(at: index) }
    }

    func quizAhead(repo: String, about: String?, again: Bool) async throws {
        if down { throw Refused(code: "unreachable", message: "down") }
        if again { quizFailed = nil }
        if about != nil || (quizOpen.isEmpty && quizFailed == nil) { write(repo, about: about) }
    }

    func close() {}

    func networkChanged() {}

    func forget() { paired = nil }

    func link() async -> Link {
        if down { return Link(path: .off, milliseconds: 0) }
        if let path = paths.first {
            if paths.count > 1 { paths.removeFirst() }
            return Link(path: path, milliseconds: path == .lan ? 9 : path == .net ? 41 : 184)
        }
        return Link(path: linked == "dev-1" ? .relay : .lan, milliseconds: linked == "dev-1" ? 184 : 9)
    }

    func highlight(file: String, text: String) -> [TextSpan] { Rust.highlight(file: file, text: text) }

    func drawing(_ source: String) -> DrawingScene? { Rust.drawing(source) }
}

private extension QuizRound {
    func withID(_ id: String) -> QuizRound {
        var copy = self
        copy.round = id
        return copy
    }
}
