scenario web_md_ios_panes M3
web_md_ios_panes() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR" before
  printf '# web_md_ios_panes DOC\n\nweb_md_ios_panes TEXT\n' >"$dir/web_md_ios_panes.md"
  printf '<title>web_md_ios_panes PAGE</title><p id="x">web_md_ios_panes WEB</p>\n' >"$dir/web_md_ios_panes.html"
  printf '<title>web_md_ios_panes NEXT</title><p>web_md_ios_panes NEXT</p>\n' >"$dir/web_md_ios_panesb.html"
  page_shows() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q "$2"; }
  doc_shows() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q "$2"; }
  kanna open --window new web --name web --history "file://$dir/web_md_ios_panes.html" >/dev/null || return 1
  kanna open --window win-1 md --name doc --split right "$dir/web_md_ios_panes.md" >/dev/null || return 1
  check wait_for 5 page_shows web "web_md_ios_panes WEB" || return 1
  check wait_for 5 doc_shows doc "web_md_ios_panes TEXT" || return 1
  kanna navigate web "file://$dir/web_md_ios_panesb.html" >/dev/null || return 1
  check wait_for 5 page_shows web "web_md_ios_panes NEXT" || return 1
  kanna navigate web --back >/dev/null || return 1
  check is web url "file://$dir/web_md_ios_panes.html" || return 1
  check wait_for 5 page_shows web "web_md_ios_panes WEB" || return 1
  before="$(layout)"
  raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc","zoom":2}}' >/dev/null
  sleep 1
  stop_app
  check is doc path "$dir/web_md_ios_panes.md" || return 1
  kanna navigate web "file://$dir/web_md_ios_panesb.html" >/dev/null || return 1
  start_app || return 1
  check test "$(layout)" == "$before" || return 1
  check is web url "file://$dir/web_md_ios_panesb.html" || return 1
  check wait_for 5 page_shows web "web_md_ios_panes NEXT" || return 1
  check python3 -c 'import sys; assert float(sys.argv[1]) > 1, sys.argv[1]' "$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field zoom)" || return 1
  kanna navigate web --back >/dev/null || return 1
  check wait_for 5 page_shows web "web_md_ios_panes WEB" || return 1
  check wait_for 5 doc_shows doc "web_md_ios_panes TEXT" || return 1
  stop_daemon; start_daemon || return 1
  check wait_for 10 app_attached || return 1
  check is web state running || return 1
  check test "$(layout)" == "$before" || return 1
  check page_shows web "web_md_ios_panes WEB" || return 1
  local udid
  udid="$(xcrun simctl list devices booted 2>/dev/null | sed -n 's/^ *iPhone.*(\([0-9A-F-]\{36\}\)) (Booted).*/\1/p' | head -1)"
  if [[ -z "$udid" ]]; then
    echo "(iOS part skipped: no booted iPhone simulator)"
    return 0
  fi
  kanna open --window win-1 ios --name sim --app com.apple.Preferences --device "$udid" --tab >/dev/null || return 1
  check is sim state running || return 1
  check test "$(raw '{"id":1,"cmd":"debug.ios","args":{"pane":"sim"}}' | field ok)" == True || return 1
  stop_app; start_app || return 1
  ios_shown() { [[ "$(raw '{"id":1,"cmd":"debug.ios","args":{"pane":"sim"}}' | field ok)" == True ]]; }
  check wait_for 60 ios_shown || return 1
  check is sim state running
}

