import SwiftUI

struct SceneInfo: Identifiable {
    var id: String { key }
    let key: String
    let name: String
}

enum Scenes {
    static let all: [(String, [SceneInfo])] = [
        ("Chat", [SceneInfo(key: "chat-answer", name: "An answer"), SceneInfo(key: "chat-working", name: "The planner working"), SceneInfo(key: "chat-queued", name: "A message while it's busy"), SceneInfo(key: "chat-question", name: "A question in chat"), SceneInfo(key: "chat-answered", name: "The question answered"), SceneInfo(key: "chat-code", name: "Code it shows"), SceneInfo(key: "code-full", name: "The code, full screen"), SceneInfo(key: "chat-none", name: "No planner running"), SceneInfo(key: "chat-quiz", name: "Asking for a quiz")]),
        ("Repos", [SceneInfo(key: "repos-sheet", name: "The repo switcher")]),
        ("Connection", [SceneInfo(key: "conn-direct", name: "Direct"), SceneInfo(key: "conn-relay", name: "Relayed"), SceneInfo(key: "conn-offline", name: "Offline: the conversation"), SceneInfo(key: "conn-shelf", name: "Offline: the Shelf")]),
        ("Shelf", [SceneInfo(key: "shelf-list", name: "The repo's tasks"), SceneInfo(key: "task-new", name: "A task on the Shelf"), SceneInfo(key: "task-running", name: "A running task"), SceneInfo(key: "task-needs", name: "A task that needs you"), SceneInfo(key: "task-blocked", name: "A blocked task"), SceneInfo(key: "pick-sheet", name: "Pick: which Mac"), SceneInfo(key: "pick-full", name: "Pick: a full Desk")]),
        ("Machines", [SceneInfo(key: "mach-list", name: "This repo's machines"), SceneInfo(key: "mach-detail", name: "A machine")]),
        ("Stats", [SceneInfo(key: "stats-top", name: "Stats: the top"), SceneInfo(key: "stats-more", name: "Stats: further down"), SceneInfo(key: "stats-scrub", name: "Stats: a day's value")]),
        ("Quiz", [SceneInfo(key: "quiz-home", name: "The mastery map"), SceneInfo(key: "quiz-sub", name: "A subsystem"), SceneInfo(key: "quiz-q", name: "A question"), SceneInfo(key: "quiz-right", name: "Answered right"), SceneInfo(key: "quiz-miss", name: "Answered wrong: the code"), SceneInfo(key: "quiz-bug", name: "Spot the bug"), SceneInfo(key: "quiz-drawing", name: "Pick the drawing"), SceneInfo(key: "quiz-end", name: "The end of the round"), SceneInfo(key: "quiz-none", name: "The next round not ready")]),
        ("Pairing", [SceneInfo(key: "pair-start", name: "First launch"), SceneInfo(key: "pair-scan", name: "Scanning"), SceneInfo(key: "pair-confirm", name: "Confirm on the Mac"), SceneInfo(key: "pair-done", name: "Paired")]),
    ]
}

extension Store {
    func taskID(_ name: String) -> UUID { repo.tasks.first { $0.name == name }?.id ?? UUID() }

    private func roundAt(_ index: Int, answers: [Int], revealed: Bool, points: Int) {
        var p = RoundPlay(def: Fixtures.today)
        p.index = index
        p.answers = answers
        p.revealed = revealed
        p.run = 0
        repo.points = points
        play = p
        roundOpen = true
    }

