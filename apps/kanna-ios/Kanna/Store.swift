import Foundation
import Network
import Observation
import SwiftUI

@MainActor
@Observable
final class Store {
    @ObservationIgnored let backend: Backend
    @ObservationIgnored private let keep: Keep
    @ObservationIgnored private var items: [String: [Item]] = [:]
    @ObservationIgnored private var queued: [Queued] = []
    @ObservationIgnored private var snippets: [String: Snippet] = [:]
    @ObservationIgnored private var monitor: NWPathMonitor?
    @ObservationIgnored private var active = false
    @ObservationIgnored private var waking = false
    @ObservationIgnored private var flushing = false
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var shelves: [String: ShelfPage] = [:]
    @ObservationIgnored private var pages: [String: TaskPage] = [:]
    @ObservationIgnored private var listing = false
    @ObservationIgnored private var relist = false
    @ObservationIgnored private var askedStats = Date.distantPast
    @ObservationIgnored private var asleep: Set<String> = []

    let fixture: Bool
    var paired: Bool
    var pairStep = PairStep.start
    var pairError: String?
    var repos: [Repo] = []
    var currentID = UserDefaults.standard.string(forKey: "repo") ?? ""
    var showRepos = false
    var showChipPop = false
    var wrapByDefault = true
    var fullLine: Int?
    var plannerPath = LinkPath.off
    var linked: Reach?
    var linkPath = LinkPath.off
    var machinePages: [String: MachinesPage] = [:]
    var statsPages: [String: StatsPage] = [:]
    var moving = false
    var noNetwork = false
    var roundTrip = 0
    var lastSeen: Date?
    var tab = AppTab.chat
    var shelfPath: [String] = []
    var pendingPick: String?
    var pickFull: String?
    var fullFor: String?
    var pickError: String?
    var picking: Set<String> = []

    static func launch() -> Store {
        let defaults = UserDefaults.standard
        if defaults.bool(forKey: "fixture") {
            return Store(backend: FixtureBackend(paired: defaults.bool(forKey: "paired"), planner: defaults.string(forKey: "planner") ?? "idle"), fixture: true)
        }
        return Store(backend: LiveBackend(), fixture: false)
    }

    init(backend: Backend, fixture: Bool) {
        self.backend = backend
        self.fixture = fixture
        keep = Keep(memory: fixture)
        paired = backend.paired != nil
        backend.onNotice = { [weak self] in self?.notice($0) }
        backend.onLost = { [weak self] in self?.lost() }
        queued = keep.queued()
        for kept in keep.repos() {
            items[kept.id] = keep.items(kept.id, machine: kept.machine)
            shelves[kept.id] = keep.page("shelf/\(kept.id)")
            for task in shelves[kept.id]?.tasks ?? [] { pages[task.id] = keep.page("task/\(task.id)") }
            machinePages[kept.id] = keep.page("machines/\(kept.id)")
            statsPages[kept.id] = keep.page("stats/\(kept.id)")
            repos.append(Repo(id: kept.id, name: kept.name, plannerMachine: kept.machine, plannerKey: kept.key, planner: Planner(status: kept.status, since: kept.since)))
            rebuild(kept.id)
        }
        lastSeen = UserDefaults.standard.object(forKey: "seen") as? Date
    }

    var repo: Repo {
        repos.first { $0.id == currentID } ?? repos.first ?? Repo(id: "", name: "Kanna", plannerMachine: machineName, planner: .stopped)
    }

    var offline: Bool { plannerPath == .off }
    var phoneName: String { backend.phoneName }
    var machineName: String { backend.paired?.machine ?? "the Mac" }
    var asOf: String { lastSeen.map { Self.clock.string(from: $0) } ?? "never" }

    private static let clock: DateFormatter = {
        let format = DateFormatter()
        format.timeStyle = .short
        format.dateStyle = .none
        return format
    }()

    var machinesPage: MachinesPage? { machinePages[repo.id] }
    var statsPage: StatsPage? { statsPages[repo.id] }
    var onlyThroughOther: Bool { linked != nil && offline }

    func milliseconds(_ machine: String) -> Int {
        if machine == repo.plannerMachine { return roundTrip }
        return Int((machinesPage?.machines.first { $0.name == machine }?.ms ?? 0).rounded())
    }

