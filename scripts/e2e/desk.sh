shown_windows() { state | python3 -c 'import json, sys; print(" ".join(w["id"] for w in json.load(sys.stdin)["windows"]))'; }

real_key() { raw "{\"id\":1,\"cmd\":\"debug.key\",\"args\":{\"key\":\"$1\",\"real\":true}}" >/dev/null; sleep 0.3; }
planner_desk() {
  raw '{"id":1,"cmd":"desk","args":{"action":"desk","new":true}}' | grep -q '"ok":true' || return 1
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || return 1
  check wait_for 10 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field lead)" == term-* ]]' || return 1
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field lead
}

scenario desk_window_cycle M4
desk_window_cycle() {
  start_daemon && start_app || return 1
  local planner; planner="$(planner_desk)" || { echo "desk_window_cycle: no Desk with a planner"; return 1; }
  printf '# desk_window_cycle\n' >"$KANNA_STATE_DIR/cycle.md"
  KANNA_TERMINAL="$planner" kanna open md "$KANNA_STATE_DIR/cycle.md" --window new >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin win-1 key)" == True ]]' || { echo "desk_window_cycle: the free window isn't in front"; app_view; return 1; }
  real_key 'cmd+`'; check test "$(dwin d1 key)" == True || { echo "desk_window_cycle: ⌘\` didn't reach the Desk"; return 1; }
  real_key 'cmd+`'; check test "$(dwin win-1 key)" == True || { echo "desk_window_cycle: ⌘\` didn't come back to the free window"; return 1; }
  real_key 'cmd+shift+`'; check test "$(dwin d1 key)" == True || { echo "desk_window_cycle: ⇧⌘\` didn't reach the Desk"; return 1; }
  real_key 'cmd+shift+`'; check test "$(dwin win-1 key)" == True || { echo "desk_window_cycle: ⇧⌘\` didn't come back to the free window"; return 1; }
}

scenario desk_window_cli M4
desk_window_cli() {
  start_daemon && start_app || return 1
  local planner; planner="$(planner_desk)" || { echo "desk_window_cli: no Desk with a planner"; return 1; }
  local ws; ws="$(ws "$planner" id)"
  listed_desk() { kanna workspace list --all | grep -q '^ d1  Desk'; }
  check wait_for 5 listed_desk || { echo "desk_window_cli: workspace list --all doesn't list d1"; kanna workspace list --all; return 1; }
  [[ "$(kanna workspace list --all)" == *"$ws, d1's planner"* ]] || { echo "desk_window_cli: the planner isn't named as d1's"; kanna workspace list --all; return 1; }
  printf '# beside the planner\n' >"$KANNA_STATE_DIR/beside.md"
  local opened; opened="$(KANNA_TERMINAL="$planner" kanna open md "$KANNA_STATE_DIR/beside.md" --tab)" || return 1
  [[ "$opened" == "opened file-1 in $ws (d1's planner), tab-"* ]] || { echo "desk_window_cli: open --tab from the planner said: $opened"; return 1; }
  check test "$(state | field windows | count)" == 0 || { echo "desk_window_cli: open --tab from the planner made a window"; kanna workspace list --all; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 tabs.0.panes)" == "[\"file-1\"]" ]]' || { echo "desk_window_cli: the Desk window doesn't show the tab"; dwin d1 tabs; return 1; }
  check test "$(raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field page)" == False || { echo "desk_window_cli: the Desk view hides the planner's new tab"; return 1; }
  sleep 1; snap d1 desk_window_cli-planner-tab >/dev/null
  printf '# by name\n' >"$KANNA_STATE_DIR/named.md"
  opened="$(kanna open md "$KANNA_STATE_DIR/named.md" --window d1 --tab)" || return 1
  [[ "$opened" == "opened file-2 in $ws (d1's planner), tab-"* ]] || { echo "desk_window_cli: open --window d1 said: $opened"; return 1; }
  kanna open --window new --name loose >/dev/null || return 1
  local loose; loose="$(win win-1 tabs.0.id)"
  local moved; moved="$(kanna move-tab "$loose" --window d1)" || { echo "desk_window_cli: move-tab --window d1 failed"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 tabs | count)" == 3 ]]' || { echo "desk_window_cli: move-tab --window d1 said $moved"; dwin d1 tabs; return 1; }
  check test "$(state | field windows | count)" == 0 || { echo "desk_window_cli: the emptied free window stayed"; return 1; }
  kanna open --window new --name front >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin win-2 key)" == True ]]' || return 1
  local focused; focused="$(kanna focus --window d1)" || { echo "desk_window_cli: focus --window d1 failed"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || { echo "desk_window_cli: focus --window d1 said $focused, but d1 isn't in front"; return 1; }
  kanna focus --window d9 >/dev/null 2>&1 && { echo "desk_window_cli: focus --window d9 found a window"; return 1; }
  return 0
}