    func apply(scene: String) {
        reset()
        paired = true
        let hold = self.hold
        switch scene {
        case "chat-answer": repo.chat = Chats.answer
        case "chat-working":
            repo.chat = Chats.working
            repo.planner = .working(Date().addingTimeInterval(-125))
            if !hold { continueWith("Why are there no merges for recipe in Stats?", "Stats counts merges on main, and recipe has only squashed commits, so it sees none.") }
        case "chat-queued":
            repo.chat = Chats.queued
            repo.planner = .elsewhere(Date().addingTimeInterval(-14 * 60))
            if !hold {
                let item = repo.chat.last?.id
                after(5) { [self] in
                    mutate("kanna") { r in
                        if let i = r.chat.firstIndex(where: { $0.id == item }), case .me(let t, _) = r.chat[i].kind { r.chat[i].kind = .me(t, .read) }
                        r.planner = .working(Date())
                    }
                    after(3) { [self] in
                        mutate("kanna") { $0.chat.append(ChatItem(kind: .planner([.p("Stats counts merges on main, and recipe has only squashed commits, so it sees none.")]))) }
                        turnEnds("kanna")
                    }
                }
            }
        case "chat-question": repo.chat = Chats.question; repo.planner = .asks
        case "chat-answered":
            repo.chat = Chats.answered
            repo.planner = .working(Date())
            if !hold { continueWith("", "Done. **Schedule export** is on the Shelf, and the old task is ended.") }
        case "chat-code": repo.chat = Chats.code
        case "code-full": repo.chat = Chats.code; fullLine = 161; fullCode = FullCode(snippet: Snippets.codex)
        case "chat-none":
            repo.chat = Chats.none
            repo.planner = .starting
            if !hold {
                let item = repo.chat.first { if case .me = $0.kind { return true } else { return false } }?.id
                after(3) { [self] in
                    mutate("kanna") { r in
                        if let i = r.chat.firstIndex(where: { $0.id == item }), case .me(let t, _) = r.chat[i].kind { r.chat[i].kind = .me(t, .read) }
                        r.planner = .working(Date())
                    }
                    after(2) { [self] in
                        mutate("kanna") { $0.chat.append(ChatItem(kind: .planner([.p("Started **Keep terminal output**. It picks up where it stopped.")]))) }
                        turnEnds("kanna")
                    }
                }
            }
        case "chat-quiz": repo.chat = Chats.quiz
        case "repos-sheet": repo.chat = Chats.switcher; showRepos = true
        case "conn-direct": repo.chat = Chats.pathOnly; net = .internet
        case "conn-relay": repo.chat = Chats.pathOnly; net = .relay; after(0.8) { [self] in showChipPop = true }
        case "conn-offline": repo.chat = Chats.offline; net = .machineDown
        case "conn-shelf": tab = .shelf; net = .machineDown
        case "shelf-list": tab = .shelf
        case "task-new": tab = .shelf; shelfPath = [taskID("Native diff viewer")]
        case "task-running":
            tab = .shelf
            let id = taskID("Keep terminal output")
            changeTask(id) { $0.thread.append(Message(from: "you", time: "9:40", text: "Keep the 10,000 line limit for now.", you: true, receipt: .waitingTurn)) }
            shelfPath = [id]
        case "task-needs": tab = .shelf; shelfPath = [taskID("Review build output")]
        case "task-blocked": tab = .shelf; shelfPath = [taskID("Check tab restoration")]
        case "pick-sheet":
            tab = .shelf
            let id = taskID("Native diff viewer")
            shelfPath = [id]
            pendingPick = id
        case "pick-full":
            tab = .shelf
            let id = taskID("Native diff viewer")
            shelfPath = [id]
            pickFull = "mbp"
            fullFor = id
        case "mach-list": tab = .machines
        case "mach-detail": tab = .machines; machinePath = [.machine("studio")]
        case "stats-top": tab = .stats
        case "stats-more": tab = .stats; scrollStats = "Tasks"
        case "stats-scrub": tab = .stats; scrollStats = "Tasks"; scrub = 21
        case "quiz-home": tab = .quiz
        case "quiz-sub": tab = .quiz; quizPath = ["Machines"]
        case "quiz-q": tab = .quiz; roundAt(1, answers: [1], revealed: false, points: 1255)
        case "quiz-right": tab = .quiz; roundAt(1, answers: [1, 0], revealed: true, points: 1275); play?.run = 2
        case "quiz-miss": tab = .quiz; roundAt(2, answers: [1, 0, 0], revealed: true, points: 1275)
        case "quiz-bug": tab = .quiz; roundAt(4, answers: [1, 0, 0, 0], revealed: false, points: 1295)
        case "quiz-drawing": tab = .quiz; roundAt(0, answers: [], revealed: false, points: 1240)
        case "quiz-end":
            tab = .quiz
            roundAt(4, answers: [1, 0, 0, 0, 3], revealed: false, points: 1320)
            repo.points = 1240
            var p = play
            p?.finished = true
            p?.earned = 85
            p?.moves = [("Daemon", 72, 76), ("The app", 88, 89)]
            play = p
            repo.streak = 7
            repo.points = 1325
            repo.nextRound = .ready
        case "quiz-none": tab = .quiz; repo.streak = 7; repo.points = 1325; repo.nextRound = .writing; scheduleWriter("kanna")
        case "pair-start": paired = false; pairStep = .start
        case "pair-scan": paired = false; pairStep = .scan
        case "pair-confirm": paired = false; pairStep = .confirm
        case "pair-done": paired = false; pairStep = .done
        default: break
        }
    }

    private func continueWith(_ text: String, _ reply: String) {
        after(3) { [self] in
            mutate("kanna") { $0.chat.append(ChatItem(kind: .planner([.p(reply)]))) }
            turnEnds("kanna")
        }
    }

    func jump(_ scene: String) {
        showDemo = false
        after(0.35) { [self] in apply(scene: scene) }
    }

    private func reset() {
        repos = Fixtures.repos
        currentID = "kanna"
        tab = .chat
        shelfPath = []
        machinePath = []
        quizPath = []
        net = .lan
        showRepos = false
        showChipPop = false
        fullCode = nil
        roundOpen = false
        play = nil
        pendingPick = nil
        pickFull = nil
        scrub = nil
        scrollStats = nil
        nextGeneration()
    }
}