scenario web_pinch_anchor M4
web_pinch_anchor() {
  start_daemon && start_app || return 1
  kanna open --window new --name shell >/dev/null || return 1
  local dir="$KANNA_STATE_DIR/pinch" before after pinched steps
  mkdir -p "$dir"
  python3 - "$dir/page.html" <<'PY'
import sys
cells = "".join(f'<div id="c{y}-{x}" style="position:absolute;left:{x * 120}px;top:{y * 120}px;width:120px;height:120px">{y}-{x}</div>' for y in range(40) for x in range(12))
open(sys.argv[1], "w").write(f'<!doctype html><html style="scroll-behavior: smooth"><body style="margin:0;height:4800px;width:1440px;position:relative">{cells}</body></html>')
PY
  KANNA_TERMINAL="$(pane shell id)" kanna open web --name pinched --tab "file://$dir/page.html" >/dev/null || return 1
  web_pinch_js() { raw "$(python3 -c 'import json, sys; print(json.dumps({"id": 1, "cmd": "debug.web", "args": {"pane": "pinched", "js": sys.argv[1]}}))' "$1")" | field value; }
  web_pinch_at() { web_pinch_js "(() => { const v = visualViewport, x = $1 / v.scale, y = $2 / v.scale; return [v.pageLeft + x, v.pageTop + y, v.scale, document.elementFromPoint(x, y)?.id]; })()"; }
  web_pinch_settled() { local last=; for _ in 1 2 3 4 5 6 7 8 9 10; do sleep 0.1; [[ "$(web_pinch_at 0 0)" == "$last" ]] && return 0; last="$(web_pinch_at 0 0)"; done; }
  web_pinch_same() { python3 -c 'import json, sys
a, b = json.loads(sys.argv[1]), json.loads(sys.argv[2])
far = max(abs(a[0] - b[0]), abs(a[1] - b[1])) * b[2]
assert far < float(sys.argv[3]) and a[3] == b[3], ("not the same content", a, b, far)' "$1" "$2" "${3:-3}"; }
  web_pinch_do() { raw "{\"id\":1,\"cmd\":\"debug.pinch\",\"args\":{\"pane\":\"pinched\",$1}}" >/dev/null; web_pinch_settled; }
  check wait_for 5 eval '[[ "$(web_pinch_js "document.readyState")" == *complete* ]]' || return 1
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"pinched\"}}" | grep -q "c0-0\|0-0"' || return 1

  web_pinch_js "scrollTo({left: 130, top: 1700, behavior: 'instant'})" >/dev/null; web_pinch_settled
  before="$(web_pinch_at 300 250)"
  web_pinch_do '"at":[300,250],"steps":[0.2,0.2,0.2]'
  after="$(web_pinch_at 300 250)"
  check python3 -c 'import json, sys; assert json.loads(sys.argv[1])[2] > 1.5, sys.argv[1]' "$after" || return 1
  check web_pinch_same "$before" "$after" || return 1

  before="$after"
  web_pinch_do '"at":[300,250],"drift":[14,-9],"steps":[0.15,0.15,0.15,0.15]'
  after="$(web_pinch_at 300 250)"
  check web_pinch_same "$before" "$after" || { echo "fingers drifting moved the anchor"; return 1; }

  before="$(web_pinch_at 90 120)"
  web_pinch_do '"at":[90,120],"steps":[-0.2,-0.2,-0.2]'
  after="$(web_pinch_at 90 120)"
  check web_pinch_same "$before" "$after" || { echo "a pinch out moved the anchor"; return 1; }

  key cmd+0; web_pinch_settled
  web_pinch_js "scrollTo({left: 130, top: 1700, behavior: 'instant'})" >/dev/null; web_pinch_settled
  before="$(web_pinch_at 333 217)"
  steps="$(python3 -c 'print(",".join(["0.008"] * 30))')"
  pinched="$(raw "{\"id\":1,\"cmd\":\"debug.pinch\",\"args\":{\"pane\":\"pinched\",\"at\":[333,217],\"steps\":[$steps]}}")"; web_pinch_settled
  after="$(web_pinch_at 333 217)"
  check web_pinch_same "$before" "$after" || { echo "small steps added up"; return 1; }
  check python3 -c 'import json, sys
x, y = json.loads(sys.argv[1])[:2]
spots = json.loads(sys.argv[2])["spots"]
assert len(spots) == 30, len(spots)
far = max(max(abs(l + 333 / s - x), abs(t + 217 / s - y)) * s for l, t, s in spots)
assert far < 3, ("a step moved the anchor", far)' "$before" "$pinched" || return 1

  key cmd+0; web_pinch_settled
  web_pinch_js "scrollTo({left: 0, top: 40, behavior: 'instant'})" >/dev/null; web_pinch_settled
  before="$(web_pinch_at 300 250)"
  web_pinch_do '"at":[300,250],"steps":[-0.2,0.25]'
  after="$(web_pinch_at 300 250)"
  check web_pinch_same "$before" "$after" || { echo "pinching out against the top and back lost the anchor"; return 1; }

  key cmd+0; web_pinch_settled
  web_pinch_js "scrollTo({left: 130, top: 1700, behavior: 'instant'})" >/dev/null; web_pinch_settled
  local mid
  mid="$(web_pinch_js '[innerWidth / 2, innerHeight / 2]')"
  local mx my; mx="$(python3 -c 'import json, sys; print(json.loads(sys.argv[1])[0])' "$mid")"; my="$(python3 -c 'import json, sys; print(json.loads(sys.argv[1])[1])' "$mid")"
  before="$(web_pinch_at "$mx" "$my")"
  kanna focus pinched >/dev/null || return 1
  key cmd+=; key cmd+=; web_pinch_settled
  after="$(web_pinch_at "$mx" "$my")"
  check python3 -c 'import json, sys; assert json.loads(sys.argv[1])[2] == 1.25, sys.argv[1]' "$after" || return 1
  check web_pinch_same "$before" "$after" || { echo "cmd+= should keep the view's middle"; return 1; }
  key cmd+-; web_pinch_settled
  after="$(web_pinch_at "$mx" "$my")"
  check web_pinch_same "$before" "$after" || { echo "cmd+- should keep the view's middle"; return 1; }
}

scenario code_pane M4
code_pane() {
  start_daemon && start_app || return 1
  local file="$KANNA_STATE_DIR/code_pane.rs"
  for i in $(seq 120); do echo "let line_$i = $i;"; done >"$file"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | field "$2"; }
  kanna open --window new code --name src "$file:80" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 80 ]]' || return 1
  check eval '(( $(code src top) > 50 && $(code src top) < 80 ))' || { echo "top $(code src top)"; return 1; }
  check is src line 80 || return 1
  check test "$(code src language)" == rust || return 1
  check wait_for 5 eval '(( $(code src spans) > 300 ))' || { echo "spans $(code src spans)"; return 1; }
  ov() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"src\"${2:-}}}" | field "overview.$1"; }
  codewin() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"win-1\",\"size\":[$1,700]}}" >/dev/null; }
  codewin 1200
  check wait_for 5 eval '[[ "$(ov shown)" == True ]]' || { echo "no overview: wanted $(ov wanted), clip $(code src clip_width) pt"; return 1; }
  check eval '(( $(ov box_height | cut -d. -f1) > 10 && $(ov top | cut -d. -f1) > 0 ))' || { echo "box $(ov box_y) $(ov box_height), top $(ov top)"; return 1; }
  check test "$(code src clip_width | cut -d. -f1)" -le "$((1200 - 98))" || { echo "the text runs under the overview: $(code src clip_width)"; return 1; }
  local tiles; tiles="$(ov rendered_tiles)"
  ov top ',"overview_click":3' >/dev/null
  check wait_for 5 eval '[[ "$(ov top)" == 0* ]]' || { echo "a click at the top left top at $(ov top)"; return 1; }
  ov top ',"overview_drag":[10,60]' >/dev/null
  check wait_for 5 eval '(( $(ov top | cut -d. -f1) > 200 ))' || { echo "dragging the box left top at $(ov top)"; return 1; }
  ov top ',"overview_marks_click":690' >/dev/null
  check wait_for 5 eval '(( $(code src top) > 90 ))' || { echo "a click at the foot of the marks left line $(code src top) at the top"; return 1; }
  check test "$(ov rendered_tiles)" == "$tiles" || { echo "tiles drawn again on scroll: $tiles, then $(ov rendered_tiles)"; return 1; }
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","pick":40}}' >/dev/null
  ov shown ',"overview_hover":true' >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"code_pane-overview-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/code_pane-overview-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"code_pane-overview-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/code_pane-overview-dark.png" "$out/" 2>/dev/null
  ov shown ',"overview_hover":false' >/dev/null
  check test "$(ov shown ',"overview_enabled":false')" == False || { echo "View › Overview off left it shown"; return 1; }
  check test "$(ov enabled)" == False || return 1
  check test "$(ov shown ',"overview_enabled":true')" == True || { echo "View › Overview on didn't bring it back"; return 1; }
  codewin 700
  check wait_for 5 eval '[[ "$(ov shown)" == False && "$(ov wanted)" == True ]]' || { echo "at 700 pt: shown $(ov shown)"; return 1; }
  codewin 1200
  check wait_for 5 eval '[[ "$(ov shown)" == True ]]' || { echo "not back at 1200 pt"; return 1; }
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","reveal":80}}' >/dev/null
  echo 'let code_pane_edit = 1;' >>"$file"
  check wait_for 5 eval 'code src text | grep -q code_pane_edit' || return 1
  check wait_for 5 eval '[[ "$(code src changed)" == 121 ]]' || { echo "changed $(code src changed)"; return 1; }
  check wait_for 5 eval '[[ "$(ov marks_by_kind.changed)" == 1* ]]' || { echo "no changed mark: $(ov marks_by_kind)"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"src","at":[120,200]}}' >/dev/null; sleep 0.3
  key() { raw "{\"id\":1,\"cmd\":\"debug.key\",\"args\":{\"key\":\"$1\"}}" >/dev/null; }
  key cmd+shift+=; key cmd+shift+=
  key cmd+f
  check wait_for 5 eval '[[ "$(code src find_bar)" == 1 ]]' || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"line_77 "}}' >/dev/null
  check wait_for 5 eval '[[ "$(ov marks_by_kind.hit)" == 1* ]]' || { echo "no find hit mark: $(ov marks_by_kind)"; return 1; }
  key return
  check wait_for 5 eval '[[ "$(code src selection_line)" == 77 && "$(code src selection_length)" == 8 ]]' || { echo "found at $(code src selection_line)"; return 1; }
  key escape
  check wait_for 5 eval '[[ "$(code src font_size)" == 14.5 ]]' || { echo "font $(code src font_size)"; return 1; }
  kanna navigate src "$file:10" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 10 ]]' || return 1
  kanna navigate src --back >/dev/null || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 80 ]]' || return 1
  press() { raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$1\"}}" >/dev/null; sleep 0.2; }
  press gg; check wait_for 5 eval '[[ "$(code src top)" == 1 ]]' || { echo "gg top $(code src top)"; return 1; }
  press 3j; press 5j; press k; check wait_for 5 eval '[[ "$(code src top)" == 8 ]]' || { echo "j k top $(code src top)"; return 1; }
  press G; check wait_for 5 eval '(( $(code src top) > 80 && $(code src top) < 100 ))' || { echo "G top $(code src top)"; return 1; }
  press b; check wait_for 5 eval '(( $(code src top) < 70 ))' || { echo "b top $(code src top)"; return 1; }
  check wait_for 5 eval '(( $(code src top) < 80 ))' || { echo "b top $(code src top)"; return 1; }
  [[ "$(kanna open --window win-1 code "$KANNA_STATE_DIR/gone.rs" 2>&1)" == *"No such file: $KANNA_STATE_DIR/gone.rs"* ]] || { echo "code_pane: a missing file"; return 1; }
  [[ "$(kanna open --window win-1 code "$KANNA_STATE_DIR" 2>&1)" == *"is a folder, not a file"* ]] || { echo "code_pane: a folder"; return 1; }
  local nested="$KANNA_STATE_DIR/code_pane-nested.rs"
  { echo 'fn outer() {'; echo '    loop {'; echo '        if ready() {'; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; } >"$nested"
  kanna open --window win-1 code --name nested "$nested" --tab >/dev/null || return 1
  check wait_for 5 eval '(( $(code nested scopes) == 3 ))' || { echo "scopes $(code nested scopes)"; return 1; }
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"nested","pick":40}}' >/dev/null
  check wait_for 5 eval '[[ "$(code nested top)" == 37 && "$(code nested pinned)" == 3 ]]' || { echo "top $(code nested top), pinned $(code nested pinned)"; return 1; }
  check test "$(code nested indent_unit)" == 4 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"nested","at":[300,300]}}' >/dev/null; sleep 0.3
  press gg; press ]]; press ]]; check wait_for 5 eval '[[ "$(code nested current)" == 3 ]]' || { echo "]] ]] current $(code nested current)"; return 1; }
  press [[; check wait_for 5 eval '[[ "$(code nested current)" == 2 ]]' || { echo "[[ current $(code nested current)"; return 1; }
  press G; press }; check wait_for 5 eval '[[ "$(code nested current)" == 63 ]]' || { echo "} current $(code nested current)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 80 ]]' || return 1
  check eval 'code src text | grep -q code_pane_edit' || return 1
  check test "$(code src font_size)" == 14.5 || { echo "font after relaunch $(code src font_size)"; return 1; }
}

scenario file_panes M4
file_panes() {
  start_daemon && start_app || return 1
  local doc="$KANNA_STATE_DIR/file_panes.md" src="$KANNA_STATE_DIR/file_panes.rs"
  printf '# file_panes DOC\n\nfile_panes ONE\n' >"$doc"
  printf 'fn one() {}\n' >"$src"
  doc_shows() { raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field text | grep -q "$1"; }
  src_shows() { raw '{"id":1,"cmd":"debug.code","args":{"pane":"src"}}' | field text | grep -q "$1"; }
  watched() { kanna raw '{"cmd":"files"}' | python3 -c 'import json, sys; print(" ".join(sorted(f["kind"] + ":" + f["path"].rsplit("/", 1)[-1] for f in json.load(sys.stdin)["files"])))'; }
  kanna open --window new md --name doc "$doc" >/dev/null || return 1
  kanna open --window win-1 code --name src --tab "$src" >/dev/null || return 1
  check wait_for 5 doc_shows "file_panes ONE" || return 1
  check wait_for 5 src_shows "fn one" || return 1
  check test "$(watched)" == "code:file_panes.rs md:file_panes.md" || { echo "watched: $(watched)"; return 1; }
  printf '# file_panes DOC\n\nfile_panes TWO\n' >"$doc"
  check wait_for 5 doc_shows "file_panes TWO" || return 1
  printf '# file_panes DOC\n\nfile_panes THREE\n' >"$doc.tmp" && mv "$doc.tmp" "$doc"
  check wait_for 5 doc_shows "file_panes THREE" || return 1
  printf 'fn one() {}\nfn two() {}\n' >"$src"
  check wait_for 5 src_shows "fn two" || return 1
  check wait_for 5 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"src\"}}" | field changed)" == 2 ]]' || return 1
  rm "$src"; sleep 0.5
  check src_shows "fn two" || return 1
  printf 'fn three() {}\n' >"$src"
  check wait_for 5 src_shows "fn three" || return 1
  stop_daemon; start_daemon || return 1
  check wait_for 10 app_attached || return 1
  check wait_for 5 eval '[[ "$(watched)" == "code:file_panes.rs md:file_panes.md" ]]' || { echo "after a daemon restart: $(watched)"; return 1; }
  printf '# file_panes DOC\n\nfile_panes FOUR\n' >"$doc"
  check wait_for 5 doc_shows "file_panes FOUR" || return 1
  stop_app; start_app || return 1
  check wait_for 5 src_shows "fn three" || return 1
  printf 'fn four() {}\n' >"$src"
  check wait_for 5 src_shows "fn four" || return 1
  kanna close src >/dev/null || return 1
  check wait_for 5 eval '[[ "$(watched)" == "md:file_panes.md" ]]' || { echo "after close: $(watched)"; return 1; }
  [[ "$(kanna open --window win-1 md "$KANNA_STATE_DIR/file_panes-gone.md" 2>&1)" == *"No such file: $KANNA_STATE_DIR/file_panes-gone.md"* ]] || { echo "file_panes: a missing doc"; return 1; }
}

