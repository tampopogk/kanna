#!/usr/bin/env bash
# End-to-end check: starts target/kmux.app on a private socket, drives it with
# target/release/kmux (the CLI), and checks the panes are really drawn (window pixels).
# Build first: scripts/build-kmux.sh && cargo build --release
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$repo_root/target/e2e"
mkdir -p "$out"
export KMUX_SOCKET="/tmp/kmux-e2e-$$.sock"
export KMUX_IGNORE_OCCLUSION=1 # render even if the window is covered
unset GHOSTTY_RESOURCES_DIR # the app's own resources, not those of a Ghostty running this script
cli="$repo_root/target/release/kmux"

"$repo_root/target/kmux.app/Contents/MacOS/kmux" --bg >"$out/kmux.log" 2>&1 &
kmux_pid=$!
state_file="${KMUX_SOCKET%.sock}.state.json" # its saved layout, private like the socket
trap 'kill $kmux_pid 2>/dev/null || true; rm -f "$KMUX_SOCKET" "$state_file"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
for _ in $(seq 50); do [[ -S "$KMUX_SOCKET" ]] && break; sleep 0.1; done
[[ -S "$KMUX_SOCKET" ]] || fail "kmux did not open $KMUX_SOCKET"

# Every pane's marker text must be on screen with glyph pixels in each cell.
drawn() { # drawn NAME MARKER [COUNT]
  local snapshot
  snapshot="$(raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$out/$1.png\",\"marker\":\"$2\"}}")"
  python3 - "$snapshot" "${3:-0}" <<'PY' || fail "panes not drawn ($1): $snapshot"
import json, sys
panes = json.loads(sys.argv[1])["panes"]
assert int(sys.argv[2]) in (0, len(panes)), f"expected {sys.argv[2]} panes"
marked = [p for p in panes if p.get("marker", {}).get("found")]
assert marked and all(p["marker"]["inked"] == p["marker"]["glyphs"] > 0 for p in marked), panes
if sys.argv[2] != "0": assert len(marked) == len(panes), "every pane should show text"
PY
}
raw() { # one control-protocol request, printed as JSON
  python3 -c 'import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.settimeout(30); s.connect(sys.argv[1])
s.sendall(sys.argv[2].encode() + b"\n"); print(s.makefile().readline().strip())' "$KMUX_SOCKET" "$1"
}
field() { python3 -c 'import json, sys; v = json.load(sys.stdin)
for k in sys.argv[1].split("."): v = v[int(k)] if k.isdigit() else v[k]
print(json.dumps(v) if isinstance(v, (dict, list)) else v)' "$1"; }

# 1. A command in a new pane, a third of the window wide.
[[ "$("$cli" open --name e2e --split right --size 1/3 --cmd "printf 'KMUX E2E OK\n'; exec sleep 600")" == *"(e2e)"* ]] || fail "open did not print the pane name"
list="$("$cli" workspace list --all --json)"
[[ "$(field windows.0.tabs.0.layout <<<"$list")" == '{"children": [{"pane": "term-1", "size": "2/3"}, {"pane": "e2e", "size": "1/3"}], "split": "row"}' ]] \
  || fail "layout: $(field windows.0.tabs.0.layout <<<"$list")"
sleep 1.5
[[ "$(field panes.1.state <<<"$("$cli" workspace list --all --json)")" == running ]] || fail "e2e is not running"

# 2. Both panes are drawn: the command's output, and the new shell's first line.
drawn two-panes "KMUX E2E OK"
drawn two-panes-shell "*" 2

# 3. A command that ends leaves its pane on screen, marked exited.
"$cli" open --name done --split down --cmd "echo bye" >/dev/null
sleep 1.5
[[ "$(field panes.2.state <<<"$("$cli" workspace list --all --json)")" == exited ]] || fail "done did not exit"

# 4. Errors map to exit codes.
set +e
"$cli" close nope 2>/dev/null; [[ $? == 4 ]] || fail "unknown pane should exit 4"
"$cli" open --name e2e 2>/dev/null; [[ $? == 1 ]] || fail "a taken name should exit 1"
"$cli" zoom 2>/dev/null; [[ $? == 2 ]] || fail "missing arguments should exit 2"
set -e

# 5. Closing panes redistributes space; closing the last closes the window.
"$cli" close done >/dev/null
"$cli" close e2e >/dev/null
[[ "$(field windows.0.tabs.0.layout <<<"$("$cli" workspace list --all --json)")" == '{"pane": "term-1"}' ]] || fail "term-1 should fill the window"
"$cli" close term-1 >/dev/null
[[ "$(field windows <<<"$("$cli" workspace list --all --json)")" == '[]' ]] || fail "the window should close with its last pane"

# 5b. send types a line into a terminal.
"$cli" open --name sh >/dev/null
sleep 1
"$cli" send sh "echo SENT-\$((40+2))"
sleep 1
drawn sent "SENT-42"

# 5c. A web pane opened before its server is up waits, then loads.
port=$((20000 + $$ % 20000))
"$cli" open web "localhost:$port" --name site --split down >/dev/null
sleep 1.5
web() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"site\"}}"; }
mkdir -p "$out/site" && echo '<h1>KMUX WEB OK</h1>' > "$out/site/index.html"
(cd "$out/site" && exec python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) &
server_pid=$!
trap 'kill $kmux_pid $server_pid 2>/dev/null || true; rm -f "$KMUX_SOCKET" "$state_file"' EXIT
for _ in $(seq 30); do [[ "$(field text <<<"$(web)")" == *"KMUX WEB OK"* ]] && break; sleep 0.5; done
[[ "$(field text <<<"$(web)")" == *"KMUX WEB OK"* ]] || fail "web pane never loaded: $(web)"
"$cli" navigate site "localhost:$port/missing" >/dev/null
[[ "$(field panes.1.url <<<"$("$cli" workspace list --all --json)")" == "http://localhost:$port/missing" ]] || fail "navigate should update the url"

