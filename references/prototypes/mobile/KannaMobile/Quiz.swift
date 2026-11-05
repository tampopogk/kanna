import SwiftUI

struct Score: View {
    var streak: Int
    var points: Int
    var size: CGFloat = 14
    @Environment(Store.self) private var store

    var body: some View {
        HStack(spacing: 12) {
            if store.score.showsStreak {
                HStack(spacing: 3) {
                    Image(systemName: "flame.fill").foregroundStyle(K.warn)
                    Text(String(streak))
                }
            }
            if store.score.showsPoints {
                HStack(spacing: 3) {
                    Image(systemName: "star.fill").foregroundStyle(K.accent)
                    Text(points.formatted())
                }
            }
        }
        .font(.system(size: size, weight: .semibold))
    }
}

struct MapPlace {
    let tile: Subsystem
    let col: Int
    let row: Int
}

func packMap(_ tiles: [Subsystem]) -> ([MapPlace], Int) {
    var used = Set<[Int]>()
    var out: [MapPlace] = []
    var rows = 0
    for tile in tiles {
        var placed = false
        var r = 0
        while !placed {
            var c = 0
            while c + tile.span.0 <= 6, !placed {
                let cells = (0..<tile.span.0).flatMap { dx in (0..<tile.span.1).map { dy in [c + dx, r + dy] } }
                if cells.allSatisfy({ !used.contains($0) }) {
                    cells.forEach { used.insert($0) }
                    out.append(MapPlace(tile: tile, col: c, row: r))
                    rows = max(rows, r + tile.span.1)
                    placed = true
                }
                c += 1
            }
            r += 1
        }
    }
    return (out, rows)
}

struct MapTile: View {
    var tile: Subsystem

    private var fill: Color { [K.m0, K.m1, K.m2, K.m3, K.m4][tile.level] }
    private var strong: Bool { tile.level == 4 }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(tile.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(strong ? K.mt4 : K.mt)
            Text((tile.pct.map { "\($0)%" } ?? "Not asked") + (tile.moved > 0 ? " · +\(tile.moved)" : ""))
                .font(.system(size: 11))
                .foregroundStyle(strong ? K.mm4 : K.mm)
        }
        .lineLimit(1)
        .minimumScaleFactor(0.8)
        .padding(.horizontal, 7)
        .padding(.vertical, 5)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background { ZStack { fill; if tile.moved > 0 { Hatch() } } }
        .overlay { if tile.level == 0 { RoundedRectangle(cornerRadius: 4).strokeBorder(K.line) } }
        .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

struct MasteryMap: View {
    var tiles: [Subsystem]
    private let cellHeight: CGFloat = 47
    private let gap: CGFloat = 2

    var body: some View {
        let (places, rows) = packMap(tiles)
        GeometryReader { geo in
            let cell = (geo.size.width - 5 * gap) / 6
            ForEach(places, id: \.tile.name) { p in
                NavigationLink(value: p.tile.name) {
                    MapTile(tile: p.tile)
                }
                .buttonStyle(.plain)
                .frame(width: CGFloat(p.tile.span.0) * cell + CGFloat(p.tile.span.0 - 1) * gap,
                       height: CGFloat(p.tile.span.1) * cellHeight + CGFloat(p.tile.span.1 - 1) * gap)
                .offset(x: CGFloat(p.col) * (cell + gap), y: CGFloat(p.row) * (cellHeight + gap))
            }
        }
        .frame(height: CGFloat(rows) * (cellHeight + gap) - gap)
    }
}

struct MapLegend: View {
    var body: some View {
        HStack(spacing: 6) {
            Text("Less")
            ForEach(Array([K.m1, K.m2, K.m3, K.m4].enumerated()), id: \.offset) { _, c in
                RoundedRectangle(cornerRadius: 2).fill(c).frame(width: 15, height: 11)
            }
            Text("More")
            Spacer()
            ZStack { K.m2; Hatch() }.clipShape(RoundedRectangle(cornerRadius: 2)).frame(width: 15, height: 11)
            Text("Moved since · commits")
        }
        .font(.system(size: 11.5))
        .foregroundStyle(K.muted)
    }
}

struct QuizHome: View {
    @Environment(Store.self) private var store

