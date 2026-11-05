import SwiftUI

struct Logo: View {
    var body: some View {
        Text("K")
            .font(.mono(34, .bold))
            .foregroundStyle(K.accent)
            .frame(width: 76, height: 76)
            .background(K.term, in: RoundedRectangle(cornerRadius: 18))
            .overlay(RoundedRectangle(cornerRadius: 18).stroke(K.line))
    }
}

struct PairingFlow: View {
    @Environment(Store.self) private var store

    var body: some View {
        Group {
            switch store.pairStep {
            case .start: PairStart()
            case .scan: PairScan()
            case .refused: PairRefused()
            case .confirm: PairConfirm()
            case .done: PairDone()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(store.pairStep == .scan ? Color(red: 0.043, green: 0.047, blue: 0.055) : K.bg)
        .animation(.snappy, value: store.pairStep)
        .onLongPressGesture(minimumDuration: 1.5) { store.showDemo = true }
    }
}

struct PairStart: View {
    @Environment(Store.self) private var store

    var body: some View {
        VStack(spacing: 10) {
            Spacer()
            Logo()
            Text("Pair with one of your machines").font(.system(size: 24, weight: .bold)).multilineTextAlignment(.center).padding(.top, 6)
            Text("On your Mac, open Kanna, then Machines, then Pair a Phone. Nothing goes through Kanna's servers: the phone and the machine check each other directly.")
                .multilineTextAlignment(.center)
                .foregroundStyle(K.muted)
            Spacer()
            PrimaryButton(title: "Scan the code", primary: true) { store.beginScan() }
                .accessibilityIdentifier("scan")
                .padding(.bottom, 12)
        }
        .padding(.horizontal, 30)
    }
}

struct FakeQR: View {
    var body: some View {
        Canvas { context, size in
            let unit = size.width / 25
            var random = LCG(x: 42)
            func cell(_ x: Int, _ y: Int, _ w: Int = 1, _ color: Color = .black) {
                context.fill(Path(CGRect(x: CGFloat(x) * unit, y: CGFloat(y) * unit, width: CGFloat(w) * unit, height: CGFloat(w) * unit)), with: .color(color))
            }
            for y in 0..<25 {
                for x in 0..<25 {
                    let finder = (x < 7 && y < 7) || (x > 17 && y < 7) || (x < 7 && y > 17)
                    if !finder && random.next() > 0.5 { cell(x, y) }
                }
            }
            for (x, y) in [(0, 0), (18, 0), (0, 18)] {
                cell(x, y, 7)
                cell(x + 1, y + 1, 5, .white)
                cell(x + 2, y + 2, 3)
            }
        }
    }
}

struct PairScan: View {
    @Environment(Store.self) private var store

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 22)
                .stroke(.white, lineWidth: 3)
                .frame(width: 244, height: 244)
                .overlay {
                    FakeQR().frame(width: 180, height: 180).padding(8).background(.white, in: RoundedRectangle(cornerRadius: 8))
                }
            VStack {
                HStack {
                    Button { store.pairStep = .start } label: { Image(systemName: "xmark").font(.system(size: 18, weight: .medium)) }
                    Spacer()
                    Text("Scan the code").fontWeight(.semibold)
                    Spacer()
                    Color.clear.frame(width: 22, height: 22)
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                Spacer()
                Text("Point at the code in Kanna, on your Mac").opacity(0.8).padding(.bottom, 40)
            }
        }
        .foregroundStyle(.white)
    }
}

struct PairRefused: View {
    @Environment(Store.self) private var store

    var body: some View {
        VStack(spacing: 10) {
            Spacer()
            Image(systemName: "camera").font(.system(size: 40)).foregroundStyle(K.muted)
            Text("Kanna needs the camera").font(.system(size: 24, weight: .bold)).padding(.top, 6)
            Text("The code on your Mac is the only way to pair. Turn the camera on for Kanna in Settings.").multilineTextAlignment(.center).foregroundStyle(K.muted)
            Spacer()
            PrimaryButton(title: "Open Settings", primary: true) { store.cameraRefused = false; store.pairStep = .start }
                .padding(.bottom, 12)
        }
        .padding(.horizontal, 30)
    }
}

struct PairConfirm: View {
    var body: some View {
        VStack(spacing: 10) {
            Logo()
            Text("Confirm on studio").font(.system(size: 24, weight: .bold)).padding(.top, 6)
            Text("Kanna on studio is asking whether to add this phone. Check it shows the same letters.").multilineTextAlignment(.center).foregroundStyle(K.muted)
            Text("7F3A · 19C2 · E0B4")
                .font(.mono(13))
                .tracking(1)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(K.panel, in: RoundedRectangle(cornerRadius: 10))
            Working(text: "Waiting for studio")
        }
        .padding(.horizontal, 30)
    }
}

struct PairDone: View {
    @Environment(Store.self) private var store

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Image(systemName: "checkmark").font(.system(size: 14, weight: .bold))
                    Text("Paired with studio").fontWeight(.semibold)
                }
                .foregroundStyle(K.ok)
                Text("Your repos").font(.system(size: 24, weight: .bold)).padding(.top, 8)
                Text("From studio, mbp, dev-1 and tests-1").foregroundStyle(K.muted).padding(.top, 3)
            }
            .padding(.horizontal, 16)
            .padding(.top, 22)
            .padding(.bottom, 10)
            K.line.frame(height: 1)
            ForEach(store.repos) { repo in
                Button { store.finishPairing(repo: repo.id) } label: {
                    RepoRow(repo: repo).overlay(alignment: .trailing) { ChevronRight().padding(.trailing, 16) }
                }
                .buttonStyle(.plain)
            }
            Spacer()
        }
    }
}
