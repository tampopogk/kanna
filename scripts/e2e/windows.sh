scenario windows_tabs_ui M4
windows_tabs_ui() {
  start_daemon && start_app || return 1
  kanna open --window new --name k1 >/dev/null || return 1
  key cmd+d; key cmd+shift+d
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "k1", "size": "1/2"}, {"children": [{"pane": "term-2", "size": "1/2"}, {"pane": "term-3", "size": "1/2"}], "size": "1/2", "split": "column"}], "split": "row"}' || return 1
  check test "$(win win-1 focused)" == term-3 || return 1
  key "cmd+]"; check test "$(win win-1 focused)" == k1 || return 1
  key "cmd+["; check test "$(win win-1 focused)" == term-3 || return 1
  key cmd+shift+return; check test "$(win win-1 zoomed)" == term-3 || return 1
  key cmd+t; check wait_for 5 eval '[[ "$(win win-1 tabs.1.layout)" == *term-4* ]]' || return 1
  check test "$(win win-1 zoomed)" == term-4 || return 1
  key "cmd+shift+["; check test "$(win win-1 zoomed)" == term-3 || return 1
  kanna close term-4 >/dev/null || return 1
  check test "$(win win-1 zoomed)" == term-3 || return 1
  key "cmd+["; check test "$(win win-1 zoomed)" == term-2 || return 1
  check test "$(win win-1 focused)" == term-2 || return 1
  key "cmd+]"; check test "$(win win-1 zoomed)" == term-3 || return 1
  drew win-1 windows_tabs_ui-zoom-term-3 1 || return 1
  check test "$(field tabBar.shown <<<"$shot")" == True || { echo "the tab bar should show while a pane is zoomed"; return 1; }
  key "cmd+]"; check test "$(win win-1 zoomed)" == k1 || return 1
  drew win-1 windows_tabs_ui-zoom-k1 1 || return 1
  key cmd+shift+return; check test "$(win win-1 zoomed)" == None || return 1
  key "cmd+["; check test "$(win win-1 focused)" == term-3 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"k1"}}' >/dev/null; sleep 0.3
  check test "$(win win-1 focused)" == k1 || return 1
  sleep 1
  drew win-1 windows_tabs_ui-splits 3 || return 1
  printf '# windows_tabs_ui DOC\n\nclick me\n' >"$KANNA_STATE_DIR/windows_tabs_ui.md"
  kanna open --window win-1 md --name doc --split right "$KANNA_STATE_DIR/windows_tabs_ui.md" >/dev/null || return 1
  key "cmd+["; check test "$(win win-1 focused)" != doc || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"doc","at":[4,200]}}' >/dev/null; sleep 0.3
  check test "$(win win-1 focused)" == doc || return 1
  mdzoom() { raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field zoom; }
  local width_before; width_before="$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field layout_width)"
  key cmd+shift+=; check python3 -c 'import sys; assert float(sys.argv[1]) > 1' "$(mdzoom)" || return 1
  check python3 -c 'import sys; assert float(sys.argv[2]) < float(sys.argv[1]), sys.argv[1:]' "$width_before" "$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field layout_width)" || { echo "⌘+ didn't re-wrap"; return 1; }
  key cmd+0; check test "$(mdzoom)" == 1 || return 1
  local pinched
  pinched="$(raw '{"id":1,"cmd":"debug.pinch","args":{"pane":"doc"}}')"
  check python3 -c 'import json, sys; z = json.loads(sys.argv[1])["zooms"]; assert z and z[-1] > 1.2, z' "$pinched" || return 1
  check test "$(raw '{"id":1,"cmd":"debug.stats"}' | field pid)" == "$app_pid" || return 1
  kanna close doc >/dev/null || return 1
  printf '<title>windows_tabs_ui PAGE</title><p>windows_tabs_ui WEB</p>\n' >"$KANNA_STATE_DIR/windows_tabs_ui.html"
  kanna open --window win-1 web --name page --split right "file://$KANNA_STATE_DIR/windows_tabs_ui.html" >/dev/null || return 1
  webzoom() { raw '{"id":1,"cmd":"debug.web","args":{"pane":"page"}}' | field "$1"; }
  check wait_for 5 eval 'webzoom text | grep -q "windows_tabs_ui WEB"' || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"page","at":[4,200]}}' >/dev/null; sleep 0.3
  check test "$(win win-1 focused)" == page || return 1
  key cmd+shift+=; check python3 -c 'import sys; assert float(sys.argv[1]) > 1' "$(webzoom zoom)" || return 1
  key cmd+-; check python3 -c 'import sys; assert float(sys.argv[1]) == 1' "$(webzoom zoom)" || return 1
  pinched="$(raw '{"id":1,"cmd":"debug.pinch","args":{"pane":"page"}}')"
  check python3 -c 'import json, sys; z = json.loads(sys.argv[1])["zooms"]; assert z and z[-1] > 1.2, z' "$pinched" || return 1
  key cmd+0; check python3 -c 'import sys; assert float(sys.argv[1]) == 1' "$(webzoom zoom)" || return 1
  key cmd+-; check python3 -c 'import sys; assert float(sys.argv[1]) < 1' "$(webzoom zoom)" || return 1
  key cmd+0
  kanna close page >/dev/null || return 1
  key cmd+t; check test "$(win win-1 tabs | count)" == 2 || return 1
  key "cmd+shift+["; check test "$(win win-1 tabs.0.active)" == True || return 1
  key "cmd+shift+]"; check test "$(win win-1 tabs.1.active)" == True || return 1
  local tab2
  tab2="$(win win-1 tabs.1.id)"
  click_tab "$tab2" 2
  for k in w e b return; do key "$k"; done
  check test "$(win win-1 tabs.1.title)" == web || return 1
  click_tab "$tab2" 2
  for k in x escape; do key "$k"; done
  check test "$(win win-1 tabs.1.title)" == web || return 1
  click_tab "$(win win-1 tabs.0.id)"; check test "$(win win-1 tabs.0.active)" == True || return 1
  click_tab "$tab2"
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"New Window"}}' >/dev/null; sleep 0.3
  check test "$(state | field windows | count)" == 2 || return 1
  check test "$(raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-2"}}' | field desk)" == False || return 1
  check test "$(dwin win-2 key)" == True || return 1
  sleep 1
  drew win-2 windows_tabs_ui-second-window 1 || return 1
  key 'cmd+`'; check test "$(dwin win-1 key)" == True || return 1
  minimized() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"win-2\"${1:-}}}" | field minimized; }
  minimized ',"minimize":true' >/dev/null; check wait_for 5 eval '[[ "$(minimized)" == True ]]' || return 1
  key 'cmd+`'; check test "$(minimized)" == True || return 1
  check test "$(dwin win-1 key)" == True || return 1
  key 'cmd+shift+`'; check test "$(minimized)" == True || return 1
  check test "$(dwin win-1 key)" == True || return 1
  minimized ',"minimize":false' >/dev/null; check wait_for 5 eval '[[ "$(minimized)" == False ]]' || return 1
  key cmd+w; check test "$(win win-1 tabs | count)" == 1 || return 1
  key cmd+shift+w; check test "$(state | field windows | count)" == 1 || return 1
  local menus
  menus="$(raw '{"id":1,"cmd":"debug.menu"}')"
  for item in "Split Right" "Split Down" "New Web Pane Right" "New Markdown Pane Below…" "Open URL…" "Back" "Forward" "Home" "Next Pane" \
              "Previous Pane" "Zoom" "Move Pane to New Window" "Reload" "Close Pane" "New Tab" "Next Tab" "Previous Tab" "Zoom In" "Zoom Out" \
              "Actual Size" "New Window" "Close Window" "Move Tab to New Window" "Next Window" "Previous Window" "Undo" "Cut" "Copy" "Paste" \
              "Select All" "New Desk" "Quit $(/usr/libexec/PlistBuddy -c "Print CFBundleName" "${app%/Contents/MacOS/Kanna}/Contents/Info.plist")"; do
    [[ "$menus" == *"\"item\":\"$item\""* ]] || { echo "no menu item \"$item\""; return 1; }
  done
  [[ "$menus" == *'"item":"Split Right","menu":"Pane","shortcut":"⌘d"'* ]] || { echo "Split Right isn't ⌘D: $menus"; return 1; }
  [[ "$menus" == *'"item":"New Web Pane Right","menu":"New Pane"'* ]] || { echo "New Web Pane Right isn't under New Pane: $menus"; return 1; }
  [[ "$(field items <<<"$menus" | python3 -c 'import json, sys; print(",".join(i["item"] for i in json.load(sys.stdin) if i["menu"] == "Pane"))')" == \
     "New Pane,Split Right,Split Down,Next Pane,Previous Pane,Zoom,Move Pane to New Window,Close Pane,Open URL…,Back,Forward,Reload,Comments,Home" ]] \
    || { echo "the Pane menu's order: $menus"; return 1; }
  [[ "$menus" == *'"item":"New Desk","menu":"File","shortcut":"⌘n"'* ]] || { echo "New Desk isn't File ⌘N: $menus"; return 1; }
  [[ "$menus" == *'"item":"New Window","menu":"Window","shortcut":""'* ]] || { echo "New Window has a shortcut: $menus"; return 1; }
  [[ "$menus" != *'"New Instance"'* && "$menus" != *'"Start a Task"'* ]] || { echo "a removed menu item is back: $menus"; return 1; }
}

