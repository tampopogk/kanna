import Foundation
import KannaLinks
import WebKit

@MainActor
final class MachineFiles: NSObject, WKURLSchemeHandler {
    struct Failure: Error {
        let message: String
    }

    typealias Read = @MainActor (String, @escaping @MainActor (Result<Data, Failure>) -> Void) -> Void
    private let read: Read
    private var live: Set<ObjectIdentifier> = []

    init(read: @escaping Read) {
        self.read = read
    }

    func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        live.insert(key)
        guard let path = MachineFileURL.path(of: task.request.url), let url = task.request.url else {
            return fail(task, key, NSURLErrorBadURL, "not a file on this page's machine")
        }
        read(path) { [weak self] (result: Result<Data, Failure>) in
            guard let self, live.remove(key) != nil else { return }
            switch result {
            case .success(let data):
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