    private var slipping: [Subsystem] { store.repo.map.filter { $0.moved >= 10 && ($0.pct ?? 0) < 50 }.sorted { $0.moved > $1.moved } }

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner(detail: "A round already on the phone still plays.")
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    HStack {
                        Text("What you know").font(.system(size: 20, weight: .bold))
                        Spacer()
                        Score(streak: store.repo.streak, points: store.repo.points)
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 10)
                    .padding(.bottom, 8)
                    MasteryMap(tiles: store.repo.map).padding(.horizontal, 16)
                    MapLegend().padding(.horizontal, 16).padding(.top, 7)
                    if !slipping.isEmpty {
                        GroupHead(title: "Slipping").padding(.top, -2)
                        ForEach(Array(slipping.enumerated()), id: \.element.id) { i, tile in
                            NavigationLink(value: tile.name) {
                                Row(title: tile.name, sub: AttributedString(tile.name == "Pipelines" ? "\(tile.moved) commits · last asked 3 weeks ago" : "\(tile.moved) commits since you last knew it"), divider: i < slipping.count - 1) { EmptyView() } trail: { ChevronRight() }
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .padding(.bottom, 8)
            }
        }
        .background(K.bg)
        .safeAreaInset(edge: .bottom, spacing: 0) { StartRoundButton().padding(.horizontal, 16).padding(.vertical, 8) }
        .offlineSub(store)
    }
}

struct StartRoundButton: View {
    @Environment(Store.self) private var store

    var body: some View {
        if let extra = store.repo.extraRound {
            Button { store.startRound(def: extra) } label: {
                StartBanner(title: "Round ready: \(extra.title)", detail: "5 questions · about 2 minutes")
            }
            .buttonStyle(.plain)
        } else if let p = store.play, !p.finished {
            Button { store.roundOpen = true } label: {
                StartBanner(title: "Back to your round", detail: "\(p.index + 1) of \(p.count)")
            }
            .buttonStyle(.plain)
        } else if store.repo.nextRound == .ready {
            Button { store.startRound() } label: {
                StartBanner(title: "Next round", detail: "5 questions · about 2 minutes · \(store.repo.missedOnce ? 3 : 2) misses back")
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("next-round")
        } else {
            StartBanner(title: "Writing your next round",
                        detail: store.offline ? "Its machine is out of reach" : "The quiz agent started when your last round did",
                        calm: true, chevron: false)
        }
    }
}

struct SubsystemView: View {
    var name: String
    @Environment(Store.self) private var store

    private var tile: Subsystem? { store.repo.map.first { $0.name == name } }

    var body: some View {
        if let t = tile {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    Hero(title: t.name) { Text(t.paths).font(.mono(12)).foregroundStyle(K.muted) }
                    VStack(alignment: .leading, spacing: 7) {
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(t.pct.map { "\($0)%" } ?? "Not asked").font(.system(size: 38, weight: .bold))
                            Text(t.was).foregroundStyle(K.muted)
                        }
                        ProgressBar(value: t.pct ?? 0)
                        if !t.notes.isEmpty {
                            Card { Text(t.notes).font(.system(size: 14)) }.padding(.top, 3)
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 14)
                    if !t.movedRows.isEmpty {
                        GroupHead(title: "Moved since you knew it", count: "\(t.moved) commits").padding(.top, -14)
                        ForEach(t.movedRows, id: \.0) { Row(title: $0.0, sub: AttributedString($0.1)) { EmptyView() } trail: { EmptyView() } }
                    }
                    if !t.misses.isEmpty {
                        GroupHead(title: "Misses waiting", count: "\(t.misses.count)")
                        ForEach(t.misses, id: \.0) { row in
                            Row(title: row.0, sub: mono(row.1)) { EmptyView() } trail: { EmptyView() }
                        }
                    }
                }
            }
            .background(K.bg)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                Group {
                    if store.repo.quizMeWriting == name {
                        StartBanner(title: "Writing it now", detail: "A card lands in Chat, and here, when it's ready", calm: true, chevron: false)
                    } else if store.repo.extraRound?.title == name || (name == "Machines" && store.repo.extraRound?.id == "machines") {
                        Button { store.startRound(def: store.repo.extraRound) } label: {
                            StartBanner(title: "Round ready: \(name)", detail: "5 questions · about 2 minutes")
                        }
                        .buttonStyle(.plain)
                    } else {
                        Button { store.quizMe(name) } label: {
                            StartBanner(title: "Quiz me on this", detail: store.offline ? "Needs its machine" : "The quiz agent writes it · about a minute")
                        }
                        .buttonStyle(.plain)
                        .disabled(store.offline)
                        .accessibilityIdentifier("quiz-me")
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }
            .pushedNav()
        }
    }