scenario tab_names M4
tab_names() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR"
  printf 'fn main() {}\n' >"$dir/tab_names.rs"
  printf '<title>tab_names PAGE</title><p>tab_names</p>\n' >"$dir/tab_names.html"
  printf '<title>tab_names NEXT</title><p>next</p>\n' >"$dir/tab_names_next.html"
  kanna open --window new >/dev/null || return 1
  check test "$(win win-1 tabs.0.title)" == "Tab 1" || return 1
  kanna open --window win-1 code "$dir/tab_names.rs" --tab >/dev/null || return 1
  check test "$(win win-1 tabs.1.title)" == tab_names.rs || return 1
  [[ "$(kanna workspace list --all)" == *'"tab_names.rs"'* ]] || { echo "workspace list doesn't show the tab as tab_names.rs"; kanna workspace list --all; return 1; }
  kanna open --window win-1 web --name page "file://$dir/tab_names.html" --tab >/dev/null || return 1
  check test "$(win win-1 tabs.2.title)" == page || return 1
  kanna rename-pane page "" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(win win-1 tabs.2.title)" == "tab_names PAGE" ]]' || { win win-1 tabs; return 1; }
  kanna navigate page-1 "file://$dir/tab_names_next.html" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(win win-1 tabs.2.title)" == "tab_names NEXT" ]]' || { win win-1 tabs; return 1; }
  shown_tabs() { raw '{"id":1,"cmd":"debug.snapshot","args":{"window":"win-1"}}' | field tabs; }
  check test "$(shown_tabs)" == '["Tab 1", "tab_names.rs", "tab_names NEXT"]' || { shown_tabs; return 1; }
  kanna rename-tab "$(win win-1 tabs.2.id)" mine >/dev/null || return 1
  kanna navigate page-1 "file://$dir/tab_names.html" >/dev/null || return 1
  sleep 1
  check test "$(win win-1 tabs.2.title)" == mine || return 1
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(win win-1 tabs.2.title)" == mine && "$(win win-1 tabs.1.title)" == tab_names.rs ]]' || { win win-1 tabs; return 1; }
}