scenario docked_pane M4
docked_pane() {
  start_daemon && start_app || return 1
  local refused shot first agent dt1 dt2 dt3
  refused="$(kanna open --name agent --dock 2>&1)" && { echo "open --dock should fail"; return 1; }
  [[ "$refused" == *"a docked pane is a task's agent: make a task with \`kanna task new\`, then \`kanna task pick\` or \`kanna task start\`"* ]] || { echo "open --dock should say how to make a task: $refused"; return 1; }
  refused="$(raw '{"id":1,"cmd":"open","args":{"type":"term","name":"agent","dock":"left","task":"task-1"}}')"
  [[ "$refused" == *'"code":"not_a_task"'* ]] || { echo "a docked open naming a task the daemon isn't starting should fail: $refused"; return 1; }
  desk_task agent "printf 'agent: zoom as a mode\n'; exec sleep 600" || return 1
  agent="$(pane agent id)"
  KANNA_TERMINAL=agent kanna open --name dt1 --cmd "printf 'test result: ok. 41 passed; 0 failed\n'; exec sleep 600" >/dev/null || return 1
  KANNA_TERMINAL=agent kanna open --name dt2 --cmd "printf 'dev server on http://localhost:3000\n'; exec sleep 600" >/dev/null || return 1
  dt1="$(pane dt1 id)" dt2="$(pane dt2 id)"
  kanna rename-tab "$(ws agent tabs.0.id)" tests >/dev/null && kanna rename-tab "$(ws agent tabs.1.id)" server >/dev/null || return 1
  check test "$(ws agent dock.edge):$(ws agent dock.width)" == "left:2/5" || return 1
  [[ "$(kanna workspace list --all)" == *"dock: $agent (agent)  left 2/5  frozen"* ]] || { echo "workspace list should show the dock frozen, on its own line: $(kanna workspace list --all)"; return 1; }
  kanna task activate agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$agent" ]]' || { app_view; return 1; }
  sleep 1
  drew d1 docked_pane-docked 2 || return 1
  key "cmd+shift+["
  check test "$(dwin d1 tab)" == "$(dwin d1 tabs.0.id)" || return 1
  check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  sleep 0.5
  drew d1 docked_pane-first-tab 2 || return 1
  key "cmd+shift+]"; check test "$(dwin d1 tab)" == "$(dwin d1 tabs.1.id)" || return 1
  refused="$(kanna close agent 2>&1)" && { echo "close on the docked pane should fail"; return 1; }
  [[ "$refused" == *"docked: it closes with its window"* ]] || { echo "close on the docked pane should say why: $refused"; return 1; }
  kanna focus agent >/dev/null || return 1
  key cmd+w; check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  key "cmd+]"; check test "$(dwin d1 focused)" == "$dt2" || return 1
  key "cmd+]"; check test "$(dwin d1 focused)" == "$agent" || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == "$agent" || return 1
  shot="$(snap d1 docked_pane-zoomed)"
  check test "$(field panes <<<"$shot" | count)" == 1 || return 1
  check test "$(field tabBar.shown <<<"$shot")" == False || { echo "the zoomed docked pane should hide the tab bar"; return 1; }
  key "cmd+]"; check test "$(dwin d1 zoomed)" == "$dt2" || return 1
  shot="$(snap d1 docked_pane-zoomed-server)"
  check test "$(field panes <<<"$shot" | count)" == 1 || return 1
  check test "$(field tabBar.shown <<<"$shot"):$(field tabBar.width <<<"$shot")" == "True:$(field width <<<"$shot")" || { echo "a zoomed tab pane should show the tab bar across the window"; return 1; }
  key "cmd+shift+["; check test "$(dwin d1 zoomed)" == "$dt1" || return 1
  key "cmd+shift+]"; check test "$(dwin d1 zoomed)" == "$dt2" || return 1
  key "cmd+["; check test "$(dwin d1 zoomed)" == "$agent" || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == None || return 1
  KANNA_TERMINAL=dt2 kanna open --name dt3 --split right --cmd "printf 'GET / 200 4ms\n'; exec sleep 600" >/dev/null || return 1
  dt3="$(pane dt3 id)"
  check test "$(dwin d1 focused)" == "$dt3" || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"agent"}}' >/dev/null; sleep 0.3
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"dt3"}}' >/dev/null; sleep 0.3
  check test "$(dwin d1 focused)" == "$dt3" || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == "$dt3" || return 1
  sleep 0.5
  drew d1 docked_pane-tab-zoomed 1 || return 1
  check python3 -c 'import json, sys; s = json.loads(sys.argv[1]); assert s["panes"][0]["width"] == s["width"] == s["tabBar"]["width"] and s["tabBar"]["shown"], s' "$shot" || return 1
  click_tab "$(dwin d1 tabs.0.id)"
  check test "$(dwin d1 tab):$(dwin d1 zoomed)" == "$(dwin d1 tabs.0.id):$dt1" || { echo "clicking a tab while zoomed should show its pane zoomed"; return 1; }
  check test "$(field tabBar.shown <<<"$(snap d1 docked_pane-tab-zoomed-clicked)")" == True || return 1
  click_tab "$(dwin d1 tabs.1.id)"
  check test "$(dwin d1 tab):$(dwin d1 zoomed)" == "$(dwin d1 tabs.1.id):$dt3" || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == None || return 1
  sleep 0.5
  drew d1 docked_pane-tab-unzoomed 3 || return 1
  kanna close dt3 >/dev/null || return 1
  refused="$(kanna move agent --dock right 2>&1)" && { echo "moving a task's agent should fail"; return 1; }
  [[ "$refused" == *"frozen: agent ("*") is frozen in its dock and doesn't move"* ]] || { echo "moving a task's agent should say why: $refused"; return 1; }
  check test "$(ws agent dock.edge)" == left || return 1
  drag '{"divider":"agent","at":0.34}'
  check python3 -c 'import sys; assert abs(float(sys.argv[1]) - 1/3) < 0.001, sys.argv[1]' "$(dwin d1 dock.width)" || { echo "$last_drag"; return 1; }
  check test "$(ws agent dock.width)" == 2/5 || return 1
  kanna resize agent 1/2 >/dev/null || return 1
  check test "$(dwin d1 dock.width)" == 0.5 || return 1
  drag '{"divider":"agent","at":0.34}'
  sleep 0.5
  drew d1 docked_pane-resized 2 || return 1
  first="$(dwin d1 tabs.0.id)"
  kanna move-tab "$first" --window new >/dev/null || return 1
  check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  check test "$(dwin d1 tabs | count)" == 1 || return 1
  check test "$(ws agent windows | count)" == 1 || { echo "a tab torn off a task's workspace should make one of its windows"; return 1; }
  refused="$(kanna move dt2 --dock 2>&1)" && { echo "docking a pane that isn't a task should fail"; return 1; }
  [[ "$refused" == *"dt2 isn't a task, and only a task's agent docks"* ]] || { echo "docking a pane that isn't a task should say why: $refused"; return 1; }
  kanna task done agent >/dev/null || return 1
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys; print([w["id"] for w in json.load(sys.stdin)["workspaces"] if "planner" not in w["where"]])')" == "[]" || return 1
  desk_task agent "printf 'agent: zoom as a mode\n'; exec sleep 600" || return 1
  agent="$(pane agent id)"
  KANNA_TERMINAL="$(dwin d1 lead.pane)" kanna task activate agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$agent" ]]' || return 1
  check wait_for 5 redrawn d1 docked_pane-tabless 1 || return 1
  check python3 -c 'import json, sys; s = json.loads(sys.argv[1]); assert s["panes"][0]["width"] == s["width"], s' "$shot" || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"newTab":"d1"}}' | grep -q '"ok":true' || { echo "the agent header should show New Tab with no tabs"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 tabs | count)" == 1 ]]' || return 1
  kanna close "$(dwin d1 tabs.0.panes.0)" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs)" == "[]" ]]' || return 1
  KANNA_TERMINAL=agent kanna open --name dt4 --tab --cmd "printf 'cargo build\n'; exec sleep 600" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs | count)" == 1 ]]' || return 1
  check wait_for 5 redrawn d1 docked_pane-tab-opened 2 || return 1
  check python3 -c 'import json, sys; s = json.loads(sys.argv[1]); w = [p["width"] for p in s["panes"] if p["id"] == sys.argv[2]][0]; assert abs(w / s["width"] - 0.4) < 0.01, s' "$shot" "$agent" || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"newTab":"d1"}}' | grep -q '"ok":true' && { echo "the agent header should hide New Tab once there's a tab"; return 1; }
  refused="$(kanna move dt4 --to agent 2>&1)" && { echo "swapping onto a frozen docked pane should fail"; return 1; }
  [[ "$refused" == *"is frozen in ws-"*"'s dock; nothing drops on it"* ]] || { echo "swapping onto a frozen docked pane should say why: $refused"; return 1; }
  check test "$(raw '{"id":1,"cmd":"debug.drag","args":{"pane":"dt4","to":{"pane":"agent","x":0.5,"y":0.5}}}' | field dropped)" == False || return 1
  check test "$(raw '{"id":1,"cmd":"debug.drag","args":{"pane":"agent","to":{"pane":"dt4","x":0.5,"y":0.5}}}' | field dropped)" == False || return 1
  check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  kanna close dt4 >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs)" == "[]" ]]' || return 1
  check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  check wait_for 5 redrawn d1 docked_pane-only-dock 1 || return 1
  check python3 -c 'import json, sys; s = json.loads(sys.argv[1]); assert s["panes"][0]["width"] == s["width"], s' "$shot" || return 1
  kanna focus agent >/dev/null || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == "$agent" || return 1
  sleep 0.5
  drew d1 docked_pane-only-dock-zoomed 1 || return 1
  check test "$(field tabBar.shown <<<"$shot")" == False || return 1
  check python3 -c 'import json, sys; s = json.loads(sys.argv[1]); assert s["panes"][0]["width"] == s["width"], s' "$shot" || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == None || return 1
  kanna task done agent >/dev/null || return 1
  [[ "$(kanna workspace list --all)" != *agent* ]] || { echo "done should close the docked pane"; return 1; }

  local before after
  desk_task back "printf 'DOCK BACK\n'; exec sleep 600" || return 1
  KANNA_TERMINAL=back kanna open --name tab1 >/dev/null || return 1
  kanna resize back 1/4 >/dev/null || return 1
  desk_task lone || return 1
  KANNA_TERMINAL="$(dwin d1 lead.pane)" kanna task activate back >/dev/null || return 1
  sleep 1.5
  check python3 -c 'import json, sqlite3, sys
saved = json.loads(sqlite3.connect(sys.argv[1]).execute("select doc from layout").fetchone()[0])
docks = {s["dock"]["pane"]: s for s in saved["workspaces"]}
back, lone = docks[sys.argv[2]], docks[sys.argv[3]]
assert back["dock"]["edge"] == "left" and back["dock"]["width"] == 0.25, back
assert lone["dock"]["edge"] == "left" and lone["tabs"] == [], lone' "$KANNA_STATE_DIR/kanna.db" "$(pane back id)" "$(pane lone id)" || return 1
  before="$(app_view | field windows)"
  stop_app; start_app || return 1
  sleep 1
  after="$(app_view | field windows)"
  check test "$after" == "$before" || { echo "before: $before"; echo "after:  $after"; return 1; }
  check wait_for 5 app_text back "DOCK BACK" || return 1
  sleep 0.5
  drew d1 docked_pane-restored 2
}

scenario dock_keeps_focus_through_tabs M4
dock_keeps_focus_through_tabs() {
  start_daemon && start_app || return 1
  local agent
  desk_task agent 'exec cat' || return 1
  KANNA_TERMINAL=agent kanna open --name spec --cmd "printf 'spec\n'; exec sleep 600" >/dev/null || return 1
  KANNA_TERMINAL=agent kanna open --name plan --cmd "printf 'plan\n'; exec sleep 600" >/dev/null || return 1
  agent="$(pane agent id)"
  kanna task activate agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$agent" ]]' || { app_view; return 1; }
  kanna focus agent >/dev/null || return 1
  check test "$(dwin d1 tab)" == "$(dwin d1 tabs.1.id)" || return 1
  key "cmd+shift+]"; check test "$(dwin d1 tab)" == "$(dwin d1 tabs.0.id)" || return 1
  check test "$(ws agent focused)" == agent || return 1
  key "cmd+shift+["; check test "$(dwin d1 tab)" == "$(dwin d1 tabs.1.id)" || return 1
  check test "$(ws agent focused)" == agent || return 1
  check test "$(dwin d1 focused)" == "$agent" || return 1
  for k in d o c k e d; do key "$k"; done
  check wait_for 5 app_text agent docked || { kanna read agent; return 1; }
  check test "$(ws agent focused)" == agent || return 1
  kanna focus plan >/dev/null || return 1
  key "cmd+shift+["; check test "$(dwin d1 tab)" == "$(dwin d1 tabs.0.id)" || return 1
  check test "$(ws agent focused)" == spec || return 1
}

