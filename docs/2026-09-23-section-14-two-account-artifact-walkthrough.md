# §14 two-account artifact walkthrough and mobile viewer device checks

Spec: `docs/specs/tasks-sessions-structured-workflows.md` §8 (artifacts, sharing) and
§14 (multi-user, iteration one). Delivery card: T12b.

This page has two parts that only a person with the hardware can run:

1. **The §14 walkthrough.** Two Kanna accounts on two machines with no pairing
   between them share one artifact through one Git remote. Machine A publishes
   and pushes. Machine B fetches by hash, views it, comments, records a
   decision and pushes back. Machine A fetches and sees B's records on the
   exact version. Only A's owner operates A's gate.
2. **Physical-device checks** for the native mobile viewer
   (`react-native-webview` on iOS and Android).

Everything short of those human steps is automated. The next section lists
what the automation proves, so the human steps only cover what it can't.

## What is already automated

| Check | Where | What it proves |
| --- | --- | --- |
| Two homes, one bare remote, real HTTP API | `crates/kanna-server/src/http_api/tests/artifacts.rs` `section_14_two_accounts_share_review_through_one_remote_without_pairing` | Two servers, each with its own home and database. A publishes a multi-file mockup from a task and pushes (refs created; pushing again reports them up to date). B has no artifact store beforehand; it fetches by hash, reads a file, records an anchored comment and a decision, and pushes. A fetches (2 records imported) and sees the anchor and decision on the exact hash. The decision moves no task on A. B cannot address A's task (404). A's own advance-stage moves A's task. The test enters that last step through the route's test hook, not a spawned agent. |
| Remote source reporting | same file, `the_artifact_remote_route_reports_what_push_and_fetch_will_use` | `GET /v1/repos/{id}/artifact-remote` reports the redacted remote and whether `.kanna/config.json` (committed) or `.kanna/config.local.json` (machine-local) chose it. A password containing `/` is redacted too (`remote_tests.rs`). |
| Desktop viewer | `apps/desktop/src/components/__tests__/ArtifactViewer.test.ts` | The remote and its config source are shown. The first push to a remote waits for confirmation. Push outcomes, refused refs, fetch outcomes, refused imports and missing earlier versions are rendered. The navigated id is emitted for tab persistence. In-flight file reads are aborted. |
| Mobile viewer | `apps/mobile/src/screens/ArtifactViewer.test.tsx` | Comments are anchored to the exact version and file. A decision is recorded as data and the UI says it operates no gate. Remote, source, first-push confirmation, push/fetch outcomes. On Android the host asks for an in-tree file without navigating (its own title, then `history.replaceState`), and the viewer opens the file from `onLoadStart` however late the JS thread answers, with no path in any URL (`on Android, opens an in-tree link without any navigation, however late the JS thread answers`). An abandoned page's reads settle before the next page's reads start. |
| Real WebKit click path | `apps/mobile/src/screens/ArtifactViewer.webkit.test.tsx` (macOS; builds `apps/mobile/tests/webkit/artifact-host-harness.swift`) | A DOM click inside the sandboxed frame in a real WKWebView reaches the navigation delegate as `kanna-host:open?path=…`. The viewer's real callback refuses that navigation and renders the page, and WebKit shows it. A hostile page's attempts to reach the delegate or leave the frame all fail: top navigation, self-navigation, popups, form submission, meta refresh and `_top` links. A positive control without the sandbox shows the same attempts reach the delegate. |
| Desktop isolation in the Tauri webview | `apps/desktop/tests/e2e/mock/artifact-viewer.test.ts` (mock E2E lane) | The probe page's fetches fail. The page reports a `connect-src` CSP violation, and a loopback listener that only the test knows about receives no request. |

Not automated: two real accounts signed in on two real machines, a remote on
another host over SSH, and the phone WebViews (iOS device, any Android).

## Part 1: the §14 walkthrough

### What you need

- **Machine A** signed in to Kanna as account A (the owner of the task and its
  gate). **Machine B** signed in as a different account B.
- **No pairing between A and B.** On each machine, open Settings → Machines. The
  other machine must not appear as paired or trusted. If it does, unpair it
  before you start: §14 must work with no pairing.
- **One Git remote both machines can reach.** Either a bare repository on a
  host both can SSH to, or a private repository on a Git host both accounts can
  push to. For a bare repository:

  ```sh
  ssh artifact-host 'git init --bare ~/kanna-artifacts.git'
  ```

  Credentials are each machine's own (SSH keys, agent, credential helper).
  Kanna stores none.
- **The same working repository** added to Kanna on both machines. The repo id
  differs per machine; that is expected.

