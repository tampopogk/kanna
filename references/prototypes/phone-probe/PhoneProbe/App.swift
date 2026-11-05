import AVFoundation
import SwiftUI

@main
struct PhoneProbeApp: App {
    var body: some Scene {
        WindowGroup { ContentView() }
    }
}

struct ContentView: View {
    @StateObject private var probe = Probe()
    @Environment(\.scenePhase) private var phase
    @State private var scanning = false

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Button("Scan the code on the Mac") { scanning = true }
                        .disabled(probe.running)
                    if probe.running { ProgressView() }
                    Text(probe.stage).font(.caption.monospaced()).accessibilityIdentifier("stage")
                    Text("finished \(probe.finished)").font(.caption.monospaced()).accessibilityIdentifier("finished")
                    Text("backs \(probe.backs.count + probe.failedBacks)").font(.caption.monospaced()).accessibilityIdentifier("backs")
                }
                Section("Checks") {
                    ForEach(probe.checks) { check in
                        VStack(alignment: .leading, spacing: 4) {
                            Label(check.name, systemImage: check.good ? "checkmark.circle" : "xmark.octagon")
                                .foregroundStyle(check.good ? Color.primary : Color.red)
                            Text(check.result).font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                        }
                    }
                }
                if !probe.backs.isEmpty {
                    Section("Back from the background") {
                        Text(probe.backs.map { "\($0) ms" }.joined(separator: " · ")).font(.caption.monospaced())
                    }
                }
            }
            .navigationTitle("Kanna probe")
        }
        .sheet(isPresented: $scanning) {
            Scanner { text in
                scanning = false
                guard let target = try? JSONDecoder().decode(Target.self, from: Data(text.utf8)) else { return }
                Task { await probe.run(target) }
            }
            .ignoresSafeArea()
        }
        .task {
            guard let text = ProcessInfo.processInfo.environment["PROBE_TARGET"], let target = try? JSONDecoder().decode(Target.self, from: Data(text.utf8)) else { return }
            await probe.run(target)
        }
        .onChange(of: phase) { _, now in
            switch now {
            case .background: probe.left()
            case .active: Task { await probe.back() }
            default: break
            }
        }
    }
}

struct Scanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    func makeUIViewController(context: Context) -> UIViewController {
        let controller = UIViewController()
        let session = context.coordinator.session
        Task.detached {
            guard await AVCaptureDevice.requestAccess(for: .video), let camera = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: camera) else { return }
            session.addInput(input)
            let output = AVCaptureMetadataOutput()
            session.addOutput(output)
            output.setMetadataObjectsDelegate(context.coordinator, queue: .main)
            output.metadataObjectTypes = [.qr]
            session.startRunning()
        }
        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.frame = UIScreen.main.bounds
        layer.videoGravity = .resizeAspectFill
        controller.view.layer.addSublayer(layer)
        return controller
    }

    func updateUIViewController(_ controller: UIViewController, context: Context) {}

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate, @unchecked Sendable {
        let session = AVCaptureSession()
        let found: (String) -> Void

        init(found: @escaping (String) -> Void) { self.found = found }

        func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
            guard let code = objects.first as? AVMetadataMachineReadableCodeObject, let text = code.stringValue else { return }
            session.stopRunning()
            found(text)
        }
    }
}