scenario desk_tasks M4
desk_tasks() {
  start_daemon && start_app || return 1
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"${2:-d1}\"}}" | field "$1"; }
  app_windows() { app_view | python3 -c 'import json, sys; print(" ".join(w["id"] for w in json.load(sys.stdin)["windows"]))'; }
  desk_task agent || return 1
  KANNA_TERMINAL=term-1 kanna open --name notes >/dev/null || return 1
  check test "$(app_windows)" == "" || { echo "desk_tasks: a task on the Desk but not active showed a window"; app_view; return 1; }
  expected="$(sid 1) is active on d1"; says kanna task activate agent || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || { echo "desk_tasks line $LINENO"; app_view; return 1; }
  check test "$(desk desk)" == True || return 1
  check test "$(desk strip)" == True || return 1
  check test "$(desk title)" == agent || return 1
  check wait_for 5 eval '[[ "$(desk header)" == "$(e2e_repo)/.kanna-worktrees/$(tid 1) · $(tid 1)" ]]' || { desk header; return 1; }
  expected="parked $(sid 1): d1 shows the Desk view, and $(sid 1) stays active"; says kanna task park agent || return 1
  check test "$(desk title)" == Desk || return 1
  check test "$(desk empty)" == True || return 1
  check test "$(desk page)" == True || return 1
  check test "$(desk tasks.0.task)" == term-1 || return 1
  check test "$(dwin d1 dock)" == None || return 1
  check test "$(desk active)" == term-1 || return 1
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Show Active Task"}}' | grep -q '"ok":true' || { echo "desk_tasks: Show Active Task didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || { echo "desk_tasks: ⌘. didn't go back to the active task"; return 1; }
  check test "$(desk stripLayout.hint)" == "⌘. Desk" || { desk stripLayout; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Show Desk"}}' | grep -q '"ok":true' || { echo "desk_tasks: Show Desk didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(desk empty)" == True && "$(desk active)" == term-1 ]]' || { echo "desk_tasks: ⌘. left term-1 inactive"; return 1; }
  sleep 0.5; snap d1 desk_tasks-empty >/dev/null
  kanna send notes 'echo PARKED-$((6*7))' >/dev/null || return 1
  check wait_for 5 shows notes PARKED-42 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"task":"term-1","window":"d1"}}' >/dev/null; sleep 0.3
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || { echo "desk_tasks line $LINENO"; app_view; return 1; }
  check test "$(dwin d1 tabs.0.panes.0)" == "$(pane notes id)" || return 1
  check test "$(desk empty)" == False || return 1
  local cheer; cheer="$(desk cheer)"; [[ -n "$cheer" ]] || return 1
  kanna task park agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(desk empty)" == True && "$(desk cheer)" != "$cheer" ]]' || { desk cheer; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(desk tasks.0.task)" == term-1 ]]' || { echo "desk_tasks: the Desk and its Shelf didn't survive the app's relaunch"; app_view; return 1; }
  check test "$(pane notes state)" == running || return 1
  expected="$(sid 1) is active on d1"; says kanna task activate term-1 || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs.0.panes.0)" == "$(pane notes id)" ]]' || return 1
  sleep 0.5; drew d1 desk_tasks-desk 2 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"deskView":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk empty)" == True ]]' || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"task":"term-1","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || { echo "desk_tasks line $LINENO"; app_view; return 1; }
  desk_task second || return 1
  key cmd+n
  check wait_for 5 eval '[[ "$(dwin d2 key)" == True ]]' || { app_view; return 1; }
  check wait_for 10 eval '[[ "$(desk lead d2)" == term-* ]]' || { echo "desk_tasks: d2 has no planning terminal"; return 1; }
  check wait_for 10 shows "$(desk lead d2)" "Folder, repo" || { kanna read "$(desk lead d2)"; return 1; }
  kanna send "$(desk lead d2)" "$(e2e_repo)" >/dev/null || return 1
  local tasks; tasks="$(e2e_repo)"
  check wait_for 10 eval '[[ "$(desk window.project d2)" == *"${tasks#/private}" ]]' || { echo "desk_tasks: d2 didn't move to the repo"; return 1; }
  KANNA_TERMINAL="$(desk lead d2)" kanna task activate second >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d2 dock.pane)" == "$(pane second id)" ]]' || return 1
  expected="$(sid 1) is on d1; brought it forward"; says kanna task activate agent || return 1
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || return 1
  kanna task park agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(desk empty)" == True ]]' || return 1
  key cmd+.
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || { echo "desk_tasks line $LINENO"; app_view; return 1; }
  check test "$(dwin d2 dock.pane)" == "$(pane second id)" || { echo "desk_tasks: one Desk's activate moved another's task"; return 1; }
  key cmd+.
  check wait_for 5 eval '[[ "$(desk empty)" == True ]]' || return 1
  key cmd+.
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || { echo "desk_tasks line $LINENO"; app_view; return 1; }
  desk_task asker "exec $repo_root/tools/mock-agent/mock-agent" || return 1
  check wait_for 5 is asker status idle || return 1
  kanna send asker ask || return 1
  check wait_for 5 is asker status needs-you || return 1
  kanna task done asker >/dev/null || return 1
  KANNA_TERMINAL=term-1 kanna open --name side >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs.1.panes.0)" == "$(pane side id)" ]]' || return 1
  kanna task park agent >/dev/null || return 1
  KANNA_TERMINAL=term-1 kanna open --name later >/dev/null || return 1
  check test "$(dwin d1 tabs)" == "[]" || return 1
  kanna task activate agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs.2.panes.0)" == "$(pane later id)" ]]' || return 1
  local own loose task; task="$(ws agent id)"
  own="$(KANNA_TERMINAL=term-1 kanna open --name own --window new)" || return 1
  [[ "$own" =~ ^"opened "term-[0-9]+" (own) in $task's "(win-[0-9]+)" ($(sid 1), on the Desk), tab-" ]] || { echo "desk_tasks: a task's open --window new: $own"; return 1; }
  own="${BASH_REMATCH[1]}"
  check test "$(win "$own" workspace)" == "$task" || return 1
  loose="$(kanna open --name loose --window new)" || return 1
  [[ "$loose" =~ " in "(win-[0-9]+)", tab-" ]] || { echo "desk_tasks: $loose"; return 1; }
  loose="${BASH_REMATCH[1]}"
  expected="assigned $loose to $task ($(sid 1), on the Desk)"; says kanna window assign "$loose" agent || return 1
  check wait_for 5 eval '[[ " $(app_windows) " == *" $own "* && " $(app_windows) " == *" $loose "* ]]' || { echo "desk_tasks: a task's windows didn't show with it on a Desk"; app_windows; return 1; }
  kanna task park agent >/dev/null || return 1
  check wait_for 5 eval '[[ " $(app_windows) " != *" $own "* && " $(app_windows) " != *" $loose "* ]]' || { echo "desk_tasks: a parked task's windows didn't close"; app_windows; return 1; }
  check test "$(ws "$task" windows)" == "[\"$own\", \"$loose\"]" || return 1
  check is own state running || return 1
  [[ "$(kanna open --name hidden --window "$own" --tab)" == "opened "*" (hidden) in $task's $own ($(sid 1), on the Desk), tab-"* ]] || { echo "desk_tasks: an open into a hidden window"; return 1; }
  kanna task activate agent >/dev/null || return 1
  check wait_for 5 eval '[[ " $(app_windows) " == *" $own "* && " $(app_windows) " == *" $loose "* ]]' || { echo "desk_tasks: a pulled task's windows didn't open"; app_windows; return 1; }
  check test "$(win "$own" tabs.1.layout.pane)" == hidden || return 1
  expected="$loose is free (out of $task ($(sid 1)))"; says kanna window free "$loose" || return 1
  kanna task park agent >/dev/null || return 1
  check wait_for 5 eval '[[ " $(app_windows) " != *" $own "* && " $(app_windows) " == *" $loose "* ]]' || { echo "desk_tasks: a freed window closed with the task"; app_windows; return 1; }
  kanna task activate agent >/dev/null || return 1
  kanna close --window "$own" >/dev/null && kanna close --window "$loose" >/dev/null || return 1
  check close_desk d1 || return 1
  check wait_for 5 eval 'kanna task list --all | placed | grep -q "^$(sid 1) .*Desk"' || { kanna task list --all; return 1; }
  check test "$(pane agent state)" == running || return 1
  check test "$(pane notes state)" == running || return 1
  local ta; ta="$(new_task ta)" && kanna task pick "$ta" >/dev/null || return 1
  local tc; tc="$(new_task tc)" || return 1
  kanna task pick "$tc" 2>"$KANNA_STATE_DIR/desk_tasks-limit" && return 1
  grep -q "has 3 tasks on the Desk: move one to the Shelf" "$KANNA_STATE_DIR/desk_tasks-limit" || { cat "$KANNA_STATE_DIR/desk_tasks-limit"; return 1; }
  desk_task elsewhere "" elsewhere || return 1
  KANNA_TERMINAL=ta kanna open --name ta-notes >/dev/null || return 1
  expected="$(sid 4) is done: closed $(pane ta id), $(pane ta-notes id)"; says kanna task done ta || return 1
  expected="$(sid 1) is done: closed term-1, $(pane notes id), $(pane side id), $(pane later id)"; says kanna task done agent || return 1
  kanna task list --all | placed | grep -q "^$(sid 1) .*Desk" && return 1
  kanna task pick "$tc" >/dev/null || return 1
}

scenario desk_by_path M4
desk_by_path() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fresh="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/fresh"
  for dir in "$repo" "$fresh"; do mkdir -p "$dir" && git -C "$dir" init -q -b main || { echo "desk_by_path line $LINENO"; return 1; }; done
  git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_by_path line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"; fresh="$(cd "$fresh" && pwd -P)"
  mkdir -p "$repo/.kanna" && printf '{"planning": {"context": 400000}}\n' >"$repo/.kanna/config.json"
  start_daemon || { echo "desk_by_path line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead d1)" == term-* ]]' || { echo "desk_by_path: no planning terminal"; kanna workspace list --all; return 1; }
  local planner; planner="$(desk lead d1)"
  check test "$(cd "$(pane "$planner" cwd)" && pwd -P)" == "$repo" || { echo "desk_by_path: Kanna started in a folder should plan there"; kanna workspace list --all --json; return 1; }
  check wait_for 10 shows "$planner" "No agent Kanna knows is installed" || { kanna read "$planner"; return 1; }
  kanna send "$planner" 'echo "context=$CLAUDE_CODE_AUTO_COMPACT_WINDOW"' >/dev/null || { echo "desk_by_path line $LINENO"; return 1; }
  check wait_for 5 shows "$planner" "context=400000" || { echo "desk_by_path: the repo's .kanna/config.json didn't set the context"; kanna read "$planner"; return 1; }
  (cd "$repo" && kanna task new "Waiting" "hello" --agent "$agent") >/dev/null || { echo "desk_by_path line $LINENO"; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" || { echo "desk_by_path line $LINENO"; return 1; }
  [[ "$opened" =~ ^brought\ forward\ (d1)\ on\ "$repo"$ ]] || { echo "desk_by_path: kanna PATH on the open project didn't bring its Desk forward: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  check wait_for 5 eval '[[ "$(desk empty "$w")" == True && "$(desk shelf "$w")" == "[\"$(tid 1)\"]" && "$(desk running "$w")" == 0 ]]' || { desk shelf "$w"; desk running "$w"; return 1; }
  check wait_for 10 eval '[[ "$(desk lead "$w")" == term-* ]]' || { echo "desk_by_path: no planning terminal"; kanna workspace list --all; return 1; }
  check test "$(desk window.dock "$w")" == None || { echo "desk_by_path: a Desk on a project with tasks docked something"; kanna workspace list --all; return 1; }
  planner="$(desk lead "$w")"
  check test "$(cd "$(pane "$planner" cwd)" && pwd -P)" == "$repo" || { echo "desk_by_path line $LINENO"; return 1; }
  opened="$(kanna "$fresh")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "desk_by_path line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk lead "$w")" == term-* ]]' || { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}"; kanna workspace list --all; return 1; }
  planner="$(desk lead "$w")"
  check test "$(cd "$(pane "$planner" cwd)" && pwd -P)" == "$fresh" || { echo "desk_by_path: the planning terminal isn't in the empty project"; kanna workspace list --all --json; return 1; }
  check wait_for 10 shows "$planner" "No agent Kanna knows is installed" || { kanna read "$planner"; return 1; }
}