### 0. Configure the remote on both machines

On **each** machine, in the working repository checkout Kanna uses, create or
extend `.kanna/config.local.json`:

```json
{ "artifacts": { "remote": "ssh://artifact-host/~/kanna-artifacts.git" } }
```

Use a machine-local file for this walkthrough. A remote in the committed
`.kanna/config.json` also works, but then anyone who can commit to the repo
chooses where pushes go. The viewer tells you which file chose the remote.

**Check (both machines):** open any artifact viewer tab (command palette →
"Open Artifact by Tree Id…"). The **Artifact remote** panel shows
`ssh://artifact-host/~/kanna-artifacts.git` and "Chosen by this machine's config
(.kanna/config.local.json)". If the URL carries a password, it shows as `***@`.

### 1. A: produce a mockup in a stage

1. On A, create a task in the working repository and have its agent produce an
   HTML mockup with at least one stylesheet or image, and publish it:
   `kanna_publish_artifact { "task_id": "<task>", "path": "<dir>", "kind": "mockup" }`.
   (`kanna-cli tool call kanna_publish_artifact --json '{…}'` is the CLI form.)
2. Note the returned `artifactId`, a 40-hex tree id. This is the hash you give
   B outside Kanna (chat, email). No relay notification is involved.
3. Open it on A (command palette → "Open Artifact by Tree Id…", paste the hash).
   The mockup renders with its assets.

**Check:** the task stays in its stage. Publishing does not advance it.

### 2. A: push

1. In the viewer, press **Push this version**.
2. **Check:** before anything is sent, a confirmation shows the hash, the remote
   URL and the config file that chose it. Accept.
3. **Check:** the outcome reads "Pushed to … : N refs created, M already up to
   date" and lists the versions sent.
4. Press **Push this version** again. **Check:** no confirmation this time.
   The outcome reports 0 refs created and the rest up to date.

### 3. B: fetch by hash and view

1. On B, open the artifact viewer for the working repository and paste the hash.
   **Check:** "No artifact with this id in this repository", with a **Fetch it
   from the artifact remote** button.
2. Press it (or type the hash and press **Fetch** in the toolbar).
   **Check:** the outcome reads "Fetched … ; K records imported", with no
   refused refs and no missing versions, and the mockup renders at the hash with
   its assets.
3. **Check:** the "Version" panel shows the same 40-hex id A published and names
   A's task id as producer. B has no such task. The id is data, not a link to
   anything B can operate.

### 4. B: comment and decide on the exact version, and push back

1. On B, in **Comments on this version**, enter a name and a comment. Anchor it
   to a file (for example the stylesheet) with a position such as `line 2`
   and an excerpt. Press **Add comment**.
2. In **Decisions on this version**, enter who and `approved` (or
   `changes requested`). **Check:** the panel says a decision "does not move any
   task or operate any gate". Press **Record decision**.
3. Optional: do steps 1–2 from B's phone instead. B's phone must be paired
   with B's own desktop (same account); open the task's action menu → **Open
   Artifact…**. The phone shows the same remote panel and forms.
4. Press **Push this version** on B and confirm. **Check:** refs are created
   for B's two records.

### 5. A: fetch and see B's records against the mockup

1. On A, with the artifact open at the same hash, press **Fetch**.
2. **Check:** the outcome reads "2 records imported" and "Received decisions are
   records; no task moved."
3. **Check:** B's comment appears under **Comments on this version** with its
   anchor. Selecting the anchor shows the stylesheet with the anchored line
   marked. B's decision appears under **Decisions on this version**.
4. If A published an earlier version with a `previous` link, press **Previous
   version**. **Check:** B's records do not appear there. Records belong to the
   exact tree id.
5. **Check:** A's task is still in the same stage. The received decision
   changed nothing.

### 6. A: the owner operates the gate

1. On A, advance (or reject) the task from its stage: task header → Advance
   Stage, or `kanna_advance_stage`. **Check:** the task moves. This is the only
   step that moved it.
2. On B, **check:** A's task is not visible anywhere (no pairing, different
   account), so B has no control that could operate it.

### Recording the result

Report which steps passed, with the hash, both machines' names, the remote form
used (bare SSH repository or Git host), and a screenshot of A's viewer at step
5.3. Anything unexpected is a finding against T12b.

## Part 2: physical-device checks for the native mobile viewer

These cover what the WKWebView harness cannot: `react-native-webview` itself,
on an iOS **device** and on **Android** (any device or emulator with Google's
WebView). The iOS simulator cannot pair to a desktop, so iOS needs a device.

### Probe artifact