scenario resize_arrange M4
resize_arrange() {
  start_daemon && start_app || return 1
  kanna open --window new --name da >/dev/null && kanna open --window win-1 --name db --split right >/dev/null || return 1
  drag '{"divider":"da","at":0.26}'
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "da", "size": "1/4"}, {"pane": "db", "size": "3/4"}], "split": "row"}' || { echo "$last_drag"; return 1; }
  drag '{"divider":"da","at":0.6}'
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "da", "size": "3/5"}, {"pane": "db", "size": "2/5"}], "split": "row"}' || return 1
  drag '{"divider":"da","at":0.01}'
  local share
  share="$(win win-1 tabs.0.layout.children.0.size)"
  check python3 -c 'import sys; s = float(sys.argv[1].rstrip("%")); assert 5 < s < 12, s' "$share" || return 1
  drag '{"divider":"da","at":0.25}'
  kanna open --window win-1 --name wide >/dev/null || return 1
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "da", "size": "1/4"}, {"pane": "db", "size": "3/8"}, {"pane": "wide", "size": "3/8"}], "split": "row"}' || return 1
  kanna focus da >/dev/null && kanna open --window win-1 --name tall >/dev/null || return 1
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"children": [{"pane": "da", "size": "1/2"}, {"pane": "tall", "size": "1/2"}], "size": "1/4", "split": "column"}, {"pane": "db", "size": "3/8"}, {"pane": "wide", "size": "3/8"}], "split": "row"}' || return 1
  key cmd+alt+right; check test "$(win win-1 focused)" == db || return 1
  key cmd+alt+left; check test "$(win win-1 focused)" == tall || return 1
  local before after
  size() { raw "{\"id\":1,\"cmd\":\"debug.text\",\"args\":{\"pane\":\"$1\"}}" | field columns; }
  before="$(size db)"
  kanna focus db >/dev/null
  key cmd+=; key cmd+shift+=
  after="$(size db)"
  check test "${after:-0}" -lt "${before:-0}" -a "${after:-0}" -gt 0 || { echo "columns $before → $after"; return 1; }
  key cmd+0
  check test "$(size db)" == "$before" || return 1
  sleep 1
  drew win-1 resize_arrange-resized 4 || return 1
  kanna open --window win-1 --name ended --cmd 'exit 3' --tab >/dev/null || return 1
  check wait_for 5 is ended state exited || return 1
  sleep 0.5
  check test "$(snap win-1 resize_arrange-exited | field panes.0.notice)" == "exited (3)" || return 1
  kanna close ended >/dev/null || return 1
  drag "{\"pane\":\"wide\",\"to\":{\"outside\":true}}"
  check test "$(state | field windows | count)" == 2 || { echo "$last_drag"; return 1; }
  sleep 1
  local frames
  frames() { python3 -c 'import json, sys; print(json.dumps(sorted([k, v] for k, v in json.load(open(sys.argv[1]))["frames"].items())))' "$KANNA_STATE_DIR/app-state.json"; }
  frames="$(frames)"
  check python3 -c 'import json, sys; f = json.loads(sys.argv[1]); assert len(f) == 2 and f[0][1] != f[1][1], f' "$frames" || return 1
  stop_app; start_app || return 1
  sleep 1
  stop_app
  check test "$(frames | field 0)" == "$(field 0 <<<"$frames")" || return 1
  frames="$(frames)"
  start_app || return 1
  sleep 1
  stop_app
  check test "$(frames)" == "$frames" || return 1
  start_app || return 1
  kanna close wide >/dev/null || return 1
  check test "$(state | field windows | count)" == 1 || return 1
  key cmd+shift+w
  check test "$(state | field windows | count)" == 0 || return 1
  check app_attached || return 1
  check test "$(kanna workspace list --all --json | field panes)" == "[]"
}