scenario desk_planner_stays M4
desk_planner_stays() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_planner_stays line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  printf '# Notes\n' >"$repo/notes.md"
  start_daemon || { echo "desk_planner_stays line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  shell() { kanna read "$1" | grep -o 'shell=[0-9][0-9]*' | tail -1; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { kanna workspace list --all; return 1; }
  local planner workspace width pid
  planner="$(desk lead)"; workspace="$(ws "$planner" id)"; width="$(ws "$planner" dock.width)"
  check wait_for 10 shows "$planner" "No agent Kanna knows is installed" || { kanna read "$planner"; return 1; }
  kanna send "$planner" 'echo "shell=$$"' >/dev/null || { echo "desk_planner_stays line $LINENO"; return 1; }
  check wait_for 5 eval '[[ -n "$(shell "$planner")" ]]' || { kanna read "$planner"; return 1; }
  pid="$(shell "$planner")"
  same() {
    [[ "$(desk lead)" == "$planner" ]] || { echo "desk_planner_stays: $1: the Desk's planning terminal is $(desk lead), not $planner"; kanna workspace list --all; return 1; }
    [[ "$(ws "$planner" id)" == "$workspace" && "$(ws "$planner" dock.width)" == "$width" ]] || { echo "desk_planner_stays: $1: the planner workspace changed"; kanna workspace list --all --json; return 1; }
    is "$planner" state running || { echo "desk_planner_stays: $1: the planning terminal stopped"; return 1; }
    kanna send "$planner" 'clear; echo "shell=$$"' >/dev/null && wait_for 5 eval '[[ "$(shell "$planner")" == "$pid" ]]' || { echo "desk_planner_stays: $1: the planning terminal's shell isn't $pid"; kanna read "$planner"; return 1; }
  }
  kanna open md "$repo/notes.md" --window new >/dev/null || { echo "desk_planner_stays line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(shown_windows)" == win-1 ]]' || { kanna workspace list --all; return 1; }
  rm "$repo/notes.md"
  stop_app; stop_daemon
  start_daemon && start_app || { echo "desk_planner_stays line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk lead)" == "$planner" && "$(shown_windows)" == win-1 ]]' || { echo "desk_planner_stays: the planning terminal or the doc's window didn't survive the app and daemon's restart"; kanna workspace list --all; return 1; }
  same "after the restart" || return 1
  key 'cmd+`'
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || { echo "desk_planner_stays: Next Window didn't bring the Desk forward"; app_view; return 1; }
  key 'cmd+`'
  check wait_for 5 eval '[[ "$(dwin win-1 key)" == True ]]' || { echo "desk_planner_stays: Next Window back to the doc's window left $(app_view | field key) in front"; return 1; }
  key cmd+w
  check wait_for 5 eval '[[ -z "$(shown_windows)" ]]' || { echo "desk_planner_stays: Close Pane didn't close the missing doc's window"; kanna workspace list --all; return 1; }
  sleep 1; same "after Close Pane in the doc's window" || return 1
  sleep 30; same "after 30s idle" || return 1
  key cmd+shift+w
  check wait_for 5 eval '! kanna workspace list --all --json | grep -q "\"$planner\""' || { echo "desk_planner_stays: closing the window left its planning terminal running"; kanna workspace list --all; return 1; }
}

scenario new_desk M4
new_desk() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "new_desk line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon && start_app || return 1
  kanna open --window new --name plain >/dev/null || { echo "new_desk line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin win-1 key)" == True ]]' || { echo "new_desk line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  raw '{"id":1,"cmd":"debug.app","args":{"keyview":true}}' >/dev/null || { echo "new_desk line $LINENO"; return 1; }
  key cmd+n
  check wait_for 5 eval '[[ "$(app_view | field windows | count)" == 2 && "$(dwin d1 key)" == True ]]' || { echo "new_desk: ⌘N opened no window"; kanna workspace list --all; return 1; }
  check test "$(desk desk d1)" == True || { echo "new_desk: ⌘N opened a window that isn't a Desk"; return 1; }
  check test "$(desk window.project d1)" == "$HOME" || { echo "new_desk: the new Desk isn't on ~"; desk window d1; return 1; }
  check wait_for 10 eval '[[ "$(desk lead d1)" == term-* ]]' || { echo "new_desk: the new Desk has no planning terminal"; kanna workspace list --all; return 1; }
  local planner; planner="$(desk lead d1)"
  check test "$(desk window.dock d1)" == None || { echo "new_desk: the new Desk docked something"; return 1; }
  check test "$(desk window.tabs d1)" == "[]" || { echo "new_desk: the new Desk has tabs"; return 1; }
  check wait_for 5 is "$planner" state running || { echo "new_desk: the planning terminal isn't running"; kanna workspace list --all; return 1; }
  check test "$(cd "$(pane "$planner" cwd)" && pwd -P)" == "$(cd ~ && pwd -P)" || { echo "new_desk: the planning terminal isn't in ~"; return 1; }
  check wait_for 10 shows "$planner" "Folder, repo or name:" || { kanna read "$planner"; return 1; }
  check wait_for 5 eval '[[ "$(desk responder d1)" == TerminalSurfaceView ]]' || { echo "new_desk: the keyboard is on $(desk responder d1), not the planning terminal"; return 1; }
  local typed; typed="$(raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"typedkeys"}}' | field responder)"
  check test "$typed" == TerminalSurfaceView || { echo "new_desk: typed keys went to $typed"; return 1; }
  check wait_for 5 shows "$planner" "typedkeys" || { echo "new_desk: typed keys didn't reach the planning terminal"; kanna read "$planner"; return 1; }
  kanna send-key "$planner" ctrl+u >/dev/null || { echo "new_desk line $LINENO"; return 1; }
  check test "$(kanna task list --all | grep -c "$planner")" == 0 || { echo "new_desk: the planning terminal is listed as a task"; return 1; }
  check test "$(raw '{"id":1,"cmd":"debug.stats"}' | field pid)" == "$app_pid" || { echo "new_desk: the new Desk is in another instance"; return 1; }
  kanna send "$planner" "$repo" >/dev/null || { echo "new_desk line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk header d1)" == "Planning · "*"${repo#/private} · main" ]]' || { desk header d1; return 1; }
  (cd "$repo" && KANNA_TERMINAL="$planner" kanna task new "Here") >/dev/null || { echo "new_desk line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk shelf d1)" == "[\"$(tid 1)\"]" ]]' || { echo "new_desk: the Shelf didn't follow the planning terminal"; desk shelf d1; return 1; }
  check eval '[[ "$(desk window.project d1)" == *"${repo#/private}" ]]' || { echo "new_desk: the Desk's project didn't follow the planning terminal"; desk window d1; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"New Desk"}}' | grep -q '"ok":true' || { echo "new_desk: File > New Desk didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(app_view | field windows | count)" == 3 && "$(desk desk d2)" == True ]]' || { echo "new_desk: New Desk opened no Desk"; kanna workspace list --all; return 1; }
  check wait_for 10 eval '[[ "$(desk lead d2)" == term-* ]]' || { echo "new_desk line $LINENO"; return 1; }
  check test "$(cd "$(pane "$(desk lead d2)" cwd)" && pwd -P)" == "$(cd ~ && pwd -P)" || { echo "new_desk: the second new Desk isn't in ~"; return 1; }
  check wait_for 10 shows "$(desk lead d2)" "Folder, repo" || { kanna read "$(desk lead d2)"; return 1; }
  check wait_for 5 eval '[[ "$(desk responder d2)" == TerminalSurfaceView ]]' || { echo "new_desk: New Desk's keyboard is on $(desk responder d2), not the planning terminal"; return 1; }
}

