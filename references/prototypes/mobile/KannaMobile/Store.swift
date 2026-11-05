import SwiftUI
import UIKit

struct FullCode: Identifiable {
    let id = UUID()
    let snippet: Snippet
}

@Observable @MainActor
final class Store {
    var paired = UserDefaults.standard.bool(forKey: "paired")
    var pairStep = PairStep.start
    var cameraRefused = false
    var repos = Fixtures.repos
    var currentID = "kanna"
    var tab = AppTab.chat
    var shelfPath: [UUID] = []
    var machinePath: [MachineRoute] = []
    var quizPath: [String] = []
    var net = Net.lan
    var showRepos = false
    var showDemo = false
    var showChipPop = false
    var fullCode: FullCode?
    var fullLine: Int?
    var roundOpen = false
    var play: RoundPlay?
    var score = ScoreMode.streak
    var wrapByDefault = true
    var severalMacs = true
    var slowWriter = false {
        didSet {
            guard !slowWriter else { return }
            let id = currentID
            after(2) { [self] in mutate(id) { $0.nextRound = .ready } }
        }
    }
    var speed = 1.0
    var appearance = Appearance.system
    var hold = false
    var pendingPick: UUID?
    var pickFull: String?
    var scrub: Int?
    var scrollStats: String?
    private var generation = 0
    func nextGeneration() { generation += 1 }
    private var pending: [UUID] = []

    static func launch() -> Store {
        let s = Store()
        let d = UserDefaults.standard
        if let a = d.string(forKey: "appearance"), let v = Appearance(rawValue: a) { s.appearance = v }
        if let m = d.string(forKey: "score"), let v = ScoreMode(rawValue: m) { s.score = v }
        s.hold = d.bool(forKey: "hold")
        s.slowWriter = d.bool(forKey: "slowWriter")
        if d.double(forKey: "speed") > 0 { s.speed = d.double(forKey: "speed") }
        if let scene = d.string(forKey: "scene") { s.apply(scene: scene) }
        if !s.hold { s.startBackground() }
        return s
    }

    var repo: Repo {
        get { repos.first { $0.id == currentID } ?? repos[0] }
        set { if let i = repos.firstIndex(where: { $0.id == currentID }) { repos[i] = newValue } }
    }

