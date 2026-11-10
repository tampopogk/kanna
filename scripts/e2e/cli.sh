exits() { local want="$1"; shift; "$@" >/dev/null 2>&1; local got=$?; [[ $got == "$want" ]] || { echo "${*}: exit $got, expected $want"; return 1; }; }

page_text() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q -- "$2"; }

scenario cli_commands M4
cli_commands() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR" expected
  cd "$dir" || return 1
  expected="opened term-1 (cli_commands) in win-1, tab-1: running"; says kanna open --window new --name cli_commands --cmd 'printf "cli_commands READY\n"; exec cat' || return 1
  expected="opened term-2 (e2e) in win-1, tab-1: running"; says kanna open --window win-1 --name e2e --split right --size 1/3 || return 1
  check test "$(layout)" == '{"children": [{"pane": "cli_commands", "size": "2/3"}, {"pane": "e2e", "size": "1/3"}], "split": "row"}' || return 1
  check wait_for 5 app_text cli_commands "cli_commands READY" || return 1
  [[ "$(kanna workspace list --all)" == *$'Free windows\nwin-1\n  focus: e2e\n  *tab-1 "cli_commands"  cli_commands:2/3 | e2e:1/3'* ]] || { echo "workspace list --all should show the free window: $(kanna workspace list --all)"; return 1; }
  expected="focused cli_commands in win-1 (in front)"; says kanna focus cli_commands || return 1
  expected="e2e is zoomed (run again to unzoom)"; says kanna zoom e2e || return 1
  check test "$(kanna workspace list --all --json | field windows.0.zoomed)" == e2e || return 1
  expected="e2e is no longer zoomed"; says kanna zoom e2e || return 1
  expected="cli_commands:3/4 | e2e:1/4"; says kanna resize e2e 1/4 || return 1
  expected="cli_commands:1/2 / e2e:1/2"; says kanna move e2e --to cli_commands --side bottom || return 1
  expected="e2e:1/4 | cli_commands:3/4"; says kanna arrange --window win-1 '{"split":"row","children":[{"pane":"e2e","size":"1/4"},{"pane":"cli_commands"}]}' || return 1
  expected="opened term-3 (t) in win-1, tab-2: running"; says kanna open --window win-1 --tab --name t || return 1
  expected='tab-2 is now "shells"'; says kanna rename-tab tab-2 shells || return 1
  expected="win-1: tab-2, tab-1"; says kanna move-tab tab-2 --index 0 || return 1
  expected="switched to tab-1 in win-1 (in front)"; says kanna focus --tab tab-1 || return 1
  expected="t"; says kanna move t --window new || return 1
  check test "$(kanna workspace list --all --json | field windows.1.tabs.0.layout)" == '{"pane": "t"}' || return 1
  expected="brought win-1 forward"; says kanna focus --window win-1 || return 1
  expected="closed win-2 (panes term-3)"; says kanna close --window win-2 || return 1
  kanna send e2e 'echo cli_commands-$((40+2))' || return 1
  check wait_for 5 shows e2e cli_commands-42 || return 1
  check wait_for 5 app_text e2e cli_commands-42 || return 1
  kanna send e2e -- echo --json || return 1
  check wait_for 5 eval 'kanna read e2e --lines 2 | grep -qx -- --json' || return 1
  expected="term-1 (cli_commands) (printf \"cli_commands READY\\n\"; exec cat) is running"; says kanna restart cli_commands || return 1
  printf '<title>cli_commands</title><p>cli_commands PAGE</p>\n' >"$dir/a.html"
  printf '<title>cli_commands B</title><p>cli_commands OTHER</p>\n' >"$dir/b.html"
  expected="opened page-1 (site) in win-1, tab-1: running"; says kanna open --window win-1 web "file://$dir/a.html" --name site --history --split down || return 1
  check wait_for 5 page_text site "cli_commands PAGE" || return 1
  expected="page-1 (site) (file://$dir/b.html) is running"; says kanna navigate site "file://$dir/b.html" || return 1
  check wait_for 5 page_text site "cli_commands OTHER" || return 1
  expected="page-1 (site) (file://$dir/a.html) is running"; says kanna navigate site --back || return 1
  check wait_for 5 page_text site "cli_commands PAGE" || return 1
  expected="page-1 (site) (file://$dir/b.html) is running"; says kanna navigate site --forward || return 1
  kanna open --window win-1 web "file://$dir/a.html" --name plain --tab >/dev/null || return 1
  exits 2 kanna navigate plain --back || return 1
  printf '# cli_commands DOC\n\ncli_commands TEXT\n' >"$dir/cli_commands.md"
  printf '# cli_commands NEXT\n' >"$dir/next.md"
  kanna open --window win-1 md cli_commands.md --name doc --tab >/dev/null || return 1
  check is doc path "$(pwd -P)/cli_commands.md" || return 1
  check wait_for 5 doc_text doc "cli_commands TEXT" || return 1
  kanna navigate doc next.md >/dev/null || return 1
  check wait_for 5 doc_text doc "cli_commands NEXT" || return 1
  kanna navigate doc --back >/dev/null || return 1
  check wait_for 5 doc_text doc "cli_commands TEXT" || return 1
  local missing
  missing="$(kanna open --window win-1 md "$dir/missing.md" --tab 2>&1)"; check test $? -eq 1 || return 1
  check grep -q "No such file" <<<"$missing" || return 1
  check test "$(kanna capabilities | tail -1)" == "Kanna.app: attached" || return 1
  check grep -q "The running Kanna supports every command." <(kanna help) || return 1
  check test "$(kanna open --window win-1 --json --name j --tab | field pane.name)" == j || return 1
  exits 4 kanna close nope || return 1
  exits 1 kanna open --window win-1 --name e2e || return 1
  exits 2 kanna zoom || return 1
  exits 5 kanna raw '{"cmd":"nope"}' || return 1
  exits 0 kanna raw '{"cmd":"debug.app"}' || return 1
  kanna close --window win-1 >/dev/null || return 1
  check test "$(kanna workspace list --all --json | field windows)" == "[]" || return 1
  expected="no windows (open one with \`kanna open\`)"; says kanna workspace list --all
}

