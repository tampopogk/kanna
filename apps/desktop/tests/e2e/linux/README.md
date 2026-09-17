# The Linux real-key lane

This lane presses keys. Not `new KeyboardEvent(...)` — keys.

Every other keyboard suite in `tests/e2e` builds a `KeyboardEvent` in the page
and dispatches it. That proves the app's handler wiring and nothing above it,
and it was green while three Linux chords were dead on the owner's desktop,
because those keystrokes never reached the webview at all: IBus takes
`Ctrl+Shift+U` for Unicode entry and GNOME takes `Ctrl+Alt+Arrow` for
workspaces, both before any application is asked. A mapping table cannot know
that. The only way to find out is to put a keystroke into the kernel and see
what comes out the other end.

So the lane injects with `ydotool`, which writes to `/dev/uinput`. What it
sends enters the same input pipeline a real keyboard does and travels the whole
way up: kernel → compositor → IBus → GTK → WebKit → the page. A chord something
up that chain has claimed simply never arrives.

Two signals are recorded per chord, because "nothing happened" has two causes
with two different fixes:

| signal | source | what "no" means |
|---|---|---|
| **arrived** | a capture-phase probe on `window` (`helpers/realKeyProbe.ts`) | the compositor or the input method took it; no handler change can help, the binding must move |
| **claimed** | `event.defaultPrevented` as that probe sees it | the app's own handler did nothing with it; the binding is wrong |

## What it covers

- tab cycling on `Ctrl+Page Up` / `Ctrl+Page Down`, and that the modal lists
  that chord rather than the unguessable one that used to work;
- the vertical navigation arrows: `Ctrl+↑` / `Ctrl+↓` (tasks) and
  `Alt+Shift+↑` / `Alt+Shift+↓` (repos, pressed **with the agent composer
  focused**, because that is where the caret is in normal use and the binding
  was dead exactly there) arriving *and* being claimed, while the
  four chords they replaced — `Ctrl+Shift+↑/↓` and `Alt+↑/↓` — are asserted
  **not** to arrive and not to be listed. That measurement is why they moved;
  if this desktop starts delivering them, this lane says so before a reader
  trusts the comment in `shortcutPlatform.ts`;
- `Ctrl+Shift+←` / `Ctrl+Shift+→` arriving, and pane focus being labelled as the
  split-view binding it is;
- nothing being listed on `Ctrl+Shift+U`, and `Ctrl+Alt+U` arriving and being
  claimed;
- history on `Alt+←` / `Alt+→`, with nothing left on the zoom chords;
- `Ctrl+Shift+C` staying out of the PTY while plain `Ctrl+C` reaches it;
- `Ctrl+Shift+Left` extending the selection inside a focused text field, and the
  same chord still being claimed outside one.

## Running it

The lane runs through the canonical E2E runner, which starts the app itself:

```sh
pnpm -C apps/desktop test:e2e:linux        # == tsx tests/e2e/run.ts linux/
```

It is not in the default suite (`vitest.config.ts` excludes `tests/e2e/linux/**`
and `run.ts` only collects `mock/` and `real/` when given no target), because it
needs things no other lane does:

**1. A Linux desktop session, logged in, with the display to itself.**
`ydotool` injects into whatever the *seat* is focused on, not into this process.
The lane checks `document.hasFocus()` before every chord, and when the window
does not have it, presses Escape (to end a desktop grab, which no window can see
past) and then walks the desktop's own switcher with Alt+Tab until the window
answers that it holds the keyboard. Neither is available as an API call: a
Wayland client may not raise itself, `set_focus` answers `ok` and changes
nothing, and a click cannot be aimed because the client is never told where it
is on screen. If focus still cannot be taken, the lane fails with that reason
rather than reporting a keymap verdict about keystrokes that went somewhere
else. Do not run it over a plain `ssh` shell with no session, and do not use the
machine while it runs.

**2. `ydotoold` running, with access to `/dev/uinput`.**

```sh
sudo apt install ydotool
sudo modprobe uinput
sudo ydotoold --socket-path=/run/user/$(id -u)/.ydotool_socket --socket-own="$(id -u):$(id -g)" &
export YDOTOOL_SOCKET=/run/user/$(id -u)/.ydotool_socket
```

`helpers/realKeys.ts` probes this at startup (a bare Shift press, which cannot
disturb anything) and reports what is missing in a sentence.

**3. A debug build.** The lane drives the app through `tauri-plugin-webdriver`
and reads Vue state through `window.__KANNA_E2E__`; both are compiled out of
release builds (`#[cfg(debug_assertions)]` and `import.meta.env.DEV`). An
installed `.deb` therefore exposes no observation channel at all and cannot host
this lane — what an installed build *can* be checked for lives in
`tests/linux-installed/`. `run.ts` starts a `./kd dev up` instance, which is a
debug build, and hands the suite its `KANNA_WEBDRIVER_PORT`.

## Running it on the VM

The keymap findings this lane was written for came from `kanna-linux-vm`
(Ubuntu 26.04.1 aarch64, GNOME/Wayland). Drive it over SSH — never through the
UTM console, which is a second seat and steals the focus the injected keys need:

```sh
ssh kanna-linux-vm
# in the checkout, with the toolchain on PATH:
export YDOTOOL_SOCKET=/run/user/$(id -u)/.ydotool_socket
pnpm -C apps/desktop test:e2e:linux
```

The VM must be showing its desktop (its display window open, a user logged in
graphically) for the run to mean anything. A passing run prints the desktop,
session type and input method it measured, because a keymap verdict is a verdict
about one particular set of compositor and input-method grabs.

## Adding a chord

Put its Linux keycode in `KEY_CODES` in `helpers/realKeys.ts` (from
`linux/input-event-codes.h`) and add a case to `helpers/realKeys.test.ts`, which
runs in the ordinary unit lane. A chord translated to the wrong keycode presses
the wrong key, and the lane would then report a confident verdict about a
keystroke nobody made.

Press only the chords a test needs. A sweep over every listed binding looks
tempting and is not safe: `Ctrl+Shift+W` is `closeTabOrWindow`, and a probe that
included it closed the app out from under the run, which then failed as
`ECONNREFUSED` on the WebDriver port and looked like an app crash.
