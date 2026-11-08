import SwiftUI

extension WorkItem {
    func line(_ overrideNeed: Bool = true) -> AttributedString {
        var parts: [String] = []
        switch status {
        case .ready: parts = ["ready"]
        case .conflicts: parts = ["conflicts"]
        case .stale: parts = ["stale", "behind \(after)"]
        case .idle where !after.isEmpty, .stopped where !after.isEmpty: parts = [status.words.lowercased(), "merges after \(after)", machine ?? ""]
        case .blocked: parts = ["blocked by \(after)"]
        case .waiting: parts = ["not started"]
        case .needs: parts = ["needs you", age, machine ?? ""]
        default: parts = [status.words.lowercased(), age, machine ?? ""]
        }
        if unreachable { parts = ["as last seen", machine.map { "\($0) can't be reached" } ?? ""] }
        var out = AttributedString(parts.filter { !$0.isEmpty }.joined(separator: " · "))
        if status == .needs, let r = out.range(of: "needs you") {
            out[r].foregroundColor = K.warn
            out[r].font = .system(size: 13, weight: .medium)
        }
        return out
    }
}

struct ShelfView: View {
    @Environment(Store.self) private var store
    @State private var query = ""

    private func filtered(_ list: [WorkItem]) -> [WorkItem] {
        guard !query.isEmpty else { return list }
        return list.filter { $0.name.lowercased().contains(query.lowercased()) }
    }

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner(detail: "This is the Shelf as it was at \(store.asOf).")
            ScrollView {
                VStack(spacing: 0) {
                    HStack(spacing: 6) {
                        Image(systemName: "magnifyingglass").font(.system(size: 14))
                        TextField("Find a task", text: $query).foregroundStyle(K.text)
                    }
                    .foregroundStyle(K.muted)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(K.panel, in: RoundedRectangle(cornerRadius: 10))
                    .padding(.horizontal, 16)
                    .padding(.top, 10)
                    .padding(.bottom, 2)
                    Group {
                        ForEach(store.repo.desks) { desk in
                            let list = store.repo.tasks.filter { $0.desk == desk.machine }
                            let rows = filtered(list)
                            if !rows.isEmpty {
                                GroupHead(title: "\(desk.machine)'s Desk", count: "\(list.count) of 3")
                                ForEach(rows) { TaskRow(item: $0) }
                            }
                        }
                        let shelf = store.repo.tasks.filter { $0.desk == nil }
                        GroupHead(title: "Shelf", count: "\(shelf.count)")
                        ForEach(filtered(shelf)) { TaskRow(item: $0) }
                        if query.isEmpty {
                            NavigationLink { DoneList() } label: {
                                Row(title: "", sub: nil, divider: false) { Text("\(store.repo.done.count) done lately").foregroundStyle(K.muted) } trail: { ChevronRight() }
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .opacity(store.offline ? 0.55 : 1)
                }
            }
        }
        .background(K.bg)
        .scrollDismissesKeyboard(.interactively)
        .refreshable { store.refreshShelf() }
        .offlineSub(store)
    }
}

struct TaskRow: View {
    var item: WorkItem

    var body: some View {
        NavigationLink(value: item.id) {
            Row(title: item.name, sub: item.line()) { Dot(status: item.status) } trail: { ChevronRight() }
        }
        .buttonStyle(.plain)
    }
}

struct DoneList: View {
    @Environment(Store.self) private var store

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                ForEach(store.repo.done, id: \.self) { name in
                    Row(title: name, sub: "done") {
                        Image(systemName: "checkmark").font(.system(size: 11, weight: .bold)).foregroundStyle(K.ok).frame(width: 10)
                    } trail: { EmptyView() }
                }
            }
        }
        .background(K.bg)
        .pushedNav()
    }
}

struct TaskDetail: View {
    var id: String
    @Environment(Store.self) private var store
    @State private var viewport: CGFloat = 0