scenario diff_pane M4
diff_pane() {
  start_daemon && start_app || return 1
  local repo="$KANNA_STATE_DIR/diff_repo"
  mkdir -p "$repo" && cd "$repo" || return 1
  dgit() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null; }
  dgit init -q -b main
  for i in $(seq 40); do echo "fn f$i() -> u32 { $i }"; done >a.rs
  echo 'x' >Cargo.lock
  dgit add . && dgit commit -qm first && dgit checkout -qb task
  sed -i '' 's/fn f5() -> u32 { 5 }/fn f5() -> u32 { 500 }/' a.rs && dgit commit -qam second
  sed -i '' 's/fn f30() -> u32 { 30 }/fn f30() -> u64 { 30 }/' a.rs
  echo 'y' >>Cargo.lock
  printf 'fn new() {}\n' >b.rs
  diffpane() { raw "{\"id\":1,\"cmd\":\"debug.diff\",\"args\":{\"pane\":\"changes\"${2:-}}}" | field "$1"; }
  kanna open --window new diff --merge-base main --name changes >/dev/null || { cd - >/dev/null; return 1; }
  cd - >/dev/null
  check wait_for 5 eval '[[ "$(diffpane files)" == *b.rs* ]]' || { echo "files $(diffpane files)"; return 1; }
  check test "$(diffpane command)" == "git diff --merge-base main" || return 1
  check python3 -c 'import json, sys
files = json.loads(sys.argv[1])
assert [f["path"] for f in files] == ["Cargo.lock", "a.rs", "b.rs"], files
assert files[0]["fold"] == "lock" and files[2]["untracked"] and files[1]["added"] == 2, files' "$(diffpane files)" || return 1
  check python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
kinds = [r["kind"] for r in rows]
assert kinds.count("header") == 3 and "note" in kinds and "fold" in kinds, kinds
assert {"kind": "added", "text": "fn f5() -> u32 { 500 }", "new": 5} in rows, rows' "$(diffpane rows)" || return 1
  check wait_for 5 eval '(( $(diffpane highlighted) >= 2 ))' || { echo "highlighted $(diffpane highlighted)"; return 1; }
  local added
  added="$(diffpane rows | python3 -c 'import json, sys; print([i for i, r in enumerate(json.load(sys.stdin)) if r["kind"] == "added"][0])')"
  check eval '[[ "$(diffpane colors ",\"colors\":$added")" != "[]" ]]' || { echo "no colours on row $added"; return 1; }
  refreshed() { check wait_for 5 eval '[[ "$(diffpane stale)" == True ]]' || { echo "no newer diff: stale $(diffpane stale)"; return 1; }; diffpane stale ',"refresh":true' >/dev/null; }
  local before; before="$(diffpane rows)"
  diffpane current ',"keys":"gn"' >/dev/null
  local reading; reading="$(diffpane current)"
  local row; row="$(diffpane rows | python3 -c 'import json, sys; print(json.dumps(json.load(sys.stdin)[int(sys.argv[1])]))' "$reading")"
  sed -i '' 's/fn f20() -> u32 { 20 }/fn f20() -> u32 { 2000 }/' "$repo/a.rs"
  check wait_for 5 eval '[[ "$(diffpane stale)" == True ]]' || { echo "the edit didn't set changed: stale $(diffpane stale)"; return 1; }
  check test "$(diffpane rows)" == "$before" || { echo "the rows moved while reading"; return 1; }
  check test "$(diffpane current)" == "$reading" || { echo "the current line moved while reading"; return 1; }
  check test "$(diffpane fresh)" == "[]" || { echo "marked fresh before a refresh: $(diffpane fresh)"; return 1; }
  check test "$(diffpane updating)" == False || { echo "updating shown while reading"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-changed.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-changed.png" "$out/" 2>/dev/null
  kanna focus changes >/dev/null
  raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+r"}}' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane rows)" == *"{ 2000 }"* && "$(diffpane stale)" == False ]]' || { echo "⌘R didn't bring the edit in"; return 1; }
  check eval '[[ "$(diffpane fresh)" != "[]" ]]' || { echo "nothing marked fresh"; return 1; }
  check python3 -c 'import json, sys
rows, current = json.loads(sys.argv[1]), int(sys.argv[2])
assert rows[current] == json.loads(sys.argv[3]), (rows[current], sys.argv[3])' "$(diffpane rows)" "$(diffpane current)" "$row" || { echo "the refresh lost the place"; return 1; }
  touch "$repo/a.rs"
  sleep 0.6
  check test "$(diffpane stale)" == False || { echo "changed after a re-diff with nothing new"; return 1; }
  diffpane view ',"keys":"g"' >/dev/null
  raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+f"}}' >/dev/null
  local typed; typed=$(raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"2000"}}')
  check eval '[[ "$typed" == *NSFindPatternFieldEditor* ]]' || { echo "⌘F showed no find field: $typed"; return 1; }
  raw '{"id":1,"cmd":"debug.key","args":{"key":"return"}}' >/dev/null
  raw '{"id":1,"cmd":"debug.key","args":{"key":"escape"}}' >/dev/null
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-unified.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-unified.png" "$out/diff_pane-unified.png" 2>/dev/null
  diffpane current ',"keys":"gn"' >/dev/null
  check python3 -c 'import json, sys
