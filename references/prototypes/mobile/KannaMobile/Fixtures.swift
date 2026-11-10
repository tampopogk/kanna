import Foundation

private func when(_ t: String) -> ChatItem { ChatItem(kind: .when(t)) }
private func me(_ t: String, _ r: Receipt = .read) -> ChatItem { ChatItem(kind: .me(t, r)) }
private func pl(_ b: Block...) -> ChatItem { ChatItem(kind: .planner(b)) }

enum Snippets {
    static let codex = Snippet(file: "crates/kanna-daemon/src/backlog.rs", lines: [
        CodeLine(160, "fn in_its_terminal(agent: &str) -> String {"),
        CodeLine(161, "    match program(agent) == \"codex\" && !agent.split_whitespace().any(|w| w == \"--no-daemon\") {", hot: true),
        CodeLine(162, "        true => format!(\"{agent} --no-daemon\"),", hot: true),
        CodeLine(163, "        false => agent.to_string(),"),
        CodeLine(164, "    }"),
        CodeLine(165, "}"),
    ], commit: "270bf73")
    static let merge = Snippet(file: "crates/kanna-daemon/src/agents.rs", lines: [
        CodeLine(514, "fn merge(mine: &[u8], base: &[u8], new: &[u8]) -> Option<Vec<u8>> {", hot: true),
        CodeLine(521, "    let out = match files {"),
        CodeLine(522, "        [Ok(mine), Ok(base), Ok(new)] => Command::new(\"git\").args([\"merge-file\", \"-p\", \"-q\"]).output().ok(),", hot: true),
    ], commit: "ad2317c")
}

enum Chats {
    static let overnight: Block = .list([
        "**Schedules tab:** the Desk view's fourth tab, with each schedule's firings and replies, and Pause, Resume, Run now.",
        "**Task events:** a task hears when main moves into conflict with it.",
        "**Stats headings:** the sections sit on a tinted band.",
    ])
    static let lookFirst = "**Review build output**: it's asking whether to keep the old colours, and everything else on it is done."
    static let scheduleQuestion = "One: **Schedule history page** asks for what the tab now does, plus an export. What should I do with it?"
    static let scheduleOptions = [
        AskOption(label: "End it", why: "The tab covers it. Drop the export."),
        AskOption(label: "Cut it down", why: "A new task with only the export."),
        AskOption(label: "Leave it", why: "I'll look at it at the Desk."),
    ]
    static var answer: [ChatItem] {
        [when("Today 8:52"), me("What landed overnight?"),
         pl(.p("Three merges on main:"), overnight, .p("Task events added a table, so `schedules_tab` and `task_events` both moved in the e2e script. Nothing is stuck. Two tasks are waiting on you, on the Shelf tab.")),
         when("9:38"), me("Which one should I look at first?"), pl(.p(lookFirst))]
    }
    static var working: [ChatItem] {
        [when("Today 9:38"), me("Which one should I look at first?"), pl(.p(lookFirst)), when("9:41"), me("Why are there no merges for recipe in Stats?")]
    }
    static var queued: [ChatItem] {
        [when("Today 9:41"), me("Why are there no merges for recipe in Stats?", .waitingTurn)]
    }
    static var question: [ChatItem] {
        [when("Today 9:44"), me("Is anything on the Shelf out of date after the schedules tab?"), pl(.p(scheduleQuestion)), ChatItem(kind: .ask(scheduleOptions))]
    }
    static var answered: [ChatItem] {
        [when("Today 9:44"), me("Is anything on the Shelf out of date after the schedules tab?"), pl(.p(scheduleQuestion)), ChatItem(kind: .answered("Cut it down"))]
    }
    static var code: [ChatItem] {
        [when("Today 10:02"), me("Explain how a Codex task stays in its own terminal"), pl(.p("Kanna adds `--no-daemon` when it builds the command, unless you already did:")), pl(.code(Snippets.codex)),
         pl(.p("Codex's shared server would run every session's hooks in the environment of whatever started it, so `kanna task name` would see the wrong pane."))]
    }
    static var none: [ChatItem] {
        [when("Yesterday 18:20"), pl(.p("That's everything on the Desk merged. Enjoy the evening.")), when("9:41"), me("Start the Keep terminal output task", .waitingPlanner), ChatItem(kind: .sys("No planner was running for kanna, so one is starting on studio."))]
    }
    static var quiz: [ChatItem] {
        [when("Today 12:10"), me("Quiz me on the relay stuff from this week"), pl(.p("Five questions on how machines link, from the last week of `kanna-net`.")), ChatItem(kind: .round("machines", nil))]
    }
    static var pathOnly: [ChatItem] {
        [when("Today 13:05"), me("Did the e2e pass?"), pl(.p("All of it, on tests-1."))]
    }
    static var offline: [ChatItem] {
        [when("Today 9:10"), me("Did the e2e pass?"), pl(.p("All of it, on tests-1.")), when("13:40"), me("Start the Native diff viewer task", .waitingMachine("studio"))]
    }
    static var switcher: [ChatItem] { [pl(.p("Three merges on main."))] }
}

