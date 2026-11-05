import SwiftUI

struct MachinesView: View {
    @Environment(Store.self) private var store

    private func line(_ name: String) -> String {
        guard let m = Fixtures.machines[name] else { return "" }
        if store.path(name) == .off { return (m.linux ? "Linux · " : "") + "last seen \(m.lastSeen ?? store.asOf)" }
        var parts: [String] = m.linux ? ["Linux"] : []
        if store.repo.plannerMachine == name { parts.append("Planner") }
        if store.repo.desks.contains(where: { $0.machine == name }) { parts.append("Desk \(store.deskCount(name)) of 3") }
        let running = store.repo.tasks.filter { $0.machine == name && $0.status == .working }.count
        if m.linux && running > 0 { parts.append(running == 1 ? "1 task running" : "\(running) tasks running") }
        return parts.joined(separator: " · ")
    }

    private func status(_ name: String) -> AnyView {
        if store.path(name) == .off { return AnyView(OfflineDot()) }
        let working = Fixtures.machines[name]?.linux == true && store.repo.tasks.contains { $0.machine == name && $0.status == .working }
        return AnyView(Dot(status: working ? .working : .idle))
    }

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner(detail: "Last seen \(store.asOf). This is how they were then.")
            ScrollView {
                VStack(spacing: 0) {
                    GroupHead(title: "Have \(store.repo.id)", count: "\(store.repo.machines.count)")
                    ForEach(store.repo.machines, id: \.self) { name in
                        NavigationLink(value: MachineRoute.machine(name)) {
                            Row(title: name, sub: AttributedString(line(name))) { status(name) } trail: {
                                VStack(alignment: .trailing, spacing: 2) {
                                    Chip(path: store.path(name))
                                    if store.path(name) != .off {
                                        Text("\(store.milliseconds(name)) ms").font(.system(size: 12)).foregroundStyle(K.muted)
                                    }
                                }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                    NavigationLink(value: MachineRoute.without) {
                        Row(title: "", sub: nil, divider: false) {
                            Text("\(store.repo.without.count) machines without \(store.repo.id)").foregroundStyle(K.muted)
                        } trail: { ChevronRight() }
                    }
                    .buttonStyle(.plain)
                }
            }
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
                ForEach(store.repo.without, id: \.self) { name in
                    Row(title: name, sub: AttributedString(Fixtures.machines[name]?.about ?? "")) { Dot(status: .idle) } trail: { Chip(path: store.path(name)) }
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
    @State private var move = false

    private var machine: Machine? { Fixtures.machines[name] }

    private var pathWords: String {
        switch store.path(name) {
        case .lan: "Direct · LAN"
        case .net: "Direct · internet"
        case .relay: "Relayed"
        case .off: "Offline"
        }
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
                            KeyValue(key: "Path from this phone", value: pathWords)
                            if store.path(name) != .off {
                                KeyValue(key: "Round trip", value: "\(store.milliseconds(name)) ms")
                                KeyValue(key: "Connected", value: "since \(machine?.since ?? "")")
                            } else {
                                KeyValue(key: "Last seen", value: machine?.lastSeen ?? store.asOf)
                            }
                        }
                        .font(.system(size: 15))
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 14)
                    GroupHead(title: "\(store.repo.id) here").padding(.top, -12)
                    if isPlanner {
                        Row(title: "Planner", sub: "idle · this repo's planner") { Dot(status: .idle) } trail: { EmptyView() }
                    }
                    ForEach(store.repo.tasks.filter { $0.machine == name || $0.desk == name }) { TaskLink(item: $0, onDesk: $0.desk == name) }
                    if store.repo.machines.count > 1 && !isPlanner {
                        PrimaryButton(title: "Run the planner here", off: store.path(name) == .off) { store.movePlanner(to: name) }
                            .padding(16)
                    } else if isPlanner {
                        PrimaryButton(title: "Run the planner on another machine", off: store.offline) { move = true }
                            .padding(16)
                    }
                }
            }
        }
        .background(K.bg)
        .pushedNav()
        .sheet(isPresented: $move) {
            VStack(spacing: 0) {
                SheetHead(title: "Run the planner on", detail: "The chat carries on with it. The old one stops.")
                ForEach(store.repo.machines.filter { $0 != name }, id: \.self) { m in
                    Button {
                        move = false
                        store.movePlanner(to: m)
                    } label: {
                        Row(title: m, sub: AttributedString(Fixtures.machines[m]?.about ?? "")) { Dot(status: store.path(m) == .off ? .stopped : .idle) } trail: { Chip(path: store.path(m)) }
                    }
                    .buttonStyle(.plain)
                    .disabled(store.path(m) == .off)
                    .opacity(store.path(m) == .off ? 0.5 : 1)
                }
                SheetCancel { move = false }
                Spacer(minLength: 0)
            }
            .presentationDetents([.height(290)])
            .presentationBackground(K.panel)
        }
    }
}

struct TaskLink: View {
    var item: WorkItem
    var onDesk = false
    @Environment(Store.self) private var store

    private var line: AttributedString {
        onDesk ? AttributedString("Desk task · ") + item.line() : item.line()
    }

    var body: some View {
        Button {
            store.tab = .shelf
            store.shelfPath = [item.id]
        } label: {
            Row(title: item.name, sub: line) { Dot(status: item.status) } trail: { ChevronRight() }
        }
        .buttonStyle(.plain)
    }
}
