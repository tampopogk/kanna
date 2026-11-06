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
            if let error = store.pairError {
                Text(error).multilineTextAlignment(.center).foregroundStyle(K.bad).padding(.top, 8)
            }
            Spacer()
            PrimaryButton(title: "Scan the code", primary: true) { store.beginScan() }
                .accessibilityIdentifier("scan")
                .padding(.bottom, 12)
        }
        .padding(.horizontal, 30)
    }
}

struct PairScan: View {
    @Environment(Store.self) private var store

    var body: some View {
        ZStack {
            if !store.fixture {
                Scanner { store.scanned($0) } refused: { store.pairStep = .refused }
                    .ignoresSafeArea()
            }
            RoundedRectangle(cornerRadius: 22)
                .stroke(.white, lineWidth: 3)
                .frame(width: 244, height: 244)
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
        .statusBarHidden()
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
            PrimaryButton(title: "Open Settings", primary: true) {
                if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                store.pairStep = .start
            }
                .padding(.bottom, 12)
        }
        .padding(.horizontal, 30)
    }
}

struct PairConfirm: View {
    @Environment(Store.self) private var store

    var body: some View {
        VStack(spacing: 10) {
            Logo()
            Text("Confirm on the Mac").font(.system(size: 24, weight: .bold)).padding(.top, 6)
            Text("Kanna is asking whether to add this phone. Check it shows this phone's name.").multilineTextAlignment(.center).foregroundStyle(K.muted)
            Text(store.phoneName)
                .font(.mono(13))
                .tracking(1)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(K.panel, in: RoundedRectangle(cornerRadius: 10))
            Working(text: "Waiting for the Mac")
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
                    Text("Paired with \(store.machineName)").fontWeight(.semibold)
                }
                .foregroundStyle(K.ok)
                Text("Your repos").font(.system(size: 24, weight: .bold)).padding(.top, 8)
                Text("From \(store.machineName)").foregroundStyle(K.muted).padding(.top, 3)
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