scenario restored_panes_draw M4
restored_panes_draw() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" dir="$KANNA_STATE_DIR/restored" shots="$KANNA_STATE_DIR/restored-shots"
  local planner task round kind blank
  mkdir -p "$repo" "$dir" "$shots" && shots="$(cd "$shots" && pwd -P)"
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || return 1
  repo="$(cd "$repo" && pwd -P)"
  drawn_app() { env -u KANNA_IGNORE_OCCLUSION -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 & app_pid=$!; wait_for 10 app_attached; }
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  drawn() {
    kanna screenshot "$1" -o "$shots/$1-$2.png" --force >/dev/null 2>&1 || return 1
    python3 -c 'import sys; assert float(sys.argv[1]) > 0.003' "$(png_ink "$shots/$1-$2.png")" 2>/dev/null || return 1
    [[ "$1" != planned && "$1" != plain && "$1" != mermaid ]] || raw '{"id":1,"cmd":"debug.md","args":{"pane":"'"$1"'"}}' | field text | grep -q "restored_panes_draw $1"
  }
  start_daemon && drawn_app || return 1
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { kanna workspace list --all; return 1; }
  planner="$(desk lead)"
  printf '# restored_panes_draw planned\n\n```mermaid\nflowchart LR\n    a[planned one] --> b[planned two]\n```\n' >"$repo/planned.md"
  printf '# restored_panes_draw plain\n\nSome text to draw.\n' >"$dir/plain.md"
  printf '# restored_panes_draw mermaid\n\n```mermaid\nflowchart LR\n    a[restored one] --> b[restored two]\n```\n\nText after it.\n' >"$dir/mermaid.md"
  printf '<title>restored_panes_draw</title><body style="background:#fff"><h1 style="font-size:80px">restored_panes_draw WEB</h1></body>\n' >"$dir/page.html"
  for i in $(seq 60); do echo "fn f$i() -> u32 { $i }"; done >"$dir/a.rs"
  git -C "$dir" init -q -b main && git -C "$dir" add a.rs && git -C "$dir" -c user.email=e@e -c user.name=e commit -qm first || return 1
  sed -i '' 's/{ 5 }/{ 500 }/' "$dir/a.rs"
  KANNA_TERMINAL="$planner" kanna open md "$repo/planned.md" --tab --name planned >/dev/null || return 1
  kanna open --window new md "$dir/plain.md" --name plain >/dev/null || return 1
  kanna open --window new md "$dir/mermaid.md" --name mermaid >/dev/null || return 1
  kanna open --window new web "file://$dir/page.html" --name page >/dev/null || return 1
  kanna open --window new code "$dir/a.rs" --name source >/dev/null || return 1
  (cd "$dir" && kanna open --window new diff --name changes >/dev/null) || return 1
  for kind in planned plain mermaid page source changes; do
    check wait_for 10 drawn "$kind" before || { cp "$shots"/*.png "$out/" 2>/dev/null; echo "restored_panes_draw: $kind doesn't draw at first"; return 1; }
  done
  (cd "$repo" && KANNA_TERMINAL="$planner" kanna task new "Elsewhere" "hello") >/dev/null || return 1
  task="$(tid 1)"
  (cd "$repo" && kanna task pick "$task" && kanna task activate "$task") >/dev/null || return 1
  check wait_for 10 eval '[[ "$(desk window.dock)" != None ]]' || return 1
  for round in layout app both; do
    case "$round" in
      layout) kanna open --window new --name nudge >/dev/null && kanna close nudge >/dev/null || return 1 ;;
      app) stop_app; drawn_app || return 1 ;;
      both) stop_app; stop_daemon; start_daemon && drawn_app || return 1 ;;
    esac
    sleep 1
    (cd "$repo" && kanna task park "$task") >/dev/null || return 1
    check wait_for 10 eval '[[ "$(desk window.dock)" == None ]]' || return 1
    raw '{"id":1,"cmd":"debug.click","args":{"tab":"'"$(desk window.tab)"'"}}' >/dev/null
    blank=()
    for kind in planned plain mermaid page source changes; do
      wait_for 10 drawn "$kind" "$round" || blank+=("$kind")
    done
    cp "$shots"/*.png "$out/" 2>/dev/null
    [[ ${#blank[@]} -eq 0 ]] || { echo "restored_panes_draw: after the $round round these don't draw: ${blank[*]}"; kanna screenshot --window d1 -o "$out/restored_panes_draw-d1.png" --force >/dev/null; return 1; }
    (cd "$repo" && kanna task activate "$task") >/dev/null || return 1
    check wait_for 10 eval '[[ "$(desk window.dock)" != None ]]' || return 1
  done
}

scenario desk_scrolls M4
desk_scrolls() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_scrolls line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "desk_scrolls line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"${2:+,$2}}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "desk_scrolls: no planning terminal"; kanna workspace list --all; return 1; }
  for i in $(seq 20); do (cd "$repo" && kanna task new "Task $i" "hello" --agent "$agent") >/dev/null || { echo "desk_scrolls line $LINENO"; return 1; }; done
  check wait_for 5 eval '[[ "$(desk shelf)" == *"$(tid 20)"* && "$(desk page)" == True ]]' || { desk shelf; return 1; }
  desk page '"size":[760,420]' >/dev/null; sleep 0.3
  check eval '(( $(desk scroll.height) > $(desk scroll.visible) + 600 ))' || { desk scroll; return 1; }
  check test "$(desk scroll.top)" == 0 || { desk scroll; return 1; }
  [[ "$(desk scroll.shelf)" != *'"'"$(tid 20)"'"'* ]] || { echo "desk_scrolls: $(tid 20) fits a short window"; desk scroll; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 20)"'","window":"d1"}}' | grep -q 'scrolled out' || { echo "desk_scrolls: clicked a row that isn't on screen"; return 1; }
  desk page '"scroll":100000' >/dev/null; sleep 0.3
  check eval '(( $(desk scroll.top) > 0 ))' || { desk scroll; return 1; }
  check eval '[[ "$(desk scroll.shelf)" == *"\"$(tid 20)\""* ]]' || { echo "desk_scrolls: $(tid 20) can't be scrolled to"; desk scroll; return 1; }
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.3; snap d1 desk_scrolls-light >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.3; snap d1 desk_scrolls-dark >/dev/null
  local top; top="$(desk scroll.top)"
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 20)"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk shelf)" != *"$(tid 20)"* && "$(desk tasks.0.title)" == "Task 20" ]]' || { desk shelf; desk tasks; return 1; }
  check eval '(( $(desk scroll.top) > 0 && $(desk scroll.top) <= top ))' || { echo "desk_scrolls: the page lost its scroll position"; desk scroll; return 1; }
  check eval '(( $(desk scroll.top) + $(desk scroll.visible) <= $(desk scroll.height) ))' || { desk scroll; return 1; }
  (cd "$repo" && kanna task new "The Shelf strip shows every parked task's time beside its name" "hello" --agent "$agent" && kanna task new "Long titles truncate before the times and the strip never overflows" "hello" --agent "$agent") >/dev/null || { echo "desk_scrolls line $LINENO"; return 1; }
  kanna task pick $(tid 21) >/dev/null && kanna task pick $(tid 22) >/dev/null || { echo "desk_scrolls line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk running)" == 3 ]]' || { desk running; return 1; }
  kanna task activate "Task 20" >/dev/null || { echo "desk_scrolls line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk strip)" == True && "$(desk title)" == "Task 20" ]]' || { desk stripLayout; return 1; }
  fits() { desk stripLayout "$1" | python3 -c 'import json, sys; g = json.load(sys.stdin); items = g["items"]
last = g["hintRight"] if g["countLeft"] is None else g["countLeft"]
sys.exit(not (len(items) == 2 and all(i["meta"].startswith("· ") for i in items) and all(i["right"] <= g["hintLeft"] for i in items) and g["hintRight"] <= last <= g["width"] and (g["countLeft"] is not None or g["width"] - g["hintRight"] <= 12)))'; }
  check eval 'fits "\"size\":[900,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "19 on the Shelf" || { desk waiting; return 1; }
  sleep 0.3; snap d1 desk_scrolls-strip-900 >/dev/null
  check eval 'fits "\"size\":[520,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "19 on the Shelf" || { echo "desk_scrolls: the Shelf count gave way before the titles"; desk stripLayout; return 1; }
  sleep 0.3; snap d1 desk_scrolls-strip-520 >/dev/null
  check eval 'fits "\"size\":[400,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "" || { echo "desk_scrolls: the Shelf count didn't give way on a narrow strip"; desk stripLayout; return 1; }
  sleep 0.3; snap d1 desk_scrolls-strip-400 >/dev/null
  desk stripLayout '"size":[300,420]' >/dev/null
  check test "$(desk stripLayout.hint)" == "⌘. Desk" || { desk stripLayout; return 1; }
  check test "$(desk stripLayout.items.0.meta)" == "" || { echo "desk_scrolls: the times didn't give way last"; desk stripLayout; return 1; }
  sleep 0.3; snap d1 desk_scrolls-strip-300 >/dev/null
}

scenario desk_times M4
desk_times() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_times line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "desk_times line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"${2:+,$2}}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "desk_times: no planning terminal"; kanna workspace list --all; return 1; }
  (cd "$repo" && kanna task new "Parked" "hello" --agent "$agent" && kanna task new "Waiting" "hello" --agent "$agent") >/dev/null || { echo "desk_times line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null || { echo "desk_times line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk times)" == *"just now"*"made just now"* ]]' || { desk times; return 1; }
  local later; later="$(desk times '"later":7500')"
  [[ "$later" == *"2h ago"*"made 2h ago"* ]] || { echo "desk_times: the empty Desk's times didn't move without a new page: $later"; return 1; }
  desk times '"later":0' >/dev/null
}

scenario desk_zoom M4
desk_zoom() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" planner task shot log
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_zoom line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "desk_zoom line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "desk_zoom: no planning terminal"; kanna workspace list --all; return 1; }
  planner="$(desk lead)"
  (cd "$repo" && kanna task new "Zoomed task" "hello" --agent "$agent") >/dev/null && kanna task pick $(tid 1) >/dev/null || { echo "desk_zoom line $LINENO"; return 1; }
  kanna task activate "Zoomed task" >/dev/null || { echo "desk_zoom line $LINENO"; return 1; }
  task="$(dwin d1 dock.pane)"
  KANNA_TERMINAL="$task" kanna open --name log --cmd "printf 'GET / 200\n'; exec sleep 600" >/dev/null || { echo "desk_zoom line $LINENO"; return 1; }
  log="$(pane log id)"
  KANNA_TERMINAL="$log" kanna open --name tests --split right --cmd "printf 'test result: ok\n'; exec sleep 600" >/dev/null || { echo "desk_zoom line $LINENO"; return 1; }
  kanna focus log >/dev/null && key cmd+shift+return || { echo "desk_zoom line $LINENO"; return 1; }
  check test "$(dwin d1 zoomed)" == "$log" || { echo "desk_zoom: ⇧⌘↩ didn't zoom the task's pane"; kanna workspace list --all; return 1; }
  shown_alone() { shot="$(snap d1 "$1")"; python3 -c 'import json, sys; s = json.loads(sys.argv[1]); p = s["panes"]; assert len(p) == 1 and p[0]["width"] == s["width"] and s["tabBar"]["shown"] == (sys.argv[2] == "tab"), s' "$shot" "$2"; }
  sleep 0.5; check shown_alone desk_zoom-task-zoomed tab || { echo "desk_zoom: the zoomed pane doesn't fill the window"; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "desk_zoom: ⌘. didn't park the task"; return 1; }
  check test "$(ws $(tid 1) where):$(ws $(tid 1) tabs.0.title)" == "{\"desk\": \"mac\"}:log" || { echo "desk_zoom: $(sid 1)'s workspace isn't on the Shelf with its tabs"; state; return 1; }
  check test "$(dwin d1 focused):$(dwin d1 zoomed)" == "$planner:None" || { echo "desk_zoom: the Shelf view came up zoomed"; kanna workspace list --all; return 1; }
  check test "$(desk page)" == True || { echo "desk_zoom: no full Shelf beside the planner"; return 1; }
  key cmd+shift+return
  check test "$(dwin d1 zoomed)" == "$planner" || { echo "desk_zoom: ⇧⌘↩ didn't zoom the planner"; kanna workspace list --all; return 1; }
  sleep 0.5; check shown_alone desk_zoom-planner-zoomed dock || { echo "desk_zoom: the zoomed planner doesn't fill the window"; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$task" ]]' || { echo "desk_zoom: ⌘. didn't pull the task back"; return 1; }
  check test "$(dwin d1 focused):$(dwin d1 zoomed)" == "$log:$log" || { echo "desk_zoom: the task didn't come back zoomed where it was"; kanna workspace list --all; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "desk_zoom line $LINENO"; return 1; }
  check test "$(dwin d1 zoomed)" == "$planner" || { echo "desk_zoom: the Shelf view didn't come back zoomed"; kanna workspace list --all; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$task" ]]' || { echo "desk_zoom line $LINENO"; return 1; }
  stop_app; start_app || { echo "desk_zoom line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(dwin d1 zoomed)" == "$log" ]]' || { echo "desk_zoom: the task's zoom didn't survive a relaunch"; kanna workspace list --all; return 1; }
  sleep 0.5; check shown_alone desk_zoom-task-relaunched tab || { echo "desk_zoom line $LINENO"; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "desk_zoom line $LINENO"; return 1; }
  check test "$(dwin d1 zoomed)" == "$planner" || { echo "desk_zoom: the Shelf's zoom didn't survive a relaunch"; kanna workspace list --all; return 1; }
  kanna zoom "$planner" >/dev/null || { echo "desk_zoom line $LINENO"; return 1; }
  check test "$(dwin d1 zoomed)" == None || { echo "desk_zoom: kanna zoom didn't turn the Shelf's zoom off"; return 1; }
  sleep 0.5; drew d1 desk_zoom-shelf 1 || return 1
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 zoomed)" == "$log" ]]' || { echo "desk_zoom: the task lost its zoom to the Shelf's"; kanna workspace list --all; return 1; }
  stop_agents
}

scenario desk_view_tab M4
desk_view_tab() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" planner task
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_view_tab line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "desk_view_tab line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field "$1"; }
  page() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"$1\",\"window\":\"d1\"}}" >/dev/null; check wait_for 5 eval '[[ "$(desk shownPage)" == "'"$1"'" ]]'; }
  to_task() { kanna task activate "Tab task" >/dev/null && check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$task" ]]'; }
  to_desk() { key "cmd+."; check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]'; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "desk_view_tab: no planning terminal"; kanna workspace list --all; return 1; }
  planner="$(desk lead)"
  check test "$(desk shownPage)" == Desk || { echo "desk_view_tab: a fresh Desk shows $(desk shownPage)"; return 1; }
  (cd "$repo" && kanna task new "Tab task" "hello" --agent "$agent") >/dev/null && kanna task pick $(tid 1) >/dev/null || { echo "desk_view_tab line $LINENO"; return 1; }
  kanna task activate "Tab task" >/dev/null && check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-* ]]' || { echo "desk_view_tab line $LINENO"; return 1; }
  task="$(dwin d1 dock.pane)"
  to_desk || { echo "desk_view_tab line $LINENO"; return 1; }
  check test "$(desk shownPage)" == Desk || { echo "desk_view_tab: ⌘. into a Desk view that chose no tab shows $(desk shownPage)"; return 1; }
  page "Merge queue" || { echo "desk_view_tab: clicking the Merge queue tab shows $(desk shownPage)"; return 1; }
  to_task && to_desk || { echo "desk_view_tab line $LINENO"; return 1; }
  check test "$(desk shownPage):$(desk queuePage)" == "Merge queue:True" || { echo "desk_view_tab: ⌘. came back to $(desk shownPage), not the Merge queue tab"; return 1; }
  to_task && KANNA_TERMINAL="$task" kanna task park >/dev/null && check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "desk_view_tab line $LINENO"; return 1; }
  check test "$(desk shownPage)" == "Merge queue" || { echo "desk_view_tab: kanna task park came back to $(desk shownPage)"; return 1; }
  sleep 1; stop_app; start_app || { echo "desk_view_tab line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk shownPage)" == "Merge queue" ]]' || { echo "desk_view_tab: a relaunch in the Desk view shows $(desk shownPage)"; return 1; }
  to_task || { echo "desk_view_tab line $LINENO"; return 1; }
  sleep 1; stop_app; start_app || { echo "desk_view_tab line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(dwin d1 dock.pane)" == "$task" ]]' && to_desk || { echo "desk_view_tab line $LINENO"; return 1; }
  check test "$(desk shownPage)" == "Merge queue" || { echo "desk_view_tab: ⌘. after a relaunch in the task shows $(desk shownPage)"; return 1; }
  printf '# beside the planner\n' >"$KANNA_STATE_DIR/beside.md"
  KANNA_TERMINAL="$planner" kanna open md "$KANNA_STATE_DIR/beside.md" --tab --name beside >/dev/null || { echo "desk_view_tab line $LINENO"; return 1; }
  local doc; doc="$(pane beside id)"
  planner_tab() { [[ "$(desk shownPage)" == None && "$(dwin d1 tabs.0.panes)" == "[\"$1\"]" && "$(dwin d1 tab)" == "$(dwin d1 tabs.0.id)" ]]; }
  check wait_for 5 planner_tab "$doc" || { echo "desk_view_tab: the planner's tab doesn't show: $(desk shownPage)"; return 1; }
  to_task && to_desk || { echo "desk_view_tab line $LINENO"; return 1; }
  check planner_tab "$doc" || { echo "desk_view_tab: ⌘. came back to $(desk shownPage), not the planner's tab"; return 1; }
  to_task || { echo "desk_view_tab line $LINENO"; return 1; }
  sleep 1; stop_app; start_app || { echo "desk_view_tab line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(dwin d1 dock.pane)" == "$task" ]]' && to_desk || { echo "desk_view_tab line $LINENO"; return 1; }
  check planner_tab "$doc" || { echo "desk_view_tab: ⌘. after a relaunch shows $(desk shownPage), not the planner's tab"; return 1; }
  to_task && kanna close "$doc" >/dev/null && to_desk || { echo "desk_view_tab line $LINENO"; return 1; }
  check test "$(desk shownPage):$(desk page)" == "Desk:True" || { echo "desk_view_tab: with its tab closed the Desk view shows $(desk shownPage)"; return 1; }
  page Stats || { echo "desk_view_tab line $LINENO"; return 1; }
  to_task && KANNA_TERMINAL="$planner" kanna open md "$KANNA_STATE_DIR/beside.md" --tab --name later >/dev/null && to_desk || { echo "desk_view_tab line $LINENO"; return 1; }
  check planner_tab "$(pane later id)" || { echo "desk_view_tab: a tab the planner opened behind a task doesn't show: $(desk shownPage)"; return 1; }
  page "Merge queue" || { echo "desk_view_tab line $LINENO"; return 1; }
  to_task && kanna task shelve "Tab task" >/dev/null && check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "desk_view_tab line $LINENO"; return 1; }
  check test "$(desk shownPage)" == "Merge queue" || { echo "desk_view_tab: Move to Shelf came back to $(desk shownPage)"; return 1; }
  stop_agents
}

scenario path_reuses_desk M4
path_reuses_desk() {
  start_daemon && start_app || { echo "path_reuses_desk line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" wt="$KANNA_STATE_DIR/wt" other="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/other"
  for dir in "$repo" "$other"; do mkdir -p "$dir" && git -C "$dir" init -q -b main && git -C "$dir" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "path_reuses_desk line $LINENO"; return 1; }; done
  git -C "$repo" worktree add -q -b side "$wt" && mkdir -p "$repo/sub" "$wt/deep" || { echo "path_reuses_desk line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"; wt="$(cd "$wt" && pwd -P)"; other="$(cd "$other" && pwd -P)"
  local opened; opened="$(cd / && kanna "$repo")" || { echo "path_reuses_desk line $LINENO"; return 1; }
  [[ "$opened" == "opened d1 on $repo" ]] || { echo "path_reuses_desk line $LINENO: $opened"; return 1; }
  expected="brought forward d1 on $repo"
  says kanna "$repo" || { echo "path_reuses_desk: kanna PATH twice opened another Desk"; return 1; }
  (cd "$repo/sub" && says kanna .) || { echo "path_reuses_desk: a subfolder didn't find the project's Desk"; return 1; }
  says kanna "$wt/deep" || { echo "path_reuses_desk: a folder in a worktree didn't find the project's Desk"; return 1; }
  check test "$(app_view | field windows | count)" == 1 || { echo "path_reuses_desk: more than one window"; kanna workspace list --all; return 1; }
  expected="opened d2 on $other"; says kanna "$other" || { echo "path_reuses_desk: another project didn't open a new Desk"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d2 key)" == True ]]' || { app_view; return 1; }
  local said; said="$(kanna "$repo")"
  [[ "$said" == "brought forward d1 on "*"${repo#/private}" ]] || { echo "stays_behind: kanna PATH said: $said"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || { echo "path_reuses_desk: the project's Desk isn't the key window"; app_view; return 1; }
  check test "$(app_view | field windows | count)" == 2 || { kanna workspace list --all; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead d1)" == term-* ]]' || { echo "path_reuses_desk: d1 has no planning terminal"; return 1; }
  desk_task t1 "" other || { echo "path_reuses_desk line $LINENO"; return 1; }
  expected="$(sid 1) is active on d2"; KANNA_TERMINAL="$(desk lead d1)" says kanna task activate t1 || { echo "path_reuses_desk: a task from another project went onto the caller's Desk"; return 1; }
  check eval '[[ "$(desk window.project d1)" == *"${repo#/private}" ]]' || { echo "path_reuses_desk: d1 isn't on its project"; desk window d1; return 1; }
  said="$(kanna "$repo")"
  [[ "$said" == "brought forward d1 on "*"${repo#/private}" ]] || { echo "path_reuses_desk: after another project's task, kanna PATH said: $said"; return 1; }
  check test "$(app_view | field windows | count)" == 2 || { kanna workspace list --all; return 1; }
}

scenario path_last_in_front M4
path_last_in_front() {
  start_daemon && start_app || { echo "path_last_in_front line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "path_last_in_front line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  expected="opened d1 on $repo"; says kanna "$repo" || return 1
  check wait_for 10 eval '[[ "$(desk lead d1)" == term-* ]]' || { echo "path_last_in_front: d1 has no planning terminal"; return 1; }
  local one; one="$(desk lead d1)"
  desk_task t1 "" repo && desk_task t2 "" repo || { echo "path_last_in_front line $LINENO"; return 1; }
  expected="$(sid 1) is active on d1"; KANNA_TERMINAL="$one" says kanna task activate t1 || { echo "path_last_in_front: a pull from d1's planning terminal"; return 1; }
  key cmd+n
  check wait_for 5 eval '[[ "$(dwin d2 key)" == True ]]' || { app_view; return 1; }
  check wait_for 10 eval '[[ "$(desk lead d2)" == term-* ]]' || { echo "path_last_in_front: d2 has no planning terminal"; return 1; }
  local two; two="$(desk lead d2)"
  check wait_for 10 shows "$two" "Folder, repo" || { kanna read "$two"; return 1; }
  kanna send "$two" "$repo" >/dev/null || { echo "path_last_in_front line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk window.project d2)" == *"${repo#/private}" ]]' || { echo "path_last_in_front: d2 didn't follow its planning terminal to the repo"; desk window d2; return 1; }
  expected="$(sid 2) is active on d2"; KANNA_TERMINAL="$two" says kanna task activate t2 || { echo "path_last_in_front: a pull from d2's planning terminal went elsewhere"; return 1; }
  local said; said="$(kanna "$repo")"
  [[ "$said" == "brought forward d2 on "*"${repo#/private}" ]] || { echo "path_last_in_front: two Desks on the project, d2 in front: $said"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d2 key)" == True ]]' || { app_view; return 1; }
  expected="$(sid 1) is on d1; brought it forward"; says kanna task activate t1 || return 1
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || { app_view; return 1; }
  said="$(kanna "$repo")"
  [[ "$said" == "brought forward d1 on "*"${repo#/private}" ]] || { echo "path_last_in_front: two Desks on the project, d1 in front: $said"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || { app_view; return 1; }
  expected="parked $(sid 2): d2 shows the Desk view, and $(sid 2) stays active"; KANNA_TERMINAL="$(pane t2 id)" says kanna task park || { echo "path_last_in_front: park from a task's terminal"; return 1; }
  check test "$(app_view | field windows | count)" == 2 || { kanna workspace list --all; return 1; }
}

scenario stays_behind M4
stays_behind() {
  start_daemon && start_app || { echo "stays_behind line $LINENO"; return 1; }
  front() { app_view | field key; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "stays_behind line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  expected="opened term-1 (a) in win-1, tab-1: running"; says kanna open --window new --name a || return 1
  check wait_for 5 eval '[[ "$(front)" == win-1 ]]' || { echo "stays_behind: the user's new window isn't in front"; return 1; }
  local a; a="$(pane a id)"
  expected="opened term-2 (b) in win-2, tab-2: running"; KANNA_TERMINAL="$a" says kanna open --window new --name b || return 1
  check wait_for 5 eval '[[ "$(app_view | field windows | count)" == 2 ]]' || { echo "stays_behind: win-2 never came"; return 1; }
  sleep 0.5; check test "$(front)" == win-1 || { echo "stays_behind: a new window from a Kanna terminal came forward: $(front)"; return 1; }
  expected="focused b in win-2 (behind the app you're in)"; KANNA_TERMINAL="$a" says kanna focus b || return 1
  sleep 0.5; check test "$(front)" == win-1 || { echo "stays_behind: kanna focus from a Kanna terminal brought $(front) forward"; return 1; }
  expected="opened d1 on $repo"; KANNA_TERMINAL="$a" says kanna "$repo" || return 1
  check wait_for 5 eval '[[ "$(app_view | field windows | count)" == 3 ]]' || { echo "stays_behind: d1 never came"; return 1; }
  sleep 0.5; check test "$(front)" == win-1 || { echo "stays_behind: a Desk opened from a Kanna terminal came forward: $(front)"; return 1; }
  expected="focused b in win-2 (in front)"; says kanna focus b || return 1
  check wait_for 5 eval '[[ "$(front)" == win-2 ]]' || { echo "stays_behind: the user's kanna focus didn't bring win-2 forward"; return 1; }
  local said; said="$(kanna "$repo")"
  [[ "$said" == "brought forward d1 on "*"${repo#/private}" ]] || { echo "stays_behind: kanna PATH said: $said"; return 1; }
  check wait_for 5 eval '[[ "$(front)" == d1 ]]' || { echo "stays_behind: the user's kanna PATH didn't bring d1 forward"; return 1; }
  check wait_for 5 python3 -c 'import json, sys; sys.exit(json.load(open(sys.argv[1])).get("order", [None])[:3] != ["d1", "win-2", "win-1"])' "$KANNA_STATE_DIR/app-state.json" || { echo "stays_behind: app-state.json doesn't have d1, win-2, win-1 in front"; cat "$KANNA_STATE_DIR/app-state.json"; return 1; }
}

scenario recent_projects M4
recent_projects() {
  start_daemon && start_app || { echo "recent_projects line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" other="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/other"
  for dir in "$repo" "$other"; do mkdir -p "$dir" && git -C "$dir" init -q -b main && git -C "$dir" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "recent_projects line $LINENO"; return 1; }; done
  repo="$(cd "$repo" && pwd -P)"; other="$(cd "$other" && pwd -P)"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  check test -z "$(kanna projects)" || { echo "recent_projects: a new Kanna has recent projects"; kanna projects; return 1; }
  local opened; opened="$(cd / && kanna "$repo")" && [[ "$opened" == "opened d1 on $repo" ]] || { echo "recent_projects line $LINENO: $opened"; return 1; }
  check wait_for 5 eval '[[ "$(kanna projects)" == "$repo" ]]' || { echo "recent_projects: kanna PATH didn't make a recent project"; kanna projects; return 1; }
  close_desk d1 || { echo "recent_projects line $LINENO"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"New Desk"}}' | grep -q '"ok":true' || { echo "recent_projects: New Desk didn't run"; return 1; }
  check wait_for 10 eval '[[ "$(desk lead d2)" == term-* ]]' || { echo "recent_projects: ⌘N opened no Desk"; kanna workspace list --all; return 1; }
  local planner; planner="$(desk lead d2)"
  check wait_for 10 shows "$planner" "Folder, repo, name or number \[1\]:" || { echo "recent_projects: a Desk in ~ didn't offer the recent projects"; kanna read "$planner"; return 1; }
  check shows "$planner" "1  local/repo" || { kanna read "$planner"; return 1; }
  kanna send-key "$planner" return || { echo "recent_projects line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk header d2)" == "Planning · "*"${repo#/private} · main" ]]' || { echo "recent_projects: Return didn't open the latest project"; desk header d2; kanna read "$planner"; return 1; }
  expected="opened d3 on $other"; says kanna "$other" || return 1
  check wait_for 5 eval '[[ "$(kanna projects | head -1)" == "$other" && "$(kanna projects | sed -n 2p)" == "$repo" ]]' || { echo "recent_projects: the latest isn't first"; kanna projects; return 1; }
  check wait_for 5 eval '[[ "$(dwin d3 key)" == True ]]' || { app_view; return 1; }
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.menu\"}" | grep -q "\"item\":\"local/repo\",\"menu\":\"Open Recent\""' || { echo "recent_projects: Open Recent doesn't list the project"; raw '{"id":1,"cmd":"debug.menu"}'; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.menu\",\"args\":{\"item\":\"local/repo\"}}" | grep -q '"ok":true' || { echo "recent_projects: Open Recent's item didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d2 key)" == True ]]' || { echo "recent_projects: Open Recent didn't bring the project's Desk forward"; app_view; return 1; }
  check test "$(app_view | field windows | count)" == 2 || { echo "recent_projects: Open Recent opened another Desk"; kanna workspace list --all; return 1; }
  check wait_for 5 eval '[[ "$(kanna projects | head -1)" == "$repo" ]]' || { echo "recent_projects: Open Recent didn't move it to the top"; kanna projects; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Clear Menu"}}' | grep -q '"ok":true' || { echo "recent_projects: Clear Menu didn't run"; return 1; }
  check wait_for 5 eval '[[ -z "$(kanna projects)" ]]' || { echo "recent_projects: Clear Menu left projects"; kanna projects; return 1; }
}

scenario desk_news M4
desk_news() {
  start_daemon && start_app || { echo "desk_news line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents" agent="$KANNA_STATE_DIR/agent"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_news line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/claude"
  printf '#!/bin/sh\nexec sleep 600\n' >"$agent" && chmod +x "$agent"
  printf '{"version": 1, "shelf": {"runners": 0}}\n' >"$repo/.kanna/config.json"
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  local opened d planner; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "desk_news line $LINENO: $opened"; return 1; }
  d="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$d\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { desk lead; return 1; }
  planner="$(desk lead)"
  wait_for 10 shows "$planner" "Install Kanna's skills? \[Y/n\]" && kanna send "$planner" "n" >/dev/null
  check wait_for 10 shows "$planner" "No agent Kanna knows is installed" || { kanna read "$planner"; return 1; }
  kanna send "$planner" "KANNA_AGENTS_HOME=$KANNA_AGENTS_HOME $fake/claude --dangerously-skip-permissions" >/dev/null || { echo "desk_news line $LINENO"; return 1; }
  check wait_for 10 shows "$planner" "mock-agent:" || { kanna read "$planner"; return 1; }
  as_planner() { (cd "$repo" && KANNA_TERMINAL="$planner" kanna "$@"); }
  news() { raw '{"id":1,"cmd":"tasks","args":{"action":"messages"}}' | python3 -c 'import json, sys
print("\n--\n".join(m["text"] for m in json.load(sys.stdin)["messages"] if m["from"] == "desk"))'; }
  for name in "Inked" "Planner's own" "Docs pass" "Unmerged"; do
    in_repo task new "$name" "go" --agent "$agent" >/dev/null || { echo "desk_news line $LINENO"; return 1; }
  done
  local picked wt; picked="$(as_planner task pick "$(tid 1)")" && [[ "$picked" =~ " in "([^ ]+)" (" ]] || { echo "desk_news line $LINENO: $picked"; return 1; }
  wt="${BASH_REMATCH[1]}"
  as_planner task pick "$(tid 2)" >/dev/null || { echo "desk_news line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(pane Inked id)" == term-* && "$(pane "Planner'"'"'s own" id)" == term-* ]]' || { echo "desk_news line $LINENO"; return 1; }
  sleep 3
  check test -z "$(news)" || { echo "desk_news: the planner's own picks were told back: $(news)"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"shelf\":\"$(tid 3)\",\"window\":\"$d\"}}" | grep -q '"ok":true' || { echo "desk_news: couldn't click Docs pass on the Shelf"; return 1; }
  check wait_for 10 eval '[[ "$(pane "Docs pass" id)" == term-* ]]' || { echo "desk_news: the Shelf click didn't pick Docs pass"; return 1; }
  local said_docs="$(sid 3) \"Docs pass\"" said_inked="$(sid 1) \"Inked\"" said_unmerged="$(sid 4) \"Unmerged\""
  check wait_for 10 eval '[[ "$(news)" == "$said_docs moved to the Desk" ]]' || { echo "desk_news: the Shelf click's pick wasn't told: $(news)"; return 1; }
  local docs picked_news; docs="$(pane "Docs pass" id)"; picked_news="$(news)"
  check wait_for 10 eval 'raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"task\":\"$docs\",\"window\":\"$d\"}}" | grep -q "\"ok\":true"' || { echo "desk_news: couldn't click Docs pass on the Desk"; return 1; }
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$docs" ]]' || { echo "desk_news: the click didn't make Docs pass active"; return 1; }
  sleep 3
  check test "$(news)" == "$picked_news" || { echo "desk_news: making a task active was told: $(news)"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Move to Shelf"}}' | grep -q '"ok":true' || { echo "desk_news: Move to Shelf didn't run"; return 1; }
  check wait_for 10 eval '[[ "$(news)" == "$picked_news"$'"'"'\n--\n'"'"'"$said_docs moved to the Shelf" ]]' || { echo "desk_news: the user's shelve wasn't told on its own: $(news)"; return 1; }
  check wait_for 15 unwrapped "$planner" "peer: [message from desk] $said_docs moved to the Desk" || { shows "$planner" "held peer message" && echo "desk_news: the planner held its Desk news for approval: it came without the envelope"; echo "desk_news: the planner didn't get its Desk news"; kanna read "$planner"; return 1; }
  git -C "$wt" -c user.email=e@e -c user.name=e commit -q --allow-empty -m inked && git -C "$repo" merge -q --ff-only "$(git -C "$wt" branch --show-current)" || { echo "desk_news line $LINENO"; return 1; }
  (cd "$wt" && KANNA_TERMINAL="$(pane Inked id)" kanna task done) >/dev/null || { echo "desk_news: the task's own done failed"; return 1; }
  check wait_for 15 eval '[[ "$(news)" == *"$said_inked merged into main and left the Desk" ]]' || { echo "desk_news: the task's own done wasn't told as merged: $(news)"; return 1; }
  check wait_for 15 unwrapped "$planner" "peer: [message from desk] $said_inked merged into main and left the Desk" || { echo "desk_news: the planner didn't get the merge"; kanna read "$planner"; return 1; }
  [[ "$(raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"events\",\"task\":\"$(tid 1)\"}}")" == *'"by":"term-'* ]] || { echo "desk_news: the done event lost its caller"; return 1; }
  as_planner task pick "$(tid 4)" >/dev/null || { echo "desk_news line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(pane Unmerged id)" == term-* ]]' || { echo "desk_news line $LINENO"; return 1; }
  raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"done\",\"task\":\"$(pane Unmerged id)\"}}" | grep -q '"ok":true' || { echo "desk_news: the app's done failed"; return 1; }
  check wait_for 15 eval '[[ "$(news)" == *"$said_unmerged ended and left the Desk, not merged into main" ]]' || { echo "desk_news: the app's done wasn't told: $(news)"; return 1; }
  local before; before="$(news)"
  as_planner task done "$(tid 2)" >/dev/null || { echo "desk_news line $LINENO"; return 1; }
  check wait_for 10 eval '! kanna workspace list --all --json | grep -q "Planner'"'"'s own"' || { echo "desk_news line $LINENO"; return 1; }
  sleep 4
  check test "$(news)" == "$before" || { echo "desk_news: the planner's own done was told back: $(news)"; return 1; }
}

scenario done_leaves_no_window M4
done_leaves_no_window() {
  start_daemon && start_app || { echo "done_leaves_no_window line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "done_leaves_no_window line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd "$repo" && kanna task new "With diff" "say 1" --agent "$agent") >/dev/null || { echo "done_leaves_no_window line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null || { echo "done_leaves_no_window line $LINENO"; return 1; }
  local task; task="$(pane "With diff" id)"
  (cd "$repo" && KANNA_TERMINAL="$task" kanna open diff --merge-base main --name changes --tab) >/dev/null || { echo "done_leaves_no_window line $LINENO"; return 1; }
  stop_daemon; start_daemon && check wait_for 10 app_attached || { echo "done_leaves_no_window line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(pane changes type)" == diff ]]' || { echo "done_leaves_no_window: the diff tab didn't survive the daemon's restart"; kanna workspace list --all; return 1; }
  kanna task done "$(tid 1)" >/dev/null || { echo "done_leaves_no_window line $LINENO"; return 1; }
  leftovers() { kanna workspace list --all --json | python3 -c 'import json, sys; s = json.load(sys.stdin); print(len(s["windows"]) + len(s.get("hiddenWindows") or []), len(s["panes"]))'; }
  check wait_for 5 eval '[[ "$(leftovers)" == "0 0" ]]' || { echo "done_leaves_no_window: windows and panes left: $(leftovers)"; kanna workspace list --all; return 1; }
}

scenario restart_while_desk_opens M4
restart_while_desk_opens() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "restart_while_desk_opens line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon && start_app || { echo "restart_while_desk_opens line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  local daemon opened w planner
  daemon="$(cat "$KANNA_STATE_DIR/daemon.pid")"
  opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "restart_while_desk_opens line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk lead "$w")" == term-* ]]' || { echo "restart_while_desk_opens: no planning terminal"; kanna workspace list --all; return 1; }
  planner="$(desk lead "$w")"
  kill "$daemon" || { echo "restart_while_desk_opens line $LINENO"; return 1; }
  wait_for 5 eval "! kill -0 $daemon" || { echo "restart_while_desk_opens: the daemon didn't stop"; return 1; }
  kanna workspace list --all >/dev/null || { echo "restart_while_desk_opens: a call made while no daemon ran wasn't answered"; return 1; }
  check wait_for 15 eval '[[ "$(cat "$KANNA_STATE_DIR/daemon.pid")" != "$daemon" ]] && app_attached' || { echo "restart_while_desk_opens: the app didn't start the next daemon"; return 1; }
  check wait_for 20 shows "$planner" "No agent Kanna knows is installed" || { echo "restart_while_desk_opens: the planning terminal didn't get past agent setup"; kanna read "$planner"; return 1; }
  ! shows "$planner" "nothing answers" || { echo "restart_while_desk_opens: the planning terminal ran while nothing answered"; kanna read "$planner"; return 1; }
}
