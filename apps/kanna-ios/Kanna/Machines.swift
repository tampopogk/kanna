import SwiftUI

enum MachineRoute: Hashable {
    case machine(String)
    case without
}

extension MachineInfo {
    var linux: Bool { os == "linux" }

    var about: String {
        let kind = os == "macos" ? "Mac" : os == "linux" ? "Linux" : (os ?? "")
        var parts = [kind]
        if let arch { parts.append(arch) }
        if let cores, cores > 0 { parts.append("\(cores) cores") }
        if let version { parts.append("Kanna \(version)") }
        return parts.filter { !$0.isEmpty }.joined(separator: " · ")
    }
}

private let clock: DateFormatter = {
    let format = DateFormatter()
    format.timeStyle = .short
    format.dateStyle = .none
    return format
}()

private func stamp(_ seconds: Double?) -> String? {
    guard let seconds else { return nil }
    let ago = Date().timeIntervalSince1970 - seconds
    if ago < 86400 { return clock.string(from: Date(timeIntervalSince1970: seconds)) }
    return "\(Int(ago / 86400))d ago"
}

struct MachinesView: View {
    @Environment(Store.self) private var store

    private var have: [MachineInfo] { store.machinesPage?.machines.filter(\.has) ?? [] }
    private var without: [MachineInfo] { store.machinesPage?.machines.filter { !$0.has } ?? [] }