    private func mono(_ t: String) -> AttributedString {
        var a = AttributedString(t)
        a.font = .mono(12)
        return a
    }
}

struct ProgressBar: View {
    var value: Int

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                K.panel2
                K.accent.frame(width: geo.size.width * CGFloat(value) / 100).clipShape(RoundedRectangle(cornerRadius: 4))
            }
        }
        .frame(height: 8)
        .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

struct RoundView: View {
    @Environment(Store.self) private var store

    var body: some View {
        Group {
            if let p = store.play {
                if p.finished { RoundEnd(play: p) } else { QuestionView(play: p).id(p.index) }
            }
        }
        .background(K.bg)
    }
}

struct RoundTop: View {
    var play: RoundPlay
    @Environment(Store.self) private var store

    private func color(_ i: Int) -> Color {
        if i < play.answers.count { return play.answers[i] == play.def.questions[i].correct ? K.ok : K.bad }
        if i == play.index && !play.revealed { return K.accent }
        return K.panel2
    }

    var body: some View {
        HStack(spacing: 10) {
            Button { store.closeRound() } label: {
                Image(systemName: "xmark").font(.system(size: 17, weight: .medium)).foregroundStyle(K.muted).frame(width: 28, height: 28)
            }
            .accessibilityIdentifier("close-round")
            HStack(spacing: 4) {
                ForEach(0..<play.count, id: \.self) { i in
                    Capsule().fill(color(i)).frame(height: 6)
                }
            }
            if store.score.showsPoints {
                HStack(spacing: 3) {
                    Image(systemName: "star.fill").foregroundStyle(K.accent)
                    Text(store.repo.points.formatted())
                }
                .font(.system(size: 14, weight: .semibold))
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 6)
        .padding(.bottom, 10)
    }
}

struct QuestionView: View {
    var play: RoundPlay
    @Environment(Store.self) private var store

    private var q: Question { play.current }
    private var picked: Int? { play.revealed ? play.answers.last : nil }
    private var missed: Bool { picked != nil && picked != q.correct }

    var body: some View {
        VStack(spacing: 0) {
            RoundTop(play: play)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    (Text("\(play.index + 1) of \(play.count)").fontWeight(.semibold).foregroundColor(K.text) + Text(" · \(q.kind) · \(q.source)"))
                        .font(.system(size: 13))
                        .foregroundStyle(K.muted)
                        .padding(.horizontal, 16)
                    Text(rich(q.text, code: q.small ? 15 : 17))
                        .font(.system(size: q.small ? 18 : 20, weight: .semibold))
                        .padding(.horizontal, 16)
                        .padding(.top, 5)
                        .padding(.bottom, 12)
                        .fixedSize(horizontal: false, vertical: true)
                    answers
                    if play.revealed { explanation }
                }
                .padding(.bottom, 16)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if play.revealed {
                PrimaryButton(title: play.index + 1 == play.count ? "See the round" : "Next question", primary: true) { store.nextQuestion() }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .accessibilityIdentifier("next-question")
            }
        }
    }

    @ViewBuilder private var answers: some View {
        switch q.body {
        case .choices(let list):
            let order = missed ? [q.correct, picked ?? 0] : Array(list.indices)
            ForEach(order, id: \.self) { i in
                ChoiceRow(mark: String(i + 1), choice: list[i], state: state(i), showSub: !play.revealed || i == q.correct || i == picked, you: i == picked && missed) { store.answer(i) }
            }
        case .lines(let list):
            ForEach(Array(list.enumerated()), id: \.offset) { i, line in
                LineRow(mark: ["A", "B", "C", "D"][i], line: line, state: state(i)) { store.answer(i) }
            }
        case .drawings:
            ForEach(0..<3, id: \.self) { i in
                DrawingRow(index: i, state: state(i)) { store.answer(i) }
            }
        }
    }

    private func state(_ i: Int) -> AnswerState {
        guard play.revealed else { return .open }
        if i == q.correct { return .right }
        if i == picked { return .wrong }
        return .fade
    }

    @ViewBuilder private var explanation: some View {
        let right = !missed
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(right ? "Right." : "Not quite.").fontWeight(.semibold).foregroundStyle(right ? K.ok : K.bad)
                if right && store.score.showsPoints { Text("+20").font(.system(size: 13, weight: .bold)).foregroundStyle(K.ok) }
                if right && play.run > 1 { Text("\(play.run) in a row").font(.system(size: 13)).foregroundStyle(K.muted) }
            }
            Text(rich(q.why)).font(.system(size: 14.5)).fixedSize(horizontal: false, vertical: true)
            if missed, let code = q.code { CodeCard(snippet: code) }
        }
        .padding(.horizontal, 16)
        .padding(.top, 4)
    }
}

