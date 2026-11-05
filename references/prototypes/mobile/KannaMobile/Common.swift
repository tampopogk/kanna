import SwiftUI

extension Status {
    var words: String {
        switch self {
        case .working: "Working"
        case .needs: "Needs you"
        case .idle: "Idle"
        case .ready: "Ready"
        case .stale: "Stale"
        case .stopped: "Stopped"
        case .waiting: "Not started"
        case .blocked: "Blocked"
        }
    }
}

struct Dot: View {
    var status: Status

    var body: some View {
        ZStack {
            switch status {
            case .working: Circle().fill(K.accent)
            case .needs: Circle().fill(K.warn)
            case .idle: Circle().fill(K.ok)
            case .ready: Circle().fill(K.ok).padding(0).overlay(Circle().stroke(K.okTint, lineWidth: 2).padding(-2))
            case .stale: Circle().fill(K.machine)
            case .stopped, .waiting: Circle().strokeBorder(K.muted, lineWidth: 1.5)
            case .blocked: Circle().strokeBorder(K.muted, style: StrokeStyle(lineWidth: 1.5, dash: [1.5, 1.5]))
            }
        }
        .frame(width: 10, height: 10)
    }
}

struct OfflineDot: View {
    var body: some View { Circle().fill(K.bad).frame(width: 10, height: 10) }
}

struct Chip: View {
    var path: LinkPath
    var on = false

    private var content: (String, String) {
        switch path {
        case .lan: ("circle.fill", "Direct · LAN")
        case .net: ("circle.fill", "Direct · internet")
        case .relay: ("point.3.connected.trianglepath.dotted", "Relayed")
        case .off: ("wifi.slash", "Offline")
        }
    }

    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: content.0)
                .font(.system(size: path == .lan || path == .net ? 7 : 11, weight: .semibold))
                .foregroundStyle(path == .lan || path == .net ? K.ok : path == .relay ? K.machine : K.bad)
            Text(content.1)
        }
        .font(.system(size: 12, weight: .medium))
        .lineLimit(1)
        .fixedSize()
        .foregroundStyle(path == .relay ? K.machine : path == .off ? K.bad : K.muted)
        .padding(.vertical, 3)
        .padding(.leading, 7)
        .padding(.trailing, 9)
        .background(path == .off ? K.badTint : K.panel, in: Capsule())
        .overlay(Capsule().stroke(on ? K.accent : .clear, lineWidth: 1.5))
    }
}

struct ChipButton: View {
    @Environment(Store.self) private var store

    var body: some View {
        @Bindable var s = store
        Button { store.showChipPop.toggle() } label: { Chip(path: store.plannerPath, on: store.showChipPop) }
            .buttonStyle(.plain)
            .accessibilityIdentifier("chip")
            .popover(isPresented: $s.showChipPop, arrowEdge: .top) {
                ConnectionPop().presentationCompactAdaptation(.popover)
            }
    }
}

struct ConnectionPop: View {
    @Environment(Store.self) private var store

    private var machine: String { store.repo.plannerMachine }

    private var title: String {
        switch store.plannerPath {
        case .lan: "Direct · LAN"
        case .net: "Direct · internet"
        case .relay: "Relayed"
        case .off: "Offline"
        }
    }

    private var about: String {
        switch store.plannerPath {
        case .lan: "On the same network as this phone, with nothing between you."
        case .net: "A direct path across the internet, encrypted end to end."
        case .relay: "Through your relay, encrypted end to end: it passes bytes on and can't read them."
        case .off: store.net == .noNetwork ? "This phone has no network." : "It may be asleep. Last seen \(store.asOf)."
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("\(machine) · \(title)").font(.system(size: 15, weight: .semibold))
            Text(about).font(.system(size: 13.5)).foregroundStyle(K.muted).fixedSize(horizontal: false, vertical: true).padding(.bottom, 4)
            if store.plannerPath != .off {
                KeyValue(key: "Round trip", value: "\(store.milliseconds(machine)) ms")
                KeyValue(key: "Direct path", value: store.plannerPath == .relay ? "Trying" : "In use")
            }
        }
        .padding(13)
        .frame(width: 322, alignment: .leading)
    }
}

struct KeyValue: View {
    var key: String
    var value: String

    var body: some View {
        HStack {
            Text(key).foregroundStyle(K.muted)
            Spacer()
            Text(value)
        }
        .font(.system(size: 13))
        .padding(.vertical, 2)
    }
}

struct PlannerSub: View {
    @Environment(Store.self) private var store