# Web history, only when asked for: back loads the previous page again.
echo '<h1>PAGE TWO</h1>' > "$out/site/two.html"
hw="$(field window <<<"$("$cli" open web "localhost:$port" --history --name hist --window new --json)")"
"$cli" navigate hist "localhost:$port/two.html" >/dev/null
"$cli" navigate hist --back >/dev/null
hist() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"hist\"}}"; }
for _ in $(seq 20); do [[ "$(field text <<<"$(hist)")" == *"KMUX WEB OK"* ]] && break; sleep 0.25; done
[[ "$(field text <<<"$(hist)")" == *"KMUX WEB OK"* ]] || fail "back should show the first page again: $(hist)"
"$cli" navigate hist --forward >/dev/null
for _ in $(seq 20); do [[ "$(field text <<<"$(hist)")" == *"PAGE TWO"* ]] && break; sleep 0.25; done
[[ "$(field text <<<"$(hist)")" == *"PAGE TWO"* ]] || fail "forward should show page two: $(hist)"
"$cli" navigate site --back >/dev/null 2>&1 && fail "a pane without --history should refuse --back"
"$cli" close --window "$hw" >/dev/null

# 5d. Layout commands move real views: swap, resize, arrange.
"$cli" move sh --to site >/dev/null
"$cli" arrange '{"split":"row","children":[{"pane":"site","size":"1/4"},{"pane":"sh"}]}' >/dev/null
[[ "$(field windows.0.tabs.0.layout <<<"$("$cli" workspace list --all --json)")" == '{"children": [{"pane": "site", "size": "1/4"}, {"pane": "sh", "size": "3/4"}], "split": "row"}' ]] \
  || fail "arrange: $(field windows.0.tabs.0.layout <<<"$("$cli" workspace list --all --json)")"
"$cli" resize site 1/2 >/dev/null
drawn arranged "*"
"$cli" close --window win-2 >/dev/null

# 6. Keyboard shortcuts, pressed through AppKit's key-event path.
key() { raw "{\"id\":1,\"cmd\":\"debug.key\",\"args\":{\"key\":\"$1\"}}" >/dev/null; sleep 0.4; }
state() { "$cli" workspace list --all --json; }
"$cli" open --name k1 >/dev/null
key "cmd+d"
key "cmd+shift+d"
layout="$(field windows.0.tabs.0.layout <<<"$(state)")"
[[ "$layout" == '{"children": [{"pane": "k1", "size": "1/2"}, {"children": [{"pane": "term-6", "size": "1/2"}, {"pane": "term-7", "size": "1/2"}], "size": "1/2", "split": "column"}], "split": "row"}' ]] \
  || fail "cmd+d / cmd+shift+d layout: $layout"
