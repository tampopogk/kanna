import Foundation
import KannaLinks
import WebKit

@MainActor
final class MachineFiles: NSObject, WKURLSchemeHandler {
    struct Failure: Error {
        let message: String
    }

    typealias Read = @MainActor (String, @escaping @MainActor (Result<(real: String, data: Data), Failure>) -> Void) -> Void
    private let read: Read
    private let folder: String
    private var realFolder: String?
    private var live: Set<ObjectIdentifier> = []

    init(page: String, read: @escaping Read) {
        folder = MachineFileURL.folder(of: page)
        self.read = read
    }

    func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        live.insert(key)
        guard let path = MachineFileURL.path(of: task.request.url), let url = task.request.url else {
            return fail(task, key, NSURLErrorBadURL, "not a file on this page's machine")
        }
        guard MachineFileURL.inside(path, folder) else {
            return fail(task, key, NSURLErrorNoPermissionsToReadFile, "\(path) is outside the page's folder \(folder)")
        }
        read(path) { [weak self] (result: Result<(real: String, data: Data), Failure>) in
            guard let self, live.remove(key) != nil else { return }
            switch result {
            case .success(let file) where realFolder.map { !MachineFileURL.inside(file.real, $0) } ?? false:
                task.didFailWithError(NSError(domain: NSURLErrorDomain, code: NSURLErrorNoPermissionsToReadFile, userInfo: [NSLocalizedDescriptionKey: "\(path) leads outside the page's folder"]))
            case .success(let file):
                if realFolder == nil { realFolder = MachineFileURL.folder(of: file.real) }
                let data = file.data
                let headers = ["Content-Type": MachineFileURL.mime(of: path), "Content-Length": "\(data.count)", "Cache-Control": "no-store"]
                task.didReceive(HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: headers) ?? URLResponse(url: url, mimeType: nil, expectedContentLength: data.count, textEncodingName: nil))
                task.didReceive(data)
                task.didFinish()
            case .failure(let failure):
                task.didFailWithError(NSError(domain: NSURLErrorDomain, code: NSURLErrorFileDoesNotExist, userInfo: [NSLocalizedDescriptionKey: failure.message]))
            }
        }
    }

    func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {
        live.remove(ObjectIdentifier(task))
    }

    private func fail(_ task: any WKURLSchemeTask, _ key: ObjectIdentifier, _ code: Int, _ message: String) {
        live.remove(key)
        task.didFailWithError(NSError(domain: NSURLErrorDomain, code: code, userInfo: [NSLocalizedDescriptionKey: message]))
    }
}