enum AnswerState { case open, right, wrong, fade }

struct MarkCircle: View {
    var text: String
    var state: AnswerState

    var body: some View {
        ZStack {
            Circle().fill(state == .right ? K.ok : state == .wrong ? K.bad : K.panel2)
            switch state {
            case .right: Image(systemName: "checkmark").font(.system(size: 11, weight: .heavy)).foregroundStyle(K.onAccent)
            case .wrong: Image(systemName: "xmark").font(.system(size: 11, weight: .heavy)).foregroundStyle(K.onAccent)
            default: Text(text).font(.system(size: 13, weight: .semibold)).foregroundStyle(K.muted)
            }
        }
        .frame(width: 24, height: 24)
    }
}

private extension View {
    func answerBox(_ state: AnswerState, background: Color = K.panel) -> some View {
        self
            .background(state == .right ? K.okTint : state == .wrong ? K.badTint : background, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(state == .right ? K.ok : state == .wrong ? K.bad : K.line))
            .opacity(state == .fade ? 0.5 : 1)
            .padding(.horizontal, 16)
            .padding(.bottom, 8)
    }
}

struct ChoiceRow: View {
    var mark: String
    var choice: Choice
    var state: AnswerState
    var showSub: Bool
    var you: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 10) {
                MarkCircle(text: mark, state: state)
                VStack(alignment: .leading, spacing: 0) {
                    Text(rich(choice.title, code: 14)).font(.system(size: 15, weight: .semibold)).foregroundStyle(K.text)
                    if showSub { Text(rich(you ? "You picked this" : choice.sub, code: 13)).font(.system(size: 13)).foregroundStyle(K.muted) }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .allowsHitTesting(state == .open)
        .answerBox(state)
    }
}

struct LineRow: View {
    var mark: String
    var line: PickLine
    var state: AnswerState
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 8) {
                ZStack {
                    Circle().fill(state == .right ? K.ok : state == .wrong ? K.bad : K.tint)
                    Text(mark).font(.system(size: 12, weight: .semibold)).foregroundStyle(state == .right || state == .wrong ? K.onAccent : K.accent)
                }
                .frame(width: 22, height: 22)
                Text(String(line.n)).font(.mono(12.5)).foregroundStyle(Color(U.num))
                CodeText(text: line.text, size: 12.5, wrap: true)
            }
            .padding(8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .allowsHitTesting(state == .open)
        .answerBox(state, background: K.term)
    }
}

struct DrawingRow: View {
    var index: Int
    var state: AnswerState
    var action: () -> Void
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                ZStack {
                    Circle().fill(state == .right ? K.ok : state == .wrong ? K.bad : K.tint)
                    Text(["A", "B", "C"][index]).font(.system(size: 13, weight: .semibold)).foregroundStyle(state == .right || state == .wrong ? K.onAccent : K.accent)
                }
                .frame(width: 24, height: 24)
                .padding(.leading, 4)
                Drawing(index: index).frame(height: 128).scaleEffect(pinch)
            }
            .padding(6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .simultaneousGesture(MagnifyGesture().updating($pinch) { value, state, _ in state = min(2.4, max(1, value.magnification)) })
        .allowsHitTesting(state == .open)
        .answerBox(state)
        .zIndex(pinch > 1 ? 2 : 0)
    }
}

struct Drawing: View {
    var index: Int
    private typealias B = (x: CGFloat, y: CGFloat, w: CGFloat, t: String)
    private typealias A = (x1: CGFloat, y1: CGFloat, x2: CGFloat, y2: CGFloat, dash: Bool)

    private var parts: ([B], [A]) {
        switch index {
        case 0: ([(4, 44, 86, "Kanna.app"), (130, 8, 70, "daemon"), (130, 84, 70, "keeper"), (238, 84, 50, "pty")],
                 [(90, 54, 130, 92, false), (200, 99, 238, 99, false), (165, 38, 165, 84, false)])
        case 1: ([(4, 8, 86, "Kanna.app"), (118, 8, 70, "daemon"), (222, 8, 62, "pty"), (150, 84, 70, "keeper")],
                 [(90, 23, 118, 23, false), (188, 23, 222, 23, false), (165, 38, 182, 84, false), (200, 84, 240, 38, true)])
        default: ([(4, 8, 86, "Kanna.app"), (196, 8, 70, "pty"), (4, 84, 86, "daemon")],
                  [(90, 23, 196, 23, false), (47, 84, 47, 38, false)])
        }
    }