scenario move_to_window M4
move_to_window() {
  start_daemon && start_app || return 1
  kanna open --window new --name doc >/dev/null && kanna open --window win-1 --name mine --split right >/dev/null || return 1
  kanna open --name user --window new >/dev/null && kanna open --window win-2 --name open --split right >/dev/null || return 1
  kanna move mine --window win-2 >/dev/null || return 1
  check test "$(win win-2 tabs.0.layout)" == '{"children": [{"pane": "user", "size": "1/2"}, {"pane": "open", "size": "1/2"}], "split": "row"}' || return 1
  check test "$(win win-2 tabs.1.layout)" == '{"pane": "mine"}' || return 1
  check test "$(win win-2 tabs.1.active)" == True || return 1
  check test "$(win win-2 focused)" == mine || return 1
  check test "$(win win-1 tabs.0.layout)" == '{"pane": "doc"}' || return 1
  kanna move mine --window win-2 >/dev/null || return 1
  check test "$(win win-2 tabs | count)" == 2 || return 1
  kanna move open --window win-2 >/dev/null || return 1
  check test "$(win win-2 tabs.0.layout)" == '{"pane": "user"}' || return 1
  check test "$(win win-2 tabs.2.layout)" == '{"pane": "open"}'
}

scenario drag_panes_tabs M4
drag_panes_tabs() {
  start_daemon && start_app || return 1
  kanna open --window new --name da >/dev/null && kanna open --window win-1 --name db --split right >/dev/null || return 1
  drag '{"pane":"da","to":{"pane":"db","x":0.5,"y":0.95}}'
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "db", "size": "1/2"}, {"pane": "da", "size": "1/2"}], "split": "column"}' || { echo "$last_drag"; return 1; }
  drag '{"pane":"da","to":{"pane":"db","x":0.5,"y":0.5}}'
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "da", "size": "1/2"}, {"pane": "db", "size": "1/2"}], "split": "column"}' || return 1
  drag '{"pane":"db","to":{"pane":"da","x":0.02,"y":0.5}}'
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "db", "size": "1/2"}, {"pane": "da", "size": "1/2"}], "split": "row"}' || return 1
  drag '{"pane":"da","to":{"plus":"win-1"}}'
  check test "$(win win-1 tabs.1.layout)" == '{"pane": "da"}' || { echo "$(win win-1 tabs) $last_drag"; return 1; }
  local first second
  first="$(win win-1 tabs.0.id)" second="$(win win-1 tabs.1.id)"
  drag "{\"pane\":\"da\",\"to\":{\"tab\":\"$first\"}}"
  check test "$(win win-1 tabs | count)" == 1 || return 1
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "db", "size": "1/2"}, {"pane": "da", "size": "1/2"}], "split": "row"}' || return 1
  kanna open --window win-1 --name dc --tab >/dev/null || return 1
  first="$(win win-1 tabs.0.id)" second="$(win win-1 tabs.1.id)"
  drag "{\"tab\":\"$second\",\"to\":{\"tab\":\"$first\"}}"
  check test "$(win win-1 tabs.0.id)" == "$second" || { echo "$(win win-1 tabs) $last_drag"; return 1; }
  drag "{\"tab\":\"$second\",\"to\":{\"outside\":true}}"
  check test "$(state | field windows | count)" == 2 || { echo "$last_drag"; return 1; }
  drag '{"pane":"dc","to":{"outside":true}}'
  check test "$(state | field windows | count)" == 2 || return 1
  drag '{"pane":"da","to":{"outside":true}}'
  check test "$(state | field windows | count)" == 3 || return 1
  local menu
  menu="$(raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"tab\":\"$(win win-1 tabs.0.id)\",\"button\":1}}")"
  check test "$(field menu <<<"$menu")" == '["Move Tab to New Window", "Close Tab"]' || return 1
  menu="$(raw '{"id":1,"cmd":"debug.click","args":{"pane":"db","button":1}}')"
  for item in "Copy" "Paste" "New Pane" "Split Right" "Close Pane" "Move Pane to New Window" "Reload"; do
    [[ "$menu" == *"\"$item\""* ]] || { echo "the pane menu has no \"$item\": $menu"; return 1; }
  done
  sleep 1
  local w
  for w in $(state | python3 -c 'import json, sys; print(" ".join(w["id"] for w in json.load(sys.stdin)["windows"]))'); do
    drew "$w" "drag_panes_tabs-$w" 1 || return 1
  done
}