    private func minutes(_ since: Date, _ now: Date) -> String {
        let m = Int(now.timeIntervalSince(since)) / 60
        return m > 0 ? " · \(m)m" : ""
    }

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            HStack(spacing: 6) {
                if store.offline {
                    Text("Planner · last seen \(store.asOf)")
                } else {
                    switch store.repo.planner {
                    case .idle: Dot(status: .idle); Text("Planner · idle")
                    case .working(let d): Dot(status: .working); Text("Planner · working" + minutes(d, context.date))
                    case .elsewhere(let d): Dot(status: .working); Text("Planner · working" + minutes(d, context.date))
                    case .asks: Dot(status: .needs); Text("Planner · asks you")
                    case .stopped: Dot(status: .stopped); Text("Planner · not running")
                    case .starting: Dot(status: .waiting); Text("Planner · starting on \(store.repo.plannerMachine)")
                    }
                }
            }
        }
    }
}

struct RepoNav<Sub: View>: ViewModifier {
    @Environment(Store.self) private var store
    @ViewBuilder var sub: () -> Sub

    func body(content: Content) -> some View {
        content
            .navigationBarTitleDisplayMode(.inline)
            .solidBar()
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { store.showRepos = true } label: {
                        VStack(alignment: .leading, spacing: 0) {
                            HStack(spacing: 3) {
                                Text(store.repo.id).font(.system(size: 17, weight: .semibold)).foregroundStyle(K.text)
                                Image(systemName: "chevron.down").font(.system(size: 10, weight: .bold)).foregroundStyle(K.muted)
                            }
                            sub().font(.system(size: 12)).foregroundStyle(K.muted).lineLimit(1).fixedSize()
                        }
                        .fixedSize()
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("repo-switcher")
                }
                .sharedBackgroundVisibility(.hidden)
                ToolbarItem(placement: .topBarTrailing) { ChipButton() }
                    .sharedBackgroundVisibility(.hidden)
            }
    }
}

extension View {
    func repoNav<Sub: View>(@ViewBuilder sub: @escaping () -> Sub) -> some View { modifier(RepoNav(sub: sub)) }
    func repoNav() -> some View { modifier(RepoNav { EmptyView() }) }

    func solidBar() -> some View {
        toolbarBackground(K.bg, for: .navigationBar)
            .toolbarBackgroundVisibility(.visible, for: .navigationBar)
    }

    func pushedNav() -> some View {
        navigationBarTitleDisplayMode(.inline)
            .solidBar()
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { ChipButton() }
                    .sharedBackgroundVisibility(.hidden)
            }
    }

    func offlineSub(_ store: Store, _ fallback: String? = nil) -> some View {
        repoNav {
            if store.offline { Text("As of \(store.asOf)") } else if let fallback { Text(fallback) }
        }
    }
}

struct OfflineBanner: View {
    @Environment(Store.self) private var store
    var detail: String?

    var body: some View {
        if store.offline {
            Banner(icon: "wifi.slash", color: K.bad, background: K.badTint,
                   title: store.net == .noNetwork ? "No network" : "Can't reach \(store.repo.plannerMachine)",
                   detail: detail ?? (store.net == .noNetwork ? "This phone is offline. What you write waits." : "Last seen \(store.asOf). It may be asleep."))
        }
    }
}

struct Banner: View {
    var icon: String
    var color: Color
    var background: Color
    var title: String
    var detail: String

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon).foregroundStyle(color).font(.system(size: 15, weight: .medium)).frame(width: 18).padding(.top, 1)
            VStack(alignment: .leading, spacing: 0) {
                Text(title).font(.system(size: 13))
                Text(detail).font(.system(size: 12.5)).foregroundStyle(K.muted)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
        .frame(maxWidth: .infinity)
        .background(background, ignoresSafeAreaEdges: [])
    }
}

struct Row<Lead: View, Trail: View>: View {
    var title: String
    var sub: AttributedString?
    var divider = true
    @ViewBuilder var lead: () -> Lead
    @ViewBuilder var trail: () -> Trail

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 11) {
                lead()
                VStack(alignment: .leading, spacing: 0) {
                    Text(title).font(.system(size: 15, weight: .medium)).foregroundStyle(K.text).lineLimit(1)
                    if let sub { Text(sub).font(.system(size: 13)).foregroundStyle(K.muted).lineLimit(1) }
                }
                Spacer(minLength: 0)
                trail()
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 9)
            .contentShape(Rectangle())
            if divider { K.line.frame(height: 1) }
        }
    }
}

