import SwiftUI

struct ChatView: View {
    @Environment(Store.self) private var store

    private var last: UUID? { store.repo.chat.last?.id }

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
                }
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
                    Button { store.answerAsk(item.id, option) } label: {
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
        case .round(let id, let result):
            RoundCard(def: store.roundDef(id), result: result, item: item.id)
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

struct RoundCard: View {
    var def: RoundDef
    var result: Int?
    var item: UUID
    @Environment(Store.self) private var store

    var body: some View {
        Button {
            guard result == nil else { return }
            store.startRound(def: def, cardItem: item)
        } label: {
            StartBanner(title: result.map { "\($0) of \(def.questions.count): \(def.title)" } ?? "Round ready: \(def.title)",
                        detail: result == nil ? "5 questions · about 2 minutes" : "Played", calm: result != nil, chevron: result == nil)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("round-card")
    }
}

struct StartBanner: View {
    var title: String
    var detail: String
    var calm = false
    var chevron = true

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 0) {
                Text(title).font(.system(size: 16, weight: .bold))
                Text(detail).font(.system(size: 13)).opacity(calm ? 1 : 0.85).foregroundStyle(calm ? K.muted : K.onAccent)
            }
            Spacer(minLength: 0)
            if chevron { Image(systemName: "chevron.right").font(.system(size: 14, weight: .semibold)) }
        }
        .foregroundStyle(calm ? K.text : K.onAccent)
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(calm ? K.panel : K.accent, in: RoundedRectangle(cornerRadius: 14))
    }
}
