import SwiftUI

@main
struct KannaMobileApp: App {
    @State private var store = Store.launch()

    var body: some Scene {
        WindowGroup {
            Root()
                .environment(store)
                .preferredColorScheme(store.appearance == .system ? nil : store.appearance == .dark ? .dark : .light)
                .tint(K.accent)
        }
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
        .sheet(isPresented: $s.showDemo) { DemoSheet() }
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
                    ShelfView().navigationDestination(for: UUID.self) { TaskDetail(id: $0) }
                }
            }
            Tab("Quiz", systemImage: "scope", value: AppTab.quiz) {
                NavigationStack(path: $s.quizPath) {
                    QuizHome().navigationDestination(for: String.self) { SubsystemView(name: $0) }
                }
            }
            Tab("Stats", systemImage: "chart.bar", value: AppTab.stats) {
                NavigationStack { StatsView() }
            }
            Tab("Machines", systemImage: "server.rack", value: AppTab.machines) {
                NavigationStack(path: $s.machinePath) {
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
        .fullScreenCover(item: $s.fullCode) { FullCodeView(snippet: $0) }
        .fullScreenCover(isPresented: $s.roundOpen) { RoundView() }
    }
}

enum MachineRoute: Hashable {
    case machine(String)
    case without
}