enum Fixtures {
    static let machines: [String: Machine] = {
        let list = [
            Machine(name: "studio", about: "Mac Studio · macOS 26 · Kanna 4.2", linux: false, ms: 4, lastSeen: nil, since: "8:52"),
            Machine(name: "mbp", about: "MacBook Pro · macOS 26 · Kanna 4.2", linux: false, ms: 9, lastSeen: nil, since: "8:52"),
            Machine(name: "dev-1", about: "Linux · Ubuntu 24.04 · Kanna 4.2", linux: true, ms: 184, lastSeen: nil, since: "8:53", alwaysRelayed: true),
            Machine(name: "tests-1", about: "Linux · Debian 13 · Kanna 4.2", linux: true, ms: 0, lastSeen: "40m ago", since: ""),
            Machine(name: "r720-1", about: "Linux · Debian 13 · Kanna 4.2", linux: true, ms: 0, lastSeen: "2d ago", since: ""),
            Machine(name: "cloud-2", about: "Linux · Ubuntu 24.04 · Kanna 4.2", linux: true, ms: 41, lastSeen: nil, since: "8:53"),
            Machine(name: "pi", about: "Linux · Raspberry Pi OS · Kanna 4.2", linux: true, ms: 7, lastSeen: nil, since: "8:53"),
        ]
        return Dictionary(uniqueKeysWithValues: list.map { ($0.name, $0) })
    }()

    static var kannaTasks: [WorkItem] {
        [
            WorkItem(name: "Keep terminal output", status: .working, desk: "studio", machine: "dev-1", age: "12m", branch: "terminal-output",
                     firstMessage: "Keep each terminal's scrollback through a daemon restart, up to the 10,000 line limit.",
                     thread: [
                        Message(from: "planner", time: "8:30", text: "The scrollback test on main changed. Merge it before you touch `vt_buffer.rs`."),
                        Message(from: "Keep terminal output", time: "8:44", text: "Merged, and the test passes. Moving on to the restore path."),
                     ]),
            WorkItem(name: "Review build output", status: .needs, desk: "studio", machine: "tests-1", agent: "codex", age: "40m",
                     lastWords: ["Everything's in except the status colours. The new palette is in `build_view.swift`, but the three old tokens are still used by the crash page.", "Keep the old colours as a fallback, or drop them and restyle the crash page here too?"], lastAt: "9:01"),
            WorkItem(name: "Restore task tabs", status: .idle, desk: "mbp", machine: "mbp", age: "2h", firstMessage: "Bring a task's tabs back in their order after a relaunch.",
                     thread: [Message(from: "Restore task tabs", time: "8:12", text: "", said: [ChatItem(kind: .planner([.p("Restored in order. It happens **here**:"), .code(Snippets.codex)]))])]),
            WorkItem(name: "Fix the page_comments pinch", status: .idle, desk: "mbp", machine: "mbp", age: "3h", firstMessage: "A pinch over a comment shouldn't zoom the page's comments away."),
            WorkItem(name: "Shelf trailing edge", status: .working, desk: "mbp", machine: "mbp", age: "active on mbp", firstMessage: "The Shelf's last row is cut off at some widths."),
            WorkItem(name: "Polish project search", status: .working, desk: nil, machine: "studio", age: "4m", branch: "project-search", firstMessage: "Rank a project's exact name above a fuzzy match."),
            WorkItem(name: "Agent prompts setting", status: .ready, desk: nil, machine: "dev-1", after: "Task events", firstMessage: "Let a project set the prompts its agents start with."),
            WorkItem(name: "Pane titles in tooltips", status: .stale, desk: nil, machine: "dev-1", after: "Restore task tabs", firstMessage: "Show a pane's whole title in its tab's tooltip."),
            WorkItem(name: "Document reconnect behavior", status: .stopped, desk: nil, machine: "dev-1", age: "yesterday", firstMessage: "Write down what the app does while the daemon restarts."),
            WorkItem(name: "Native diff viewer", status: .waiting, desk: nil, firstMessage: "Show a task's branch diff in a tab, side by side, with comments like doc markup. Mockup first, then the spec, in `references/mockups/diff.html`. Say what to do with renames."),
            WorkItem(name: "Check tab restoration", status: .blocked, desk: nil, agent: "codex", after: "Restore task tabs", firstMessage: "Reopen a project after a crash and check every tab comes back in its place, with its scroll and zoom."),
            WorkItem(name: "Schedule history page", status: .waiting, desk: nil, firstMessage: "A page with every schedule's firings and replies, and an export of them."),
        ]
    }

