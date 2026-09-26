// A real WKWebView for the artifact viewer's host document (macOS only).
//
// react-native-webview on iOS is a WKWebView whose navigation delegate asks
// the viewer's `onShouldStartLoadWithRequest`. This harness stands in for that
// delegate: it loads one host document exactly as the viewer's `source.html`
// is loaded, clicks an element inside the sandboxed artifact frame the way a
// finger would reach the page (a DOM click, from an isolated script world the
// page cannot see), and reports every navigation and window request WebKit
// hands the delegate. Like the viewer it allows only the host document and the
// frame document it sets, and refuses everything else.
//
// usage: artifact-host-harness <host.html[|next-host.html...]> <css selector to click in the frame, or ""> <seconds>
// output: one JSON object per line on stdout.

import AppKit
import WebKit

final class Harness: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler {
  var load = 0

  func emit(_ event: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: event) else { return }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([0x0a]))
  }

  func webView(
    _ webView: WKWebView,
    decidePolicyFor navigationAction: WKNavigationAction,
    decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
  ) {
    let url = navigationAction.request.url?.absoluteString ?? ""
    let mainFrame = navigationAction.targetFrame?.isMainFrame ?? false
    emit(["kind": "navigation", "url": url, "mainFrame": mainFrame])
    decisionHandler(url == "about:blank" || url == "about:srcdoc" ? .allow : .cancel)
  }

  func webView(
    _ webView: WKWebView,
    createWebViewWith configuration: WKWebViewConfiguration,
    for navigationAction: WKNavigationAction,
    windowFeatures: WKWindowFeatures
  ) -> WKWebView? {
    emit(["kind": "window-open", "url": navigationAction.request.url?.absoluteString ?? ""])
    return nil
  }

  func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
    guard var body = message.body as? [String: Any] else { return }
    body["mainFrame"] = message.frameInfo.isMainFrame
    body["load"] = load
    emit(body)
  }
}

let arguments = CommandLine.arguments
let paths = arguments.count > 1 ? arguments[1].split(separator: "|", omittingEmptySubsequences: false).map(String.init) : []
guard arguments.count == 4, !paths.isEmpty,
      let firstHTML = try? String(contentsOfFile: paths[0], encoding: .utf8),
      let seconds = Double(arguments[3]) else {
  FileHandle.standardError.write("usage: artifact-host-harness <host.html[|next-host.html...]> <selector> <seconds>\n".data(using: .utf8)!)
  exit(2)
}
let selector = String(data: try! JSONSerialization.data(withJSONObject: [arguments[2]]), encoding: .utf8)!

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let harness = Harness()
let configuration = WKWebViewConfiguration()
configuration.websiteDataStore = .nonPersistent()
let world = WKContentWorld.world(name: "kanna-artifact-harness")
configuration.userContentController.add(harness, contentWorld: world, name: "harness")
// Runs in every frame, in a world of its own: the artifact page has no
// `webkit.messageHandlers` and cannot see this script. It reports what each
// frame shows and, in the artifact frame, performs the click.
configuration.userContentController.addUserScript(WKUserScript(
  source: """
  (function () {
    var post = function (event) { window.webkit.messageHandlers.harness.postMessage(event); };
    var selector = \(selector)[0];
    var cookieBefore = "", cookieAfter = "", cookieReadError = "", cookieWriteError = "";
    try { cookieBefore = document.cookie; } catch (error) { cookieReadError = error.name; }
    try { document.cookie = "kanna_frame_cookie=secret; SameSite=None; Secure"; } catch (error) { cookieWriteError = error.name; }
    try { cookieAfter = document.cookie; } catch (error) { cookieReadError = error.name; }
    post({ kind: "document", top: window === window.top, text: document.body ? document.body.innerText : "",
      cookieBefore: cookieBefore, cookieAfter: cookieAfter,
      cookieReadError: cookieReadError, cookieWriteError: cookieWriteError });
    if (window !== window.top && selector) {
      setTimeout(function () {
        var target = document.querySelector(selector);
        post({ kind: "click", selector: selector, found: Boolean(target) });
        if (target) target.click();
      }, 250);
    }
  })();
  """,
  injectionTime: .atDocumentEnd,
  forMainFrameOnly: false,
  in: world
))

let window = NSWindow(
  contentRect: NSRect(x: 0, y: 0, width: 390, height: 700),
  styleMask: [.borderless],
  backing: .buffered,
  defer: false
)
let webView = WKWebView(frame: window.contentView!.bounds, configuration: configuration)
webView.navigationDelegate = harness
webView.uiDelegate = harness
window.contentView!.addSubview(webView)
window.orderBack(nil)
webView.loadHTMLString(firstHTML, baseURL: nil)
if paths.count > 1 {
  let interval = seconds / Double(paths.count)
  for index in 1..<paths.count {
    DispatchQueue.main.asyncAfter(deadline: .now() + interval * Double(index)) {
      guard let html = try? String(contentsOfFile: paths[index], encoding: .utf8) else { return }
      harness.load = index
      webView.loadHTMLString(html, baseURL: nil)
    }
  }
}
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
app.run()
