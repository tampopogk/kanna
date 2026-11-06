import SwiftUI

@main
struct KannaApp: App {
    @State private var store = Store.launch()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            Root()
                .environment(store)
                .tint(K.accent)
                .onChange(of: phase, initial: true) {
                    if phase != .inactive { store.foreground(phase == .active) }
                }
        }
    }
}

struct MainTabs: View {
    @Environment(Store.self) private var store

    var body: some View {
        @Bindable var s = store
        TabView(selection: $s.tab) {
            Tab("Chat", systemImage: "bubble.left", value: AppTab.chat) {
                NavigationStack { ChatView() }
            }
            Tab("Shelf", systemImage: "tray", value: AppTab.shelf) {
                NavigationStack(path: $s.shelfPath) {
                    ShelfView().navigationDestination(for: String.self) { TaskDetail(id: $0) }
                }
            }
            Tab("Stats", systemImage: "chart.bar", value: AppTab.stats) {
                NavigationStack { StatsView() }
            }
            Tab("Machines", systemImage: "desktopcomputer", value: AppTab.machines) {
                NavigationStack {
                    MachinesView().navigationDestination(for: MachineRoute.self) { route in
                        switch route {
                        case .machine(let name): MachineDetail(name: name)
                        case .without: WithoutRepoView()
                        }
                    }
                }
            }
        }
        .overlay {
            if store.showChipPop {
                ZStack(alignment: .top) {
                    Color.black.opacity(0.001).onTapGesture { store.showChipPop = false }
                    ConnectionPop()
                        .padding(.leading, 56)
                        .padding(.trailing, 12)
                        .padding(.top, 52)
                }
                .transition(.opacity)
            }
        }
        .animation(.snappy, value: store.showChipPop)
        .sheet(isPresented: $s.showRepos) { RepoSheet() }
        .alert("Couldn't do that", isPresented: Binding(get: { store.pickError != nil }, set: { if !$0 { store.pickError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(store.pickError ?? "")
        }
        .onChange(of: store.tab) { store.afterTabChange() }
    }
}

struct Root: View {
    @Environment(Store.self) private var store

    var body: some View {
        @Bindable var s = store
        Group {
            if store.paired {
                MainTabs()
            } else {
                PairingFlow()
            }
        }
    }
}