    static var kannaMap: [Subsystem] {
        [
            Subsystem(name: "Daemon", span: (3, 3), pct: 72, moved: 12, paths: "crates/kanna-daemon · docs/specs/architecture.md", was: "was 75% on Oct 20", notes: "You know the task lifecycle well, and slip on what restarts adopt."),
            Subsystem(name: "The app", span: (3, 2), pct: 88, paths: "apps/kanna", was: "was 86% on Oct 20", notes: "Strong on panes and tabs."),
            Subsystem(name: "Machines", span: (2, 2), pct: 24, moved: 41, paths: "crates/kanna-net · docs/specs/machine-*", was: "was 58% on Oct 8", notes: "You know who owns what here, but slip on what happens when a link drops.",
                      movedRows: [("The relay path", "14 commits · never asked"), ("Agent sign-in across machines", "11 commits · never asked")],
                      misses: [("Who starts the peer log", "kanna-net/src/pair.rs:104"), ("Why a server can't add a peer", "kanna-net/src/node.rs:975")]),
            Subsystem(name: "CLI", span: (1, 2), pct: 91, paths: "crates/kanna", was: "was 90% on Oct 12"),
            Subsystem(name: "Keeper", span: (1, 1), pct: 85, paths: "crates/kanna-keeper", was: "was 85% on Oct 1"),
            Subsystem(name: "Terminal", span: (2, 1), pct: 55, paths: "crates/kanna-mux", was: "was 61% on Oct 15", notes: "Unsure of the flow control on a terminal's output."),
            Subsystem(name: "Panes", span: (2, 1), pct: 68, paths: "apps/kanna/Sources/Kanna/Panes", was: "was 70% on Oct 15"),
            Subsystem(name: "Cloud", span: (2, 1), pct: nil, paths: "docs/specs/kanna-cloud.md"),
            Subsystem(name: "Pipelines", span: (2, 1), pct: 31, moved: 19, paths: "crates/kanna-daemon/src/pipelines", was: "was 44% on Oct 14", notes: "Last asked three weeks ago."),
            Subsystem(name: "Diff", span: (1, 1), pct: 60, paths: "apps/kanna/Sources/KannaDiff", was: "was 60% on Oct 9"),
            Subsystem(name: "Stats", span: (1, 1), pct: 74, paths: "crates/kanna-daemon/src/stats", was: "was 72% on Oct 22"),
            Subsystem(name: "Build", span: (2, 1), pct: 47, moved: 8, paths: "scripts · docs/specs/build-macs.md", was: "was 50% on Oct 18"),
            Subsystem(name: "Skills", span: (2, 1), pct: 66, paths: ".agents/kanna-skills", was: "was 66% on Oct 25"),
        ]
    }