rows, current = json.loads(sys.argv[1]), int(sys.argv[2])
assert rows[current]["kind"] in ("added", "removed") and rows[current]["text"].startswith("fn f5"), rows[current]' "$(diffpane rows)" "$(diffpane current)" || return 1
  diffpane current ',"keys":"]"' >/dev/null
  check test "$(diffpane selected)" == b.rs || { echo "selected $(diffpane selected)"; return 1; }
  diffwin() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"win-1\",\"size\":[$1,700]}}" >/dev/null; }
  diffwin 1400
  check wait_for 5 eval '[[ "$(diffpane list)" == True ]]' || { echo "no files list at $(diffpane width) pt"; return 1; }
  dov() { diffpane "overview.$1" "${2:-}"; }
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[1400,320]}}' >/dev/null
  check wait_for 5 eval '[[ "$(dov shown)" == True && "$(diffpane list)" == True ]]' || { echo "short window: overview $(dov shown) wanted $(dov wanted), list $(diffpane list)"; return 1; }
  check eval '(( $(dov box_height | cut -d. -f1) > 5 ))' || { echo "box $(dov box_height)"; return 1; }
  check python3 -c 'import json, sys
marks = json.loads(sys.argv[1])
assert marks.get("changed", 0) >= 2 and marks.get("added", 0) >= 1, marks' "$(dov marks_by_kind)" || return 1
  dov top ',"overview_click":200' >/dev/null
  check wait_for 5 eval '(( $(dov top | cut -d. -f1) > 0 ))' || { echo "a click in the diff overview left top at $(dov top)"; return 1; }
  dov top ',"overview_drag":[1,-300]' >/dev/null
  check wait_for 5 eval '[[ "$(dov top)" == 0* ]]' || { echo "dragging up left top at $(dov top)"; return 1; }
  dov shown ',"overview_hover":true' >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-overview-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-overview-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-overview-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-overview-dark.png" "$out/" 2>/dev/null
  dov shown ',"overview_hover":false' >/dev/null
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[1000,320]}}' >/dev/null
  check wait_for 5 eval '[[ "$(dov shown)" == True && "$(diffpane list)" == False ]]' || { echo "at 1000 pt the list should go first: overview $(dov shown), list $(diffpane list)"; return 1; }
  diffpane current ',"keys":"G"' >/dev/null
  local scrolled tall
  scrolled="$(diffpane columns.0.y)"; tall="$(diffpane columns.0.text_height)"
  check python3 -c 'import sys; assert float(sys.argv[1]) > 100, sys.argv[1]' "$scrolled" || { echo "G left the short window at y $scrolled"; return 1; }
  raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+f"}}' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane columns.0.inset_top)" != 0 ]]' || { echo "⌘F showed no find bar"; return 1; }
  check test "$(diffpane columns.0.y)" == "$scrolled" || { echo "opening find scrolled the diff from $scrolled to $(diffpane columns.0.y)"; return 1; }
  check test "$(diffpane columns.0.text_height)" == "$tall" || { echo "opening find made the text $(diffpane columns.0.text_height) tall, not $tall"; return 1; }
  diffpane current ',"keys":"gG"' >/dev/null
  check python3 -c 'import sys; y, was, inset = map(float, sys.argv[1:]); assert y == was - inset, (y, was, inset)' "$(diffpane columns.0.y)" "$scrolled" "$(diffpane columns.0.inset_top)" || { echo "with find open the end of the diff is out of reach"; return 1; }
  diffpane current ',"keys":"G"' >/dev/null
  scrolled="$(diffpane columns.0.y)"
  raw '{"id":1,"cmd":"debug.key","args":{"key":"escape"}}' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane columns.0.inset_top)" == 0 ]]' || { echo "Esc left the find bar open"; return 1; }
  check test "$(diffpane columns.0.y)" == "$scrolled" || { echo "closing find scrolled the diff from $scrolled to $(diffpane columns.0.y)"; return 1; }
  check test "$(diffpane columns.0.text_height)" == "$tall" || { echo "closing find made the text $(diffpane columns.0.text_height) tall, not $tall"; return 1; }
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[640,320]}}' >/dev/null
  check wait_for 5 eval '[[ "$(dov shown)" == False && "$(diffpane list)" == False ]]' || { echo "at 640 pt: overview $(dov shown), list $(diffpane list)"; return 1; }
  diffwin 1400
  check wait_for 5 eval '[[ "$(diffpane list)" == True ]]' || { echo "no files list back at $(diffpane width) pt"; return 1; }
  diffpane view ',"zoom":6' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane list)" == False ]]' || { echo "files list zoomed in at $(diffpane width) pt"; return 1; }
  diffpane view ',"zoom":0' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane list)" == True ]]' || { echo "no files list back from zoom"; return 1; }
  diffwin 960
  check wait_for 5 eval '[[ "$(diffpane list)" == False ]]' || { echo "files list at $(diffpane width) pt"; return 1; }
  diffpane current ',"keys":"["' >/dev/null
  check test "$(diffpane selected)" == a.rs || { echo "without the list, [ selected $(diffpane selected)"; return 1; }
  diffpane current ',"keys":"]"' >/dev/null
  check test "$(diffpane selected)" == b.rs || { echo "without the list, ] selected $(diffpane selected)"; return 1; }
  diffwin 1400
  check python3 -c 'import json, sys