Publish this directory from any task (kind `mockup`) and open it on the phone
(task action menu → **Open Artifact…**, paste the hash).

`index.html`:

```html
<!doctype html>
<meta charset="utf-8">
<link rel="stylesheet" href="css/site.css">
<h1>Probe index</h1>
<p><a href="pages/about.html">In-tree link</a></p>
<ul id="probe"></ul>
<form id="f" action="https://example.com/form" method="post"></form>
<script>
function report(name, outcome) {
  var li = document.createElement("li");
  li.textContent = name + ": " + outcome;
  li.className = /^(blocked|absent|opaque)/.test(outcome) ? "ok" : "bad";
  document.getElementById("probe").appendChild(li);
}
function attempt(name, action, blocked) {
  try { var v = action(); report(name, blocked(v) ? "blocked (" + v + ")" : "ALLOWED (" + v + ")"); }
  catch (e) { report(name, "blocked (" + e.name + ")"); }
}
report("origin", self.origin === "null" ? "opaque" : "NOT OPAQUE " + self.origin);
attempt("read host document", function () { return parent.document.title; }, function () { return false; });
attempt("RN bridge", function () { return typeof window.ReactNativeWebView; }, function (v) { return v === "undefined"; });
attempt("cookies", function () { return document.cookie; }, function () { return false; });
attempt("localStorage", function () { return localStorage.length; }, function () { return false; });
attempt("window.open", function () { return window.open("https://example.com/"); }, function (v) { return v === null; });
fetch("https://example.com/").then(function () { report("fetch", "ALLOWED"); }, function (e) { report("fetch", "blocked (" + e.name + ")"); });
setTimeout(function () {
  attempt("form submit", function () { document.getElementById("f").submit(); return "submitted"; }, function () { return false; });
  attempt("top navigation", function () { top.location.href = "https://example.com/"; return "assigned"; }, function () { return false; });
  attempt("forged host-open", function () { top.location.href = "kanna-host:open?path=pages%2Fabout.html"; return "assigned"; }, function () { return false; });
}, 500);
</script>
```

`css/site.css`: `h1 { color: rgb(12, 34, 56); } li.ok { color: green; } li.bad { color: red; font-weight: bold; }`

`pages/about.html`: `<link rel="stylesheet" href="../css/site.css"><h1>About (same tree)</h1>`

Some "assigned" results are expected. The sandbox lets the assignment run
and refuses the navigation itself. Pass or fail is whether the viewer
stays on the probe page.

### iOS device

- [ ] The probe page renders with the dark-blue heading (the stylesheet loaded
      from the tree).
- [ ] Every probe line reads `blocked`, `absent` or `opaque`, except `form
      submit`, `top navigation` and `forged host-open`, which may read
      `ALLOWED (assigned|submitted)`. **After 2 s the viewer still shows the
      probe page**: no Safari, no other app, no blank page, and the header
      path is still `index.html`.
- [ ] Tap **In-tree link**. The viewer shows `pages/about.html` with the same
      heading color. The header path changes to `pages/about.html`.
- [ ] **Entrypoint** returns to the probe page.
- [ ] Comments/decisions: record an anchored comment and a decision. Both
      appear under the exact version, and the decision note says it operates no
      gate.
- [ ] Push/fetch: the remote panel shows the remote and its config file. The
      first push asks for confirmation. A fetch of a hash the desktop lacks
      opens it.

### Android

All of the iOS items, plus:

- [ ] Tap **In-tree link** several times while the phone is busy (for example
      right after opening the viewer, while records are still rendering).
      **No "Webpage not available" / `ERR_UNKNOWN_URL_SCHEME` page appears at
      any point.** The viewer goes straight from the page to its own
      "Loading pages/about.html…" state and then shows the page.
- [ ] On Android a host-open never navigates: the host sets its own title and
      calls `history.replaceState`, and the viewer reads the title from
      `onLoadStart`. So `adb logcat -s RNCWebViewClient` prints no "Did not
      receive response to shouldOverrideUrlLoading in time, defaulting to allow
      loading" during those taps. A warning there means a navigation reached
      the library's 250 ms fail-open again.
- [ ] A full `adb logcat` capture taken over those taps has no line containing
      `kanna-host` or an artifact file path (`pages/about`, `pages%2Fabout`,
      `css/site.css`). Chromium's `cr_CookieManager` "Bad port" lines for bare
      `about:blank` and `data:text/html;charset=utf-8;base64,` URLs appear on
      every page load and are expected; they name no file.

### Recording the result

For each platform, give the device model, OS version and WebView version
(Android: Settings → Apps → Android System WebView), and list any unchecked
item with a screenshot.
