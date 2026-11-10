import AVFoundation
import SwiftUI

struct Scanner: UIViewControllerRepresentable {
    let found: (String) -> Void
    let refused: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    func makeUIViewController(context: Context) -> Preview {
        let controller = Preview()
        let coordinator = context.coordinator
        controller.layerHost.session = coordinator.session
        Task {
            guard await AVCaptureDevice.requestAccess(for: .video) else {
                refused()
                return
            }
            await coordinator.begin()
        }
        return controller
    }

    func updateUIViewController(_ controller: Preview, context: Context) {}

    static func dismantleUIViewController(_ controller: Preview, coordinator: Coordinator) {
        coordinator.session.stopRunning()
    }

    final class Preview: UIViewController {
        let layerHost = AVCaptureVideoPreviewLayer()

        override func viewDidLoad() {
            super.viewDidLoad()
            layerHost.videoGravity = .resizeAspectFill
            view.layer.addSublayer(layerHost)
        }

        override func viewDidLayoutSubviews() {
            super.viewDidLayoutSubviews()
            layerHost.frame = view.bounds
        }
    }

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate, @unchecked Sendable {
        let session = AVCaptureSession()
        let found: (String) -> Void
        private var done = false

        init(found: @escaping (String) -> Void) { self.found = found }

        func begin() async {
            guard let camera = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: camera), session.canAddInput(input) else { return }
            session.addInput(input)
            let output = AVCaptureMetadataOutput()
            guard session.canAddOutput(output) else { return }
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            output.metadataObjectTypes = [.qr]
            let session = self.session
            await Task.detached { session.startRunning() }.value
        }

        func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
            guard !done, let code = objects.first as? AVMetadataMachineReadableCodeObject, let text = code.stringValue else { return }
            done = true
            session.stopRunning()
            found(text)
        }
    }
}