rows, current = json.loads(sys.argv[1]), int(sys.argv[2])
assert rows[current]["text"] == "fn new() {}", rows[current]' "$(diffpane rows)" "$(diffpane current)" || return 1
  diffpane current ',"keys":"~"' >/dev/null
  check wait_for 5 eval 'kanna workspace list --all --json | grep -q "diff_repo/b.rs"' || { echo "no code pane for b.rs"; kanna workspace list --all; return 1; }
  kanna focus changes >/dev/null
  diffpane view ',"keys":"s"' >/dev/null
  check wait_for 5 is changes view split || { echo "view $(pane changes view)"; return 1; }
  check eval '[[ "$(diffpane rows)" == *"\"right\": \"blank\""* || "$(diffpane rows)" == *"\"left\": \"blank\""* ]]' || { echo "no blank side in split"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane.png" "$out/diff_pane.png" 2>/dev/null
  diffpane view ',"fold":"a.rs"' >/dev/null
  check python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
assert not any(r["text"].startswith("fn f5") for r in rows), "a.rs still open"' "$(diffpane rows)" || return 1
  diffpane view ',"fold":"a.rs"' >/dev/null
  diffpane view ',"zoom":12' >/dev/null
  diffpane view ',"keys":"gG"' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane sticky)" != None ]]' || { echo "no sticky header at the bottom: current $(diffpane current) of $(diffpane rows | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')"; raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-sticky.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-sticky.png" "$out/" 2>/dev/null; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-sticky.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-sticky.png" "$out/" 2>/dev/null
  check python3 -c 'import json, sys
for c in json.loads(sys.argv[1]): assert c["x"] == -c["inset_left"] and c["inset_left"] == c["gutter"], c' "$(diffpane columns)" || return 1
  diffpane view ',"zoom":0' >/dev/null
  printf 'one\r\ntwo\xe2\x80\xa8three\r\n' >"$repo/crlf.txt"
  refreshed || return 1
  check wait_for 5 eval '[[ "$(diffpane files)" == *crlf.txt* ]]' || { echo "crlf.txt didn't arrive"; return 1; }
  check python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
assert {"kind": "added", "text": "one\r", "new": 1, "left": "blank", "right": "added"} in rows, [r for r in rows if r.get("new") == 1]' "$(diffpane rows)" || return 1
  printf 'fn later() {}\n' >"$repo/c.rs"
  refreshed || return 1
  check wait_for 5 eval '[[ "$(diffpane files)" == *c.rs* ]]' || { echo "the untracked c.rs didn't arrive"; return 1; }
  watchers() { raw '{"id":1,"cmd":"diffs"}' | python3 -c 'import json, sys; print(" ".join(json.dumps(d["args"]) + ":" + str(len(d["views"])) for d in json.load(sys.stdin)["diffs"]))'; }
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "watchers: $(watchers)"; return 1; }
  (cd "$repo" && kanna open --window win-1 diff --merge-base main --name changes2 >/dev/null) || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:2" ]]' || { echo "two views, watchers: $(watchers)"; return 1; }
  kanna close changes2 >/dev/null || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "after closing one view, watchers: $(watchers)"; return 1; }
  stop_daemon; start_daemon || return 1
  check wait_for 10 app_attached || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "after a daemon restart, watchers: $(watchers)"; return 1; }
  printf 'fn restarted() {}\n' >"$repo/d.rs"
  refreshed || return 1
  check wait_for 5 eval '[[ "$(diffpane files)" == *d.rs* ]]' || { echo "d.rs didn't arrive after a daemon restart: $(diffpane files)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(diffpane view)" == split && "$(diffpane files)" == *c.rs* ]]' || { echo "after relaunch: $(diffpane view) $(diffpane files) $(diffpane error) $(watchers)"; return 1; }
  before="$(diffpane files)"
  mv "$repo/.git" "$repo/.git-away"
  check wait_for 5 eval '[[ "$(diffpane notice)" == *"not a git repository"* ]]' || { echo "git's error while reading: notice $(diffpane notice), error $(diffpane error)"; mv "$repo/.git-away" "$repo/.git"; return 1; }
  check test "$(diffpane files)" == "$before" || { echo "git's error replaced the diff being read"; mv "$repo/.git-away" "$repo/.git"; return 1; }
  check test "$(diffpane error)" == None || { echo "error $(diffpane error)"; mv "$repo/.git-away" "$repo/.git"; return 1; }
  mv "$repo/.git-away" "$repo/.git"
  check wait_for 5 eval '[[ "$(diffpane notice)" == None && "$(diffpane stale)" == False ]]' || { echo "git back: notice $(diffpane notice), stale $(diffpane stale)"; return 1; }
  kanna navigate changes diff --staged >/dev/null || return 1
  check wait_for 5 eval '[[ "$(diffpane command)" == "git diff --staged" && "$(diffpane files)" == "[]" ]]' || { echo "staged: $(diffpane command) $(diffpane files)"; return 1; }
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--staged\"]:1" ]]' || { echo "after navigating, watchers: $(watchers)"; return 1; }
  kanna navigate changes diff --no-such-flag >/dev/null || return 1
  check wait_for 5 eval '[[ "$(diffpane error)" == *no-such-flag* ]]' || { echo "a failed diff shows no error: $(diffpane error)"; return 1; }
  desk_task diffagent 'exec sleep 600' || return 1
  (cd "$repo" && KANNA_TERMINAL=diffagent kanna open diff --merge-base main --name deskdiff >/dev/null) || return 1
  deskdiff() { raw '{"id":1,"cmd":"debug.diff","args":{"pane":"deskdiff"}}' | field "$1"; }
  kanna task activate diffagent >/dev/null || return 1
  local agent; agent="$(pane diffagent id)"
  deskwin() { app_view | python3 -c 'import json, sys; print(next((w["id"] for w in json.load(sys.stdin)["windows"] if (w.get("dock") or {}).get("pane") == sys.argv[1]), ""))' "$agent"; }
  check wait_for 5 eval '[[ -n "$(deskwin)" ]]' || { echo "the agent's Desk isn't shown: $(app_view)"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$(deskwin)\",\"size\":[1728,1000]}}" >/dev/null
  check wait_for 5 eval '[[ "$(deskdiff width)" == 1036* && "$(deskdiff files)" == *b.rs* ]]' || { echo "beside the agent at 2/5 in 1728 pt: $(deskdiff width) pt"; return 1; }
  check test "$(deskdiff list)" == False || { echo "files list beside the agent at $(deskdiff width) pt"; return 1; }
  kanna resize diffagent 1/5 >/dev/null || return 1
  check wait_for 5 eval '[[ "$(deskdiff list)" == True ]]' || { echo "no files list beside the agent at 1/5: $(deskdiff width) pt"; return 1; }
}

scenario diff_pane_worktree_gone M4
diff_pane_worktree_gone() {
  start_daemon && start_app || return 1
  local repo="$KANNA_STATE_DIR/gone_repo" tree="$KANNA_STATE_DIR/gone_tree"
  mkdir -p "$repo" && cd "$repo" || return 1
  dgit() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null; }
  dgit init -q -b main
  for i in $(seq 200); do echo "fn f$i() -> u32 { $i }"; done >a.rs
  dgit add . && dgit commit -qm first && dgit worktree add -q -b task "$tree"
  cd - >/dev/null
  for i in $(seq 200); do echo "fn f$i() -> u64 { $i }"; done >"$tree/a.rs"
  gone() { raw "{\"id\":1,\"cmd\":\"debug.diff\",\"args\":{\"pane\":\"gone\"${2:-}}}" | field "$1"; }
  (cd "$tree" && kanna open --window new diff --merge-base main --name gone >/dev/null) || return 1
  check wait_for 5 eval '[[ "$(gone files)" == *a.rs* ]]' || { echo "files $(gone files) error $(gone error)"; return 1; }
  gone view ',"zoom":-40' >/dev/null
  check wait_for 5 eval '[[ "$(gone zoom)" == 0.72 ]]' || { echo "zoomed out to $(gone zoom)"; return 1; }
  gone view ',"keys":"G"' >/dev/null
  check wait_for 5 eval '[[ "$(gone sticky)" == a.rs ]]' || { echo "no sticky header zoomed out: $(gone sticky)"; return 1; }
  git -C "$repo" worktree remove --force "$tree" || return 1
  check wait_for 5 eval '[[ "$(gone notice)" != None || "$(gone error)" != None ]]' || { echo "the removed worktree went unnoticed: notice $(gone notice), error $(gone error)"; return 1; }
  for keys in g G g G; do gone view ",\"keys\":\"$keys\"" >/dev/null; done
  gone view ',"zoom":-1' >/dev/null
  check app_attached || { echo "the app died after its worktree went"; return 1; }
  check test "$(gone sticky)" == a.rs || { echo "sticky $(gone sticky) after the worktree went"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane_worktree_gone.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane_worktree_gone.png" "$out/" 2>/dev/null
}