    static func smallMap(_ names: [(String, Int?, Int)]) -> [Subsystem] {
        let spans = [(3, 2), (3, 2), (2, 1), (2, 1), (2, 1), (3, 1), (3, 1)]
        return names.enumerated().map { i, n in
            Subsystem(name: n.0, span: spans[min(i, spans.count - 1)], pct: n.1, moved: n.2, paths: n.0.lowercased(), was: "last asked in October")
        }
    }

    static var repos: [Repo] {
        [
            Repo(id: "kanna", plannerMachine: "studio", planner: .idle, lastLine: "Two tasks are waiting on you", chat: Chats.answer, tasks: kannaTasks,
                 done: ["Schedules tab", "Task events", "Stats headings", "Fix pinch anchor", "CLI split by command"],
                 desks: [Desk(machine: "studio", open: true), Desk(machine: "mbp", open: true)],
                 machines: ["studio", "mbp", "dev-1", "tests-1"], without: ["r720-1", "pi"], map: kannaMap, seed: 0),
            Repo(id: "recipe", plannerMachine: "mbp", planner: .working(Date().addingTimeInterval(-360)), lastLine: "Working · 6m",
                 chat: [when("Today 9:35"), me("Add the shopping list page"), pl(.p("Started **Shopping list** on mbp. I'll say when it's up for review."))],
                 tasks: [WorkItem(name: "Shopping list", status: .working, desk: "mbp", machine: "mbp", age: "6m", firstMessage: "A page that gathers a week's ingredients."),
                         WorkItem(name: "Scale servings", status: .waiting, desk: nil, firstMessage: "Scale a recipe's amounts by its servings.")],
                 done: ["Import from a URL"], desks: [Desk(machine: "mbp", open: true)], machines: ["mbp", "dev-1"], without: ["studio", "tests-1"],
                 map: smallMap([("Parser", 80, 0), ("Pages", 62, 6), ("Storage", 45, 3), ("Sync", nil, 0), ("Search", 30, 11)]), seed: 100, playedToday: true),
            Repo(id: "kmux", plannerMachine: "studio", planner: .stopped, lastLine: "The 0.9 tag is pushed",
                 chat: [when("Yesterday 17:02"), me("Tag 0.9 when the tests pass"), pl(.p("The 0.9 tag is pushed."))],
                 tasks: [WorkItem(name: "Sixel images", status: .stopped, desk: nil, machine: "studio", age: "3d", firstMessage: "Draw sixel images in a terminal pane.")],
                 done: ["0.9 release notes", "Fix font fallback"], desks: [Desk(machine: "studio", open: true)], machines: ["studio", "mbp"], without: ["dev-1", "tests-1"],
                 map: smallMap([("Terminal", 70, 2), ("Splits", 84, 0), ("Protocol", 58, 4), ("Web panes", 40, 9)]), seed: 200, streak: 3, points: 480),
            Repo(id: "homelab", plannerMachine: "r720-1", planner: .idle, lastLine: "Offline · r720-1 last seen 2d ago",
                 chat: [when("Tuesday 21:14"), me("Is the backup job green?"), pl(.p("Green, and last night's snapshot is on the NAS."))],
                 tasks: [WorkItem(name: "Rotate the backup keys", status: .stopped, desk: nil, machine: "r720-1", age: "2d", firstMessage: "Rotate the keys the backup job signs with.")],
                 done: ["Move DNS to the Pi"], desks: [], machines: ["r720-1", "pi"], without: ["studio"],
                 map: smallMap([("Ansible", 66, 0), ("Backups", 52, 2), ("Network", 35, 7)]), seed: 300, down: true, lastSeen: "2d ago", streak: 0, points: 120),
        ]
    }

