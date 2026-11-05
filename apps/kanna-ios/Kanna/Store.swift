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
    var noNetwork = false
    var roundTrip = 0
    var lastSeen: Date?

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
            items[kept.id] = keep.items(kept.id)
            repos.append(Repo(id: kept.id, name: kept.name, plannerMachine: kept.machine, planner: Planner(status: kept.status, since: kept.since)))
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

    func milliseconds(_ machine: String) -> Int { roundTrip }

    func foreground(_ on: Bool) {
        active = on
        if on {
            watchNetwork()
            wake()
        } else {
            generation += 1
            backend.close()
            plannerPath = .off
        }
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
        guard active, backend.paired != nil, offline, !waking else { return }
        waking = true
        generation += 1
        let mine = generation
        Task {
            defer { waking = false }
            do {
                let hello = try await backend.connect()
                guard mine == generation else { return }
                arrived(hello)
                await measure()
                try await catchUp(currentID)
                flush()
                poll(mine)
            } catch let refused as Refused where refused.code == "revoked" {
                forgetPairing()
            } catch {
                guard mine == generation else { return }
                plannerPath = .off
                retry()
            }
        }
    }

    private func retry() {
        Task {
            try? await Task.sleep(for: .seconds(3))
            wake()
        }
    }

    private func poll(_ mine: Int) {
        Task {
            while mine == generation, !offline {
                try? await Task.sleep(for: .seconds(5))
                guard mine == generation else { return }
                await measure()
            }
        }
    }

    private func measure() async {
        let link = await backend.link()
        guard link.path != .off else { return }
        plannerPath = link.path
        roundTrip = link.milliseconds
        seen()
    }

    private func seen() {
        lastSeen = Date()
        UserDefaults.standard.set(lastSeen, forKey: "seen")
    }

    private func lost() {
        plannerPath = .off
        retry()
    }

    private func arrived(_ hello: Hello) {
        seen()
        let previous = repos
        repos = hello.projects.map { project in
            var repo = previous.first { $0.id == project.repo } ?? Repo(id: project.repo, name: project.name, plannerMachine: hello.name, planner: .stopped)
            repo.name = project.name
            repo.plannerMachine = project.planner ?? hello.name
            repo.planner = Planner(status: project.status, since: project.since)
            return repo
        }
        if !repos.contains(where: { $0.id == currentID }) { currentID = repos.first?.id ?? "" }
        for old in keep.repos() { keep.context.delete(old) }
        for (order, project) in hello.projects.enumerated() {
            keep.context.insert(KeptRepo(id: project.repo, name: project.name, machine: project.planner ?? hello.name, status: project.status, since: project.since, order: order))
        }
        keep.save()
        repos.map(\.id).forEach(rebuild)
        pairError = nil
    }

    private func catchUp(_ id: String) async throws {
        guard !id.isEmpty else { return }
        let held = items[id] ?? []
        let unread = held.first { $0.kind == "me" && $0.read == nil }?.seq
        let after = max(0, (unread ?? (held.last?.seq ?? 0) + 1) - 1)
        let page = try await backend.read(repo: id, after: after)
        merge(page.items, repo: id)
        change(id) { $0.planner = Planner(status: page.status, since: page.since) }
    }

    private func merge(_ fresh: [Item], repo id: String) {
        var held = Dictionary((items[id] ?? []).map { ($0.seq, $0) }, uniquingKeysWith: { $1 })
        for item in fresh {
            held[item.seq] = item
            keep.put(item, repo: id)
            if let client = item.client, let done = queued.first(where: { $0.id == client }) {
                queued.removeAll { $0.id == client }
                keep.context.delete(done)
            }
        }
        keep.save()
        items[id] = held.values.sorted { $0.seq < $1.seq }
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
        }
    }

    private func rebuild(_ id: String) {
        guard let index = repos.firstIndex(where: { $0.id == id }) else { return }
        let planner = repos[index].planner
        let machine = repos[index].plannerMachine
        var chat: [ChatItem] = []
        var last = ""
        var previous: Double?
        for item in items[id] ?? [] {
            if previous == nil || item.at - (previous ?? 0) > 3600 {
                chat.append(ChatItem(id: "\(item.seq)-when", kind: .when(Self.clock.string(from: Date(timeIntervalSince1970: item.at)))))
            }
            previous = item.at
            let key = "\(item.seq)"
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
                } else {
                    chat.append(ChatItem(id: "\(key)-ask", kind: .ask((item.doc.choices ?? []).map { AskOption(label: $0.label, why: $0.why ?? "") }), seq: item.seq))
                }
            case "show":
                chat.append(ChatItem(id: key, kind: .planner([.code(snippet(item, repo: id))]), seq: item.seq))
            case "sys":
                chat.append(ChatItem(id: key, kind: .sys(item.doc.text ?? ""), seq: item.seq))
            default: break
            }
        }
        for message in queued where message.repo == id {
            let old = Date().timeIntervalSince(message.at) > 86400
            chat.append(ChatItem(id: message.id, kind: .me(message.text, old ? .dayOld : offline ? .waitingMachine(machine) : .sent)))
        }
        repos[index].chat = chat
        repos[index].lastLine = last
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

    private func snippet(_ item: Item, repo: String) -> Snippet {
        let key = "\(repo)/\(item.seq)"
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
        let made = Snippet(id: key, file: file, lines: coded, commit: String((item.doc.commit ?? "").prefix(7)))
        snippets[key] = made
        return made
    }

    func selectRepo(_ id: String) {
        currentID = id
        UserDefaults.standard.set(id, forKey: "repo")
        showRepos = false
        guard !offline else { return }
        Task { try? await catchUp(id) }
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
                arrived(try await backend.connect())
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
        selectRepo(id)
        paired = true
        active = true
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
        repos = []
        currentID = ""
        paired = false
        pairStep = .start
        plannerPath = .off
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
            while !offline, let next = queued.first(where: { Date().timeIntervalSince($0.at) <= 86400 }) {
                do {
                    let item = try await backend.send(repo: next.repo, id: next.id, text: next.text)
                    merge([item], repo: next.repo)
                } catch let refused as Refused where refused.code != "unreachable" {
                    drop(next.id)
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

    func answerAsk(_ item: ChatItem, _ option: AskOption) {
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