    private var item: WorkItem? { store.task(id) }

    private func meta(_ t: WorkItem) -> String {
        var parts = [t.status.words]
        if t.desk == nil && (t.status == .waiting || t.status == .blocked) { parts.append("on the Shelf") }
        if t.status == .working, !t.age.isEmpty { parts.append(t.age) }
        if t.status != .waiting && t.status != .blocked, let m = t.machine { parts.append(m) }
        parts.append(t.agent)
        return parts.joined(separator: " · ")
    }

    var body: some View {
        @Bindable var s = store
        if let t = item {
            VStack(spacing: 0) {
                OfflineBanner()
                if t.status == .needs {
                    Banner(icon: "exclamationmark.circle", color: K.warn, background: K.warnTint, title: "Needs you", detail: "Stopped \(t.age) ago, waiting for an answer")
                }
                Hero(title: t.name) {
                    HStack(spacing: 6) {
                        Dot(status: t.status)
                        Text(meta(t))
                    }
                    .font(.system(size: 13))
                    .foregroundStyle(K.muted)
                    if let b = t.branch { Text(b).font(.mono(12)).foregroundStyle(K.muted) }
                    deskLine(t)
                }
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        if t.status == .blocked, let wait = store.repo.tasks.first(where: { $0.name == t.after }) {
                            Label2("Waits for")
                            NavigationLink(value: wait.id) {
                                Card {
                                    HStack(spacing: 9) {
                                        Dot(status: wait.status)
                                        VStack(alignment: .leading, spacing: 0) {
                                            Text(wait.name).foregroundStyle(K.text)
                                            Text("\(wait.status.words.lowercased()) on \(wait.desk ?? "the Shelf")'s Desk, not ready yet").font(.system(size: 13)).foregroundStyle(K.muted)
                                        }
                                        Spacer(minLength: 0)
                                        ChevronRight()
                                    }
                                }
                            }
                            .buttonStyle(.plain)
                        }
                        if let first = t.firstMessage {
                            Label2("First message")
                            Card { Text(rich(first)).font(.system(size: 15)) }
                        }
                        ForEach(t.thread) { ThreadRow(message: $0) }
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 14)
                    .frame(maxWidth: .infinity, minHeight: viewport, alignment: t.thread.isEmpty ? .topLeading : .bottomLeading)
                    .tapDismissesKeyboard()
                }
                .onGeometryChange(for: CGFloat.self, of: \.size.height) { viewport = $0 }
                .defaultScrollAnchor(t.thread.isEmpty ? .top : .bottom)
                .scrollDismissesKeyboard(.interactively)
            }
            .background(K.bg)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                Composer(placeholder: t.status == .needs ? "Answer the task" : "Message this task") { store.message(task: id, text: $0) }
            }
            .pushedNav()
            .task(id: id) { await store.read(id) }
            .sheet(isPresented: Binding(get: { store.pendingPick == id }, set: { if !$0 { store.pendingPick = nil } })) { WhichDesk(id: id) }
            .sheet(isPresented: Binding(get: { store.fullFor == id && store.pickFull != nil }, set: { if !$0 { store.pickFull = nil; store.fullFor = nil } })) { FullDesk() }
        }
    }

    @ViewBuilder private func deskLine(_ t: WorkItem) -> some View {
        if let desk = t.desk {
            let closed = store.repo.desks.first { $0.machine == desk }?.open == false
            Text(closed ? "On \(desk)'s Desk · shows when it next opens" : "On \(desk)'s Desk").font(.system(size: 13)).foregroundStyle(K.muted)
        } else {
            let blocked = t.status == .blocked
            if t.unreachable {
                Text("\(t.machine ?? "Its machine") can't be reached, so it can't be picked").font(.system(size: 13)).foregroundStyle(K.muted)
            } else {
                PrimaryButton(title: t.picking ? "Picking…" : "Pick for a Desk", off: blocked || store.offline || t.picking) { store.pick(id) }
                    .padding(.top, 6)
                    .accessibilityIdentifier("pick")
            }
        }
    }
}