    static let today = RoundDef(id: "today", title: "Next round", about: "about today's changes", questions: [
        Question(kind: "Architecture", source: "docs/kanna-spec.md", text: "Which drawing shows how a terminal's bytes reach Kanna.app?", body: .drawings, correct: 1,
                 why: "The keeper only holds the pty so it outlives a restart. The daemon has the master, passed over `SCM_RIGHTS`, and reads the bytes on their way to the app.",
                 endLabel: "Who reads a terminal", endFile: "docs/kanna-spec.md:141", subsystem: "Daemon", small: true),
        Question(kind: "Behaviour", source: "6dae46b", text: "When a Codex task starts, how does Kanna run `codex`?", body: .choices([
            Choice(title: "With `--no-daemon`", sub: "Its session runs in the task's own terminal"),
            Choice(title: "In Codex's shared background server", sub: "One session per task"),
            Choice(title: "As `codex exec`", sub: "With the first message as its prompt"),
            Choice(title: "With `resume`", sub: "And the task's session ID, as Claude does"),
        ]), correct: 0, why: "Codex's shared server runs every session's hooks with the environment of whatever started it, so the hook would see the wrong pane.",
                 code: Snippets.codex, endLabel: "Codex runs with --no-daemon", endFile: "kanna-daemon/src/backlog.rs:160", subsystem: "Daemon"),
        Question(kind: "Where it lives", source: "ad2317c", text: "You edited an installed skill, and Kanna ships a new version. Where are the two merged?", body: .choices([
            Choice(title: "In the `kanna` CLI", sub: "When you next run a command"),
            Choice(title: "In the daemon", sub: "With `git merge-file`"),
            Choice(title: "In the app", sub: "In a merge view you approve"),
            Choice(title: "In the agent", sub: "On its first turn after the update"),
        ]), correct: 1, why: "The daemon keeps each file as it installed it and runs `git merge-file` on yours, that base and the new one. Only a conflict brings in your editor.",
                 code: Snippets.merge, endLabel: "Merging your skill edits", endFile: "kanna-daemon/src/agents.rs:514", subsystem: "Daemon", small: true),
        Question(kind: "Why", source: "docs/specs/code-tour.md", text: "A comment on code snaps to the whole function. Who works out its lines?", body: .choices([
            Choice(title: "The app", sub: "The daemon asks over the app link"),
            Choice(title: "The daemon", sub: "It reads the file anyway"),
            Choice(title: "The agent", sub: "It names the range itself"),
            Choice(title: "The CLI", sub: "Before it sends the comment"),
        ]), correct: 0, why: "Tree-sitter is compiled into the app only: it costs about 18 MB, so the daemon doesn't link it too. The daemon asks the app and stores the lines it answers.",
                 endLabel: "The app snaps comments", endFile: "docs/specs/code-tour.md:115", subsystem: "The app"),
        Question(kind: "Spot the bug", source: "ad2317c", text: "This is `merge` from today's agent setup, with one bug put in. Which line?", body: .lines([
            PickLine(n: 521, text: "let out = match files {"),
            PickLine(n: 522, text: "[Ok(..), ..] => Command::new(\"git\").args([\"merge-file\", \"-p\", \"-q\"])"),
            PickLine(n: 525, text: "let _ = fs::remove_dir_all(&dir);"),
            PickLine(n: 526, text: "out.filter(|o| o.status.code() != Some(1)).map(|o| o.stdout)"),
        ]), correct: 3, why: "`git merge-file` exits 0 when clean and with the number of conflicts otherwise, so a merge only counts at `Some(0)`. This line lets a two-conflict merge through.",
                 code: Snippet(file: "crates/kanna-daemon/src/agents.rs", lines: [CodeLine(526, "    out.filter(|o| o.status.code() == Some(0)).map(|o| o.stdout)", hot: true)], commit: "ad2317c"),
                 endLabel: "git merge-file's exit code", endFile: "kanna-daemon/src/agents.rs:526", subsystem: "Daemon", small: true),
    ])

