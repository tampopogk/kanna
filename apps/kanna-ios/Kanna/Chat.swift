import SwiftUI

struct ChatView: View {
    @Environment(Store.self) private var store
    @State private var viewport: CGFloat = 0

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner()
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        ForEach(store.repo.chat) { item in
                            ChatRow(item: item)
                                .id(item.id)
                                .transition(.opacity.combined(with: .move(edge: .bottom)))
                        }
                        if !store.offline { WorkingRow().id("working") }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                    .padding(.bottom, 8)
                    .frame(minHeight: viewport, alignment: .bottom)
                    .tapDismissesKeyboard()
                }
                .onGeometryChange(for: CGFloat.self, of: \.size.height) { viewport = $0 }
                .defaultScrollAnchor(.bottom)
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: store.repo.chat.count) {
                    withAnimation(.snappy) { proxy.scrollTo("working", anchor: .bottom) }
                }
                .onChange(of: store.currentID) {
                    proxy.scrollTo("working", anchor: .bottom)
                }
            }
        }
        .background(K.bg)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            Composer(placeholder: asking ? "Or answer in your own words" : "Message the planner") { store.send($0) }
        }
        .repoNav { PlannerSub() }
    }

    private var asking: Bool { store.repo.planner == .asks }
}

struct WorkingRow: View {
    @Environment(Store.self) private var store

    var body: some View {
        switch store.repo.planner {
        case .working: Working(text: "Working").transition(.opacity)
        case .elsewhere: Working(text: "Working on something else").transition(.opacity)
        default: EmptyView()
        }
    }
}

struct ChatRow: View {
    var item: ChatItem
    @Environment(Store.self) private var store

    var body: some View {
        switch item.kind {
        case .when(let t):
            Text(t).font(.system(size: 12)).foregroundStyle(K.muted).frame(maxWidth: .infinity)
        case .me(let t, let receipt):
            VStack(alignment: .trailing, spacing: 4) {
                MyBubble(text: t).frame(maxWidth: 300, alignment: .trailing)
                ReceiptLine(receipt: receipt)
                if receipt == .dayOld {
                    HStack(spacing: 12) {
                        Button("Discard") { store.resendAged(item.id, keep: false) }
                        Button("Keep waiting") { store.resendAged(item.id, keep: true) }
                    }
                    .font(.system(size: 13, weight: .semibold))
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        case .planner(let blocks):
            VStack(alignment: .leading, spacing: 8) {
                ForEach(blocks) { BlockView(block: $0) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .ask(let options):
            VStack(spacing: 0) {
                ForEach(Array(options.enumerated()), id: \.element.id) { i, option in
                    Button { store.answerAsk(item, option) } label: {
                        VStack(alignment: .leading, spacing: 0) {
                            Text(option.label).font(.system(size: 15, weight: .semibold)).foregroundStyle(K.accent)
                            Text(option.why).font(.system(size: 13)).foregroundStyle(K.muted)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 10)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(store.offline)
                    if i < options.count - 1 { K.line.frame(height: 1) }
                }
            }
            .background(K.panel, in: RoundedRectangle(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(K.line))
        case .answered(let label):
            (Text("You chose ") + Text(label).fontWeight(.semibold).foregroundColor(K.text))
                .font(.system(size: 13))
                .foregroundStyle(K.muted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(K.panel, in: RoundedRectangle(cornerRadius: 14))
                .overlay(RoundedRectangle(cornerRadius: 14).stroke(K.line))
        case .sys(let t):
            Text(t)
                .font(.system(size: 13))
                .multilineTextAlignment(.center)
                .foregroundStyle(K.muted)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(K.panel, in: RoundedRectangle(cornerRadius: 12))
                .frame(maxWidth: .infinity)
        }
    }
}

struct BlockView: View {
    var block: Block

    var body: some View {
        switch block {
        case .p(let t):
            Text(rich(t)).font(.system(size: 15)).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
        case .list(let items):
            VStack(alignment: .leading, spacing: 3) {
                ForEach(items, id: \.self) { item in
                    HStack(alignment: .firstTextBaseline, spacing: 7) {
                        Text("•")
                        Text(rich(item)).fixedSize(horizontal: false, vertical: true)
                    }
                    .font(.system(size: 15))
                }
            }
        case .code(let snippet):
            CodeCard(snippet: snippet)
        }
    }
}