struct Label2: View {
    var text: String
    init(_ text: String) { self.text = text }
    var body: some View { Text(text).font(.system(size: 12)).foregroundStyle(K.muted).padding(.bottom, -8) }
}

struct ThreadRow: View {
    var message: Message
    @Environment(Store.self) private var store

    var body: some View {
        if !message.said.isEmpty {
            VStack(alignment: .leading, spacing: 12) {
                Label2("It said, \(message.time)")
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(message.said) { ChatRow(item: $0) }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("last-words")
            }
        } else if message.you {
            VStack(alignment: .trailing, spacing: 2) {
                Text("you · \(message.time)").font(.system(size: 12)).foregroundStyle(K.muted)
                MyBubble(text: message.text)
                ReceiptLine(receipt: message.receipt).padding(.top, 2)
                if message.receipt == .dayOld {
                    HStack(spacing: 12) {
                        Button("Discard") { store.resendAged(message.id, keep: false) }
                        Button("Keep waiting") { store.resendAged(message.id, keep: true) }
                    }
                    .font(.system(size: 13, weight: .semibold))
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        } else {
            VStack(alignment: .leading, spacing: 2) {
                Text("\(message.from) · \(message.time)").font(.system(size: 12)).foregroundStyle(K.muted)
                Card { Text(verbatim: message.text).font(.system(size: 15)) }
            }
        }
    }
}

struct SheetHead: View {
    var title: String
    var detail: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title).font(.system(size: 17, weight: .bold))
            Text(detail).font(.system(size: 13)).foregroundStyle(K.muted)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.top, 24)
        .padding(.bottom, 8)
    }
}

struct SheetCancel: View {
    var action: () -> Void

    var body: some View {
        Button("Cancel", action: action)
            .font(.system(size: 15, weight: .semibold))
            .frame(maxWidth: .infinity)
            .padding(.vertical, 12)
    }
}

struct WhichDesk: View {
    var id: String
    @Environment(Store.self) private var store

    var body: some View {
        VStack(spacing: 0) {
            SheetHead(title: "Which Desk?", detail: "It starts when it's picked.")
            ForEach(store.repo.desks) { desk in
                let count = store.deskCount(desk.machine)
                let state = count >= 3 ? "full" : desk.open ? "open now" : "closed, shows when it opens"
                let closedNote = count >= 3 && !desk.open ? " · closed" : ""
                Button { store.choose(desk: desk.machine, for: id) } label: {
                    Row(title: desk.machine, sub: AttributedString("\(count) of 3 · \(state)\(closedNote)")) {
                        Dot(status: desk.open ? .idle : .stopped)
                    } trail: {
                        if count >= 3 { ChevronRight() }
                    }
                }
                .buttonStyle(.plain)
            }
            SheetCancel { store.pendingPick = nil }
            Spacer(minLength: 0)
        }
        .presentationDetents([.height(250)])
        .presentationBackground(K.panel)
    }
}

struct FullDesk: View {
    @Environment(Store.self) private var store

    var body: some View {
        let desk = store.pickFull ?? ""
        VStack(spacing: 0) {
            SheetHead(title: "\(desk)'s Desk is full", detail: "Move one to the Shelf to make room.")
            ForEach(store.repo.tasks.filter { $0.desk == desk }) { t in
                Button { store.shelve(t.id) } label: {
                    Row(title: t.name, sub: AttributedString("\(t.status.words.lowercased()) · \(t.age)")) { Dot(status: t.status) } trail: { EmptyView() }
                }
                .buttonStyle(.plain)
            }
            SheetCancel { store.pickFull = nil; store.fullFor = nil }
            Spacer(minLength: 0)
        }
        .presentationDetents([.height(300)])
        .presentationBackground(K.panel)
    }
}