struct ChevronRight: View {
    var body: some View {
        Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold)).foregroundStyle(K.muted)
    }
}

struct GroupHead: View {
    var title: String
    var count = ""

    var body: some View {
        HStack {
            Text(title).fontWeight(.semibold).foregroundStyle(K.text)
            Spacer()
            Text(count)
        }
        .font(.system(size: 13))
        .foregroundStyle(K.muted)
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 5)
    }
}

struct Composer: View {
    var placeholder: String
    var disabled = false
    var onSend: (String) -> Void
    @State private var text = ""
    @FocusState private var focused: Bool

    private var ready: Bool { !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(placeholder, text: $text, axis: .vertical)
                .lineLimit(1...5)
                .focused($focused)
                .font(.system(size: 15))
                .padding(.horizontal, 14)
                .padding(.vertical, 9)
                .background(K.panel, in: RoundedRectangle(cornerRadius: 19))
                .overlay(RoundedRectangle(cornerRadius: 19).stroke(K.line))
                .accessibilityIdentifier("composer")
            Button {
                let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !t.isEmpty else { return }
                text = ""
                onSend(t)
            } label: {
                Image(systemName: "arrow.up")
                    .font(.system(size: 16, weight: .semibold))
                    .frame(width: 38, height: 38)
                    .background(ready ? K.accent : K.panel2, in: Circle())
                    .foregroundStyle(ready ? K.onAccent : K.muted)
            }
            .buttonStyle(.plain)
            .disabled(!ready)
            .accessibilityIdentifier("send")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}

struct ReceiptLine: View {
    var receipt: Receipt

    private var content: (Bool, String)? {
        switch receipt {
        case .none: nil
        case .sent: (false, "Sent")
        case .read: (false, "Read")
        case .waitingTurn: (true, "Waiting for its turn to end")
        case .waitingPlanner: (true, "Waiting for the planner")
        case .waitingStart: (true, "Waits until it starts")
        case .waitingMachine(let m): (true, "Sends when \(m) is back")
        case .dayOld: (true, "A day old, not sent")
        }
    }

    var body: some View {
        if let content {
            HStack(spacing: 4) {
                if content.0 { Image(systemName: "clock").font(.system(size: 10, weight: .semibold)) }
                Text(content.1)
            }
            .font(.system(size: 11.5))
            .foregroundStyle(K.muted)
        }
    }
}

struct MyBubble: View {
    var text: String

    var body: some View {
        Text(rich(text))
            .font(.system(size: 15))
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(K.tint, in: UnevenRoundedRectangle(topLeadingRadius: 16, bottomLeadingRadius: 16, bottomTrailingRadius: 5, topTrailingRadius: 16))
    }
}

struct Pulse: View {
    @State private var phase = 0

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<3) { i in
                Circle().fill(K.machine).frame(width: 6, height: 6).opacity(phase == i ? 1 : phase == (i + 2) % 3 ? 0.6 : 0.3)
            }
        }
        .onReceive(Timer.publish(every: 0.4, on: .main, in: .common).autoconnect()) { _ in
            withAnimation(.easeInOut(duration: 0.3)) { phase = (phase + 1) % 3 }
        }
    }
}

struct Working: View {
    var text: String

    var body: some View {
        HStack(spacing: 8) {
            Pulse()
            Text(text).font(.system(size: 14)).foregroundStyle(K.muted)
        }
    }
}

struct PrimaryButton: View {
    var title: String
    var primary = false
    var off = false
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .frame(maxWidth: .infinity)
                .padding(10)
                .background(primary ? K.accent : K.panel2, in: RoundedRectangle(cornerRadius: 12))
                .foregroundStyle(off ? K.muted : primary ? K.onAccent : K.accent)
                .opacity(off ? 0.42 : 1)
        }
        .buttonStyle(.plain)
        .disabled(off)
    }
}

struct Card<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        content()
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(K.panel, in: RoundedRectangle(cornerRadius: 12))
    }
}

struct Hero<Extra: View>: View {
    var title: String
    @ViewBuilder var extra: () -> Extra

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.system(size: 21, weight: .bold)).foregroundStyle(K.text)
            extra()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 10)
        .overlay(alignment: .bottom) { K.line.frame(height: 1) }
    }
}

