// Checks, in real WebKit (the engine of the packaged app), what the production
// Content-Security-Policy of `src-tauri/tauri.conf.json` allows for the plugin sandbox worker.
//
//   swiftc -O dev/csp-worker-check/main.swift -o /tmp/cspcheck && /tmp/cspcheck
//
// Tauri sends the CSP header on HTML responses only, so a same-origin worker script is served
// without one: `new Function` works inside it (the plugin sandbox needs that) while the page itself
// stays blocked from eval, and a `blob:` worker (which inherits the page's CSP) cannot start at all.
// Expected output:
//   RESULT {"pageEval":"blocked: EvalError","sameOriginWorker":"allowed","moduleWorker":"allowed","blobWorker":"worker error: blocked"}
// Keep the `csp` string in sync with `app.security.csp` when that changes.

import Cocoa
import WebKit

// Mimics Tauri's `tauri://` protocol: the CSP header is sent on HTML responses only.
let csp = "default-src 'self' ipc: http://ipc.localhost; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: asset: http://asset.localhost; font-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost"

let files: [String: (String, String)] = [
  "/index.html": ("text/html", """
<!doctype html><html><head><meta charset="utf-8"></head><body><script src="/main.js"></script></body></html>
"""),
  "/main.js": ("text/javascript", """
const out = {};
const done = () => { window.webkit.messageHandlers.report.postMessage(JSON.stringify(out)); };
// 1. eval on the page itself must stay blocked by the CSP
try { new Function('return 1')(); out.pageEval = 'allowed'; } catch (e) { out.pageEval = 'blocked: ' + e.name; }
// 2. a same-origin worker script (how Vite serves the plugin worker)
const w = new Worker('/worker.js');
w.onmessage = (m) => { out.sameOriginWorker = m.data; step2(); };
w.onerror = (e) => { out.sameOriginWorker = 'worker error: ' + (e.message || 'unknown'); step2(); };
// 3. a module worker (how the app creates it: { type: 'module' })
function step2() {
  const m = new Worker('/worker.mjs', { type: 'module' });
  m.onmessage = (ev) => { out.moduleWorker = ev.data; step3(); };
  m.onerror = (e) => { out.moduleWorker = 'worker error: ' + (e.message || 'unknown'); step3(); };
}
// 4. a blob worker (inherits the page CSP)
function step3() {
  try {
    const url = URL.createObjectURL(new Blob(["try { new Function('return 1')(); postMessage('allowed') } catch (e) { postMessage('blocked: ' + e.name) }"], { type: 'text/javascript' }));
    const b = new Worker(url);
    b.onmessage = (ev) => { out.blobWorker = ev.data; done(); };
    b.onerror = (e) => { out.blobWorker = 'worker error: ' + (e.message || 'blocked'); done(); };
  } catch (e) { out.blobWorker = 'threw: ' + e.name; done(); }
}
setTimeout(() => { out.timeout = true; done(); }, 6000);
"""),
  "/worker.js": ("text/javascript", "try { new Function('return 1')(); postMessage('allowed') } catch (e) { postMessage('blocked: ' + e.name) }"),
  "/worker.mjs": ("text/javascript", "try { new Function('return 1')(); postMessage('allowed') } catch (e) { postMessage('blocked: ' + e.name) }\nexport {}"),
]

class Handler: NSObject, WKURLSchemeHandler {
  func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
    let path = task.request.url?.path ?? "/"
    guard let (mime, body) = files[path.isEmpty || path == "/" ? "/index.html" : path] else {
      task.didFailWithError(NSError(domain: "x", code: 404)); return
    }
    var headers = ["Content-Type": mime, "Access-Control-Allow-Origin": "tauri://localhost"]
    if mime == "text/html" { headers["Content-Security-Policy"] = csp }
    let resp = HTTPURLResponse(url: task.request.url!, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: headers)!
    task.didReceive(resp)
    task.didReceive(body.data(using: .utf8)!)
    task.didFinish()
  }
  func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}

class Report: NSObject, WKScriptMessageHandler {
  func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
    print("RESULT " + (m.body as? String ?? "?"))
    exit(0)
  }
}

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let cfg = WKWebViewConfiguration()
cfg.setURLSchemeHandler(Handler(), forURLScheme: "tauri")
cfg.userContentController.add(Report(), name: "report")
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 400, height: 300), configuration: cfg)
web.load(URLRequest(url: URL(string: "tauri://localhost/index.html")!))
DispatchQueue.main.asyncAfter(deadline: .now() + 12) { print("RESULT timeout-no-report"); exit(1) }
app.run()
