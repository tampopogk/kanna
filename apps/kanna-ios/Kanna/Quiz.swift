import SwiftUI

struct MapPlace {
    let tile: Tile
    let span: (Int, Int)
    let col: Int
    let row: Int
}

private func span(_ tile: Tile, of total: Int, count: Int) -> (Int, Int) {
    let cells = total > 0 ? Double(tile.lines) / Double(total) * 36 : 36 / Double(max(count, 1))
    if cells >= 8 { return (3, 3) }
    if cells >= 5 { return (3, 2) }
    if cells >= 3 { return (2, 2) }
    if cells >= 2 || tile.name.count > 6 { return (2, 1) }
    return (1, 1)
}

func packMap(_ tiles: [Tile]) -> ([MapPlace], Int) {
    let total = tiles.reduce(0) { $0 + $1.lines }
    var used = Set<[Int]>()
    var out: [MapPlace] = []
    var rows = 0
    for tile in tiles.sorted(by: { $0.lines > $1.lines }) {
        let size = span(tile, of: total, count: tiles.count)
        var placed = false
        var r = 0
        while !placed {
            var c = 0
            while c + size.0 <= 6, !placed {
                let cells = (0..<size.0).flatMap { dx in (0..<size.1).map { dy in [c + dx, r + dy] } }
                if cells.allSatisfy({ !used.contains($0) }) {
                    cells.forEach { used.insert($0) }
                    out.append(MapPlace(tile: tile, span: size, col: c, row: r))
                    rows = max(rows, r + size.1)
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
    var tile: Tile

    private var fill: Color { [K.m0, K.m1, K.m2, K.m3, K.m4][min(tile.level, 4)] }
    private var strong: Bool { tile.level == 4 }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(tile.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(strong ? K.mt4 : K.mt)
            Text((tile.mastery.map { "\($0)%" } ?? "Not asked") + (tile.moved > 0 ? " · +\(tile.moved)" : ""))
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
    var tiles: [Tile]
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
                .frame(width: CGFloat(p.span.0) * cell + CGFloat(p.span.0 - 1) * gap,
                       height: CGFloat(p.span.1) * cellHeight + CGFloat(p.span.1 - 1) * gap)
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

struct QuizHome: View {
    @Environment(Store.self) private var store

    private var slipping: [Tile] { store.repo.map.filter(\.slipping).sorted { $0.moved > $1.moved } }

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner(detail: "A round already on the phone still plays.")
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    Text("What you know")
                        .font(.system(size: 20, weight: .bold))
                        .padding(.horizontal, 16)
                        .padding(.top, 10)
                        .padding(.bottom, 8)
                    if store.repo.map.isEmpty {
                        Card { Text("The map comes with your first round: the quiz writer draws it while it reads the code.").font(.system(size: 14)).foregroundStyle(K.muted) }
                            .padding(.horizontal, 16)
                    } else {
                        MasteryMap(tiles: store.repo.map).padding(.horizontal, 16)
                        MapLegend().padding(.horizontal, 16).padding(.top, 7)
                    }
                    if !slipping.isEmpty {
                        GroupHead(title: "Slipping").padding(.top, -2)
                        ForEach(Array(slipping.enumerated()), id: \.element.id) { i, tile in
                            NavigationLink(value: tile.name) {
                                Row(title: tile.name, sub: AttributedString("\(tile.moved) commits since you last knew it"), divider: i < slipping.count - 1) { EmptyView() } trail: { ChevronRight() }
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
        if let p = store.play, !p.finished {
            Button { store.roundOpen = true } label: {
                StartBanner(title: "Back to your round", detail: "\(p.index + 1) of \(p.count)")
            }
            .buttonStyle(.plain)
        } else if let round = store.unfinishedRound {
            Button { store.start(round) } label: {
                StartBanner(title: "Back to your round", detail: "\(round.answers.count + 1) of \(round.questions.count)")
            }
            .buttonStyle(.plain)
        } else if let extra = store.extraRound {
            Button { store.start(extra) } label: {
                StartBanner(title: "Round ready: \(extra.title)", detail: "5 questions · about 2 minutes")
            }
            .buttonStyle(.plain)
        } else if let next = store.nextRound {
            let back = next.questions.filter { $0.again != nil }.count
            Button { store.start(next) } label: {
                StartBanner(title: "Next round", detail: "5 questions · about 2 minutes" + (back > 0 ? " · \(back) \(back == 1 ? "miss" : "misses") back" : ""))
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("next-round")
        } else if let failed = store.repo.failed {
            Button { store.tryWritingAgain() } label: {
                StartBanner(title: "Couldn't write your next round", detail: failed + ". Try again", calm: true)
            }
            .buttonStyle(.plain)
            .disabled(store.offline)
            .accessibilityIdentifier("try-writing")
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

    private var tile: Tile? { store.repo.map.first { $0.name == name } }

    private var was: String {
        guard let t = tile, let was = t.was, let at = t.wasAt else { return "" }
        return "was \(was)% on \(Date(timeIntervalSince1970: at).formatted(.dateTime.month(.abbreviated).day()))"
    }

    private var writing: Bool { store.repo.writing?.about == name || store.repo.queued.contains(name) }

    var body: some View {
        if let t = tile {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    Hero(title: t.name) { Text(t.paths.joined(separator: " · ")).font(.mono(12)).foregroundStyle(K.muted) }
                    VStack(alignment: .leading, spacing: 7) {
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(t.mastery.map { "\($0)%" } ?? "Not asked").font(.system(size: 38, weight: .bold))
                            Text(was).foregroundStyle(K.muted)
                        }
                        ProgressBar(value: t.mastery ?? 0)
                        if !t.notes.isEmpty {
                            Card { Text(t.notes).font(.system(size: 14)) }.padding(.top, 3)
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 14)
                    if !t.commits.isEmpty {
                        GroupHead(title: "Moved since you knew it", count: "\(t.moved) commits").padding(.top, -14)
                        ForEach(t.commits, id: \.self) { Row(title: $0.subject, sub: AttributedString(Date(timeIntervalSince1970: $0.at).formatted(.relative(presentation: .named)))) { EmptyView() } trail: { EmptyView() } }
                    }
                    if !t.misses.isEmpty {
                        GroupHead(title: "Misses waiting", count: "\(t.misses.count)")
                        ForEach(t.misses, id: \.self) { row in
                            Row(title: row.text, sub: mono(row.file)) { EmptyView() } trail: { EmptyView() }
                        }
                    }
                }
            }
            .background(K.bg)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                Group {
                    if writing {
                        StartBanner(title: "Writing it now", detail: "A card lands in Chat, and here, when it's ready", calm: true, chevron: false)
                    } else if let ready = store.subjectRound(name) {
                        Button { store.start(ready) } label: {
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
        if i < play.answers.count { return play.answers[i] == play.round.questions[i].answer ? K.ok : K.bad }
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
    private var missed: Bool { picked != nil && picked != q.answer }
    private var small: Bool { q.text.count > 64 }

    var body: some View {
        VStack(spacing: 0) {
            RoundTop(play: play)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    (Text("\(play.index + 1) of \(play.count)").fontWeight(.semibold).foregroundColor(K.text) + Text(" · \(q.kindName) · \(q.source ?? q.file)"))
                        .font(.system(size: 13))
                        .foregroundStyle(K.muted)
                        .padding(.horizontal, 16)
                    Text(rich(q.text, code: small ? 15 : 17))
                        .font(.system(size: small ? 18 : 20, weight: .semibold))
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
        if let drawings = q.drawings {
            ForEach(Array(drawings.enumerated()), id: \.offset) { i, source in
                DrawingRow(index: i, source: source, state: state(i)) { store.answer(i) }
            }
        } else if let lines = q.lines {
            ForEach(Array(lines.enumerated()), id: \.offset) { i, line in
                LineRow(mark: ["A", "B", "C", "D"][i % 4], line: line, file: q.file, state: state(i)) { store.answer(i) }
            }
        } else {
            let list = q.choices ?? []
            let order = missed ? [q.answer, picked ?? 0] : Array(list.indices)
            ForEach(order, id: \.self) { i in
                ChoiceRow(mark: String(i + 1), choice: list[i], state: state(i), showSub: !play.revealed || i == q.answer || i == picked, you: i == picked && missed) { store.answer(i) }
            }
        }
    }

    private func state(_ i: Int) -> AnswerState {
        guard play.revealed else { return .open }
        if i == q.answer { return .right }
        if i == picked { return .wrong }
        return .fade
    }

    @ViewBuilder private var explanation: some View {
        let right = !missed
        VStack(alignment: .leading, spacing: 8) {
            Text(right ? "Right." : "Not quite.").fontWeight(.semibold).foregroundStyle(right ? K.ok : K.bad)
            Text(rich(q.why)).font(.system(size: 14.5)).fixedSize(horizontal: false, vertical: true)
            if missed, let code = q.code { CodeCard(snippet: store.snippet(code, key: "\(play.round.id)/\(play.index)")) }
        }
        .padding(.horizontal, 16)
        .padding(.top, 4)
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
                    Text(rich(choice.label, code: 14)).font(.system(size: 15, weight: .semibold)).foregroundStyle(K.text)
                    if showSub, let sub = you ? "You picked this" : choice.why { Text(rich(sub, code: 13)).font(.system(size: 13)).foregroundStyle(K.muted) }
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
    var file: String
    var state: AnswerState
    var action: () -> Void
    @Environment(Store.self) private var store

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 8) {
                ZStack {
                    Circle().fill(state == .right ? K.ok : state == .wrong ? K.bad : K.tint)
                    Text(mark).font(.system(size: 12, weight: .semibold)).foregroundStyle(state == .right || state == .wrong ? K.onAccent : K.accent)
                }
                .frame(width: 22, height: 22)
                Text(String(line.n)).font(.mono(12.5)).foregroundStyle(Color(U.num))
                CodeText(text: line.text, spans: store.backend.highlight(file: file, text: line.text), size: 12.5, wrap: true)
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
    var source: String
    var state: AnswerState
    var action: () -> Void
    @Environment(Store.self) private var store
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                ZStack {
                    Circle().fill(state == .right ? K.ok : state == .wrong ? K.bad : K.tint)
                    Text(["A", "B", "C"][index % 3]).font(.system(size: 13, weight: .semibold)).foregroundStyle(state == .right || state == .wrong ? K.onAccent : K.accent)
                }
                .frame(width: 24, height: 24)
                .padding(.leading, 4)
                Drawing(scene: store.backend.drawing(source)).frame(height: 128).scaleEffect(pinch)
            }
            .padding(6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("drawing-\(index)")
        .simultaneousGesture(MagnifyGesture().updating($pinch) { value, state, _ in state = min(2.4, max(1, value.magnification)) })
        .allowsHitTesting(state == .open)
        .answerBox(state)
        .zIndex(pinch > 1 ? 2 : 0)
    }
}

struct Drawing: View {
    var scene: DrawingScene?

    var body: some View {
        Canvas { context, size in
            guard let scene, scene.width > 0, scene.height > 0 else { return }
            let scale = min(size.width / scene.width, size.height / scene.height, 1)
            let dx = (size.width - scene.width * scale) / 2
            let dy = (size.height - scene.height * scale) / 2
            func p(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: dx + x * scale, y: dy + y * scale) }
            for edge in scene.edges {
                guard let first = edge.points.first, let last = edge.points.last, edge.points.count > 1 else { continue }
                var line = Path()
                line.move(to: p(first.x, first.y))
                edge.points.dropFirst().forEach { line.addLine(to: p($0.x, $0.y)) }
                context.stroke(line, with: .color(K.muted), style: StrokeStyle(lineWidth: 1.5, dash: edge.dashed ? [4, 3] : []))
                let before = edge.points[edge.points.count - 2]
                let end = p(last.x, last.y)
                let start = p(before.x, before.y)
                let angle = atan2(end.y - start.y, end.x - start.x)
                var head = Path()
                head.move(to: end)
                head.addLine(to: CGPoint(x: end.x - 7 * cos(angle - 0.45), y: end.y - 7 * sin(angle - 0.45)))
                head.addLine(to: CGPoint(x: end.x - 7 * cos(angle + 0.45), y: end.y - 7 * sin(angle + 0.45)))
                head.closeSubpath()
                context.fill(head, with: .color(K.muted))
                if let at = edge.at {
                    context.draw(Text(edge.label).font(.system(size: 10)).foregroundColor(K.muted), at: p(at.x, at.y))
                }
            }
            for node in scene.nodes {
                let rect = CGRect(origin: p(node.x, node.y), size: CGSize(width: node.width * scale, height: node.height * scale))
                let shape = Path(roundedRect: rect, cornerRadius: 6)
                context.fill(shape, with: .color(K.card))
                context.stroke(shape, with: .color(K.axis))
                context.draw(Text(node.label).font(.system(size: 12)).foregroundColor(K.text), at: CGPoint(x: rect.midX, y: rect.midY))
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
                        Text(play.round.says ?? "about what's new on main").foregroundStyle(K.muted)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.top, 26)
                    .padding(.bottom, 14)
                    ForEach(Array(play.round.questions.enumerated()), id: \.offset) { i, q in
                        let ok = i < play.answers.count && play.answers[i] == q.answer
                        HStack(spacing: 9) {
                            Image(systemName: ok ? "checkmark" : "xmark").font(.system(size: 12, weight: .heavy)).foregroundStyle(ok ? K.ok : K.bad).frame(width: 16)
                            VStack(alignment: .leading, spacing: 0) {
                                Text(q.topic).font(.system(size: 14))
                                Text("\(q.file):\(q.line)").font(.mono(11.5)).foregroundStyle(K.muted).lineLimit(1).truncationMode(.head)
                            }
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 6)
                    }
                    if !play.moves.isEmpty {
                        GroupHead(title: "The map moved")
                        VStack(spacing: 9) {
                            ForEach(play.moves, id: \.name) { move in
                                VStack(spacing: 4) {
                                    HStack {
                                        Text(move.name)
                                        Spacer()
                                        Text("\(move.from)% → \(move.to)%").foregroundStyle(K.muted)
                                    }
                                    .font(.system(size: 14))
                                    ProgressBar(value: move.to)
                                }
                            }
                        }
                        .padding(.horizontal, 16)
                    }
                    if store.offline {
                        Text("These answers sync when \(store.repo.plannerMachine) is back.")
                            .font(.system(size: 13)).foregroundStyle(K.muted).padding(16)
                    }
                }
            }
            HStack(spacing: 8) {
                PrimaryButton(title: "Done") { store.closeRound() }.accessibilityIdentifier("round-done")
                PrimaryButton(title: store.nextRound != nil ? "Another round" : "Writing…", primary: true, off: store.nextRound == nil) { store.anotherRound() }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 8)
        }
    }
}