    private func line(_ m: MachineInfo) -> String {
        if store.path(m.name) == .off {
            return (m.linux ? "Linux · " : "") + (stamp(m.seen).map { "last seen \($0)" } ?? "not seen lately")
        }
        var parts: [String] = m.linux ? ["Linux"] : []
        if m.planner { parts.append("Planner") }
        if store.repo.desks.contains(where: { $0.machine == m.name }) { parts.append("Desk \(store.deskCount(m.name)) of 3") }
        let running = store.repo.tasks.filter { $0.machine == m.name && $0.status == .working }.count
        if running > 0 { parts.append(running == 1 ? "1 task running" : "\(running) tasks running") }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder private func status(_ m: MachineInfo) -> some View {
        if store.path(m.name) == .off {
            OfflineDot()
        } else {
            Dot(status: store.repo.tasks.contains { $0.machine == m.name && $0.status == .working } ? .working : .idle)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner(detail: store.onlyThroughOther ? "Asking \(store.linked?.name ?? "another machine") instead." : "Last seen \(store.asOf). This is how they were then.")
            ScrollView {
                VStack(spacing: 0) {
                    GroupHead(title: "Have \(store.repo.name)", count: "\(have.count)")
                    ForEach(have) { m in
                        NavigationLink(value: MachineRoute.machine(m.name)) {
                            Row(title: m.name, sub: AttributedString(line(m))) { status(m) } trail: {
                                VStack(alignment: .trailing, spacing: 2) {
                                    Chip(path: store.path(m.name))
                                    if store.path(m.name) != .off, store.milliseconds(m.name) > 0 {
                                        Text("\(store.milliseconds(m.name)) ms").font(.system(size: 12)).foregroundStyle(K.muted)
                                    }
                                }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                    if !without.isEmpty {
                        NavigationLink(value: MachineRoute.without) {
                            Row(title: "", sub: nil, divider: false) {
                                Text(without.count == 1 ? "1 machine without \(store.repo.name)" : "\(without.count) machines without \(store.repo.name)").foregroundStyle(K.muted)
                            } trail: { ChevronRight() }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .refreshable { store.refreshMachines() }
        }
        .background(K.bg)
        .offlineSub(store)
    }
}

struct WithoutRepoView: View {
    @Environment(Store.self) private var store

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                ForEach(store.machinesPage?.machines.filter { !$0.has } ?? []) { m in
                    Row(title: m.name, sub: AttributedString(m.about)) { Dot(status: .idle) } trail: { Chip(path: store.path(m.name)) }
                }
            }
        }
        .background(K.bg)
        .pushedNav()
    }
}

struct MachineDetail: View {
    var name: String
    @Environment(Store.self) private var store
    @State private var choosing = false

    private var machine: MachineInfo? { store.machinesPage?.machines.first { $0.name == name } }
    private var others: [MachineInfo] { store.machinesPage?.machines.filter { $0.has && $0.name != name } ?? [] }

    private var pathWords: String {
        switch store.path(name) {
        case .lan: "Direct · LAN"
        case .net: "Direct · internet"
        case .relay: "Relayed"
        case .off: "Offline"
        }
    }

    private var pathFrom: String {
        if name == store.linked?.name { return "Path from this phone" }
        return "Path from \(store.machinesPage?.from ?? "the planner machine")"
    }

    var body: some View {
        let isPlanner = store.repo.plannerMachine == name
        VStack(spacing: 0) {
            Hero(title: name) {
                Text(machine?.about ?? "").font(.system(size: 13)).foregroundStyle(K.muted)
            }
            ScrollView {
                VStack(spacing: 0) {
                    Card {
                        VStack(spacing: 0) {
                            KeyValue(key: pathFrom, value: pathWords)
                            if store.path(name) != .off {
                                if store.milliseconds(name) > 0 { KeyValue(key: "Round trip", value: "\(store.milliseconds(name)) ms") }
                                if let since = stamp(machine?.since) { KeyValue(key: "Connected", value: "since \(since)") }
                            } else {
                                KeyValue(key: "Last seen", value: stamp(machine?.seen) ?? "not lately")
                            }
                        }
                        .font(.system(size: 15))
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 14)
                    GroupHead(title: "\(store.repo.name) here").padding(.top, -12)
                    if isPlanner {
                        Row(title: "Planner", sub: "this repo's planner") { Dot(status: plannerDot) } trail: { EmptyView() }
                    }
                    ForEach(store.repo.tasks.filter { $0.machine == name || $0.desk == name }) { TaskLink(item: $0, onDesk: $0.desk == name) }
                    if !isPlanner, machine?.has == true {
                        PrimaryButton(title: store.moving ? "Moving…" : "Run the planner here", off: !canMove(name)) { store.movePlanner(to: name) }
                            .padding(16)
                            .accessibilityIdentifier("run-here")
                    } else if isPlanner, !others.isEmpty {
                        PrimaryButton(title: "Run the planner on another machine", off: store.linked == nil) { choosing = true }
                            .padding(16)
                    }
                }
            }
        }
        .background(K.bg)
        .pushedNav()
        .sheet(isPresented: $choosing) {
            VStack(spacing: 0) {
                SheetHead(title: "Run the planner on", detail: "The phone talks to that machine's planner. It won't know this conversation.")
                ForEach(others) { m in
                    Button {
                        choosing = false
                        store.movePlanner(to: m.name)
                    } label: {
                        Row(title: m.name, sub: AttributedString(m.about)) { Dot(status: store.path(m.name) == .off ? .stopped : .idle) } trail: { Chip(path: store.path(m.name)) }
                    }
                    .buttonStyle(.plain)
                    .disabled(!canMove(m.name))
                    .opacity(canMove(m.name) ? 1 : 0.5)
                }
                SheetCancel { choosing = false }
                Spacer(minLength: 0)
            }
            .presentationDetents([.height(290)])
            .presentationBackground(K.panel)
        }
    }

    private func canMove(_ machine: String) -> Bool {
        store.linked != nil && !store.moving && store.path(machine) != .off
    }

    private var plannerDot: Status {
        switch store.repo.planner {
        case .working, .elsewhere: .working
        case .asks, .blocked: .needs
        case .idle: .idle
        case .starting: .waiting
        case .stopped: .stopped
        }
    }
}

struct TaskLink: View {
    var item: WorkItem
    var onDesk = false
    @Environment(Store.self) private var store

    private var line: String {
        (onDesk ? "Desk task · " : "") + item.age
    }

    var body: some View {
        Button {
            store.tab = .shelf
            store.shelfPath = [item.id]
        } label: {
            Row(title: item.name, sub: AttributedString(line)) { Dot(status: item.status) } trail: { ChevronRight() }
        }
        .buttonStyle(.plain)
    }
}