[[ "$(field windows.0.focused <<<"$(state)")" == term-7 ]] || fail "the new pane should take focus"
key "cmd+]"; [[ "$(field windows.0.focused <<<"$(state)")" == k1 ]] || fail "cmd+] should wrap to k1"
key "cmd+["; [[ "$(field windows.0.focused <<<"$(state)")" == term-7 ]] || fail "cmd+[ should wrap back to term-7"
key "cmd+shift+return"; [[ "$(field windows.0.zoomed <<<"$(state)")" == term-7 ]] || fail "cmd+shift+return should zoom"
key "cmd+["; [[ "$(field windows.0.zoomed <<<"$(state)")" == term-6 ]] || fail "cmd+[ should keep zoom on, on term-6"
key "cmd+]"; [[ "$(field windows.0.zoomed <<<"$(state)")" == term-7 ]] || fail "cmd+] should keep zoom on, on term-7"
key "cmd+shift+return"; [[ "$(field windows.0.zoomed <<<"$(state)")" == None ]] || fail "cmd+shift+return should turn zoom off"
sleep 1
drawn three-panes "*" 3
# ⌘R reloads a web pane in place; on a terminal Reload is greyed out and ⌘R goes to the program.
reload_enabled() { raw '{"id":1,"cmd":"debug.menu"}' | python3 -c 'import json, sys
print(next(i["enabled"] for i in json.load(sys.stdin)["items"] if i["item"] == "Reload"))'; }
[[ "$(reload_enabled)" == False ]] || fail "Reload should be greyed out on a terminal"
echo '<h1>BEFORE RELOAD</h1>' > "$out/site/reload.html"
"$cli" open web "localhost:$port/reload.html" --name rl --tab >/dev/null
rl() { raw '{"id":1,"cmd":"debug.web","args":{"pane":"rl"}}'; }
for _ in $(seq 20); do [[ "$(field text <<<"$(rl)")" == *"BEFORE RELOAD"* ]] && break; sleep 0.25; done
[[ "$(field text <<<"$(rl)")" == *"BEFORE RELOAD"* ]] || fail "the page to reload never loaded: $(rl)"
[[ "$(reload_enabled)" == True ]] || fail "Reload should be enabled on a web pane"
echo '<h1>AFTER RELOAD</h1>' > "$out/site/reload.html"
key "cmd+r"
for _ in $(seq 20); do [[ "$(field text <<<"$(rl)")" == *"AFTER RELOAD"* ]] && break; sleep 0.25; done
[[ "$(field text <<<"$(rl)")" == *"AFTER RELOAD"* ]] || fail "cmd+r should reload the web pane: $(rl)"
"$cli" close rl >/dev/null
key "cmd+t"; [[ "$(field windows.0.tabs <<<"$(state)" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')" == 2 ]] || fail "cmd+t should add a tab"
key "cmd+shift+["; [[ "$(field windows.0.tabs.0.active <<<"$(state)")" == True ]] || fail "cmd+shift+[ should go to the first tab"
key "cmd+shift+]"; [[ "$(field windows.0.tabs.1.active <<<"$(state)")" == True ]] || fail "cmd+shift+] should go to the second tab"
# Double-click a tab to rename it: Enter saves, Escape cancels.
tab2="$(field windows.0.tabs.1.id <<<"$(state)")"
raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"tab\":\"$tab2\",\"clicks\":2}}" >/dev/null; sleep 0.3
for k in w e b return; do key "$k"; done
[[ "$(field windows.0.tabs.1.title <<<"$(state)")" == web ]] || fail "double-click rename should name the tab web, got $(field windows.0.tabs.1.title <<<"$(state)")"
raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"tab\":\"$tab2\",\"clicks\":2}}" >/dev/null; sleep 0.3
for k in x escape; do key "$k"; done
[[ "$(field windows.0.tabs.1.title <<<"$(state)")" == web ]] || fail "escape should keep the name"
key "cmd+n"; [[ "$(field windows.1.key <<<"$(state)")" == True ]] || fail "cmd+n should open a key window"
key "cmd+\`"; [[ "$(field windows.0.key <<<"$(state)")" == True ]] || fail "cmd+\` should cycle to the first window"
key "cmd+w"; [[ "$(field windows.0.tabs <<<"$(state)" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')" == 1 ]] || fail "cmd+w should close the only pane in tab 2"
key "cmd+shift+w"
[[ "$(field windows <<<"$(state)" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')" == 1 ]] || fail "cmd+shift+w should close the window"

# Menus: every action in the spec's table (§5) has a menu item.
menus="$(raw '{"id":1,"cmd":"debug.menu"}')"
for item in "Split Right" "Split Down" "Open URL…" "Zoom" "Move Pane to New Window" "Reload" "Close Pane" "New Tab" \
            "New Window" "Close Window" "Move Tab to New Window" "Next Window" "New Instance" "Copy" "Paste" "Select All"; do
  [[ "$menus" == *"\"item\": \"$item\""* || "$menus" == *"\"item\":\"$item\""* ]] || fail "no menu item \"$item\""
done

# 7. Dragging: dividers snap, the ⋯ grip moves panes, tabs reorder and tear off.
drag() { last_drag="$1 → $(raw "{\"id\":1,\"cmd\":\"debug.drag\",\"args\":$1}")"; sleep 0.3; }
win() { # win ID FIELD: a field of one window in `kmux workspace list --all --json`
  "$cli" workspace list --all --json | python3 -c 'import json, sys
w = next(w for w in json.load(sys.stdin)["windows"] if w["id"] == sys.argv[1])
v = w
for k in sys.argv[2].split("."): v = v[int(k)] if k.isdigit() else v.get(k)
print(json.dumps(v, sort_keys=True))' "$1" "$2"
}
count_windows() { "$cli" workspace list --all --json | python3 -c 'import json, sys; print(len(json.load(sys.stdin)["windows"]))'; }
dw="$(field window <<<"$("$cli" open --window new --name da --json)")"
"$cli" open --window "$dw" --name db --split right >/dev/null
drag '{"divider":"da","at":0.26}'
[[ "$(win "$dw" tabs.0.layout)" == '{"children": [{"pane": "da", "size": "1/4"}, {"pane": "db", "size": "3/4"}], "split": "row"}' ]] \
  || fail "divider drag should snap to 1/4: $(win "$dw" tabs.0.layout)"
drag '{"pane":"da","to":{"pane":"db","x":0.5,"y":0.95}}'
[[ "$(win "$dw" tabs.0.layout)" == '{"children": [{"pane": "db", "size": "1/2"}, {"pane": "da", "size": "1/2"}], "split": "column"}' ]] \
  || fail "dropping on the bottom edge should dock below: $(win "$dw" tabs.0.layout) ($last_drag)"
drag '{"pane":"da","to":{"pane":"db","x":0.5,"y":0.5}}'
[[ "$(win "$dw" tabs.0.layout)" == '{"children": [{"pane": "da", "size": "1/2"}, {"pane": "db", "size": "1/2"}], "split": "column"}' ]] \
  || fail "dropping in the middle should swap: $(win "$dw" tabs.0.layout)"
drag "{\"pane\":\"da\",\"to\":{\"plus\":\"$dw\"}}"
[[ "$(win "$dw" tabs.1.layout)" == '{"pane": "da"}' ]] || fail "dropping on + should open a new tab: $(win "$dw" tabs)"
first="$(win "$dw" tabs.0.id | tr -d '"')"; second="$(win "$dw" tabs.1.id | tr -d '"')"
drag "{\"tab\":\"$second\",\"to\":{\"tab\":\"$first\"}}"
[[ "$(win "$dw" tabs.0.id | tr -d '"')" == "$second" ]] || fail "dragging a tab before another should reorder: $(win "$dw" tabs)"
before="$(count_windows)"
drag "{\"tab\":\"$second\",\"to\":{\"outside\":true}}"
[[ "$(count_windows)" == $((before + 1)) ]] || fail "dragging a tab outside kmux should open a window"
drag '{"pane":"db","to":{"outside":true}}'
[[ "$(count_windows)" == $((before + 1)) ]] || fail "the last pane leaving its window should move the window, not add one"
sleep 0.5
drawn dragged "*"

# 7b. A docked pane: outside the window's tabs, on its left or right edge,
# shown whichever tab is; it closes only with its window.
dkw="$(field window <<<"$("$cli" open --window new --name dt1 --cmd "printf 'DOCK TAB ONE\n'; exec sleep 600" --json)")"
"$cli" open --window "$dkw" --name dt2 --tab --cmd "printf 'DOCK TAB TWO\n'; exec sleep 600" >/dev/null
opened="$("$cli" open --window "$dkw" --name agent --dock --cmd "printf 'DOCK AGENT\n'; exec sleep 600")"
[[ "$opened" == *"(agent) docked left in $dkw: running" ]] || fail "open --dock should say where it docked: $opened"
[[ "$(win "$dkw" dock)" == '{"edge": "left", "pane": "agent", "width": "2/5"}' ]] || fail "open --dock: $(win "$dkw" dock)"
[[ "$("$cli" list)" == *"dock: term-"*" (agent)  left 2/5"* ]] || fail "list should show the dock on its own line: $("$cli" list)"
sleep 1
drawn docked "DOCK AGENT"
drawn docked-tab "DOCK TAB TWO"
key "cmd+1"
[[ "$(win "$dkw" tabs.0.active)" == true && "$(win "$dkw" dock.pane)" == '"agent"' ]] || fail "cmd+1 should show the first tab and leave the dock: $(win "$dkw" tabs)"
sleep 0.5
drawn docked-tab1 "DOCK AGENT"
drawn docked-tab1-tab "DOCK TAB ONE"
key "cmd+2"
[[ "$(win "$dkw" tabs.1.active)" == true ]] || fail "cmd+2 should show the second tab"
refused="$("$cli" close agent 2>&1)" && fail "close on the docked pane should fail"
[[ "$refused" == *"docked: it closes with its window"* ]] || fail "close on the docked pane should say why: $refused"
"$cli" focus agent >/dev/null
key "cmd+w"
[[ "$(win "$dkw" dock.pane)" == '"agent"' ]] || fail "cmd+w should do nothing in the docked pane"
key "cmd+]"; [[ "$(win "$dkw" focused)" == '"dt2"' ]] || fail "cmd+] should go from the dock into the shown tab: $(win "$dkw" focused)"
key "cmd+]"; [[ "$(win "$dkw" focused)" == '"agent"' ]] || fail "cmd+] should come back to the dock: $(win "$dkw" focused)"
key "cmd+shift+return"
[[ "$(win "$dkw" zoomed)" == '"agent"' ]] || fail "zoom on the docked pane"
[[ "$(field panes <<<"$(raw '{"id":1,"cmd":"debug.snapshot"}')" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')" == 1 ]] || fail "a zoomed docked pane should fill the window"
key "cmd+]"; [[ "$(win "$dkw" zoomed)" == '"dt2"' ]] || fail "cmd+] should keep zoom on, on dt2: $(win "$dkw" zoomed)"
[[ "$(field panes <<<"$(raw '{"id":1,"cmd":"debug.snapshot"}')" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')" == 1 ]] || fail "a zoomed tab pane should cover the docked pane"
key "cmd+shift+return"
"$cli" move agent --dock right >/dev/null
[[ "$(win "$dkw" dock)" == '{"edge": "right", "pane": "agent", "width": "2/5"}' ]] || fail "move --dock right: $(win "$dkw" dock)"
drag '{"divider":"agent","at":0.34}'
[[ "$(win "$dkw" dock.width)" == '"1/3"' ]] || fail "dragging the dock's divider should snap to 1/3: $(win "$dkw" dock) ($last_drag)"
sleep 0.5
drawn docked-right "DOCK AGENT"
dt1tab="$(win "$dkw" tabs.0.id | tr -d '"')"
"$cli" move-tab "$dt1tab" --window new >/dev/null
[[ "$(win "$dkw" dock.pane)" == '"agent"' && "$(win "$dkw" tabs | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')" == 1 ]] \
  || fail "moving a tab to another window should leave the dock: $(win "$dkw" tabs)"
"$cli" close dt1 >/dev/null
[[ "$(field pane.state <<<"$("$cli" restart agent --json)")" == running ]] || fail "restart should work on the docked pane"
"$cli" move agent --tab >/dev/null
[[ "$(win "$dkw" dock)" == null && "$(win "$dkw" tabs | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')" == 2 ]] || fail "move --tab should undock into a new tab: $(win "$dkw" tabs)"
"$cli" move agent --dock >/dev/null
"$cli" close dt2 >/dev/null
[[ "$(win "$dkw" tabs)" == '[]' && "$(win "$dkw" dock.pane)" == '"agent"' ]] || fail "closing the last tab should leave the window with its docked pane"
"$cli" close --window "$dkw" >/dev/null
[[ "$("$cli" list)" != *agent* ]] || fail "closing the window should close its docked pane"

# 7c. A frozen docked pane doesn't move, and nothing drops on it.
fzw="$(field window <<<"$("$cli" open --window new --name ft --cmd "printf 'FROZEN TAB\n'; exec sleep 600" --json)")"
opened="$("$cli" open --window "$fzw" --name fagent --dock --frozen --cmd "printf 'FROZEN AGENT\n'; exec sleep 600")"
[[ "$opened" == *"(fagent) docked left, frozen, in $fzw: running" ]] || fail "open --dock --frozen should say it's frozen: $opened"
[[ "$("$cli" list)" == *"(fagent)  left 2/5  frozen"* ]] || fail "list should show the dock frozen: $("$cli" list)"
refused="$("$cli" move fagent --dock right 2>&1)" && fail "moving a frozen docked pane should fail"
[[ "$refused" == *"frozen: fagent ("*") is frozen in its dock and doesn't move"* ]] || fail "moving a frozen docked pane should say why: $refused"
refused="$("$cli" move ft --to fagent 2>&1)" && fail "swapping onto a frozen docked pane should fail"
[[ "$refused" == *"is frozen in $fzw's dock; nothing drops on it"* ]] || fail "swapping onto a frozen docked pane should say why: $refused"
[[ "$(field dropped <<<"$(raw '{"id":1,"cmd":"debug.drag","args":{"pane":"ft","to":{"pane":"fagent","x":0.5,"y":0.5}}}')")" == False ]] || fail "a drop on a frozen docked pane should do nothing"
[[ "$(field dropped <<<"$(raw '{"id":1,"cmd":"debug.drag","args":{"pane":"fagent","to":{"pane":"ft","x":0.5,"y":0.5}}}')")" == False ]] || fail "a frozen docked pane shouldn't drag"
[[ "$(win "$fzw" dock.pane)" == '"fagent"' && "$(win "$fzw" tabs.0.layout)" == '{"pane": "ft"}' ]] || fail "nothing should have moved: $(win "$fzw" dock) $(win "$fzw" tabs)"
"$cli" resize fagent 1/3 >/dev/null || fail "a frozen docked pane should still resize"
"$cli" close --window "$fzw" >/dev/null
[[ "$("$cli" list)" != *fagent* ]] || fail "closing the window should close its frozen docked pane"

# 8. Instances: a second kmux beside this one, with its own socket and
# windows. kmux run inside one of its panes controls that instance.
other="e2e$$"
other_socket="$HOME/Library/Application Support/kmux/kmux-$other.sock"
# (KMUX_NO_STATE: its socket is in the real kmux folder, so it keeps no saved layout there.)
env -u KMUX_SOCKET KMUX_NO_STATE=1 "$repo_root/target/kmux.app/Contents/MacOS/kmux" --bg --instance "$other" >"$out/kmux-$other.log" 2>&1 &
other_pid=$!
trap 'kill $kmux_pid $server_pid $other_pid 2>/dev/null || true; rm -f "$KMUX_SOCKET" "$state_file"' EXIT
for _ in $(seq 50); do [[ -S "$other_socket" ]] && break; sleep 0.1; done
[[ -S "$other_socket" ]] || fail "instance $other did not open $other_socket"
[[ "$(field instance <<<"$("$cli" --instance "$other" capabilities --json)")" == "$other" ]] || fail "capabilities should name the instance"
[[ "$(env -u KMUX_SOCKET "$cli" instances)" == *" $other "* ]] || fail "kmux instances should list $other: $(env -u KMUX_SOCKET "$cli" instances)"
"$cli" --instance "$other" open --name parent --cmd "$cli open --name child --split down --no-wait; exec sleep 600" >/dev/null
for _ in $(seq 50); do [[ "$("$cli" --instance "$other" list)" == *child* ]] && break; sleep 0.1; done
[[ "$("$cli" --instance "$other" list)" == *child* ]] || fail "kmux inside a pane should reach its own instance"
[[ "$("$cli" list)" != *child* ]] || fail "the child pane went to the wrong instance"
kill "$other_pid"; wait "$other_pid" 2>/dev/null || true
[[ ! -e "$other_socket" ]] || fail "instance $other should remove its socket when it quits"

# 8b. Mermaid diagrams, laid out by merman and drawn natively (no web view).
dg="$(raw '{"id":1,"cmd":"debug.diagram","args":{"source":"flowchart LR\n  A[KMUX DIAGRAM] -->|yes| B{Ok?}","png":"'"$out"'/diagram.png"}}')"
[[ "$(field type <<<"$dg")" == flowchart* && "$dg" == *'"KMUX DIAGRAM"'* && "$dg" == *'"yes"'* ]] || fail "flowchart should lay out with its labels: $dg"
[[ -s "$out/diagram.png" ]] || fail "debug.diagram should write a PNG"
[[ "$(field supported <<<"$(raw '{"id":1,"cmd":"debug.diagram","args":{"source":"pie\n \"a\": 1"}}')")" == False ]] || fail "pie isn't drawn yet"
[[ "$(raw '{"id":1,"cmd":"debug.diagram","args":{"source":"flowchart LR\n  A -->"}}')" == *"Diagram error"* ]] || fail "invalid diagrams should say so"

# 8c. Markdown panes: rendered natively (no web view), diagrams drawn,
# live reload, navigate, back/forward, magnification (keys and a smooth pinch, never
# re-wrapping the text, focused or not), clicks beside the page, a missing file.
mkdir -p "$out/docs"
cat > "$out/docs/spec.md" <<'MD'
# KMUX MD OK

Some **bold** text and a table:

| a | b |
|---|---|
| 1 | 2 |

```mermaid
flowchart LR
    A[cart] --> B[paid]
```

See [the other page](other.md).
MD
echo '# OTHER PAGE' > "$out/docs/other.md"
mdw="$(field window <<<"$("$cli" open md "$out/docs/spec.md" --name doc --window new --json)")"
md() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"doc\"}}"; }
mdtext() { field text <<<"$(md)"; }
mdzoom() { field zoom <<<"$(md)"; }
[[ "$(mdtext)" == *"KMUX MD OK"*"Some bold text"* ]] || fail "markdown pane did not render: $(mdtext)"
[[ "$(mdtext)" != *"flowchart LR"* && "$(mdtext)" != *"**"* ]] || fail "markdown syntax and diagram source should not show: $(mdtext)"
[[ "$(field diagrams.0.labels <<<"$(md)")" == *cart*paid* ]] || fail "the mermaid diagram should be drawn: $(md)"
echo "Edited on disk" >> "$out/docs/spec.md"
for _ in $(seq 20); do [[ "$(mdtext)" == *"Edited on disk"* ]] && break; sleep 0.25; done
[[ "$(mdtext)" == *"Edited on disk"* ]] || fail "markdown pane should reload when the file changes"
width="$(field layout_width <<<"$(md)")"
raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+="}}' >/dev/null
raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+="}}' >/dev/null
[[ "$(mdzoom)" == 1.25 ]] || fail "⌘= twice should magnify to 125%: $(mdzoom)"
[[ "$(field layout_width <<<"$(md)")" == "$width" ]] || fail "magnifying should not re-wrap the text: $width → $(field layout_width <<<"$(md)")"
"$cli" navigate doc "$out/docs/other.md" >/dev/null
[[ "$(mdtext)" == *"OTHER PAGE"* ]] || fail "navigate should show the other file: $(mdtext)"
[[ "$(mdzoom)" == 1.25 ]] || fail "zoom should stay when the pane moves to another file: $(mdzoom)"
raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+0"}}' >/dev/null
[[ "$(mdzoom)" == 1 ]] || fail "⌘0 should go back to actual size: $(mdzoom)"
pinch="$(raw '{"id":1,"cmd":"debug.pinch","args":{"pane":"doc","steps":[0.05,0.05,0.05,0.05]}}')"
python3 - "$pinch" <<'PY' || fail "a pinch should zoom smoothly: $pinch"
import json, sys
zooms = json.loads(sys.argv[1])["zooms"]
assert len(zooms) == 4 and all(b > a for a, b in zip([1] + zooms, zooms)), zooms
assert abs(zooms[-1] - 1.05 ** 4) < 0.01, zooms  # follows the fingers, not the ⌘= steps
PY
[[ "$(field layout_width <<<"$(md)")" == "$width" ]] || fail "a pinch should not re-wrap the text"
sleep 1 # the scroll view finishes the gesture before keys act again
raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+0"}}' >/dev/null
[[ "$(mdzoom)" == 1 ]] || fail "⌘0 after a pinch should go back to actual size: $(mdzoom)"
# Pinch and keys reach the pane with its focus outline showing, focused or not.
"$cli" open --window "$mdw" --name mdterm --split right >/dev/null
mdfocused() { win "$mdw" focused | tr -d '"'; }
mdpinch() { # mdpinch OUTLINE: pinches the doc open, printing the zoom before and after
  local before; before="$(mdzoom)"
  echo "$before → $(field zooms.3 <<<"$(raw "{\"id\":1,\"cmd\":\"debug.pinch\",\"args\":{\"pane\":\"doc\",\"outline\":$1,\"steps\":[0.05,0.05,0.05,0.05]}}")")"
}
zoomed() { python3 -c 'import sys; a, b = map(float, sys.argv[1].split(" → ")); sys.exit(not b > a * 1.2)' "$1"; }
[[ "$(mdfocused)" == mdterm ]] || fail "the new terminal should take focus"
pinched="$(mdpinch false)"; zoomed "$pinched" || fail "a pinch should zoom an unfocused markdown pane: $pinched"
raw '{"id":1,"cmd":"focus","args":{"pane":"doc"}}' >/dev/null; sleep 1
pinched="$(mdpinch true)"; zoomed "$pinched" || fail "a pinch should zoom the focused markdown pane: $pinched"
sleep 1
key "cmd+0"; key "cmd+="
[[ "$(mdzoom)" == 1.1 ]] || fail "⌘= should magnify the focused markdown pane: $(mdzoom)"
for _ in 1 2 3 4 5 6; do key "cmd+-"; done
[[ "$(mdzoom)" == 0.5 ]] || fail "⌘− should step down to 50%: $(mdzoom)"
raw '{"id":1,"cmd":"focus","args":{"pane":"mdterm"}}' >/dev/null; sleep 0.3
raw '{"id":1,"cmd":"debug.click","args":{"pane":"doc","at":[4,200]}}' >/dev/null; sleep 0.3
[[ "$(mdfocused)" == doc ]] || fail "clicking beside a magnified-out page should focus its pane: $(mdfocused)"
key "cmd+0"
# Back and forward (the CLI, and the mouse's buttons 4 and 5) return to where
# each file was being read: its scroll and magnification.
{ echo "# LONG PAGE"; for i in $(seq 300); do echo; echo "Line $i of a long page."; done; } > "$out/docs/long.md"
"$cli" navigate doc "$out/docs/long.md" >/dev/null
raw '{"id":1,"cmd":"focus","args":{"pane":"doc"}}' >/dev/null; sleep 0.3
key "cmd+="
raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc","scroll":900}}' >/dev/null
read_at="$(field scroll <<<"$(md)")"
"$cli" navigate doc --back >/dev/null
[[ "$(mdtext)" == *"OTHER PAGE"* && "$(mdzoom)" == 1 ]] || fail "--back should return to the other page at its own size: $(mdzoom) $(mdtext)"
mouse() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"doc\",\"button\":$1}}" >/dev/null; sleep 0.3; }
mouse 4
[[ "$(mdtext)" == *"LONG PAGE"* ]] || fail "mouse button 5 should go forward: $(mdtext)"
back_at="$(field scroll <<<"$(md)")"
[[ "$(mdzoom)" == 1.1 ]] && python3 -c 'import sys; sys.exit(abs(float(sys.argv[1]) - float(sys.argv[2])) > 1)' "$read_at" "$back_at" \
  || fail "forward should return to where the page was read ($read_at at 1.1): $back_at at $(mdzoom)"
mouse 3
[[ "$(mdtext)" == *"OTHER PAGE"* ]] || fail "mouse button 4 should go back: $(mdtext)"
"$cli" navigate doc --back >/dev/null
[[ "$(mdtext)" == *"KMUX MD OK"* ]] || fail "--back should reach the first file: $(mdtext)"
"$cli" navigate doc --back >/dev/null 2>&1 && fail "--back with nothing behind should fail"
mouse 3
[[ "$(mdtext)" == *"KMUX MD OK"* ]] || fail "mouse back with nothing behind should do nothing: $(mdtext)"
missing="$("$cli" open md "$out/docs/nope.md" --window "$mdw" 2>&1)" && fail "a missing file should fail"
[[ "$missing" == *"No such file"* ]] || fail "a missing file should say so: $missing"
folder="$("$cli" open md "$out/docs" --window "$mdw" 2>&1)" && fail "a folder should fail"
[[ "$folder" == *"is a folder"* ]] || fail "a folder should say so: $folder"
"$cli" close --window "$mdw" >/dev/null

# 9. iOS panes: the simulator's screen in a pane, with taps and Home.
# Boots a simulator if none is (it stays booted). KMUX_E2E_IOS=0 skips this.
if [[ "${KMUX_E2E_IOS:-1}" != 0 ]] && xcrun simctl list devices available 2>/dev/null | grep -q iPhone; then
  bad="$("$cli" open ios --app com.apple.Preferences --device "No Such Phone" 2>&1)" && fail "an unknown device should fail"
  [[ "$bad" == *'Unknown device "No Such Phone"'*Available:* ]] || fail "an unknown device should list the available ones: $bad"
  "$cli" open ios --name phone --app com.apple.Preferences --window new >/dev/null || fail "the ios pane did not start"
  phone="$(field pane.id <<<"$("$cli" restart phone --json)")"
  [[ "$(field width <<<"$(raw "{\"id\":1,\"cmd\":\"debug.ios\",\"args\":{\"pane\":\"phone\"}}")")" != 0 ]] || fail "the ios pane shows no screen"
  sleep 3
  ink() { field panes.0.ink <<<"$(raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$out/ios-$1.png\"}}")"; }
  settings="$(ink settings)"
  python3 -c "import sys; sys.exit(float(sys.argv[1]) < 0.05)" "$settings" || fail "the ios pane is blank (ink $settings)"
  raw "{\"id\":1,\"cmd\":\"debug.ios\",\"args\":{\"pane\":\"phone\",\"home\":true}}" >/dev/null
  sleep 2
  [[ "$(ink home)" != "$settings" ]] || fail "Home did not change the screen"
  "$cli" close "$phone" >/dev/null
fi

# 10. Persistence: a layout comes back after kmux restarts. A second kmux on
# its own private socket (so its state file is private too) builds a layout,
# is stopped with SIGTERM (saving on quit) and started again on the same file.
p_socket="/tmp/kmux-e2e-$$-p.sock"
p_state="/tmp/kmux-e2e-$$-p.state.json"
p_start() { KMUX_SOCKET="$p_socket" "$repo_root/target/kmux.app/Contents/MacOS/kmux" --bg "$@" >>"$out/kmux-persist.log" 2>&1 &
  p_pid=$!
  for _ in $(seq 50); do [[ -S "$p_socket" ]] && break; sleep 0.1; done
  [[ -S "$p_socket" ]] || fail "the persistence kmux did not open $p_socket"; }
p_stop() { kill -TERM "$p_pid"; wait "$p_pid" 2>/dev/null || true; [[ ! -e "$p_socket" ]] || fail "kmux should remove its socket when it quits"; }
p_cli() { KMUX_SOCKET="$p_socket" "$cli" "$@"; }
p_raw() { KMUX_SOCKET="$p_socket" raw "$1"; }
trap 'kill $kmux_pid $server_pid ${p_pid:-} 2>/dev/null || true; rm -f "$KMUX_SOCKET" "$state_file" "$p_socket" "$p_state" "$p_state.bad"' EXIT
rm -f "$p_state" "$p_state.bad"
# What a restore must bring back: everything in `list` except whether panes have started yet.
p_layout() { p_cli workspace list --all --json | python3 -c 'import json, sys
d = json.load(sys.stdin)
for p in d["panes"]:
    for k in ("state", "exitCode", "error"): p.pop(k, None)
print(json.dumps(d, sort_keys=True))'; }
p_frames() { python3 -c 'import json, sys; print(json.dumps([w.get("frame") for w in json.load(open(sys.argv[1]))["windows"]]))' "$p_state"; }
mkdir -p "$out/persist"
p_start
p_cli open --name srv --cwd "$out/persist" --cmd "printf 'PERSIST %s\n' \"\$(pwd)\"; exec sleep 600" >/dev/null
p_cli open web "localhost:$port" --name site --history --split right --size 1/3 >/dev/null
p_cli open md "$out/docs/spec.md" --name doc --split down --size 1/4 >/dev/null
p_cli open --name sh2 --tab >/dev/null
p_cli rename-tab "$(field windows.0.tabs.1.id <<<"$(p_cli workspace list --all --json)")" shells >/dev/null
sleep 1
p_cli send sh2 "cd /usr/bin" # the shell reports its new directory (OSC 7)
p_cli open --name logs --window new >/dev/null
p_cli open web "localhost:$port/two.html" --split down >/dev/null
p_cli focus doc >/dev/null
p_raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+="}}' >/dev/null
p_cli focus site >/dev/null
p_cli zoom site >/dev/null
sleep 1.5
before="$(p_layout)"
[[ "$before" == *'"cwd": "/usr/bin"'* ]] || fail "a terminal's cwd should follow the shell: $before"
[[ -f "$p_state" && "$(stat -f %Lp "$p_state")" == 600 ]] || fail "the layout should be saved (mode 600) soon after a change: $(ls -l "$p_state" 2>&1)"
python3 - "$p_state" <<'PY' || fail "the debounced save should hold the whole layout: $(cat "$p_state")"
import json, sys
s = json.load(open(sys.argv[1]))
assert s["version"] == 2 and len(s["windows"]) == 2, s
assert [t["title"] for t in s["windows"][0]["tabs"]] == ["Tab 1", "shells"], s
assert {p["name"]: p.get("cwd") for p in s["panes"]}["sh2"] == "/usr/bin", s
assert all(w.get("frame") for w in s["windows"]), s
PY
frames="$(p_frames)"
p_stop
[[ -f "$p_state" ]] || fail "the layout should still be saved after quitting"
p_start
sleep 2
after="$(p_layout)"
[[ "$after" == "$before" ]] || fail "the restored layout differs:
before: $before
after:  $after"
for _ in $(seq 20); do [[ "$(field text <<<"$(p_raw '{"id":1,"cmd":"debug.text","args":{"pane":"srv"}}')")" == *"PERSIST $out/persist"* ]] && break; sleep 0.25; done
[[ "$(field text <<<"$(p_raw '{"id":1,"cmd":"debug.text","args":{"pane":"srv"}}')")" == *"PERSIST $out/persist"* ]] || fail "srv should run its command again in its cwd"
[[ "$(field zoom <<<"$(p_raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}')")" == 1.1 ]] || fail "doc should keep its zoom"
for _ in $(seq 20); do [[ "$(field text <<<"$(p_raw '{"id":1,"cmd":"debug.web","args":{"pane":"site"}}')")" == *"KMUX WEB OK"* ]] && break; sleep 0.25; done
[[ "$(field text <<<"$(p_raw '{"id":1,"cmd":"debug.web","args":{"pane":"site"}}')")" == *"KMUX WEB OK"* ]] || fail "site should load its URL again"
[[ "$(p_frames)" == "$frames" ]] || fail "windows should come back where they were: $frames → $(p_frames)"
[[ "$(field pane.id <<<"$(p_cli open --name new --json)")" == term-4 ]] || fail "terminal IDs should carry on after the restored ones"
p_stop
# A file kmux can't read is set aside, and kmux starts fresh; so does --fresh.
echo '{ "version": 1, "windows": [' >"$p_state"
p_start
sleep 1
[[ "$(p_cli workspace list --all --json | python3 -c 'import json, sys; d = json.load(sys.stdin); print(len(d["windows"]), len(d["panes"]))')" == "1 1" ]] || fail "a bad state file should give a fresh window"
[[ -f "$p_state.bad" ]] || fail "the bad state file should be kept aside"
p_stop
p_start --fresh
sleep 1
[[ "$(p_cli workspace list --all --json | python3 -c 'import json, sys; print(len(json.load(sys.stdin)["panes"]))')" == 1 ]] || fail "--fresh should start with one new window"
p_stop
rm -f "$p_state" "$p_state.bad"

# A docked pane comes back with its window: edge, width, focus, and a window
# with only a docked pane.
p_start
p_cli open --name tab1 >/dev/null
p_cli open --name agent --dock right --cmd "printf 'DOCK BACK\n'; exec sleep 600" >/dev/null
p_cli resize agent 1/4 >/dev/null
p_cli open --name lone --dock --window new >/dev/null
sleep 1.5
python3 - "$p_state" <<'PY' || fail "the state file should hold the docks: $(cat "$p_state")"
import json, sys
s = json.load(open(sys.argv[1]))
docks = [w.get("dock") for w in s["windows"]]
assert docks[0]["edge"] == "right" and docks[0]["width"] == 0.25, docks
assert docks[1]["edge"] == "left" and s["windows"][1]["tabs"] == [], s["windows"][1]
PY
before="$(p_layout)"
p_stop
p_start
sleep 1.5
after="$(p_layout)"
[[ "$after" == "$before" ]] || fail "the docked panes should come back:
before: $before
after:  $after"
for _ in $(seq 20); do [[ "$(field text <<<"$(p_raw '{"id":1,"cmd":"debug.text","args":{"pane":"agent"}}')")" == *"DOCK BACK"* ]] && break; sleep 0.25; done
[[ "$(field text <<<"$(p_raw '{"id":1,"cmd":"debug.text","args":{"pane":"agent"}}')")" == *"DOCK BACK"* ]] || fail "the docked terminal should run its command again"
p_raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"window\":\"win-1\",\"path\":\"$out/docked-restored.png\"}}" >/dev/null
p_stop
rm -f "$p_state" "$p_state.bad"

# A product's terminal wrapper (Kanna's keeper): every terminal runs through
# it, and terminals brought back after a restart are told so (KMUX_RESTORED).
wrapper="$out/wrapper.sh"; wrap_log="$out/wrapper.log"; rm -f "$wrap_log"
cat >"$wrapper" <<'SH'
#!/bin/sh
printf '%s restored=%s args=%s\n' "$KMUX_TERMINAL" "${KMUX_RESTORED:-0}" "$*" >>"$WRAP_LOG"
[ "$1" = "--" ] && shift
[ $# -gt 0 ] && exec "$@"
exec /bin/sh
SH
chmod +x "$wrapper"
export KMUX_TERMINAL_WRAPPER="$wrapper" WRAP_LOG="$wrap_log"
p_start
p_cli open --name w --cmd "echo WRAPPED; exec sleep 600" >/dev/null
for _ in $(seq 20); do [[ "$(field text <<<"$(p_raw '{"id":1,"cmd":"debug.text","args":{"pane":"w"}}')")" == *WRAPPED* ]] && break; sleep 0.25; done
[[ "$(field text <<<"$(p_raw '{"id":1,"cmd":"debug.text","args":{"pane":"w"}}')")" == *WRAPPED* ]] || fail "a command should run through the wrapper"
wrapped() { [[ -f "$wrap_log" ]] && wc -l <"$wrap_log" | tr -d ' ' || echo 0; }
for _ in $(seq 40); do [[ "$(wrapped)" == 2 ]] && break; sleep 0.25; done # both terminals have started
p_stop
p_start
for _ in $(seq 40); do [[ "$(wrapped)" == 4 ]] && break; sleep 0.25; done
p_stop
unset KMUX_TERMINAL_WRAPPER WRAP_LOG
python3 - "$wrap_log" <<'PY' || fail "the wrapper should run every terminal, told when it is restored: $(cat "$wrap_log")"
import sys
lines = open(sys.argv[1]).read().splitlines()
assert len(lines) == 4, lines
first, cmd = sorted(lines[:2]); again, cmd_again = sorted(lines[2:])
assert first == "term-1 restored=0 args=", first
assert cmd.startswith("term-2 restored=0 args=-- ") and "-c echo WRAPPED" in cmd, cmd
assert again == "term-1 restored=1 args=", again
assert cmd_again.startswith("term-2 restored=1 args=-- ") and "-c echo WRAPPED" in cmd_again, cmd_again
PY
rm -f "$p_state" "$p_state.bad"

echo "e2e OK (snapshot: $out/two-panes.png)"