scenario cli_without_app M4
cli_without_app() {
  start_daemon || return 1
  local expected
  expected="opened term-1 (cli_without_app) in win-1, tab-1: running"; says kanna open --window new --name cli_without_app --cmd 'echo cli_without_app READY; exec cat' || return 1
  check wait_for 5 shows cli_without_app "cli_without_app READY" || return 1
  kanna send cli_without_app cli_without_app-SENT || return 1
  check wait_for 5 shows cli_without_app cli_without_app-SENT || return 1
  expected="term-1 (cli_without_app) (echo cli_without_app READY; exec cat) is running"; says kanna restart cli_without_app || return 1
  check grep -q "Kanna.app: not attached" <(kanna capabilities) || return 1
  printf '# cli_without_app DOC\n' >"$KANNA_STATE_DIR/cli_without_app.md"
  local args
  while IFS= read -r args; do
    eval "kanna $args" >/dev/null || { echo "failed without the app: kanna $args"; return 1; }
  done <<NEEDS
open --window win-1 --name gone --split right
close gone
open --window win-1 --name doc --split right md $KANNA_STATE_DIR/cli_without_app.md
resize cli_without_app 2/3
open --window win-1 --tab --name other
rename-tab tab-1 kept
focus --tab tab-1
NEEDS
  exits 6 kanna raw '{"cmd":"debug.app"}' || return 1
  check grep -q "KANNA_SOCKET is set, so kanna doesn't start Kanna.app" <(kanna raw '{"cmd":"debug.app"}' 2>&1) || return 1
  start_app || return 1
  check test "$(layout)" == '{"children": [{"pane": "cli_without_app", "size": "2/3"}, {"pane": "doc", "size": "1/3"}], "split": "row"}' || return 1
  check test "$(win win-1 tabs.0.title)" == kept || return 1
  check wait_for 5 doc_text doc "cli_without_app DOC" || return 1
  expected="focused cli_without_app in win-1 (in front)"; says kanna focus cli_without_app || return 1
  stop_app
  check test "$(kanna workspace list --all --json | field windows.0.id)" == win-1
}