    var body: some View {
        Canvas { context, size in
            let scale = min(size.width / 292, size.height / 122)
            let dx = (size.width - 292 * scale) / 2
            let dy = (size.height - 122 * scale) / 2
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: dx + x * scale, y: dy + y * scale) }
            let (boxes, arrows) = parts
            for b in boxes {
                let rect = CGRect(origin: p(b.x, b.y), size: CGSize(width: b.w * scale, height: 30 * scale))
                let shape = Path(roundedRect: rect, cornerRadius: 6 * scale)
                context.fill(shape, with: .color(K.card))
                context.stroke(shape, with: .color(K.axis))
                context.draw(Text(b.t).font(.system(size: 12)).foregroundColor(K.text), at: CGPoint(x: rect.midX, y: rect.midY))
            }
            for a in arrows {
                var line = Path()
                line.move(to: p(a.x1, a.y1))
                line.addLine(to: p(a.x2, a.y2))
                context.stroke(line, with: .color(K.muted), style: StrokeStyle(lineWidth: 1.5, dash: a.dash ? [4, 3] : []))
                let angle = atan2(a.y2 - a.y1, a.x2 - a.x1)
                var head = Path()
                head.move(to: p(a.x2, a.y2))
                head.addLine(to: p(a.x2 - 7 * cos(angle - 0.45), a.y2 - 7 * sin(angle - 0.45)))
                head.addLine(to: p(a.x2 - 7 * cos(angle + 0.45), a.y2 - 7 * sin(angle + 0.45)))
                head.closeSubpath()
                context.fill(head, with: .color(K.muted))
            }
        }
    }
}

struct RoundEnd: View {
    var play: RoundPlay
    @Environment(Store.self) private var store

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    VStack(spacing: 2) {
                        Text("\(play.right) of \(play.count)").font(.system(size: 38, weight: .bold))
                        Text(play.def.about).foregroundStyle(K.muted)
                        HStack(spacing: 20) {
                            if store.score.showsStreak {
                                HStack(spacing: 3) {
                                    Image(systemName: "flame.fill").foregroundStyle(K.warn)
                                    Text("\(store.repo.streak) days")
                                }
                            }
                            if store.score.showsPoints {
                                HStack(spacing: 3) {
                                    Image(systemName: "star.fill").foregroundStyle(K.accent)
                                    Text("+\(play.earned)")
                                }
                            }
                        }
                        .font(.system(size: 16, weight: .semibold))
                        .padding(.top, 8)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.top, 26)
                    .padding(.bottom, 14)
                    ForEach(Array(play.def.questions.enumerated()), id: \.offset) { i, q in
                        let ok = i < play.answers.count && play.answers[i] == q.correct
                        HStack(spacing: 9) {
                            Image(systemName: ok ? "checkmark" : "xmark").font(.system(size: 12, weight: .heavy)).foregroundStyle(ok ? K.ok : K.bad).frame(width: 16)
                            VStack(alignment: .leading, spacing: 0) {
                                Text(q.endLabel).font(.system(size: 14))
                                Text(q.endFile).font(.mono(11.5)).foregroundStyle(K.muted).lineLimit(1)
                            }
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 6)
                    }
                    GroupHead(title: "The map moved")
                    VStack(spacing: 9) {
                        ForEach(play.moves, id: \.0) { move in
                            VStack(spacing: 4) {
                                HStack {
                                    Text(move.0)
                                    Spacer()
                                    Text("\(move.1)% → \(move.2)%").foregroundStyle(K.muted)
                                }
                                .font(.system(size: 14))
                                ProgressBar(value: move.2)
                            }
                        }
                    }
                    .padding(.horizontal, 16)
                    if store.offline {
                        Text("These answers sync when \(store.repo.plannerMachine) is back.")
                            .font(.system(size: 13)).foregroundStyle(K.muted).padding(16)
                    }
                }
            }
            HStack(spacing: 8) {
                PrimaryButton(title: "Done") { store.closeRound() }.accessibilityIdentifier("round-done")
                PrimaryButton(title: store.repo.nextRound == .ready ? "Another round" : "Writing…", primary: true, off: store.repo.nextRound != .ready) { store.anotherRound() }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 8)
        }
    }
}