scenario log_pane M4
log_pane() {
  start_daemon && start_app || return 1
  local repo; repo="$(e2e_repo logs)" || return 1
  lgit() { git -C "$repo" -c user.name=Ann -c user.email=ann@example.invalid -c commit.gpgsign=false "$@" >/dev/null; }
  lc() { echo "$2" >>"$repo/$1" && lgit add "$1" && lgit commit -qm "$2"; }
  lc a.txt "main 0"
  lgit checkout -qb long; for i in 1 2 3 4 5; do lc long.txt "long $i"; done
  lgit checkout -q main; lc a.txt "main 1"; lgit merge -q --no-ff long -m "Merge long"
  for b in x y z; do lgit checkout -qb "$b" main~1; lc "$b.txt" "$b 1"; done
  lgit checkout -q main; lgit merge -q --no-ff x y z -m Octopus
  lgit checkout -qb lived; lc l.txt "lived 1"
  lgit checkout -q main; lc a.txt "main 2"
  lgit checkout -q lived; lgit merge -q --no-ff main -m "Merge branch 'main' into lived"; lc l.txt "lived 2"
  lgit checkout -q main
  for i in 3 4 5 6 7 8 9; do lc a.txt "main $i"; done
  lgit checkout -q lived; lgit merge -q --no-ff main -m "Merge main into lived again"; lgit checkout -q main
  desk_task "Labelled task" 'exec sleep 600' logs || return 1
  local tree; tree="$(ls -d "$repo"/.kanna-worktrees/task-* | head -1)"
  check wait_for 10 test -d "$tree" || { echo "no worktree for the task"; return 1; }
  git -C "$tree" branch -m labelled >/dev/null 2>&1
  git -C "$tree" -c user.name=Bo -c user.email=bo@example.invalid -c commit.gpgsign=false commit -q --allow-empty -m "task work" || return 1
  logpane() { raw "{\"id\":1,\"cmd\":\"debug.log\",\"args\":{\"pane\":\"history\"${2:-}}}" | field "$1"; }
  rowsof() { logpane rows | python3 -c "import json, sys; rows = json.load(sys.stdin); $1"; }
  hash_of() { git -C "$repo" log --all --format='%H %s' | awk -v s="$1" '{h=$1; $1=""; if (substr($0, 2) == s) { print h; exit }}'; }
  (cd "$repo" && kanna open --window new log --name history >/dev/null) || return 1
  check wait_for 10 eval '[[ "$(logpane total)" == 22 ]]' || { echo "total $(logpane total), error $(logpane error), range $(logpane range)"; return 1; }
  check test "$(logpane range)" == '["labelled", "main"]' || { echo "range $(logpane range)"; return 1; }
  check rowsof '
by = {r["subject"]: r for r in rows}
assert "long 1" not in by, "the long branch should fold"
m = by["Merge long"]; assert m["folded"] and m["fold"] == 5 and m["lane"] == 0, m
assert all(r["lane"] == 0 for r in rows if r["spine"]), [r for r in rows if r["spine"] and r["lane"]]
o = by["Octopus"]; assert len(o["parents"]) == 4 and len(set(o["parents"])) == 4, o
t = by["task work"]; assert t["lane"] > 0 and t["labels"][0]["name"] == "Labelled task" and t["labels"][0]["place"] == "desk" and t["labels"][0]["ahead"] == 1, t
assert "lived 1" not in by, "lived is not in the default range"' || return 1
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"log_pane.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/log_pane.png" "$out/" 2>/dev/null
  kanna navigate history log main lived >/dev/null || return 1
  check wait_for 10 eval '[[ "$(logpane command)" == "git log main lived" && "$(logpane total)" == 25 ]]' || { echo "navigate: $(logpane command) $(logpane total)"; return 1; }
  check rowsof '
by = {r["subject"]: r for r in rows}
assert by["lived 1"]["lane"] > 0 and len(by["Merge branch '"'"'main'"'"' into lived"]["parents"]) == 2, by["lived 1"]' || return 1
  local merge; merge="$(hash_of "Merge long")"
  logpane open ",\"fold\":\"$merge\"" >/dev/null
  check rowsof '
by = {r["subject"]: r for r in rows}
assert len(rows) == 25 and by["Merge long"]["open"] and by["long 3"]["lane"] > 0, len(rows)' || return 1
  logpane open ",\"select\":\"$(hash_of "long 3")\",\"keys\":\"h\"" >/dev/null
  check test "$(logpane selected)" == "$merge" || { echo "← from inside the side selected $(logpane selected)"; return 1; }
  check rowsof 'assert len(rows) == 20, len(rows)' || return 1
  logpane selected ',"keys":"g"' >/dev/null
  local top; top="$(logpane selected)"
  check wait_for 5 eval '[[ "$(logpane detail)" == "$top" ]]' || { echo "detail $(logpane detail) for $top"; return 1; }
  logpane selected ',"keys":"jj"' >/dev/null
  local reading; reading="$(logpane selected)"
  local before; before="$(logpane rows)"
  lgit checkout -q lived; lc l.txt "lived 3"; lgit checkout -q main
  check wait_for 5 eval '[[ "$(logpane stale)" == True ]]' || { echo "the commit didn't hold: stale $(logpane stale)"; return 1; }
  check test "$(logpane held)" == "1 new commit" || { echo "held $(logpane held)"; return 1; }
  check test "$(logpane rows)" == "$before" || { echo "the rows moved while reading"; return 1; }
  check test "$(logpane selected)" == "$reading" || { echo "the selection moved while reading"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"log_pane-held.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/log_pane-held.png" "$out/" 2>/dev/null
  kanna focus history >/dev/null
  raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+r"}}' >/dev/null
  check wait_for 5 eval '[[ "$(logpane stale)" == False && "$(logpane rows)" == *"lived 3"* ]]' || { echo "⌘R didn't bring the commit in"; return 1; }
  check test "$(logpane selected)" == "$reading" || { echo "the refresh lost the selection"; return 1; }
  check eval '[[ "$(logpane fresh)" == *"$(hash_of "lived 3")"* ]]' || { echo "fresh $(logpane fresh)"; return 1; }
  logpane hits ',"find":"lived"' >/dev/null
  check eval '[[ "$(logpane hits | python3 -c "import json,sys; print(len(json.load(sys.stdin)))")" == 5 ]]' || { echo "hits $(logpane hits)"; return 1; }
  logpane hits ',"find":"long 2"' >/dev/null
  logpane hits ',"next":true' >/dev/null
  check test "$(logpane selected)" == "$(hash_of "long 2")" || { echo "find didn't go to long 2: $(logpane selected)"; return 1; }
  check eval '[[ "$(logpane open)" == *"$merge"* ]]' || { echo "find didn't unfold its merge: $(logpane open)"; return 1; }
  logpane hits ',"find":""' >/dev/null
  logpane selected ",\"select\":\"$(hash_of "lived 2")\",\"keys\":\"~\"" >/dev/null
  check wait_for 5 eval 'kanna workspace list --all --json | grep -q "$(hash_of "lived 2" | cut -c1-12)^!"' || { echo "↩ opened no diff of lived 2"; kanna workspace list --all; return 1; }
  kanna focus history >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"log_pane-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/log_pane-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"log_pane-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/log_pane-dark.png" "$out/" 2>/dev/null
  logs() { raw '{"id":1,"cmd":"logs"}' | python3 -c 'import json, sys; print(" ".join(json.dumps(d["args"]) + ":" + str(len(d["views"])) for d in json.load(sys.stdin)["logs"]))'; }
  check wait_for 5 eval '[[ "$(logs)" == "[\"main\", \"lived\"]:1" ]]' || { echo "logs: $(logs)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 10 eval '[[ "$(logpane total)" == 26 && "$(logpane command)" == "git log main lived" ]]' || { echo "after relaunch: $(logpane total) $(logpane command) $(logpane error)"; return 1; }
  kanna navigate history log no-such-branch >/dev/null || return 1
  check wait_for 5 eval '[[ "$(logpane error)" == *no-such-branch* ]]' || { echo "a bad range shows no error: $(logpane error)"; return 1; }
}

scenario code_tour M4
code_tour() {
  start_daemon && start_app || return 1
  local tour="$KANNA_STATE_DIR/code_tour.rs" other="$KANNA_STATE_DIR/code_tour-other.rs"
  { echo 'fn outer() {'; echo '    loop {'; echo '        if ready() {'; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; } >"$tour"
  for i in $(seq 30); do echo "let line_$i = $i;"; done >"$other"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | field "$2"; }
  local said id
  said="$(kanna comment --window new "$tour:3" "Waits until it's ready.")" || return 1
  [[ "$said" == "opened file-"*" · code_tour.rs:3–61" ]] || { echo "said: $said"; return 1; }
  id="$(awk '{print $2}' <<<"$said")"
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 3 && "$(code $id comment_last)" == 61 ]]' || return 1
  check wait_for 5 eval '[[ "$(code $id comment_text)" == "Waits until it'"'"'s ready." ]]' || return 1
  check eval '(( $(code $id comment_gap) > 20 ))' || { echo "gap $(code $id comment_gap)"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_box_visible)" == 1 ]]' || { echo "box at $(code $id comment_box_visible)"; return 1; }
  said="$(kanna comment --window win-1 "$tour:40-42" "Each step.")" || return 1
  [[ "$said" == "$id · code_tour.rs:40–42" ]] || { echo "again: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 40 && "$(code $id comment_last)" == 42 ]]' || return 1
  check wait_for 5 eval '[[ "$(code $id comment_box_visible)" == 1 ]]' || return 1
  said="$(kanna comment --window win-1 "$other:7" "Line seven.")" || return 1
  [[ "$said" == "opened file-"*" · code_tour-other.rs:7" && "$said" != *" $id "* ]] || { echo "other: $said"; return 1; }
  [[ "$(kanna comment --window win-1 "$KANNA_STATE_DIR/gone.rs:3" "Nothing." 2>&1)" == *"No such file: $KANNA_STATE_DIR/gone.rs"* ]] || { echo "code_tour: a missing file"; return 1; }
  [[ "$(kanna comment --window win-1 "$other:99" "Past the end." 2>&1)" == *"has 30 lines"* ]] || { echo "code_tour: past the end"; return 1; }
  local doc="$KANNA_STATE_DIR/code_tour.md" agent md
  printf '# Tabs\n\nA tab container holds **several** panes\nand shows one at a time.\n\n- Panes only: it holds panes.\n- One active: the rest keep running.\n' >"$doc"
  agent="$(kanna open --window win-1 --name code_touragent --cmd cat | awk '{print $2}')"
  said="$(KANNA_TERMINAL=$agent kanna comment "$doc:4" "The key idea: one pane shows at a time.")" || return 1
  [[ "$said" == "c1 · code_tour.md:3 in file-"* ]] || { echo "doc: $said"; return 1; }
  md="${said##* in }"
  mdf() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$md\"}}" | field "$1"; }
  check wait_for 5 eval '[[ "$(mdf threads.0.messages.0)" == "agent: The key idea: one pane shows at a time." ]]' || return 1
  check wait_for 5 eval '[[ "$(mdf markup)" == True && "$(mdf agent)" == "$agent" ]]' || { echo "markup $(mdf markup) agent $(mdf agent)"; return 1; }
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$md\",\"mark\":\"c1\"}}" | grep -q "\"y\""' || { echo "c1 has no mark"; return 1; }
  said="$(KANNA_TERMINAL=$agent kanna comment "$doc:7" "The others keep running.")" || return 1
  [[ "$said" == "c2 · code_tour.md:7 in $md" ]] || { echo "doc again: $said"; return 1; }
  check wait_for 5 eval '[[ "$(mdf active)" == c2 ]]' || { echo "active $(mdf active)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 40 && "$(code $id comment_text)" == "Each step." ]]' || return 1
}