scenario zsh_completion M4
zsh_completion() {
  PATH="$bin:$PATH" start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR/zsh_completion"
  mkdir -p "$dir/sub" && touch "$dir/alpha-notes.md" "$dir/alpha.html" "$dir/alpha.txt"
  kanna open --window new --name zsh_completion --cwd "$dir" --cmd "zsh -f -i" >/dev/null || return 1
  kanna open --window win-1 --name zsh_completion-web --tab web "$dir/alpha.html" >/dev/null || return 1
  kanna open --window win-1 --name notes --tab md "$dir/alpha-notes.md" >/dev/null || return 1
  kanna focus zsh_completion >/dev/null || return 1
  sleep 1
  kanna send zsh_completion "PATH='$bin':\$PATH; stty cols 120; source <(kanna completions zsh); clear; print READY\$((1 + 1))" || return 1
  check wait_for 10 shows zsh_completion READY2 || return 1
  zsh_completion_tab() {
    raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$1\"}}" >/dev/null
    sleep 0.3; key tab; [[ -n "$2" ]] && key tab
    local want="$3"
    wait_for 3 eval 'zsh_completion_screen=$(kanna read zsh_completion); printf "%s\n" "$zsh_completion_screen" | grep -qE -- "$want"'
    [[ -n "${4:-}" ]] && { raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$4.png\"}}" >/dev/null; cp "$KANNA_STATE_DIR/$4.png" "$out/$4.png" 2>/dev/null; }
    key ctrl+c; sleep 0.3
    printf "%s\n" "$zsh_completion_screen" | grep -qE -- "$3" || { echo "kanna $1⇥ didn't show $3:"; printf "%s\n" "$zsh_completion_screen" | tail -8; return 1; }
  }
  check zsh_completion_tab "kanna open md al" "" 'open md alpha-notes\.md *$' || return 1
  check zsh_completion_tab "kanna markup alpha." "" 'markup alpha\.html *$' || return 1
  check zsh_completion_tab "kanna markup al" 2 'alpha-notes\.md.*alpha\.html|alpha\.html.*alpha-notes\.md' || return 1
  check zsh_completion_tab "kanna nav" "" 'kanna navigate *$' || return 1
  check zsh_completion_tab "kanna open " 2 '^md +-- A markdown file' || return 1
  check test "$(printf "%s\n" "$zsh_completion_screen" | grep -oE '^(term|web|md|ios) ' | tr -d ' \n')" == termwebmdios || return 1
  check zsh_completion_tab "kanna close " 2 'zsh_completion-web +-- page-[0-9]+ · web · alpha\.html' zsh_completion-panes || return 1
  check zsh_completion_tab "kanna focus --tab " 2 'tab-[0-9]+ +-- "' || return 1
  check zsh_completion_tab "kanna open --split " 2 'right' || return 1
  check zsh_completion_tab "kanna open md alpha-notes.md --sp" "" '--split *$' || return 1
  kanna open --window win-1 --name zsh_completion-login --tab --cwd "$dir" >/dev/null || return 1
  check wait_for 10 eval '[[ -n "$(kanna read zsh_completion-login | tr -d "[:space:]")" ]]' || return 1
  sleep 2
  kanna send zsh_completion-login 'print -r -- "zsh_completion:$_comps[kanna]:${ZDOTDIR-home}:${KANNA_ZSH_ZDOTDIR-gone}"' || return 1
  check wait_for 5 shows zsh_completion-login 'zsh_completion:_kanna:home:gone'
}