    static let mergeBack = Question(kind: "A miss, back again", source: "ad2317c", text: "A new Kanna ships a skill you've changed. What runs `git merge-file` on it?", body: .choices([
        Choice(title: "The `kanna` CLI", sub: "On the next command"),
        Choice(title: "The agent", sub: "On its first turn"),
        Choice(title: "The daemon", sub: "With the installed copy as the base"),
        Choice(title: "Kanna.app", sub: "When it relaunches"),
    ]), correct: 2, why: "The daemon. It keeps each file as it installed it, so it has a base to merge against.", code: Snippets.merge,
        endLabel: "Merging your skill edits, again", endFile: "kanna-daemon/src/agents.rs:514", subsystem: "Daemon")

    static let machines5 = RoundDef(id: "machines", title: "Machine links", about: "about how machines link", questions: [
        Question(kind: "Behaviour", source: "docs/specs/machines.md", text: "When does a machine use a relay?", body: .choices([
            Choice(title: "Only when it's away or the LAN fails", sub: "Direct first, whenever it can"),
            Choice(title: "Always, for its first packet", sub: "Then it upgrades to direct"),
            Choice(title: "For control messages", sub: "Terminals go direct"),
            Choice(title: "Whenever two machines differ in version", sub: "To translate between them"),
        ]), correct: 0, why: "LAN first: machines connect directly whenever they can, and the app shows which path each one is on.", endLabel: "When the relay's used", endFile: "docs/specs/machines.md:30", subsystem: "Machines"),
        Question(kind: "Why", source: "docs/specs/machines.md", text: "What can your relay do with what it carries?", body: .choices([
            Choice(title: "Read task names", sub: "They're in the clear for routing"),
            Choice(title: "Add a device", sub: "If you've signed in to it"),
            Choice(title: "Only pass ciphertext on", sub: "It can't read it or add a device"),
            Choice(title: "Hold messages while a peer is away", sub: "For a day"),
        ]), correct: 2, why: "End to end: a relay carries only ciphertext and can't add a device.", endLabel: "What a relay can see", endFile: "docs/specs/machines.md:33", subsystem: "Machines"),
        Question(kind: "Behaviour", source: "docs/specs/machines.md", text: "What does signing a new peer into the log take?", body: .choices([
            Choice(title: "A gesture in the app", sub: "On a machine already in the log"),
            Choice(title: "`kanna machine pair` alone", sub: "The code is proof enough"),
            Choice(title: "A signature from every peer", sub: "So all of them agree"),
            Choice(title: "Nothing: any peer can add one", sub: "Over its link"),
        ]), correct: 0, why: "Signing a peer takes a gesture in the app, so a process on a machine can't quietly add one.", endLabel: "Signing a peer", endFile: "docs/specs/machines.md:89", subsystem: "Machines"),
        Question(kind: "Behaviour", source: "docs/specs/machines.md", text: "A peer is revoked, then a later entry adds it back. Which wins?", body: .choices([
            Choice(title: "The add", sub: "It's the newest entry"),
            Choice(title: "The revocation", sub: "It beats any later add"),
            Choice(title: "Whichever most peers have seen", sub: "By majority"),
            Choice(title: "Neither: the log stops", sub: "Until you choose"),
        ]), correct: 1, why: "A revocation beats any later add, so a stolen key can't sign its way back.", endLabel: "Revocation wins", endFile: "docs/specs/machines.md:89", subsystem: "Machines"),
        Question(kind: "Behaviour", source: "docs/specs/machines.md", text: "A name comes back with another key. What happens?", body: .choices([
            Choice(title: "It takes over the name", sub: "The log keeps the newest key"),
            Choice(title: "It's refused", sub: "A name stays with its key"),
            Choice(title: "It joins as `name-2`", sub: "Beside the old one"),
            Choice(title: "The app asks you", sub: "On every machine"),
        ]), correct: 1, why: "A name that comes back with another key is refused.", endLabel: "A name and its key", endFile: "docs/specs/machines.md:89", subsystem: "Machines"),
    ])

    static func subsystemRound(_ name: String) -> RoundDef {
        if name == "Machines" { return machines5 }
        return RoundDef(id: "sub-" + name, title: name, about: "about \(name)", questions: today.questions)
    }
}