scenario code_tour_steps M4
code_tour_steps() {
  start_daemon && start_app || return 1
  local tour="$KANNA_STATE_DIR/code_tour_steps.rs" other="$KANNA_STATE_DIR/code_tour_steps-other.rs" said id more agent
  { echo 'fn outer() {'; echo '    loop {'; echo '        if ready() {'; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; } >"$tour"
  for i in $(seq 30); do echo "let line_$i = $i;"; done >"$other"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"${3:-}}}" | field "$2"; }
  press() { code "$1" pressed ",\"tour\":\"$2\""; }
  agent="$(kanna open --window new --name agent --cmd cat | awk '{print $2}')"
  [[ "$(kanna tour "$tour:3" "One." 2>&1)" == *"run it from the agent's terminal"* ]] || { echo "code_tour_steps: a tour without an agent"; return 1; }
  [[ "$(KANNA_TERMINAL=$agent kanna tour "$tour:3" "One." "$other:99" "Past." 2>&1)" == *"has 30 lines"* ]] || { echo "code_tour_steps: a bad stop"; return 1; }
  [[ "$(KANNA_TERMINAL=$agent kanna tour)" == "no tour" ]] || { echo "code_tour_steps: a refused tour was kept"; return 1; }
  said="$(KANNA_TERMINAL=$agent kanna tour "$tour:3" "One." "$tour:40-42" "Two." "$other:7" "Three.")" || return 1
  [[ "$said" == "tour of 3 · 1 of 3 · opened file-"*" · code_tour_steps.rs:3–61" ]] || { echo "said: $said"; return 1; }
  id="$(grep -o 'file-[0-9]*' <<<"$said")"
  check wait_for 5 eval '[[ "$(code $id tour.at)" == 1 && "$(code $id tour.of)" == 3 && "$(code $id tour.back)" == False && "$(code $id tour.next)" == "Next ›" ]]' || { echo "footer $(code $id tour)"; return 1; }
  check wait_for 5 eval '[[ "$(code $id tour.visible)" == True ]]' || return 1
  [[ "$(press $id back)" == False ]] || { echo "code_tour_steps: Back on the first stop"; return 1; }
  [[ "$(press $id next)" == True ]] || return 1
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 40 && "$(code $id tour.at)" == 2 && "$(code $id tour.back)" == True ]]' || { echo "stop 2: $(code $id tour)"; return 1; }
  sleep 0.5
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"code_tour_steps.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/code_tour_steps.png" "$out/code_tour_steps.png" 2>/dev/null
  press $id next >/dev/null
  check wait_for 5 eval '[[ "$(KANNA_TERMINAL=$agent kanna tour | head -1)" == "at 3 of 3 · "*"code_tour_steps-other.rs:7" ]]' || { KANNA_TERMINAL=$agent kanna tour; return 1; }
  more="$(kanna workspace list --all --json | python3 -c 'import json, sys
print(next(p["id"] for p in json.load(sys.stdin)["panes"] if (p.get("path") or "").endswith("-other.rs")))')"
  check wait_for 5 eval '[[ "$(code $more tour.at)" == 3 && "$(code $more tour.next)" == Done ]]' || { echo "stop 3: $(code $more tour)"; return 1; }
  check wait_for 5 eval '[[ "$(code $id tour)" == None && "$(code $id comment_first)" == 40 ]]' || { echo "the last stop kept its footer: $(code $id tour)"; return 1; }
  said="$(KANNA_TERMINAL=$agent kanna tour add --next "$tour:20-22" "An answer.")" || return 1
  [[ "$said" == "added 1 · 4 of 4 · $id · code_tour_steps.rs:20–22" ]] || { echo "added: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 20 && "$(code $id tour.at)" == 4 && "$(code $id tour.next)" == Done ]]' || return 1
  check wait_for 5 eval '[[ "$(code $more tour)" == None ]]' || return 1
  said="$(KANNA_TERMINAL=$agent kanna tour add "$other:12" "Later.")" || return 1
  [[ "$said" == "added 1 · at 4 of 5" ]] || { echo "added later: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id tour.of)" == 5 && "$(code $id tour.next)" == "Next ›" ]]' || return 1
  press $id back >/dev/null
  check wait_for 5 eval '[[ "$(code $more tour.at)" == 3 && "$(code $more tour.of)" == 5 ]]' || return 1
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(code $more tour.at)" == 3 && "$(code $more comment_first)" == 7 ]]' || { echo "after a relaunch: $(code $more tour)"; return 1; }
  press $more next >/dev/null
  check wait_for 5 eval '[[ "$(code $id tour.at)" == 4 ]]' || return 1
  press $id next >/dev/null
  check wait_for 5 eval '[[ "$(code $more tour.at)" == 5 && "$(code $more comment_first)" == 12 ]]' || return 1
  press $more next >/dev/null
  check wait_for 5 eval '[[ "$(KANNA_TERMINAL=$agent kanna tour)" == "no tour" && "$(code $more tour)" == None && "$(code $more comment_first)" == 12 ]]' || { echo "done: $(code $more tour)"; return 1; }
  local doc="$KANNA_STATE_DIR/code_tour_steps.md" md
  printf '# Trip\n\nThe keeper queues\nthe message.\n\n- It sends.\n' >"$doc"
  mdf() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$md\"${2:-}}}" | field "$1"; }
  said="$(KANNA_TERMINAL=$agent kanna tour "$doc:4" "The whole trip." "$tour:3" "Then the code.")" || return 1
  [[ "$said" == "tour of 2 · 1 of 2 · opened file-"*" · code_tour_steps.md:3 · c1" ]] || { echo "doc: $said"; return 1; }
  md="$(grep -o 'file-[0-9]*' <<<"$said")"
  check wait_for 5 eval '[[ "$(mdf tour.thread)" == c1 && "$(mdf tour.at)" == 1 && "$(mdf tour.next)" == "Next ›" && "$(mdf tour.back)" == False ]]' || { echo "doc footer: $(mdf tour)"; return 1; }
  check wait_for 5 eval '[[ "$(mdf active)" == c1 && "$(mdf threads.0.messages.0)" == "agent: The whole trip." && "$(mdf markup)" == True ]]' || { echo "doc thread: $(mdf active) $(mdf threads)"; return 1; }
  sleep 0.5
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"code_tour_steps-md.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/code_tour_steps-md.png" "$out/code_tour_steps-md.png" 2>/dev/null
  [[ "$(mdf pressed ',"tour":"next"')" == True ]] || return 1
  check wait_for 5 eval '[[ "$(code $id tour.at)" == 2 && "$(code $id tour.next)" == Done && "$(code $id comment_first)" == 3 ]]' || { echo "after the doc: $(code $id tour)"; return 1; }
  check wait_for 5 eval '[[ "$(mdf tour)" == None ]]' || { echo "the doc kept its footer: $(mdf tour)"; return 1; }
  press $id back >/dev/null
  check wait_for 5 eval '[[ "$(mdf tour.thread)" == c1 && "$(mdf tour.at)" == 1 && "$(mdf active)" == c1 ]]' || { echo "back to the doc: $(mdf tour)"; return 1; }
  [[ "$(mdf threads | python3 -c 'import json, sys; print(len(json.load(sys.stdin)))')" == 1 ]] || { echo "code_tour_steps: going back wrote another thread"; return 1; }
  mdf pressed ',"tour":"next"' >/dev/null
  check wait_for 5 eval '[[ "$(code $id tour.at)" == 2 ]]' || return 1
  press $id next >/dev/null
  check wait_for 5 eval '[[ "$(KANNA_TERMINAL=$agent kanna tour)" == "no tour" && "$(code $id tour)" == None ]]' || return 1
  [[ -z "$(kanna read "$agent" | tr -d '[:space:]')" ]] || { echo "code_tour_steps: stepping sent the agent: $(kanna read "$agent")"; return 1; }
}