    func path(_ machine: String) -> LinkPath {
        if machine == linked?.name { return linkPath }
        guard let found = machinesPage?.machines.first(where: { $0.name == machine }), found.up else { return .off }
        switch (found.path, found.lan) {
        case ("direct", true): return .lan
        case ("direct", _): return .net
        case ("relay", _): return .relay
        default: return .off
        }
    }

    func foreground(_ on: Bool) {
        active = on
        if on {
            watchNetwork()
            wake()
        } else {
            generation += 1
            backend.close()
            unlinked()
        }
    }

    private func unlinked() {
        plannerPath = .off
        linkPath = .off
        linked = nil
    }

    private func reach(_ key: String) -> Reach? {
        guard !key.isEmpty else { return nil }
        return backend.paired?.known?.first { $0.key == key }
    }

    private func others(than key: String) -> [Reach] {
        (backend.paired?.known ?? []).filter { $0.key != key }
    }

    private func watchNetwork() {
        guard monitor == nil, !fixture else { return }
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            let up = path.status == .satisfied
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.noNetwork = !up
                    if up { self?.wake() }
                }
            }
        }
        monitor.start(queue: .global(qos: .utility))
        self.monitor = monitor
    }

    private func wake() {
        guard active, backend.paired != nil, linked == nil, !waking else { return }
        waking = true
        generation += 1
        let mine = generation
        Task {
            defer { if mine == generation { waking = false } }
            do {
                let target = reach(repo.plannerKey)
                let hello = try await dial(target, mine)
                guard mine == generation else { return }
                arrived(hello, on: hello.key)
                await measure()
                try await catchUp(currentID)
                flush()
                poll(mine)
                if repo.plannerKey != hello.key, !onlyThroughOther { relink() }
            } catch let refused as Refused where refused.code == "revoked" {
                forgetPairing()
            } catch {
                guard mine == generation else { return }
                unlinked()
                retry()
            }
        }
    }

    private func dial(_ target: Reach?, _ mine: Int) async throws -> Hello {
        do {
            let hello = try await backend.connect(to: target)
            asleep.remove(hello.key)
            return hello
        } catch let refused as Refused where refused.code == "revoked" {
            throw refused
        } catch {
            asleep.insert(target?.key ?? backend.paired?.key ?? "")
            for other in others(than: target?.key ?? backend.paired?.key ?? "") {
                guard mine == generation else { throw error }
                if let hello = try? await backend.connect(to: other) { return hello }
            }
            throw error
        }
    }

    private func relink() {
        generation += 1
        backend.close()
        unlinked()
        waking = false
        wake()
    }

    private func retry() {
        Task {
            try? await Task.sleep(for: .seconds(3))
            wake()
        }
    }

    private func poll(_ mine: Int) {
        Task {
            while mine == generation, linked != nil {
                try? await Task.sleep(for: .seconds(5))
                guard mine == generation else { return }
                await measure()
                if tab == .shelf { refreshShelf() }
                if tab == .machines { refreshMachines() }
                if tab == .stats { refreshStats() }
                if onlyThroughOther { await comeBack(mine) }
            }
        }
    }

    private func comeBack(_ mine: Int) async {
        let id = repo.id
        guard let page = try? await backend.machines(repo: id), mine == generation else { return }
        applyMachines(page, repo: id)
        if page.machines.contains(where: { $0.key == repo.plannerKey && $0.up }) { relink() }
    }

    private func measure() async {
        let link = await backend.link()
        guard link.path != .off else { return }
        linkPath = link.path
        if linked?.key == repo.plannerKey || repo.plannerKey.isEmpty {
            plannerPath = link.path
            roundTrip = link.milliseconds
        }
        seen()
    }

    private func seen() {
        lastSeen = Date()
        UserDefaults.standard.set(lastSeen, forKey: "seen")
    }

    private func lost() {
        unlinked()
        retry()
    }

    private func arrived(_ hello: Hello, on key: String) {
        seen()
        linked = Reach(name: hello.name, key: key, addrs: hello.addrs)
        let previous = repos
        repos = hello.projects.map { project in
            var repo = previous.first { $0.id == project.repo } ?? Repo(id: project.repo, name: project.name, plannerMachine: hello.name, planner: .stopped)
            repo.name = project.name
            repo.plannerMachine = project.planner ?? hello.name
            repo.plannerKey = project.plannerKey ?? key
            repo.down = asleep.contains(repo.plannerKey)
            repo.lastSeen = asOf
            if let status = project.status { repo.planner = Planner(status: status, since: project.since ?? 0) }
            return repo
        }
        if !repos.contains(where: { $0.id == currentID }) { currentID = repos.first?.id ?? "" }
        for old in keep.repos() { keep.context.delete(old) }
        for (order, repo) in repos.enumerated() {
            let status: String
            switch repo.planner {
            case .working: status = "working"
            case .elsewhere: status = "elsewhere"
            case .asks: status = "asks"
            case .blocked: status = "blocked"
            case .starting: status = "starting"
            case .idle: status = "idle"
            case .stopped: status = "not_running"
            }
            keep.context.insert(KeptRepo(id: repo.id, name: repo.name, machine: repo.plannerMachine, key: repo.plannerKey, status: status, since: 0, order: order))
        }
        keep.save()
        repos.map(\.id).forEach(rebuild)
        pairError = nil
    }

    private func reread() {
        guard !offline || onlyThroughOther else { return }
        Task {
            guard let hello = try? await backend.hello(), let key = linked?.key else { return }
            arrived(hello, on: key)
            if repo.plannerKey != key { relink() }
        }
    }

    private func catchUp(_ id: String) async throws {
        guard !id.isEmpty else { return }
        let here = linked?.name
        let held = (items[id] ?? []).filter { $0.machine == here }
        let unread = held.first { $0.kind == "me" && $0.read == nil }?.seq
        let after = max(0, (unread ?? (held.last?.seq ?? 0) + 1) - 1)
        let page = try await backend.read(repo: id, after: after)
        merge(page.items, repo: id)
        change(id) { $0.planner = Planner(status: page.status, since: page.since) }
        await list(id)
    }

    private func merge(_ fresh: [Item], repo id: String) {
        let era = fresh.first?.machine ?? linked?.name ?? repos.first { $0.id == id }?.plannerMachine ?? ""
        var held = Dictionary((items[id] ?? []).map { ("\($0.machine ?? "")/\($0.seq)", $0) }, uniquingKeysWith: { $1 })
        for var item in fresh {
            item.machine = era
            held["\(era)/\(item.seq)"] = item
            keep.put(item, repo: id)
            if let client = item.client, let done = queued.first(where: { $0.id == client }) {
                queued.removeAll { $0.id == client }
                keep.context.delete(done)
            }
        }
        keep.save()
        items[id] = held.values.sorted { ($0.at, $0.seq) < ($1.at, $1.seq) }
        rebuild(id)
    }

    private func change(_ id: String, _ apply: (inout Repo) -> Void) {
        guard let index = repos.firstIndex(where: { $0.id == id }) else { return }
        apply(&repos[index])
        rebuild(id)
    }

    private func notice(_ notice: Notice) {
        seen()
        switch notice {
        case .chat(let repo, let item): merge([item], repo: repo)
        case .planner(let repo, let status, let since): change(repo) { $0.planner = Planner(status: status, since: since) }
        case .tasks(let repo): if repo == currentID { refreshShelf() }
        case .projects: reread()
        }
    }

    private func rebuild(_ id: String) {
        guard let index = repos.firstIndex(where: { $0.id == id }) else { return }
        let planner = repos[index].planner
        let machine = repos[index].plannerMachine
        var chat: [ChatItem] = []
        var last = ""
        var previous: Double?
        var era: String?
        func moved(to place: String, id key: String) {
            chat.append(ChatItem(id: key, kind: .sys("The planner is on \(place) now. It doesn't know this conversation.")))
        }
        for item in items[id] ?? [] {
            let from = item.machine ?? machine
            if let era, era != from { moved(to: from, id: "\(from)/\(item.seq)-moved") }
            era = from
            if previous == nil || item.at - (previous ?? 0) > 3600 {
                chat.append(ChatItem(id: "\(from)/\(item.seq)-when", kind: .when(Self.clock.string(from: Date(timeIntervalSince1970: item.at)))))
            }
            previous = item.at
            let key = "\(from)/\(item.seq)"
            switch item.kind {
            case "me":
                if item.doc.choice == nil { chat.append(ChatItem(id: key, kind: .me(item.doc.text ?? "", receipt(item, planner)), seq: item.seq)) }
            case "say":
                last = item.doc.text ?? last
                chat.append(ChatItem(id: key, kind: .planner(Self.blocks(item.doc.text ?? "")), seq: item.seq))
            case "ask":
                last = item.doc.text ?? last
                chat.append(ChatItem(id: key, kind: .planner(Self.blocks(item.doc.text ?? "")), seq: item.seq))
                if let chosen = item.doc.chosen {
                    chat.append(ChatItem(id: "\(key)-chosen", kind: .answered(chosen), seq: item.seq))
                } else if from == machine {
                    chat.append(ChatItem(id: "\(key)-ask", kind: .ask((item.doc.choices ?? []).map { AskOption(label: $0.label, why: $0.why ?? "") }), seq: item.seq))
                }
            case "show":
                chat.append(ChatItem(id: key, kind: .planner([.code(snippet(item, repo: id))]), seq: item.seq))
            case "sys":
                chat.append(ChatItem(id: key, kind: .sys(item.doc.text ?? ""), seq: item.seq))
            default: break
            }
        }
        if let era, era != machine { moved(to: machine, id: "\(machine)-moved") }
        for message in queued where message.repo == id && message.task == nil {
            let old = Date().timeIntervalSince(message.at) > 86400
            chat.append(ChatItem(id: message.id, kind: .me(message.text, old ? .dayOld : offline ? .waitingMachine(machine) : .sent)))
        }
        repos[index].chat = chat
        repos[index].lastLine = last
        rebuildShelf(index)
    }

    private func rebuildShelf(_ index: Int) {
        let id = repos[index].id
        let shelf = shelves[id]
        repos[index].desks = shelf?.desks ?? []
        repos[index].done = shelf?.done ?? []
        repos[index].tasks = (shelf?.tasks ?? []).map { work($0, repo: id, machine: repos[index].plannerMachine) }
    }

    private func work(_ task: ShelfTask, repo: String, machine: String) -> WorkItem {
        let page = pages[task.id]
        let status = Status(wire: task.status)
        var item = WorkItem(id: task.id, name: task.name, status: status, desk: task.desk, machine: task.machine)
        item.agent = task.agent ?? "claude"
        item.age = Self.age(task.since)
        item.after = (task.after ?? []).map { $0.name ?? $0.id }.joined(separator: ", ")
        item.branch = task.branch
        item.firstMessage = page?.task.message ?? task.message
        item.unreachable = task.unreachable == true
        item.picking = picking.contains(task.id)
        var thread: [Message] = (page?.thread ?? []).enumerated().map { index, line in
            let time = Self.clock.string(from: Date(timeIntervalSince1970: line.at))
            return Message(id: "\(task.id)/\(index)", from: line.from, time: time, text: line.text, you: line.you, receipt: line.you ? Self.receipt(line, status) : .none, at: line.at)
        }
        func reply(_ id: String, _ at: Double, _ said: [ChatItem]) {
            let message = Message(id: id, from: task.name, time: Self.clock.string(from: Date(timeIntervalSince1970: at)), text: "", at: at, said: said)
            thread.insert(message, at: thread.firstIndex { $0.at > at } ?? thread.count)
        }
        for words in page?.said ?? [] {
            let id = "\(task.id)/said/\(words.at)"
            reply(id, words.at, [ChatItem(id: id, kind: .planner(Self.blocks(words.text)))])
        }
        for entry in page?.items ?? [] {
            let id = "\(task.id)/item/\(entry.seq)"
            let answered = page?.thread.contains { $0.you && $0.at > entry.at } == true
            reply(id, entry.at, taskItems(entry, task: task.id, id: id, answered: answered))
        }
        let sent = Set((page?.thread ?? []).compactMap(\.client))
        for message in queued where message.task == task.id && !sent.contains(message.id) {
            let old = Date().timeIntervalSince(message.at) > 86400
            thread.append(Message(id: message.id, from: "you", time: Self.clock.string(from: message.at), text: message.text, you: true, receipt: old ? .dayOld : offline ? .waitingMachine(machine) : .sent))
        }
        item.thread = thread
        return item
    }

    private func taskItems(_ entry: Item, task: String, id: String, answered: Bool) -> [ChatItem] {
        if entry.kind == "show" {
            return [ChatItem(id: id, kind: .planner([.code(snippet(entry, repo: task, task: task))]), seq: entry.seq, task: task)]
        }
        var said = [ChatItem(id: id, kind: .planner(Self.blocks(entry.doc.text ?? "")), seq: entry.seq, task: task)]
        if entry.kind == "ask" {
            if let chosen = entry.doc.chosen {
                said.append(ChatItem(id: "\(id)-chosen", kind: .answered(chosen), seq: entry.seq, task: task))
            } else if !answered {
                said.append(ChatItem(id: "\(id)-ask", kind: .ask((entry.doc.choices ?? []).map { AskOption(label: $0.label, why: $0.why ?? "") }), seq: entry.seq, task: task))
            }
        }
        return said
    }

    private static func receipt(_ line: ThreadLine, _ status: Status) -> Receipt {
        if line.read != nil { return .read }
        switch status {
        case .waiting, .blocked, .stopped, .ready, .stale: return .waitingStart
        case .working, .needs: return .waitingTurn
        default: return .sent
        }
    }

    private static func age(_ since: Double) -> String {
        let seconds = Int(Date().timeIntervalSince1970 - since)
        if seconds < 60 { return "just now" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        if seconds < 86400 { return "\(seconds / 3600)h" }
        return seconds < 172800 ? "yesterday" : "\(seconds / 86400)d"
    }

    private func applyShelf(_ page: ShelfPage, repo id: String) {
        shelves[id] = page
        keep.put(page: page, key: "shelf/\(id)")
        guard let index = repos.firstIndex(where: { $0.id == id }) else { return }
        rebuildShelf(index)
    }

    private func list(_ id: String) async {
        guard !id.isEmpty else { return }
        if let page = try? await backend.tasks(repo: id) { applyShelf(page, repo: id) }
    }

    func refreshShelf() {
        guard !offline else { return }
        guard !listing else {
            relist = true
            return
        }
        listing = true
        Task {
            defer { listing = false }
            repeat {
                relist = false
                await list(currentID)
                if let open = shelfPath.last { await read(open) }
            } while relist
        }
    }

    func read(_ id: String) async {
        guard let page = try? await backend.task(id) else { return }
        pages[id] = page
        keep.put(page: page, key: "task/\(id)")
        rebuild(currentID)
    }

    private func receipt(_ item: Item, _ planner: Planner) -> Receipt {
        if item.read != nil { return .read }
        switch planner {
        case .working, .elsewhere, .asks, .blocked: return .waitingTurn
        case .starting, .stopped: return .waitingPlanner
        case .idle: return .sent
        }
    }

    static func blocks(_ text: String) -> [Block] {
        text.components(separatedBy: "\n\n").compactMap { part in
            let lines = part.split(separator: "\n").map(String.init)
            guard !lines.isEmpty else { return nil }
            if lines.allSatisfy({ $0.hasPrefix("- ") }) { return .list(lines.map { String($0.dropFirst(2)) }) }
            return .p(part)
        }
    }

    private func snippet(_ item: Item, repo: String, task: String? = nil) -> Snippet {
        let key = "\(repo)/\(item.machine ?? "")/\(item.seq)"
        if let kept = snippets[key] { return kept }
        let lines = item.doc.lines ?? []
        let file = item.doc.file ?? ""
        let first = item.doc.line ?? 1
        let marked = item.doc.hot ?? []
        let hot = marked.count == 2 && marked[0] <= marked[1] ? marked[0]...marked[1] : 0...0
        var offset = 0
        var coded: [CodeLine] = []
        let spans = backend.highlight(file: file, text: lines.joined(separator: "\n"))
        for (index, text) in lines.enumerated() {
            let length = text.utf8.count
            let inside = spans.filter { $0.start < offset + length && $0.end > offset }.map { TextSpan(start: max($0.start, offset) - offset, end: min($0.end, offset + length) - offset, capture: $0.capture) }
            coded.append(CodeLine(first + index, text, hot: hot.contains(first + index), spans: inside))
            offset += length + 1
        }
        let made = Snippet(id: key, file: file, lines: coded, commit: String((item.doc.commit ?? "").prefix(7)), task: task)
        snippets[key] = made
        return made
    }

    func task(_ id: String) -> WorkItem? { repo.tasks.first { $0.id == id } }

    func deskCount(_ machine: String) -> Int { repo.tasks.filter { $0.desk == machine }.count }

    func pick(_ id: String) {
        guard !offline else { return }
        if repo.desks.count > 1 {
            pendingPick = id
        } else if let desk = repo.desks.first {
            choose(desk: desk.machine, for: id)
        } else {
            pickError = "None of your machines is a Mac, so there's no Desk to pick it for."
        }
    }

    func choose(desk: String, for id: String) {
        pendingPick = nil
        if deskCount(desk) >= 3 {
            pickFull = desk
            fullFor = id
            return
        }
        landPick(id, desk: desk, room: nil)
    }

    func shelve(_ moved: String) {
        guard let id = fullFor, let desk = pickFull else { return }
        pickFull = nil
        fullFor = nil
        landPick(id, desk: desk, room: moved)
    }

    private func landPick(_ id: String, desk: String, room: String?) {
        let index = repos.firstIndex { $0.id == currentID }
        picking.insert(id)
        if let index { rebuildShelf(index) }
        Task {
            defer {
                picking.remove(id)
                if let index, index < repos.count { rebuildShelf(index) }
            }
            do {
                try await backend.pick(task: id, desk: desk, room: room)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
            } catch let refused as Refused where refused.code == "task_limit" {
                pickFull = desk
                fullFor = id
            } catch {
                pickError = error.localizedDescription
            }
            await list(currentID)
            await read(id)
        }
    }

    func message(task id: String, text: String) {
        let message = Queued(id: UUID().uuidString, repo: currentID, text: text, at: Date(), task: id)
        queued.append(message)
        keep.context.insert(message)
        keep.save()
        rebuild(currentID)
        flush()
    }

    func selectRepo(_ id: String) {
        currentID = id
        UserDefaults.standard.set(id, forKey: "repo")
        showRepos = false
        guard paired else { return }
        if linked == nil || (repo.plannerKey != linked?.key && !repo.plannerKey.isEmpty) {
            relink()
            return
        }
        plannerPath = linkPath
        guard !offline else { return }
        Task { try? await catchUp(id) }
        afterTabChange()
    }

    func afterTabChange() {
        switch tab {
        case .shelf: refreshShelf()
        case .machines: refreshMachines()
        case .stats:
            askedStats = .distantPast
            refreshStats()
        case .chat: break
        }
    }

    private func applyMachines(_ page: MachinesPage, repo id: String) {
        machinePages[id] = page
        keep.put(page: page, key: "machines/\(id)")
    }

    func refreshMachines() {
        guard linked != nil else { return }
        let id = currentID
        Task { if let page = try? await backend.machines(repo: id) { applyMachines(page, repo: id) } }
    }

    func refreshStats() {
        guard !offline, Date().timeIntervalSince(askedStats) >= 60 else { return }
        askedStats = Date()
        let id = currentID
        Task {
            guard let page = try? await backend.stats(repo: id) else { return }
            statsPages[id] = page
            keep.put(page: page, key: "stats/\(id)")
        }
    }

    func movePlanner(to machine: String) {
        guard linked != nil, !moving else { return }
        moving = true
        let id = currentID
        Task {
            defer { moving = false }
            do {
                let moved = try await backend.move(repo: id, machine: machine)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                change(id) {
                    $0.plannerMachine = moved.planner
                    $0.plannerKey = moved.plannerKey
                    $0.planner = .starting
                }
                relink()
            } catch {
                pickError = "Couldn't move the planner: \(error.localizedDescription)"
            }
        }
    }

    func beginScan() {
        pairError = nil
        pairStep = .scan
        if fixture {
            Task {
                try? await Task.sleep(for: .seconds(4))
                scanned("fixture")
            }
        }
    }

    func scanned(_ code: String) {
        pairStep = .confirm
        Task {
            do {
                _ = try await backend.pair(scanned: code)
                let hello = try await backend.connect(to: nil)
                arrived(hello, on: hello.key)
                plannerPath = .lan
                pairStep = .done
                await measure()
            } catch {
                pairError = error.localizedDescription
                pairStep = .start
            }
        }
    }

    func finishPairing(repo id: String) {
        paired = true
        active = true
        selectRepo(id)
        Task {
            try? await catchUp(id)
            flush()
            poll(generation)
        }
    }

    func forgetPairing() {
        generation += 1
        backend.forget()
        keep.wipe()
        items = [:]
        queued = []
        shelves = [:]
        pages = [:]
        machinePages = [:]
        statsPages = [:]
        repos = []
        currentID = ""
        paired = false
        pairStep = .start
        unlinked()
    }

    func send(_ text: String) {
        let message = Queued(id: UUID().uuidString, repo: currentID, text: text, at: Date())
        queued.append(message)
        keep.context.insert(message)
        keep.save()
        rebuild(currentID)
        flush()
    }

    private func flush() {
        guard !offline, !flushing else { return }
        flushing = true
        Task {
            defer { flushing = false }
            while !offline, let next = queued.first(where: { message in Date().timeIntervalSince(message.at) <= 86400 && repos.first { $0.id == message.repo }?.plannerKey == linked?.key }) {
                do {
                    if let task = next.task {
                        try await backend.tell(task: task, id: next.id, text: next.text)
                        await read(task)
                        drop(next.id)
                        continue
                    }
                    let item = try await backend.send(repo: next.repo, id: next.id, text: next.text)
                    merge([item], repo: next.repo)
                } catch let refused as Refused where refused.code == "moved" {
                    reread()
                    return
                } catch let refused as Refused where refused.code != "unreachable" {
                    drop(next.id)
                    if next.task != nil {
                        pickError = "Couldn't send that: \(refused.message)"
                        continue
                    }
                    let repo = next.repo
                    let warning = Item(seq: (items[repo]?.last?.seq ?? 0), at: Date().timeIntervalSince1970, kind: "sys", doc: Doc(text: "Couldn't send that: \(refused.message)"))
                    merge([warning], repo: repo)
                } catch {
                    return
                }
            }
        }
    }

    private func drop(_ id: String) {
        guard let message = queued.first(where: { $0.id == id }) else { return }
        queued.removeAll { $0.id == id }
        keep.context.delete(message)
        keep.save()
        rebuild(message.repo)
    }

    func resendAged(_ id: String, keep hold: Bool) {
        if hold {
            queued.first { $0.id == id }?.at = Date()
            keep.save()
            rebuild(currentID)
            flush()
        } else {
            drop(id)
        }
    }

    private func chosen(_ task: String, seq: Int, _ choice: String?) {
        if let index = pages[task]?.items.firstIndex(where: { $0.seq == seq }) {
            pages[task]?.items[index].doc.chosen = choice
            rebuild(currentID)
        }
    }

    func answerAsk(_ item: ChatItem, _ option: AskOption) {
        if let task = item.task {
            chosen(task, seq: item.seq, option.label)
            Task {
                do {
                    try await backend.choose(task: task, id: UUID().uuidString, seq: item.seq, choice: option.label)
                    await read(task)
                } catch {
                    chosen(task, seq: item.seq, nil)
                    pickError = "Couldn't send that: \(error.localizedDescription)"
                }
            }
            return
        }
        let repo = currentID
        if let index = items[repo]?.firstIndex(where: { $0.seq == item.seq }) {
            items[repo]?[index].doc.chosen = option.label
            rebuild(repo)
        }
        Task {
            do {
                let sent = try await backend.choose(repo: repo, id: UUID().uuidString, seq: item.seq, choice: option.label)
                merge([sent], repo: repo)
            } catch {
                if let index = items[repo]?.firstIndex(where: { $0.seq == item.seq }) {
                    items[repo]?[index].doc.chosen = nil
                    rebuild(repo)
                }
            }
        }
    }
}