png_size() { python3 -c 'import struct, sys
d = open(sys.argv[1], "rb").read(24)
assert d[:8] == b"\x89PNG\r\n\x1a\n"
print("%dx%d" % struct.unpack(">II", d[16:24]))' "$1"; }

scenario screenshot M4
screenshot() {
  start_daemon || return 1
  env -u KANNA_DEBUG -u KANNA_IGNORE_OCCLUSION "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  check wait_for 10 eval '[[ "$(kanna capabilities)" == *"Kanna.app: attached"* ]]' || return 1
  local shots="$KANNA_STATE_DIR/shots"; mkdir -p "$shots" && shots="$(cd "$shots" && pwd -P)"
  kanna open --window new --name screenshot --cmd 'for i in $(seq 80); do echo "screenshot $i the quick brown fox jumps over the lazy dog"; done; exec cat' >/dev/null || return 1
  check wait_for 5 shows screenshot "screenshot 80" || return 1
  printf '# screenshot\n\nA markdown pane.\n' >"$KANNA_STATE_DIR/screenshot.md"
  kanna open --window win-1 md "$KANNA_STATE_DIR/screenshot.md" --name doc --split right >/dev/null || return 1
  check wait_for 5 is doc state running || return 1
  raw '{"id":1,"cmd":"debug.app"}' | grep -q '"unknown command' || { echo "screenshot: the app answers debug commands"; return 1; }
  local window pane doc
  window="$(cd "$shots" && kanna screenshot --window win-1 -o win-1.png)" || { echo "screenshot line $LINENO"; return 1; }
  check test "$window" == "$shots/win-1.png" || return 1
  pane="$(cd "$shots" && kanna screenshot screenshot --out screenshot.png)" || { echo "screenshot line $LINENO"; return 1; }
  doc="$(kanna screenshot doc --json)" || { echo "screenshot line $LINENO"; return 1; }
  cp "$shots"/*.png "$out/" 2>/dev/null
  local ww wh pw ph; IFS=x read -r ww wh <<<"$(png_size "$shots/win-1.png")"; IFS=x read -r pw ph <<<"$(png_size "$shots/screenshot.png")"
  check test "$ww" -ge 800 -a "$wh" -ge 400 || return 1
  check test $((pw * 2 - ww)) -ge -4 -a $((pw * 2 - ww)) -le 4 || { echo "screenshot: pane $pw wide in a window $ww wide"; return 1; }
  check test "$ph" -lt "$wh" -a "$ph" -gt $((wh / 2)) || return 1
  check python3 -c 'import json, sys
r = json.loads(sys.argv[1]); assert r["pane"] == "file-1" and r["window"] == "win-1" and r["path"].endswith(".png") and r["width"] > 0, r' "$doc" || return 1
  check test "$(png_size "$(echo "$doc" | field path)")" == "$(echo "$doc" | field width)x$(echo "$doc" | field height)" || return 1
  local ink; ink="$(png_ink "$shots/screenshot.png")"
  check python3 -c 'import sys; assert 0.01 < float(sys.argv[1]) < 0.5, sys.argv[1]' "$ink" || { echo "screenshot: the terminal pane isn't legible (ink $ink)"; return 1; }
  (cd "$shots" && kanna screenshot screenshot -o screenshot.png) 2>/dev/null && { echo "screenshot: it overwrote screenshot.png"; return 1; }
  (cd "$shots" && kanna screenshot screenshot -o screenshot.png --force) >/dev/null || { echo "screenshot line $LINENO"; return 1; }
  kanna open --window win-1 --tab --name other >/dev/null || return 1
  check test "$(kanna screenshot screenshot --json | field hidden)" == True || { echo "screenshot: a pane in a hidden tab wasn't drawn off-screen"; return 1; }
  kanna open --window new --name elsewhere >/dev/null || return 1
  local caller; caller="$(KANNA_TERMINAL="$(pane other id)" kanna screenshot --json | field window)" || return 1
  check test "$caller" == win-1 || { echo "screenshot: from other's terminal, with win-2 in front, it shot $caller"; return 1; }
  local refused; refused="$(kanna screenshot 2>&1)" && { echo "screenshot: it shot with no target from outside Kanna"; return 1; }
  [[ "$refused" == *"screenshot: outside a Kanna terminal there's no window of yours: say PANE or --window WINDOW"* ]] || { echo "screenshot: $refused"; return 1; }
  refused="$(KANNA_TERMINAL=term-99 kanna screenshot 2>&1)" && { echo "screenshot: it shot for an unknown terminal"; return 1; }
  [[ "$refused" == *"term-99 isn't in a window that's showing: say PANE or --window WINDOW"* ]] || { echo "screenshot: $refused"; return 1; }
}

png_alike() { python3 -c 'import struct, sys, zlib
def pixels(path):
    d = open(path, "rb").read(); i = 8; idat = b""
    while i < len(d):
        n, kind = struct.unpack(">I4s", d[i:i + 8]); body = d[i + 8:i + 8 + n]; i += 12 + n
        if kind == b"IHDR": w, h, depth, color = struct.unpack(">IIBB", body[:10])
        if kind == b"IDAT": idat += body
    assert depth == 8 and color in (2, 6)
    bpp = 3 if color == 2 else 4; raw = zlib.decompress(idat); stride = w * bpp; prev = bytearray(stride); rows = []
    for y in range(h):
        f = raw[y * (stride + 1)]; line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0; b = prev[x]; c = prev[x - bpp] if x >= bpp else 0
            if f == 1: line[x] = (line[x] + a) & 255
            elif f == 2: line[x] = (line[x] + b) & 255
            elif f == 3: line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c; pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line); prev = line
    return w, h, [[tuple(r[x:x + 3]) for x in range(0, stride, bpp * 2)] for r in rows[::2]]
(w1, h1, a), (w2, h2, b) = pixels(sys.argv[1]), pixels(sys.argv[2])
assert (w1, h1) == (w2, h2), f"{sys.argv[1]} is {w1}x{h1}, {sys.argv[2]} is {w2}x{h2}"
total = sum(len(r) for r in a)
off = sum(1 for ra, rb in zip(a, b) for p, q in zip(ra, rb) if sum(abs(p[k] - q[k]) for k in range(3)) > 60)
assert off / total < float(sys.argv[3]), f"{sys.argv[1]} and {sys.argv[2]} differ in {off / total:.2%} of their pixels"' "$1" "$2" "${3:-0.02}"; }

scenario screenshot_hidden M4
screenshot_hidden() {
  start_daemon && start_app || return 1
  local shots="$KANNA_STATE_DIR/shots"; mkdir -p "$shots" && shots="$(cd "$shots" && pwd -P)"
  minimized() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$1\"${2:-}}}" | field minimized; }
  seen() { kanna workspace list --all --json | python3 -c 'import json, sys
s = json.load(sys.stdin)
print(json.dumps([[w.get(k) for k in ("id", "tab", "focused", "zoomed")] for w in s["windows"]] + [[ws.get(k) for k in ("id", "tab", "focused", "zoomed", "where")] for ws in s.get("workspaces", [])]))'; }
  shot() { kanna screenshot "$1" -o "$shots/$2.png" "${@:3}" --json; }
  hidden() { local said; said="$(shot "$@")" || { echo "screenshot_hidden: kanna screenshot $1 failed: $said"; return 1; }; [[ "$(field hidden <<<"$said")" == True ]] || { echo "screenshot_hidden: $1 wasn't drawn off-screen: $said"; return 1; }; }
  alike() { cp "$shots/$1.png" "$shots/$2.png" "$out/" 2>/dev/null; png_alike "$shots/$1.png" "$shots/$2.png" "${3:-0.02}" || { echo "screenshot_hidden: $2 doesn't look like $1"; return 1; }; }

  kanna open --window new --name lines --cmd 'for i in $(seq 40); do echo "hidden $i the quick brown fox jumps over the lazy dog"; done; exec cat' >/dev/null || return 1
  check wait_for 5 shows lines "hidden 40" || return 1
  sleep 0.5; shot lines lines-shown >/dev/null || { echo "screenshot_hidden line $LINENO"; return 1; }
  kanna open --window win-1 --tab --name other >/dev/null || return 1
  check wait_for 5 is other state running || return 1
  sleep 0.5
  local before; before="$(seen)"
  hidden lines lines-hidden || return 1
  check test "$(seen)" == "$before" || { echo "screenshot_hidden: shooting a hidden tab changed the layout: $before → $(seen)"; return 1; }
  alike lines-shown lines-hidden || return 1
  local sized w h; sized="$(shot lines lines-sized --size 400x300)" || return 1
  w="$(field width <<<"$sized")"; h="$(field height <<<"$sized")"
  check test "$w" == 400 -o "$w" == 800 || { echo "screenshot_hidden: --size 400x300 gave $w×$h"; return 1; }
  check test $((h * 4)) == $((w * 3)) || { echo "screenshot_hidden: --size 400x300 gave $w×$h"; return 1; }
  check test "$(png_size "$shots/lines-sized.png")" == "${w}x$h" || return 1
  local shown; shown="$(kanna screenshot other --size 400x300 2>&1)" && { echo "screenshot_hidden: --size on a showing pane"; return 1; }
  [[ "$shown" == *"--size is for a pane that isn't showing"* ]] || { echo "screenshot_hidden: $shown"; return 1; }

  printf '# Hidden\n\nA markdown pane, drawn while its window is minimised.\n\n- one\n- two\n' >"$KANNA_STATE_DIR/hidden.md"
  kanna open --window new md "$KANNA_STATE_DIR/hidden.md" --name doc >/dev/null || return 1
  check wait_for 5 is doc state running || return 1
  check wait_for 5 doc_text doc "drawn while its window" || return 1
  local doc_window; sleep 0.5; doc_window="$(shot doc doc-shown | field window)" || { echo "screenshot_hidden line $LINENO"; return 1; }
  minimized "$doc_window" ',"minimize":true' >/dev/null; check wait_for 5 eval '[[ "$(minimized "$doc_window")" == True ]]' || return 1
  before="$(seen)"
  hidden doc doc-hidden || return 1
  check test "$(minimized "$doc_window")" == True || { echo "screenshot_hidden: shooting the minimised window's pane brought it back"; return 1; }
  check test "$(seen)" == "$before" || return 1
  alike doc-shown doc-hidden || return 1
  minimized "$doc_window" ',"minimize":false' >/dev/null

  printf '<!doctype html><body style="background:#f4efe6;font:24px sans-serif"><h1>Hidden page</h1><p style="color:#a33">drawn off-screen by WebKit</p></body>\n' >"$KANNA_STATE_DIR/hidden.html"
  kanna open --window win-1 web "file://$KANNA_STATE_DIR/hidden.html" --name page --tab >/dev/null || return 1
  check wait_for 5 page_text page "drawn off-screen" || return 1
  sleep 0.5; shot page page-shown >/dev/null || { echo "screenshot_hidden line $LINENO"; return 1; }
  kanna focus other >/dev/null || return 1
  check wait_for 5 eval '[[ "$(kanna screenshot page --json | field hidden)" == True ]]' || return 1
  before="$(seen)"
  hidden page page-hidden || return 1
  check test "$(seen)" == "$before" || return 1
  alike page-shown page-hidden || return 1

  local probe_id; probe_id="$(new_task Probe "$(probe '\033[?1004hPROBE ' '\007' 0)")" || return 1
  kanna task pick "$probe_id" >/dev/null || return 1
  kanna task activate Probe >/dev/null || return 1
  check wait_for 10 shows Probe READY || return 1
  sleep 1; shot Probe probe-shown >/dev/null || { echo "screenshot_hidden line $LINENO"; return 1; }
  kanna task park Probe >/dev/null || return 1
  sleep 1
  local reports; reports="$(kanna read Probe | grep -c '^IN ')"
  before="$(seen)"
  hidden Probe probe-hidden || return 1
  hidden Probe probe-hidden-2 || return 1
  sleep 1
  check test "$(kanna read Probe | grep -c '^IN ')" == "$reports" || { echo "screenshot_hidden: shooting the parked agent reported focus to it"; kanna read Probe | tail -5; return 1; }
  check test "$(seen)" == "$before" || { echo "screenshot_hidden: shooting the parked agent changed the layout: $before → $(seen)"; return 1; }
  alike probe-shown probe-hidden 0.04 || return 1

  local runner; runner="$(new_task Runner 'for i in $(seq 20); do echo "shelf $i the quick brown fox"; done; exec cat')" || return 1
  kanna task pick "$runner" >/dev/null && kanna task activate Runner >/dev/null || return 1
  check wait_for 10 shows Runner "shelf 20" || return 1
  sleep 1; shot Runner runner-shown >/dev/null || { echo "screenshot_hidden line $LINENO"; return 1; }
  kanna task shelve Runner >/dev/null || return 1
  check wait_for 5 eval '[[ "$(cd "$(e2e_repo)" && kanna task list | placed | grep -c "Runner  Shelf")" == 1 ]]' || { (cd "$(e2e_repo)" && kanna task list); return 1; }
  hidden Runner runner-hidden || return 1
  alike runner-shown runner-hidden || return 1

  kanna task stop Runner >/dev/null || return 1
  local refused; refused="$(kanna screenshot Runner 2>&1)" && { echo "screenshot_hidden: shot a stopped Shelf task's agent"; return 1; }
  [[ "$refused" == *"which is stopped on the Shelf: nothing is running to draw"* ]] || { echo "screenshot_hidden: $refused"; return 1; }
}

scenario bash_completion M4
bash_completion() {
  SHELL=/bin/bash PATH="$bin:$PATH" start_daemon && start_app || return 1
  kanna open --window new --name bash_completion >/dev/null || return 1
  check wait_for 10 eval '[[ -n "$(kanna read bash_completion | tr -d "[:space:]")" ]]' || return 1
  sleep 2
  kanna send bash_completion 'echo "bash_completion:$(complete -p kanna):${PROMPT_COMMAND-none}:${KANNA_BASH_INIT-gone}"' || return 1
  check wait_for 5 shows bash_completion 'bash_completion:complete -o filenames -F _kanna kanna:'
  kanna read bash_completion | grep -E '^bash_completion:' | grep -qF ':gone' || { kanna read bash_completion | tail -4; return 1; }
}

scenario cli_per_instance M4
cli_per_instance() {
  local zdot="$KANNA_STATE_DIR/zdot" other="$KANNA_STATE_DIR/other/bin" line='[[ -n $KANNA_CLI ]] && path=(${KANNA_CLI:h} $path)'
  mkdir -p "$zdot" "$other" && printf '#!/bin/sh\necho another kanna\n' >"$other/kanna" && chmod +x "$other/kanna" || { echo "cli_per_instance line $LINENO"; return 1; }
  printf 'path=(%q $path)\n%s\n' "$other" "$line" >"$zdot/.zprofile"
  printf 'path=(%q $path)\n%s\n' "$other" "$line" >"$zdot/.zshrc"
  SHELL=/bin/zsh ZDOTDIR="$zdot" start_daemon || { echo "cli_per_instance line $LINENO"; return 1; }
  kanna open --window new --name cli_per_instance >/dev/null || return 1
  kanna send cli_per_instance 'echo "cli=$KANNA_CLI="; echo "which=$(which kanna)="; echo "first=$path[1]= second=$path[2]="' || return 1
  check wait_for 5 unwrapped cli_per_instance "cli=$bin/kanna=" || { echo "cli_per_instance: KANNA_CLI isn't the instance's CLI"; kanna read cli_per_instance; return 1; }
  check wait_for 5 unwrapped cli_per_instance "which=$bin/kanna=" || { echo "cli_per_instance: plain kanna isn't the instance's"; kanna read cli_per_instance; return 1; }
  check unwrapped cli_per_instance "first=$bin= second=$other=" || { echo "cli_per_instance: another kanna wasn't earlier on PATH before the line"; kanna read cli_per_instance; return 1; }
  kanna open --window win-1 --name cli_per_instance-cmd --cmd 'echo "cmd=$(which kanna)="; exec cat' >/dev/null || return 1
  check wait_for 5 unwrapped cli_per_instance-cmd "cmd=$bin/kanna=" || { echo "cli_per_instance: a command pane's kanna isn't the instance's"; kanna read cli_per_instance-cmd; return 1; }
  return 0
}

scenario cli_caller M4
cli_caller() {
  start_daemon && start_app || { echo "cli_caller line $LINENO"; return 1; }
  local file="$KANNA_STATE_DIR/cli_caller.rs" doc="$KANNA_STATE_DIR/cli_caller.md" refused
  for i in $(seq 20); do echo "let line_$i = $i;"; done >"$file"
  printf '# cli_caller\n\nA doc.\n' >"$doc"
  for line in "open --name x" "open md $doc" "comment $file:3 Three." "markup $doc" "arrange '{\"pane\":\"x\"}'"; do
    refused="$(eval "kanna $line" 2>&1)"; check test $? -eq 2 || { echo "cli_caller: kanna $line outside Kanna: $refused"; return 1; }
    [[ "$refused" == *"${line%% *}: outside a Kanna terminal there's no window of yours: add --window WINDOW"* ]] || { echo "cli_caller: kanna $line said: $refused"; return 1; }
  done
  refused="$(kanna screenshot 2>&1)"; check test $? -eq 2 || return 1
  [[ "$refused" == *"screenshot: outside a Kanna terminal there's no window of yours: say PANE or --window WINDOW"* ]] || { echo "cli_caller: screenshot said: $refused"; return 1; }
  refused="$(kanna task park 2>&1)"; check test $? -eq 2 || return 1
  [[ "$refused" == *"task: park needs a TASK, or run it from the task's terminal"* ]] || { echo "cli_caller: task park said: $refused"; return 1; }
  check test "$(state | field windows | count)" == 0 || { echo "cli_caller: a refused command opened a window"; return 1; }
  refused="$(KANNA_TERMINAL=term-99 kanna open --name x 2>&1)" && { echo "cli_caller: an unknown caller opened a pane"; return 1; }
  [[ "$refused" == *"term-99 isn't one of this Kanna's panes"*"--window"* ]] || { echo "cli_caller: an unknown caller: $refused"; return 1; }
  expected="opened term-1 (a) in win-1, tab-1: running"; says kanna open --window new --name a || return 1
  expected="opened term-2 (b) in win-2, tab-2: running"; says kanna open --window new --name b || return 1
  check wait_for 5 eval '[[ "$(dwin win-2 key)" == True ]]' || { echo "cli_caller: the app doesn't have win-2 in front"; return 1; }
  check test "$(state | python3 -c 'import json, sys; print([w["id"] for w in json.load(sys.stdin)["windows"] if "key" in w])')" == "[]" || { echo "cli_caller: the daemon names a key window"; return 1; }
  refused="$(kanna raw '{"cmd":"open","args":{"type":"term"}}' 2>&1)" && { echo "cli_caller: an open naming no window, with two windows, opened: $refused"; return 1; }
  [[ "$refused" == *"several windows"*"--window"* ]] || { echo "cli_caller: an open naming no window: $refused"; return 1; }
  expected="focused a in win-1 (in front)"; says kanna focus a || return 1
  check wait_for 5 eval '[[ "$(dwin win-1 key)" == True ]]' || { echo "cli_caller: focus a didn't bring win-1 forward"; return 1; }
  kanna focus b >/dev/null || return 1
  local a; a="$(pane a id)"
  expected="opened term-3 (c) in win-1, tab-1: running"; KANNA_TERMINAL="$a" says kanna open --name c || { echo "cli_caller: open from a's terminal"; return 1; }
  local said code
  said="$(KANNA_TERMINAL="$a" kanna comment "$file:3" "Three.")" || return 1
  [[ "$said" == "opened file-"*" · cli_caller.rs:3" ]] || { echo "cli_caller: comment said $said"; return 1; }
  code="$(awk '{print $2}' <<<"$said")"
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys
s = json.load(sys.stdin); print(next(w["id"] for w in s["windows"] if any(sys.argv[1] in json.dumps(t) for t in w["tabs"])))' "$code")" == win-1 || { echo "cli_caller: the comment's pane isn't in a's window"; return 1; }
  expected="$code · cli_caller.rs:5"; KANNA_TERMINAL="$a" says kanna comment "$file:5" "Five." || { echo "cli_caller: a second comment from a's terminal didn't reuse its pane"; return 1; }
  kanna focus b >/dev/null || return 1
  expected="$code · cli_caller.rs:7"; says kanna comment --window win-1 "$file:7" "Seven." || { echo "cli_caller: a comment --window win-1 didn't reuse its pane"; return 1; }
  KANNA_TERMINAL="$a" kanna markup "$doc" --name spec --tab >/dev/null || return 1
  check test "$(pane spec type)" == md || return 1
  kanna focus b >/dev/null || return 1
  expected="a:1/2 | c:1/2"; KANNA_TERMINAL="$a" says kanna arrange '{"split":"row","children":[{"pane":"a"},{"pane":"c"}]}' || { echo "cli_caller: arrange from a's terminal"; kanna workspace list --all; return 1; }
  check test "$(win win-2 tabs.0.layout.pane)" == b || { echo "cli_caller: arrange touched win-2"; return 1; }
}