scenario code_pinned_line M4
code_pinned_line() {
  start_daemon && start_app || return 1
  local file="$KANNA_STATE_DIR/code_pinned_line.rs"
  nested() { echo 'fn outer() {'; echo '    loop {'; echo "        if $1() {"; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; }
  nested ready >"$file"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | field "$2"; }
  key() { raw "{\"id\":1,\"cmd\":\"debug.key\",\"args\":{\"key\":\"$1\"}}" >/dev/null; }
  kanna open --window new code --name src "$file" >/dev/null || return 1
  check wait_for 5 eval '(( $(code src scopes) == 3 ))' || { echo "scopes $(code src scopes)"; return 1; }
  nested ready_now >"$file.new" && mv "$file.new" "$file"
  check wait_for 5 eval '[[ "$(code src changed)" == 3 ]]' || { echo "changed $(code src changed)"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"src","at":[300,300]}}' >/dev/null; sleep 0.3
  key cmd+f
  check wait_for 5 eval '[[ "$(code src find_bar)" == 1 ]]' || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"ready"}}' >/dev/null
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","select":[3,11,5]}}' >/dev/null
  check wait_for 5 eval '[[ "$(code src selection_line)" == 3 && "$(code src selection_length)" == 5 ]]' || return 1
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","pick":40}}' >/dev/null
  check wait_for 5 eval '[[ "$(code src pinned)" == 3 && "$(code src pinned_rows.2.line)" == 3 ]]' || { echo "pinned $(code src pinned_rows)"; return 1; }
  check wait_for 5 eval '[[ "$(code src pinned_rows.2.matches)" == "[[11, 5]]" ]]' || { echo "matches $(code src pinned_rows)"; return 1; }
  check test "$(code src pinned_rows.2.selection)" == "[[11, 5]]" || { echo "selection $(code src pinned_rows)"; return 1; }
  check test "$(code src pinned_rows.2.changed)" == True || { echo "changed $(code src pinned_rows)"; return 1; }
  for row in 0 1; do
    check test "$(code src pinned_rows.$row.selection)$(code src pinned_rows.$row.matches)$(code src pinned_rows.$row.changed)" == "[][]False" || { echo "row $row $(code src pinned_rows)"; return 1; }
  done
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","select":[2,4,4]}}' >/dev/null
  check wait_for 5 eval '[[ "$(code src pinned_rows.1.selection)" == "[[4, 4]]" && "$(code src pinned_rows.2.selection)" == "[]" ]]' || { echo "reselect $(code src pinned_rows)"; return 1; }
  local scrolled; scrolled="$(code src scroll_y)"
  key escape
  check wait_for 5 eval '[[ "$(code src find_bar)" == 0 ]]' || return 1
  check wait_for 5 eval '[[ "$(code src pinned_rows.2.matches)" == "[]" ]]' || { echo "after find $(code src pinned_rows)"; return 1; }
  check test "$(code src scroll_y)" == "$scrolled" || { echo "closing find scrolled the view from $scrolled to $(code src scroll_y)"; return 1; }
}

scenario serial_pane M4
serial_pane() {
  local devices="$KANNA_STATE_DIR/serial-devices"
  serial_device=""
  mkdir -p "$devices"
  export KANNA_SERIAL_PORTS="$devices/ports.json"
  plug() {
    : >"$devices/out"
    python3 "$repo_root/tools/fake-serial/fake-serial.py" "$devices" >"$devices/out" 2>&1 &
    serial_device=$!
    wait_for 5 test -s "$devices/out"
  }
  trap 'stop_pid "$serial_device"' EXIT
  plug || { echo "serial_pane: the fake device didn't start"; return 1; }
  start_daemon && start_app || return 1
  ser() { raw "{\"id\":1,\"cmd\":\"debug.serial\",\"args\":{\"pane\":\"esp\"${2:-}}}" | field "$1"; }
  kanna open --window new serial 303a:1001 --name esp >/dev/null || return 1
  check is esp type serial || return 1
  check wait_for 5 eval '[[ "$(ser state)" == connected && "$(ser send)" == True ]]' || { echo "serial_pane: $(ser state)"; return 1; }
  check test "$(pane esp title)" == "USB JTAG/serial debug unit · $(basename "$(cat "$devices/out")")" || { echo "serial_pane: title $(pane esp title)"; return 1; }
  kanna serial send esp reboot >/dev/null || return 1
  check wait_for 5 eval 'ser text | grep -q "app: ready, free heap"' || { echo "serial_pane: no boot in the view: $(ser text | tail -3)"; return 1; }
  ser state ',"type":"wifi status"' >/dev/null
  check wait_for 5 eval 'kanna serial read esp --last 3 | grep -q "rssi -58"' || { echo "serial_pane: the send line didn't reach the device"; return 1; }
  check test "$(ser history)" == '["wifi status"]' || { echo "serial_pane: history $(ser history)"; return 1; }
  check wait_for 5 eval 'ser text | grep -q "→ wifi status"' || return 1
  check wait_for 5 eval 'ser status | grep -q "ESP-IDF log levels"' || { echo "serial_pane: status $(ser status)"; return 1; }
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"serial_pane-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/serial_pane-dark.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"serial_pane-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/serial_pane-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  kanna serial send esp binary >/dev/null || return 1
  check wait_for 5 eval 'ser text | grep -qF "\\x00\\x01\\x02\\xfe\\xffraw"' || { echo "serial_pane: bytes that aren't text: $(ser text | tail -2)"; return 1; }
  check test "$(ser hex ',"hex":true')" == True || return 1
  check wait_for 5 eval 'ser text | grep -q "fe ff 72 61 77"' || { echo "serial_pane: hex view: $(ser text | tail -3)"; return 1; }
  ser hex ',"hex":false' >/dev/null
  check test "$(ser paused ',"pause":true')" == True || return 1
  kanna serial send esp crash >/dev/null || return 1
  check wait_for 5 eval '[[ "$(ser unseen)" -gt 0 ]]' || { echo "serial_pane: paused, but no new lines counted"; return 1; }
  check test "$(ser following ',"follow":true')" == True || return 1
  check wait_for 5 eval 'ser text | grep -q "Guru Meditation"' || return 1
  stop_pid "$serial_device"; serial_device=""
  check wait_for 5 eval '[[ "$(ser state)" == unplugged && "$(ser send)" == False ]]' || { echo "serial_pane: unplugging left $(ser state)"; return 1; }
  check wait_for 5 eval 'ser status | grep -q "unplugged · waiting for 303a:1001"' || { echo "serial_pane: status $(ser status)"; return 1; }
  plug || return 1
  check wait_for 10 eval '[[ "$(ser state)" == connected ]]' || { echo "serial_pane: plugging back in left $(ser state)"; return 1; }
  check test "$(ser path)" == "$(cat "$devices/out")" || { echo "serial_pane: back on $(ser path)"; return 1; }
  check wait_for 5 eval 'ser text | grep -q "── back "' || return 1
  local rows; rows="$(ser rows)"
  stop_app && start_app || return 1
  check wait_for 10 eval '[[ "$(ser rows)" -ge "$rows" ]]' || { echo "serial_pane: after a relaunch the view has $(ser rows) rows, not $rows"; return 1; }
  kanna close esp >/dev/null || return 1
  check wait_for 5 eval 'kanna serial list | grep -q "serial-" && false || true' || return 1
  unset KANNA_SERIAL_PORTS
}