lum() { python3 -c 'import sys
r, g, b = bytes.fromhex(sys.argv[1].lstrip("#"))
print(int(0.2126 * r + 0.7152 * g + 0.0722 * b))' "$1"; }
tabbar() { raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$1.png\"}}" | field tabBar.background; cp "$KANNA_STATE_DIR/$1.png" "$out/$1.png" 2>/dev/null; }

scenario tab_drag M4
tab_drag() {
  start_daemon && start_app || return 1
  kanna open --window new --name ta >/dev/null && kanna open --window win-1 --name tb --tab >/dev/null && kanna open --window win-1 --name tc --tab >/dev/null || return 1
  kanna open --window new --name td >/dev/null || return 1
  local a b c d dropped
  a="$(win win-1 tabs.0.id)" b="$(win win-1 tabs.1.id)" c="$(win win-1 tabs.2.id)" d="$(win win-2 tabs.0.id)"
  dropped() { python3 -c 'import json, sys; print(json.loads(sys.argv[1].split(" → ", 1)[1])["dropped"])' "$last_drag"; }
  drag "{\"tab\":\"$a\",\"to\":{\"tab\":\"$c\"}}"
  check test "$(tab_ids win-1)" == "$b $c $a" || { echo "$(tab_ids win-1) $last_drag"; return 1; }
  drag "{\"tab\":\"$a\",\"to\":{\"tab\":\"$b\"}}"
  check test "$(tab_ids win-1)" == "$a $b $c" || { echo "$last_drag"; return 1; }
  drag "{\"tab\":\"$b\",\"to\":{\"tab\":\"$b\"}}"
  check test "$(dropped)" == False || { echo "$last_drag"; return 1; }
  check test "$(tab_ids win-1)" == "$a $b $c" || return 1
  drag "{\"tab\":\"$a\",\"to\":{\"tab\":\"$c\"},\"cancel\":true}"
  check test "$(dropped)" == False || { echo "$last_drag"; return 1; }
  check test "$(tab_ids win-1)" == "$a $b $c" || return 1
  drag "{\"tab\":\"$c\",\"to\":{\"tab\":\"$d\"}}"
  check test "$(tab_ids win-2)" == "$c $d" || { echo "$last_drag"; return 1; }
  check test "$(tab_ids win-1)" == "$a $b" || return 1
  drag "{\"tab\":\"$b\",\"to\":{\"plus\":\"win-2\"}}"
  check test "$(tab_ids win-2)" == "$c $d $b" || { echo "$last_drag"; return 1; }
  check test "$(win win-2 tabs.2.active)" == True || return 1
  drag "{\"tab\":\"$d\",\"to\":{\"outside\":true}}"
  check test "$(state | field windows | count)" == 3 || { echo "$last_drag"; return 1; }
  check test "$(tab_ids win-2)" == "$c $b" || return 1
  check test "$(state | python3 -c 'import json, sys, re; print(" ".join(" ".join(t["id"] for t in w["tabs"]) for w in json.load(sys.stdin)["windows"] if w["id"] not in ("win-1", "win-2")))')" == "$d" || return 1
  drag "{\"tab\":\"$a\",\"to\":{\"tab\":\"$d\"}}"
  check test "$(state | field windows | count)" == 2 || { echo "$last_drag"; return 1; }
  drag "{\"tab\":\"$c\",\"to\":{\"pane\":\"ta\",\"x\":0.5,\"y\":0.5}}"
  check test "$(dropped)" == True || { echo "$last_drag"; return 1; }
  check test "$(state | field windows | count)" == 3 || { echo "$last_drag"; return 1; }
  check test "$(tab_ids win-2)" == "$b" || return 1
  local frame holder torn
  frame() { raw '{"id":1,"cmd":"debug.app"}' | python3 -c 'import json, sys; print(json.load(sys.stdin)["frames"].get(sys.argv[1]))' "$1"; }
  holder() { state | python3 -c 'import json, sys; print([w["id"] for w in json.load(sys.stdin)["windows"] if sys.argv[1] in [t["id"] for t in w["tabs"]]][0])' "$1"; }
  holder="$(holder "$d")"
  drag "{\"tab\":\"$a\",\"to\":{\"pane\":\"tb\",\"x\":0.5,\"y\":0.5},\"fill\":true}"
  check test "$(state | field windows | count)" == 4 || { echo "$last_drag"; return 1; }
  torn="$(holder "$a")"
  check test "$torn" != "$holder" || return 1
  check test "$(frame "$torn")" != None || return 1
  check test "$(frame "$torn")" == "$(frame "$holder")" || { echo "$(frame "$torn") vs $(frame "$holder")"; return 1; }
}
tab_ids() { win "$1" tabs | python3 -c 'import json, sys; print(" ".join(t["id"] for t in json.load(sys.stdin)))'; }

scenario chrome_and_drops M4
chrome_and_drops() {
  start_daemon && start_app || return 1
  kanna open --window new --name chrome_and_drops --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows chrome_and_drops READY || return 1
  local shade n=0
  for look in dark light dark; do
    n=$((n + 1))
    terminal background "{\"appearance\":\"$look\"}" >/dev/null 2>&1
    sleep 0.5
    shade="$(lum "$(tabbar "chrome_and_drops-$n-$look")")"
    if [[ $look == dark ]]; then check test "$shade" -lt 96 || return 1; else check test "$shade" -gt 160 || return 1; fi
  done
  local file="$KANNA_STATE_DIR/Screen Shot (1).png"
  printf '\x89PNG\r\n\x1a\n' >"$file"
  raw "{\"id\":1,\"cmd\":\"debug.drop\",\"args\":{\"pane\":\"chrome_and_drops\",\"files\":[\"$file\",\"/tmp\"]}}" >/dev/null
  check wait_for 5 eval 'kanna read chrome_and_drops | tr -d "\\n" | sed "s/IN //g" | grep -qF "^[200~${KANNA_STATE_DIR}/Screen\\ Shot\\ \\(1\\).png /tmp^[201~"' || { kanna read chrome_and_drops; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.drop\",\"args\":{\"pane\":\"chrome_and_drops\",\"image\":\"$file\"}}" >/dev/null
  check wait_for 5 eval 'kanna read chrome_and_drops | tr -d "\\n" | sed "s/IN //g" | grep -qE "kanna-drops/image-[0-9-]+-[0-9A-F]{8}\\.png\\^\\[201~"' || { kanna read chrome_and_drops; return 1; }
  local dropped
  dropped="$(kanna read chrome_and_drops | tr -d "\n" | sed "s/IN //g" | grep -oE "/[^~]*kanna-drops/image-[0-9-]+-[0-9A-F]{8}\.png" | tail -1)"
  check cmp -s "$file" "$dropped" || return 1
  raw "{\"id\":1,\"cmd\":\"debug.drop\",\"args\":{\"pane\":\"chrome_and_drops\",\"files\":[\"/tmp/a\\nb\"]}}" >/dev/null
  check wait_for 5 eval 'kanna read chrome_and_drops | tr -d "\\n" | sed "s/IN //g" | grep -qF "^[200~\$'"'"'/tmp/a\\x0ab'"'"'^[201~"' || { kanna read chrome_and_drops; return 1; }
  raw '{"id":1,"cmd":"debug.drop","args":{"pane":"chrome_and_drops","text":"chrome_and_drops TEXT"}}' >/dev/null
  check wait_for 5 eval 'kanna read chrome_and_drops | tr -d "\\n" | sed "s/IN //g" | grep -qF "^[200~chrome_and_drops TEXT^[201~"'
}

scenario pane_cycle_keyboard M4
pane_cycle_keyboard() {
  start_daemon && start_app || return 1
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  holds() { local id; id="$(pane "$1" id)"; [[ "$(dwin d1 focused)" == "$id" && "$(desk keyboard)" == "$id" && "$(desk highlighted)" == "$([[ "$(desk key)" == True ]] && echo "[\"$id\"]" || echo "[]")" ]]; }
  cycle() {
    key "$1"
    check wait_for 5 holds "$2" || { echo "pane_cycle_keyboard: $1 to $2: focused $(dwin d1 focused), keyboard $(desk keyboard) ($(desk responder)), highlighted $(desk highlighted)"; return 1; }
    [[ "$(pane "$2" type)" == term ]] || return 0
    local typed="typed-$RANDOM"
    raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$typed\\n\"}}" >/dev/null
    check wait_for 5 app_text "$2" "$typed" || { kanna read "$2"; return 1; }
  }
  printf '# Cycle\n\nA markdown pane.\n' >"$KANNA_STATE_DIR/cycle.md"
  desk_task agent 'exec cat' || return 1
  new_task waiting >/dev/null || return 1
  KANNA_TERMINAL=agent kanna open --name left --cmd 'exec cat' >/dev/null || return 1
  kanna task activate agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$(pane agent id)" ]]' || { app_view; return 1; }
  kanna focus left >/dev/null || return 1
  KANNA_TERMINAL=left kanna open md "$KANNA_STATE_DIR/cycle.md" --name doc --split right >/dev/null || return 1
  kanna focus doc >/dev/null || return 1
  KANNA_TERMINAL=left kanna open code "$KANNA_STATE_DIR/cycle.md" --name src --split down >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs.0.panes)" == "[\"$(pane left id)\", \"$(pane doc id)\", \"$(pane src id)\"]" ]]' || { app_view; return 1; }
  kanna focus agent >/dev/null || return 1
  check wait_for 5 holds agent || return 1
  cycle "cmd+]" left && cycle "cmd+]" doc && cycle "cmd+]" src && cycle "cmd+]" agent \
    && cycle "cmd+[" src && cycle "cmd+[" doc && cycle "cmd+[" left && cycle "cmd+[" agent || return 1
  click_tab "$(dwin d1 tabs.0.id)" 2
  check wait_for 5 eval '[[ "$(desk responder)" == NSTextView ]]' || { echo "pane_cycle_keyboard: no tab rename field"; desk responder; return 1; }
  kanna focus left >/dev/null || return 1
  sleep 0.5
  check test "$(desk responder)" == NSTextView || { echo "pane_cycle_keyboard: kanna focus took the keyboard from the tab rename"; return 1; }
  cycle "cmd+]" doc && cycle "cmd+]" src && cycle "cmd+]" agent || return 1
  key cmd+.
  check wait_for 5 eval '[[ "$(desk empty)" == True && "$(desk search.shown)" == True ]]' || { desk search; return 1; }
  key cmd+f
  check wait_for 5 eval '[[ "$(desk search.focused)" == True ]]' || { echo "pane_cycle_keyboard: no Desk view search"; return 1; }
  key cmd+.
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$(pane agent id)" ]]' || { echo "pane_cycle_keyboard: ⌘. didn't show the active task"; return 1; }
  cycle "cmd+]" left && cycle "cmd+[" agent
}

scenario pane_sizes M4
pane_sizes() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" want
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "pane_sizes line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  sizer() {
    cat >"$KANNA_STATE_DIR/$1" <<PY
#!/usr/bin/env python3
import os, signal, time
def draw(*_):
    cols, rows = os.get_terminal_size(0)
    open("$KANNA_STATE_DIR/$1.sizes", "a").write(f"{rows}x{cols}\n")
    os.write(1, f"\033[?1049h\033[2J\033[H{rows}x{cols}".encode())
signal.signal(signal.SIGWINCH, draw)
draw()
while True: time.sleep(1)
PY
    chmod +x "$KANNA_STATE_DIR/$1"
  }
  sizer docked && sizer shelved && sizer onshelf
  saw() { sort -u "$KANNA_STATE_DIR/$1.sizes" 2>/dev/null | xargs; }
  view() { echo "$(terminal rows "{\"pane\":\"$1\"}")x$(terminal cols "{\"pane\":\"$1\"}")"; }
  only() { [[ "$(saw docked)" == "$1" && "$(saw shelved)" == "$1" ]]; }
  start_daemon && start_app || { echo "pane_sizes line $LINENO"; return 1; }
  desk_task docked "exec $KANNA_STATE_DIR/docked" repo && kanna task activate docked >/dev/null || { echo "pane_sizes line $LINENO"; return 1; }
  (cd "$repo" && kanna task new Shelved "go" --agent "$KANNA_STATE_DIR/shelved") >/dev/null && kanna task pick $(tid 2) >/dev/null || { echo "pane_sizes line $LINENO"; return 1; }
  check wait_for 10 eval '[[ -s "$KANNA_STATE_DIR/docked.sizes" && -s "$KANNA_STATE_DIR/shelved.sizes" ]]' || { echo "pane_sizes: the programs didn't start"; return 1; }
  sleep 1; want="$(view docked)"
  [[ "$want" =~ ^[0-9]+x[0-9]+$ && "$want" != 24x80 ]] || { echo "pane_sizes: the docked pane is $want"; return 1; }
  check only "$want" || { echo "pane_sizes: the agents saw $(saw docked) and $(saw shelved), not only the Desk's $want"; return 1; }
  (cd "$repo" && kanna task new Backlogged "go" --agent "$KANNA_STATE_DIR/onshelf") >/dev/null && kanna task start $(tid 3) >/dev/null || { echo "pane_sizes line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(tail -1 "$KANNA_STATE_DIR/onshelf.sizes" 2>/dev/null)" == "$want" ]]' || { echo "pane_sizes: the agent running on the Shelf is $(tail -1 "$KANNA_STATE_DIR/onshelf.sizes" 2>/dev/null), not the Desk's $want"; return 1; }
  stop_app; start_app || { echo "pane_sizes line $LINENO"; return 1; }
  sleep 2
  check only "$want" || { echo "pane_sizes: after a relaunch the agents saw $(saw docked) and $(saw shelved), not only $want"; return 1; }
  stop_daemon; start_daemon || { echo "pane_sizes line $LINENO"; return 1; }
  check wait_for 10 app_attached && sleep 2 || { echo "pane_sizes line $LINENO"; return 1; }
  check only "$want" || { echo "pane_sizes: after a daemon restart the agents saw $(saw docked) and $(saw shelved), not only $want"; return 1; }
  stop_app; stop_daemon; start_daemon || { echo "pane_sizes line $LINENO"; return 1; }
  kanna restart Shelved >/dev/null || { echo "pane_sizes line $LINENO"; return 1; }
  check wait_for 5 is Shelved state running && sleep 1 || { echo "pane_sizes line $LINENO"; return 1; }
  check only "$want" || { echo "pane_sizes: started again with no app, the Shelf's agent saw $(saw shelved), not $want"; return 1; }
  start_app && sleep 2 || { echo "pane_sizes line $LINENO"; return 1; }
  check only "$want" || { echo "pane_sizes: back with the app, the agents saw $(saw docked) and $(saw shelved), not only $want"; return 1; }
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1","size":[1100,700]}}' >/dev/null; sleep 1
  local wider; wider="$(view docked)"
  [[ "$wider" != "$want" ]] || { echo "pane_sizes: a bigger window left the docked pane $want"; return 1; }
  last() { tail -1 "$KANNA_STATE_DIR/$1.sizes"; }
  check wait_for 5 eval '[[ "$(last docked):$(last shelved):$(last onshelf)" == "$wider:$wider:$wider" ]]' || { echo "pane_sizes: in a bigger window the agents are $(last docked), $(last shelved) and $(last onshelf), not $wider"; return 1; }
  kanna task activate Shelved >/dev/null && sleep 1 || { echo "pane_sizes line $LINENO"; return 1; }
  check test "$(view Shelved)" == "$wider" || { echo "pane_sizes: pulled onto the Desk, the agent's view is $(view Shelved), not $wider"; return 1; }
  check test "$(tail -1 "$KANNA_STATE_DIR/shelved.sizes")" == "$wider" || return 1
}

scenario toasts M4
toasts() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "toasts line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "toasts line $LINENO"; return 1; }
  check test "$(kanna toast "Early" "raised before the app")" == "raised toast-1; no app is attached, so it shows when one attaches within a minute" || return 1
  (cd "$repo" && kanna task new "Lint fixes" "hello 1" --agent "$agent" && kanna task new "Docs pass" "hello 2" --agent "$agent") >/dev/null || { echo "toasts line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null || { echo "toasts line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  toasted() { raw '{"id":1,"cmd":"debug.toasts"}' | python3 -c 'import json, sys; r = json.load(sys.stdin); print(r["window"], " ".join(t["id"] for t in r["toasts"]))'; }
  toast_field() { raw '{"id":1,"cmd":"debug.toasts"}' | python3 -c 'import json, sys; r = json.load(sys.stdin); t = next(t for t in r["toasts"] if t["id"] == sys.argv[1]); print(t[sys.argv[2]])' "$1" "$2"; }
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  check wait_for 10 eval '[[ "$(toasted)" == "d1 toast-1" ]]' || { echo "toasts: the toast raised before the app isn't shown in d1: $(toasted)"; return 1; }
  check test "$(kanna toast "Build finished" "scripts/build.sh took 3m 12s")" == "raised toast-2" || return 1
  check wait_for 5 eval '[[ "$(toasted)" == "d1 toast-2 toast-1" ]]' || { echo "toasts: the newest isn't on top: $(toasted)"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts"}' | python3 -c 'import json, sys
r = json.load(sys.stdin); a, b = (t["frame"] for t in r["toasts"])
sys.exit(not (a[1] > b[1] and abs(a[0] - b[0]) < 1 and a[2] == 340))' || { echo "toasts: the stack isn't one column, newest above"; raw '{"id":1,"cmd":"debug.toasts"}'; return 1; }
  check test "$(kanna toast "Messages wait for Lint fixes" "mock can't take messages" --task "$(tid 1)" --key messages:lint:mock)" == "raised toast-3" || return 1
  check test "$(kanna toast "Messages wait for Lint fixes" "mock can't take messages" --task "$(tid 1)" --key messages:lint:mock)" == "already raised a toast with key messages:lint:mock" || return 1
  check wait_for 5 eval '[[ "$(toasted)" == "d1 toast-3 toast-2 toast-1" ]]' || { echo "toasts: the keyed toast: $(toasted)"; return 1; }
  check test "$(toast_field toast-3 about)" == "Lint fixes" || return 1
  check test "$(toast_field toast-3 pane)" == "$(pane "Lint fixes" id)" || { echo "toasts: the task's toast isn't about its agent"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"toasts.png"}}' >/dev/null && cp "$KANNA_STATE_DIR/toasts.png" "$out/toasts.png"
  check test "$(desk empty)" == True || { echo "toasts: the Desk isn't empty before the click"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"click":"toast-3"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk title)" == *" — Lint fixes"* ]]' || { echo "toasts: clicking the toast didn't bring Lint fixes onto the Desk: $(desk title)"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 focused)" == "$(pane "Lint fixes" id)" ]]' || { echo "toasts: the task's agent isn't focused"; return 1; }
  check wait_for 5 eval '[[ "$(toasted)" == "d1 toast-2 toast-1" ]]' || { echo "toasts: the clicked toast stayed: $(toasted)"; return 1; }
  check test "$(kanna toast "Docs pass waits" --task "$(tid 2)")" == "raised toast-4" || return 1
  check wait_for 5 eval '[[ "$(toasted)" == "d1 toast-4"* ]]' || return 1
  raw '{"id":1,"cmd":"debug.toasts","args":{"click":"toast-4"}}' >/dev/null
  check wait_for 15 eval '[[ "$(desk title)" == *" — Docs pass"* ]]' || { echo "toasts: clicking a Shelf task's toast didn't shelve and show it: $(desk title)"; desk note; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 focused)" == "$(pane "Docs pass" id)" ]]' || { echo "toasts: the Shelf task's agent isn't focused"; return 1; }
  kanna toast "Kept" "the pointer is on it" >/dev/null && kanna toast "Fades" >/dev/null || { echo "toasts line $LINENO"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"hover":"toast-5"}}' >/dev/null
  check wait_for 12 eval '[[ "$(toasted)" == "d1 toast-5" ]]' || { echo "toasts: after 6s only the hovered toast should be left: $(toasted)"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"hover":"toast-5","on":false}}' >/dev/null
  check wait_for 5 eval '[[ "$(toasted)" == "d1 " ]]' || { echo "toasts: the toast left by the pointer didn't fade: $(toasted)"; return 1; }
  local closed; closed="$(kanna toast "Closed")" && closed="${closed#raised }" && check wait_for 5 eval '[[ "$(toasted)" == "d1 $closed" ]]' || return 1
  raw "{\"id\":1,\"cmd\":\"debug.toasts\",\"args\":{\"click\":\"$closed\",\"close\":true}}" >/dev/null
  check wait_for 2 eval '[[ "$(toasted)" == "d1 " ]]' || { echo "toasts: × didn't close it"; return 1; }
}

scenario about_panel M4
about_panel() {
  start_daemon && start_app || return 1
  local plist="${KANNA_E2E_APP:-$repo_root/target/Kanna.app}/Contents/Info.plist"
  local name version commit channel label
  name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$plist")"
  version="$(/usr/libexec/PlistBuddy -c 'Print :KannaVersion' "$plist")"
  commit="$(/usr/libexec/PlistBuddy -c 'Print :KannaCommit' "$plist")"
  channel="$(/usr/libexec/PlistBuddy -c 'Print :KannaChannel' "$plist")"
  label="${commit:0:12}"
  [[ "$channel" == production ]] || label="$channel $label"
  raw "{\"id\":1,\"cmd\":\"debug.menu\",\"args\":{\"item\":\"About $name\"}}" | grep -q '"ok":true' || { echo "about_panel: About $name didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.about\"}" | field lines)" == *"Version $version ($label)"* ]]' || { echo "about_panel: $(raw '{"id":1,"cmd":"debug.about"}')"; return 1; }
}