    func after(_ seconds: Double, _ work: @escaping @MainActor () -> Void) {
        let g = generation
        let delay = seconds * speed
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(delay))
            guard g == self.generation else { return }
            withAnimation(.snappy) { work() }
        }
    }

    private func startBackground() {
        after(25) { [self] in
            mutate("recipe") {
                $0.chat.append(ChatItem(kind: .planner([.p("**Shopping list** is up for review on mbp.")])))
            }
            turnEnds("recipe")
        }
    }

    func mutate(_ id: String, _ change: (inout Repo) -> Void) {
        if let i = repos.firstIndex(where: { $0.id == id }) { change(&repos[i]) }
    }

    func path(_ machine: String) -> LinkPath {
        guard let m = Fixtures.machines[machine] else { return .off }
        if m.lastSeen != nil || net == .machineDown || net == .noNetwork { return .off }
        if m.alwaysRelayed || net == .relay { return .relay }
        return net == .internet ? .net : .lan
    }

    var plannerPath: LinkPath { repo.down ? .off : path(repo.plannerMachine) }
    var offline: Bool { plannerPath == .off }
    var asOf: String { repo.down ? repo.lastSeen : "9:12" }

    func milliseconds(_ machine: String) -> Int {
        let base = Fixtures.machines[machine]?.ms ?? 0
        switch path(machine) {
        case .relay: return max(base, 184)
        case .net: return base + 31
        default: return base
        }
    }

    func selectRepo(_ id: String) {
        currentID = id
        shelfPath = []
        machinePath = []
        quizPath = []
        showRepos = false
    }

    func finishPairing(repo id: String) {
        currentID = id
        tab = .chat
        paired = true
        if !hold { UserDefaults.standard.set(true, forKey: "paired") }
    }

    func forgetPairing() {
        UserDefaults.standard.set(false, forKey: "paired")
        paired = false
        pairStep = .start
        showDemo = false
    }

    func beginScan() {
        pairStep = cameraRefused ? .refused : .scan
        guard !cameraRefused else { return }
        after(2) { [self] in
            guard pairStep == .scan else { return }
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            pairStep = .confirm
            after(2.5) { [self] in
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                pairStep = .done
            }
        }
    }

    func clock() -> String {
        let c = Calendar.current.dateComponents([.hour, .minute], from: Date())
        return String(format: "%d:%02d", c.hour ?? 0, c.minute ?? 0)
    }

    func stamp(_ id: String) {
        let t = clock()
        mutate(id) { r in
            let has = r.chat.reversed().compactMap { item -> String? in
                if case .when(let w) = item.kind { return w }
                return nil
            }.first
            if has?.hasSuffix(t) != true { r.chat.append(ChatItem(kind: .when(t))) }
        }
    }

    func send(_ text: String) {
        let id = currentID
        stamp(id)
        let item = ChatItem(kind: .me(text, .none))
        let foldedAsk = repo.chat.lastIndex { if case .ask = $0.kind { return true } else { return false } }
        mutate(id) { $0.chat.append(item) }
        if offline {
            setReceipt(id, item.id, .waitingMachine(repo.plannerMachine))
            return
        }
        switch repo.planner {
        case .idle:
            setReceipt(id, item.id, .sent)
            after(0.5) { [self] in begin(id, item.id, text) }
        case .asks:
            mutate(id) { r in
                if let foldedAsk { r.chat[foldedAsk].kind = .answered("your own words") }
            }
            setReceipt(id, item.id, .sent)
            after(0.5) { [self] in
                setReceipt(id, item.id, .read)
                mutate(id) { $0.planner = .working(Date()) }
                after(2.2) { [self] in deliver(id, [(0, .planner([.p("Understood. I'll do it that way.")]))]) }
            }
        case .working, .elsewhere:
            setReceipt(id, item.id, .waitingTurn)
            pending.append(item.id)
            if case .elsewhere = repo.planner, !hold, !scheduledEnd { scheduledEnd = true; after(6) { [self] in scheduledEnd = false; turnEnds(id) } }
        case .stopped:
            setReceipt(id, item.id, .waitingPlanner)
            mutate(id) {
                $0.chat.append(ChatItem(kind: .sys("No planner was running for \(id), so one is starting on \($0.plannerMachine).")))
                $0.planner = .starting
            }
            after(3) { [self] in
                mutate(id) { $0.planner = .idle }
                begin(id, item.id, text)
            }
        case .starting:
            setReceipt(id, item.id, .waitingPlanner)
            pending.append(item.id)
        }
    }

    private var scheduledEnd = false

    private func setReceipt(_ id: String, _ item: UUID, _ receipt: Receipt) {
        mutate(id) { r in
            guard let i = r.chat.firstIndex(where: { $0.id == item }), case .me(let t, _) = r.chat[i].kind else { return }
            r.chat[i].kind = .me(t, receipt)
        }
    }

    private func begin(_ id: String, _ item: UUID, _ text: String) {
        setReceipt(id, item, .read)
        mutate(id) { $0.planner = .working(Date()); $0.lastLine = "Working" }
        let steps = script(for: text, repo: id)
        after(1.6) { [self] in deliver(id, steps) }
    }

    private func deliver(_ id: String, _ steps: [(Double, ChatKind)]) {
        guard let first = steps.first else { return turnEnds(id) }
        after(first.0) { [self] in
            mutate(id) { $0.chat.append(ChatItem(kind: first.1)) }
            UIImpactFeedbackGenerator(style: .soft).impactOccurred()
            if case .ask = first.1 {
                mutate(id) { $0.planner = .asks; $0.lastLine = "Asks you" }
                return
            }
            deliver(id, Array(steps.dropFirst()))
        }
    }

    func turnEnds(_ id: String) {
        mutate(id) { r in
            r.planner = .idle
            for item in r.chat.reversed() {
                if case .planner(let blocks) = item.kind, case .p(let t)? = blocks.last {
                    r.lastLine = String(t.prefix(40)).replacingOccurrences(of: "*", with: "")
                    break
                }
            }
        }
        guard !pending.isEmpty else { return }
        let next = pending.removeFirst()
        guard let r = repos.first(where: { $0.id == id }), let item = r.chat.first(where: { $0.id == next }), case .me(let text, _) = item.kind else { return }
        begin(id, next, text)
    }

    func script(for text: String, repo id: String) -> [(Double, ChatKind)] {
        let t = text.lowercased()
        if t.contains("quiz") {
            return [(1.2, .planner([.p("Five questions on how machines link, from the last week of `kanna-net`.")])),
                    (2.0, .round("machines", nil))]
        }
        if t.contains("explain") || t.contains("codex") || t.contains("code") || t.contains("line ") {
            return [(1.0, .planner([.p("Kanna adds `--no-daemon` when it builds the command, unless you already did:")])),
                    (1.0, .planner([.code(Snippets.codex)])),
                    (1.0, .planner([.p("Codex's shared server would run every session's hooks in the environment of whatever started it, so `kanna task name` would see the wrong pane.")]))]
        }
        if t.contains("shelf") || t.contains("out of date") {
            return [(1.4, .planner([.p(Chats.scheduleQuestion)])), (0.4, .ask(Chats.scheduleOptions))]
        }
        if t.contains("landed") || t.contains("overnight") || t.contains("merged") {
            return [(1.5, .planner([.p("Three merges on main:"), Chats.overnight, .p("Nothing is stuck. Two tasks are waiting on you, on the Shelf tab.")]))]
        }
        if t.contains("start") {
            let name = repos.first { $0.id == id }?.tasks.first { t.contains($0.name.lowercased()) && $0.status != .blocked }
            if let name, name.status == .waiting || name.status == .stopped {
                let task = name.id
                after(2.8) { [self] in
                    mutate(id) { r in
                        guard let i = r.tasks.firstIndex(where: { $0.id == task }) else { return }
                        r.tasks[i].status = .working
                        r.tasks[i].age = "just now"
                        r.tasks[i].machine = r.tasks[i].machine ?? "studio"
                    }
                }
                return [(1.2, .planner([.p("Started **\(name.name)** on the Shelf, on \(name.machine ?? "studio").")]))]
            }
            return [(1.4, .planner([.p("Which task? Say its name as it reads on the Shelf.")]))]
        }
        if t.contains("first") {
            return [(1.2, .planner([.p(Chats.lookFirst)]))]
        }
        return [(1.4, .planner([.p("This is a prototype, so I only know a few lines. Try: what landed overnight, explain Codex, is anything on the Shelf out of date, quiz me, or start a task by its name.")]))]
    }

    func answerAsk(_ item: UUID, _ option: AskOption) {
        let id = currentID
        mutate(id) { r in
            if let i = r.chat.firstIndex(where: { $0.id == item }) { r.chat[i].kind = .answered(option.label) }
            r.planner = .working(Date())
        }
        let reply: String
        switch option.label {
        case "End it":
            reply = "Ended **Schedule history page**. The tab covers it."
            after(2) { [self] in removeSchedulePage(id, replace: false) }
        case "Cut it down":
            reply = "Done. **Schedule export** is on the Shelf, and the old task is ended."
            after(2) { [self] in removeSchedulePage(id, replace: true) }
        default:
            reply = "Left as it is. It'll be there at the Desk."
        }
        after(2.2) { [self] in deliver(id, [(0, .planner([.p(reply)]))]) }
    }

    private func removeSchedulePage(_ id: String, replace: Bool) {
        mutate(id) { r in
            r.tasks.removeAll { $0.name == "Schedule history page" }
            r.done.insert("Schedule history page", at: 0)
            if replace { r.tasks.append(WorkItem(name: "Schedule export", status: .waiting, desk: nil, firstMessage: "Export a schedule's firings and replies as CSV from the Schedules tab.")) }
        }
    }

    func setNet(_ value: Net) {
        let wasOffline = net == .machineDown || net == .noNetwork
        withAnimation(.snappy) { net = value }
        if wasOffline && value != .machineDown && value != .noNetwork { flushQueue() }
    }

    func walkOut() {
        setNet(.lan)
        after(1.2) { [self] in setNet(.relay) }
        after(7) { [self] in setNet(.internet) }
    }

    func flushQueue() {
        let id = currentID
        let waiting: [(UUID, String)] = repo.chat.compactMap { item in
            if case .me(let text, let r) = item.kind {
                if case .waitingMachine = r { return (item.id, text) }
            }
            return nil
        }
        for (item, text) in waiting {
            setReceipt(id, item, .sent)
            after(0.5) { [self] in begin(id, item, text) }
        }
        for t in repo.tasks {
            changeTask(t.id) { task in
                for i in task.thread.indices {
                    if case .waitingMachine = task.thread[i].receipt { task.thread[i].receipt = .read }
                }
            }
        }
    }

    func ageQueue() {
        let id = currentID
        mutate(id) { r in
            for i in r.chat.indices {
                if case .me(let t, .waitingMachine) = r.chat[i].kind { r.chat[i].kind = .me(t, .dayOld) }
            }
        }
    }

    func resendAged(_ item: UUID, keep: Bool) {
        let id = currentID
        mutate(id) { r in
            guard let i = r.chat.firstIndex(where: { $0.id == item }) else { return }
            if keep, case .me(let t, _) = r.chat[i].kind { r.chat[i].kind = .me(t, .waitingMachine(r.plannerMachine)) }
            else { r.chat.remove(at: i) }
        }
        if keep && !offline { flushQueue() }
    }

    func demoElsewhere() {
        let id = currentID
        mutate(id) { $0.planner = .elsewhere(Date().addingTimeInterval(-14 * 60)) }
        scheduledEnd = true
        after(12) { [self] in scheduledEnd = false; turnEnds(id) }
    }

    func demoStopPlanner() { mutate(currentID) { $0.planner = .stopped } }

    func demoPrompt(_ text: String) {
        showDemo = false
        tab = .chat
        send(text)
    }

    func movePlanner(to machine: String) {
        let id = currentID
        mutate(id) { $0.planner = .starting; $0.plannerMachine = machine }
        after(2) { [self] in
            mutate(id) {
                $0.planner = .idle
                $0.chat.append(ChatItem(kind: .sys("The planner for \(id) now runs on \(machine).")))
            }
        }
    }

    func task(_ id: UUID) -> WorkItem? { repo.tasks.first { $0.id == id } }

    func changeTask(_ id: UUID, _ change: (inout WorkItem) -> Void) {
        if let i = repo.tasks.firstIndex(where: { $0.id == id }) { change(&repo.tasks[i]) }
    }

    func deskCount(_ machine: String) -> Int { repo.tasks.filter { $0.desk == machine }.count }

    func pick(_ id: UUID) {
        if severalMacs && repo.desks.count > 1 {
            pendingPick = id
        } else if let desk = repo.desks.first {
            choose(desk: desk.machine, for: id)
        }
    }

    func choose(desk: String, for id: UUID) {
        pendingPick = nil
        if deskCount(desk) >= 3 {
            after(0.5) { [self] in
                pickFull = desk
                fullFor = id
            }
            return
        }
        changeTask(id) { $0.picking = true }
        after(0.9) { [self] in
            changeTask(id) { t in
                t.picking = false
                t.desk = desk
                if t.status == .waiting || t.status == .stopped {
                    t.status = .working
                    t.age = "just now"
                }
                t.machine = t.machine ?? desk
            }
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        }
    }

    var fullFor: UUID?

    func shelve(_ moved: UUID) {
        guard let id = fullFor, let desk = pickFull else { return }
        changeTask(moved) { $0.desk = nil }
        pickFull = nil
        fullFor = nil
        choose(desk: desk, for: id)
    }

    func message(task id: UUID, text: String) {
        guard let t = task(id) else { return }
        let repoID = currentID
        let msg: Message
        let time = clock()
        if offline {
            msg = Message(from: "you", time: time, text: text, you: true, receipt: .waitingMachine(repo.plannerMachine))
        } else if t.status == .waiting || t.status == .blocked {
            msg = Message(from: "you", time: time, text: text, you: true, receipt: .waitingStart)
        } else {
            msg = Message(from: "you", time: time, text: text, you: true, receipt: t.status == .working ? .waitingTurn : .sent)
        }
        changeTask(id) { $0.thread.append(msg) }
        guard !offline, t.status != .waiting, t.status != .blocked else { return }
        let wasNeeds = t.status == .needs
        let stopped = t.status == .stopped || t.status == .idle
        if wasNeeds || stopped { after(0.6) { [self] in withRepo(repoID) { changeTask(id) { $0.status = .working; $0.age = "just now" } } } }
        after(wasNeeds || stopped ? 1 : 5) { [self] in
            withRepo(repoID) { changeTask(id) { t in
                if let i = t.thread.firstIndex(where: { $0.id == msg.id }) { t.thread[i].receipt = .read }
            } }
            after(3) { [self] in
                withRepo(repoID) { changeTask(id) { t in
                    t.thread.append(Message(from: t.name, time: time, text: "Got it. Carrying on with that."))
                } }
            }
        }
    }

    private func withRepo(_ id: String, _ work: () -> Void) {
        let saved = currentID
        currentID = id
        work()
        currentID = saved
    }

    func startRound(def: RoundDef? = nil, cardItem: UUID? = nil) {
        if let play, !play.finished, def == nil || play.def.id == def?.id {
            roundOpen = true
            return
        }
        let id = currentID
        var chosen = def ?? nextDef()
        if repo.missedOnce && chosen.id == "machines" {
            chosen.questions[0] = Fixtures.mergeBack
        }
        play = RoundPlay(def: chosen, startScore: repo.points, cardItem: cardItem)
        roundOpen = true
        if def == nil {
            mutate(id) { $0.nextRound = .writing; $0.nextIndex += 1 }
            scheduleWriter(id)
        } else if repo.extraRound?.id == def?.id {
            mutate(id) { $0.extraRound = nil }
        }
    }

    func scheduleWriter(_ id: String) {
        guard !slowWriter, !hold else { return }
        after(14) { [self] in if !slowWriter { mutate(id) { $0.nextRound = .ready } } }
    }

    func nextDef() -> RoundDef {
        repo.nextIndex % 2 == 0 ? Fixtures.today : Fixtures.machines5
    }

    func answer(_ choice: Int) {
        guard var p = play, !p.revealed else { return }
        let q = p.current
        p.answers.append(choice)
        p.revealed = true
        let right = choice == q.correct
        if right {
            p.run += 1
            p.earned += 20
            repo.points += 20
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } else {
            p.run = 0
            repo.missedOnce = true
            UINotificationFeedbackGenerator().notificationOccurred(.warning)
        }
        withAnimation(.snappy) { play = p }
    }

    func nextQuestion() {
        guard var p = play else { return }
        if p.index + 1 >= p.count {
            finishRound(&p)
        } else {
            p.index += 1
            p.revealed = false
        }
        withAnimation(.snappy) { play = p }
    }

    private func finishRound(_ p: inout RoundPlay) {
        p.finished = true
        p.earned += 5
        repo.points += 5
        if !repo.playedToday {
            repo.streak += 1
            repo.playedToday = true
        }
        var gains: [String: Int] = [:]
        for (a, q) in zip(p.answers, p.def.questions) { gains[q.subsystem, default: 0] += a == q.correct ? 2 : -1 }
        var moves: [(String, Int, Int)] = []
        for (name, gain) in gains.sorted(by: { $0.key < $1.key }) {
            guard let i = repo.map.firstIndex(where: { $0.name == name }) else { continue }
            let old = repo.map[i].pct ?? 0
            let new = min(100, max(0, old + gain))
            repo.map[i].pct = new
                moves.append((name, old, new))
        }
        p.moves = moves
        let right = p.right
        if let card = p.cardItem {
            mutate(currentID) { r in
                if let i = r.chat.firstIndex(where: { $0.id == card }), case .round(let id, _) = r.chat[i].kind { r.chat[i].kind = .round(id, right) }
            }
        }
    }

    func closeRound() {
        roundOpen = false
        if play?.finished == true { play = nil }
    }

    func anotherRound() {
        guard repo.nextRound == .ready else { return }
        play = nil
        startRound()
    }

    func quizMe(_ subsystem: String) {
        let id = currentID
        mutate(id) { $0.quizMeWriting = subsystem }
        after(4.5) { [self] in
            let def = Fixtures.subsystemRound(subsystem)
            mutate(id) {
                $0.quizMeWriting = nil
                $0.extraRound = def
                $0.chat.append(ChatItem(kind: .round(def.id, nil)))
            }
        }
    }

    func roundDef(_ id: String) -> RoundDef {
        if id == "machines" { return Fixtures.machines5 }
        if id.hasPrefix("sub-") { return Fixtures.subsystemRound(String(id.dropFirst(4))) }
        return Fixtures.today
    }
}
