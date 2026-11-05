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

struct Root: View {
    @Environment(Store.self) private var store

    var body: some View {
        @Bindable var s = store
        Group {
            if store.paired {
                NavigationStack { ChatView() }
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
            } else {
                PairingFlow()
            }
        }
    }
}