struct RepoSheet: View {
    @Environment(Store.self) private var store

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Repos").font(.system(size: 17, weight: .bold)).padding(.horizontal, 16).padding(.top, 22)
            Text("From your machines").font(.system(size: 13)).foregroundStyle(K.muted).padding(.horizontal, 16).padding(.bottom, 6)
            ForEach(store.repos) { repo in
                Button { store.selectRepo(repo.id) } label: { RepoRow(repo: repo, check: repo.id == store.currentID) }
                    .buttonStyle(.plain)
            }
            Button {
                store.showRepos = false
                store.after(0.4) { [store] in store.showDemo = true }
            } label: {
                Row(title: "Prototype controls", sub: "Scoring, connection, every screen", divider: false) {
                    Image(systemName: "slider.horizontal.3").foregroundStyle(K.muted).frame(width: 10)
                } trail: { ChevronRight() }
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("prototype-controls")
            Spacer(minLength: 0)
        }
        .presentationDetents([.height(372)])
        .presentationDragIndicator(.visible)
        .presentationBackground(K.panel)
    }
}

struct RepoRow: View {
    var repo: Repo
    var check = false
    @Environment(Store.self) private var store

    private var line: String {
        if repo.down { return "Offline · \(repo.plannerMachine) last seen \(repo.lastSeen)" }
        if case .working(let d) = repo.planner {
            let m = Int(Date().timeIntervalSince(d)) / 60
            return m > 0 ? "Working · \(m)m" : "Working"
        }
        return repo.lastLine
    }

    var body: some View {
        Row(title: repo.id, sub: AttributedString(line)) {
            if repo.down {
                OfflineDot()
            } else {
                switch repo.planner {
                case .idle: Dot(status: .idle)
                case .working, .elsewhere: Dot(status: .working)
                case .asks: Dot(status: .needs)
                case .stopped, .starting: Dot(status: .stopped)
                }
            }
        } trail: {
            if check { Image(systemName: "checkmark").foregroundStyle(K.accent).font(.system(size: 15, weight: .semibold)) }
        }
    }
}

struct DemoSheet: View {
    @Environment(Store.self) private var store

    var body: some View {
        @Bindable var s = store
        NavigationStack {
            Form {
                Section("Quiz score (D10)") {
                    Picker("Show", selection: $s.score) {
                        ForEach(ScoreMode.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    Toggle("Quiz agent is slow (D12)", isOn: $s.slowWriter)
                }
                Section("Code (D9)") {
                    Toggle("Wrap by default", isOn: $s.wrapByDefault)
                }
                Section("Connection") {
                    Picker("Path", selection: Binding(get: { store.net }, set: { store.setNet($0) })) {
                        ForEach(Net.allCases) { Text($0.label).tag($0) }
                    }
                    Button("Walk out of the house") { store.showDemo = false; store.walkOut() }
                    Button("Age queued messages by a day (D7)") { store.showDemo = false; store.ageQueue() }
                }
                Section("Planner") {
                    Button("Busy with something else (D3)") { store.showDemo = false; store.tab = .chat; store.demoElsewhere() }
                    Button("Stop it, so the next message starts one (D2)") { store.showDemo = false; store.tab = .chat; store.demoStopPlanner() }
                    Button("Ask it what's out of date") { store.demoPrompt("Is anything on the Shelf out of date?") }
                    Button("Ask it to explain code") { store.demoPrompt("Explain how a Codex task stays in its own terminal") }
                    Button("Ask it for a quiz") { store.demoPrompt("Quiz me on the relay stuff from this week") }
                }
                Section("Macs (D4)") {
                    Toggle("Several Macs", isOn: $s.severalMacs)
                    Toggle("mbp is closed", isOn: Binding(
                        get: { store.repo.desks.first { $0.machine == "mbp" }?.open == false },
                        set: { closed in store.mutate(store.currentID) { r in
                            if let i = r.desks.firstIndex(where: { $0.machine == "mbp" }) { r.desks[i].open = !closed }
                        } }))
                }
                Section("Pairing") {
                    Toggle("Camera refused", isOn: $s.cameraRefused)
                    Button("Forget this phone", role: .destructive) { store.forgetPairing() }
                }
                Section("Look and timing") {
                    Picker("Appearance", selection: $s.appearance) {
                        ForEach(Appearance.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    Picker("Timing", selection: $s.speed) {
                        Text("Fast").tag(0.4)
                        Text("As it would be").tag(1.0)
                        Text("Slow").tag(2.5)
                    }
                    .pickerStyle(.segmented)
                }
                ForEach(Scenes.all, id: \.0) { group in
                    Section("Jump to: \(group.0)") {
                        ForEach(group.1) { scene in
                            Button(scene.name) { store.jump(scene.key) }
                        }
                    }
                }
            }
            .navigationTitle("Prototype")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { store.showDemo = false } }
            }
        }
    }
}
