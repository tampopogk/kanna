scenario task_shelf M4
task_shelf() {
  start_daemon && start_app || { echo "task_shelf line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_shelf line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  local added; added="$(in_repo task new "Fix it" "hello there" --agent "$agent" 2>&1)"
  [[ "$added" =~ ^"added "(task-[0-9a-f]{4,8})" \"Fix it\" to the Shelf of $repo"$ && "${BASH_REMATCH[1]}" == "$(sid 1)" ]] || { echo "task_shelf line $LINENO: $added"; return 1; }
  in_repo task new "Plain" >/dev/null || { echo "task_shelf line $LINENO"; return 1; }
  [[ "$(cd "$KANNA_STATE_DIR" && kanna task new "Nowhere" 2>&1)" == *"belongs to a git repo"* ]] || { echo "task_shelf line $LINENO"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 1)"' +Fix it +Shelf +stopped$' || { in_repo task list; return 1; }
  expected="took $(sid 2) off the Shelf"; says kanna task done $(tid 2) || { echo "task_shelf line $LINENO"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "task_shelf line $LINENO"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 1)"' +Fix it +Shelf' || { echo "the Shelf didn't survive a restart"; in_repo task list; return 1; }
  local started wt; started="$(kanna task pick $(tid 1))" || { echo "task_shelf line $LINENO"; return 1; }
  [[ "$started" =~ ^"moved $(sid 1) to the Desk: term-1 in $repo/.kanna-worktrees/"(task-[0-9a-f]{8})" ("(task-[0-9a-f]{8})")"$ && "${BASH_REMATCH[1]}" == "${BASH_REMATCH[2]}" ]] || { echo "task_shelf line $LINENO: $started"; return 1; }
  wt="$repo/.kanna-worktrees/${BASH_REMATCH[1]}"
  check wait_for 5 eval '[[ "$(pane "Fix it" id)" == term-1 ]]' || { echo "task_shelf line $LINENO"; return 1; }
  check test "$(git -C "$wt" branch --show-current)" == "${wt##*/}" || { echo "task_shelf line $LINENO"; return 1; }
  check test -z "$(git -C "$repo" status --porcelain)" || { echo "task_shelf line $LINENO"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 1)"' +Fix it +Desk' || { in_repo task list; return 1; }
  check wait_for 5 is "Fix it" status idle || { echo "task_shelf line $LINENO"; return 1; }
  KANNA_TERMINAL=term-1 kanna task send planner "done" >/dev/null || { echo "task_shelf line $LINENO"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 1)"' +Fix it +Desk +replied ' || { echo "task_shelf: a message waiting for the planner isn't replied"; in_repo task list; return 1; }
  git -C "$wt" -c user.email=e@e -c user.name=e commit -q --allow-empty -m work || { echo "task_shelf line $LINENO"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 1)"' +Fix it +Desk +replied ' || { echo "task_shelf: an unmerged task reads as merged"; in_repo task list; return 1; }
  git -C "$repo" -c user.email=e@e -c user.name=e merge -q --no-ff -m merged "${wt##*/}" || { echo "task_shelf line $LINENO"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 1)"' +Fix it +Desk +merged ' || { echo "task_shelf: a task merged into main isn't merged"; in_repo task list; return 1; }
  in_repo --json task list | grep -q '"merged":true' || { echo "task_shelf: --json doesn't say merged"; return 1; }
  expected="named $(sid 1) \"Fixed\""; says kanna task name term-1 Fixed || { echo "task_shelf line $LINENO"; return 1; }
  for n in 3 4 5; do in_repo task new "More $n" --agent "$agent" >/dev/null || return 1; done
  kanna task pick $(tid 3) >/dev/null && kanna task pick $(tid 4) >/dev/null || { echo "task_shelf line $LINENO"; return 1; }
  kanna task pick $(tid 5) 2>"$KANNA_STATE_DIR/task_shelf-limit" && { echo "task_shelf line $LINENO"; return 1; }
  grep -q "has 3 tasks on the Desk" "$KANNA_STATE_DIR/task_shelf-limit" || { cat "$KANNA_STATE_DIR/task_shelf-limit"; return 1; }
  in_repo task list | placed | grep -q '^'"$(sid 5)"' .*Shelf' || { echo "task_shelf line $LINENO"; return 1; }
  check test "$(ls "$repo/.kanna-worktrees" | wc -l | tr -d ' ')" == 3 || { echo "task_shelf line $LINENO"; return 1; }
  in_repo open --window new --name plain >/dev/null || { echo "task_shelf line $LINENO"; return 1; }
  in_repo move plain --dock 2>"$KANNA_STATE_DIR/task_shelf-dock" && { echo "task_shelf line $LINENO"; return 1; }
  grep -q "plain isn't a task, and only a task's agent docks" "$KANNA_STATE_DIR/task_shelf-dock" || { cat "$KANNA_STATE_DIR/task_shelf-dock"; return 1; }
  in_repo task list | placed | grep -q ' plain$' && { echo "task_shelf line $LINENO"; return 1; }
  kanna task activate Fixed >/dev/null || { echo "task_shelf line $LINENO"; return 1; }
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys; print([w["dock"].get("frozen") for w in json.load(sys.stdin)["workspaces"] if w["dock"]["id"] == "term-1"])')" == '[True]' || { kanna workspace list --all; return 1; }
  kanna move Fixed --dock right 2>"$KANNA_STATE_DIR/task_shelf-frozen" && { echo "task_shelf line $LINENO"; return 1; }
  grep -q "frozen: Fixed (term-1) is frozen in its dock and doesn't move" "$KANNA_STATE_DIR/task_shelf-frozen" || { cat "$KANNA_STATE_DIR/task_shelf-frozen"; return 1; }
  kanna move plain --to Fixed 2>"$KANNA_STATE_DIR/task_shelf-swap" && { echo "task_shelf line $LINENO"; return 1; }
  grep -q "Fixed (term-1) is frozen in .*'s dock; nothing drops on it" "$KANNA_STATE_DIR/task_shelf-swap" || { cat "$KANNA_STATE_DIR/task_shelf-swap"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 1)"' +Fixed ' || { in_repo task list; return 1; }
  kanna close plain >/dev/null || { echo "task_shelf line $LINENO"; return 1; }
  kanna task done Fixed >/dev/null || { echo "task_shelf line $LINENO"; return 1; }
  kanna task pick $(tid 5) >/dev/null || { echo "task_shelf line $LINENO"; return 1; }
  check wait_for 10 eval '[[ ! -e "$wt" ]]' || { echo "task_shelf: the done task's worktree, merged and clean, was kept"; return 1; }
}

scenario desk_shelf M4
desk_shelf() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_shelf line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  mkdir -p "$repo/.kanna" && printf '{"planning": {"context": 400000}}\n' >"$repo/.kanna/config.json"
  rm "$KANNA_AGENTS_HOME/.kanna-v4/agents.json"
  start_daemon || { echo "desk_shelf line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"${2:-d1}\"}}" | field "$1"; }
  local planner; check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { desk lead; kanna workspace list --all; return 1; }
  planner="$(desk lead)"
  check test "$(desk page)" == True || { echo "desk_shelf line $LINENO"; return 1; }
  check test "$(desk strip)" == False || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 10 shows "$planner" "Install Kanna's skills? \[Y/n\]" || { echo "desk_shelf: the planning terminal didn't ask to install Kanna's skills"; kanna read "$planner"; return 1; }
  check shows "$planner" "Kanna needs its skills installed" || { kanna read "$planner"; return 1; }
  kanna send "$planner" "y" >/dev/null || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 10 shows "$planner" "Folder, repo or name:" || { echo "desk_shelf: the planning terminal in ~ didn't ask where"; kanna read "$planner"; return 1; }
  check shows "$planner" "Kanna's skills are installed" || { kanna read "$planner"; return 1; }
  check test -f "$KANNA_AGENTS_HOME/.agents/skills/kanna-tasks/SKILL.md" || { echo "desk_shelf: answering yes didn't install the skills"; return 1; }
  check grep -q "$KANNA_SOCKET" "$KANNA_AGENTS_HOME/.claude/settings.json" || return 1
  ! grep -q SessionStart "$KANNA_AGENTS_HOME/.claude/settings.json" || { echo "desk_shelf: setup installed a SessionStart hook"; return 1; }
  check test "$(cd "$(pane "$planner" cwd)" && pwd -P)" == "$(cd ~ && pwd -P)" || { echo "desk_shelf: Kanna started from no folder should plan in ~"; return 1; }
  check test "$(kanna task list --all | grep -c "$planner")" == 0 || { echo "desk_shelf: the planning terminal is listed as a task"; kanna task list; return 1; }
  kanna send "$planner" "$repo" >/dev/null || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk header)" == "Planning · "*"${repo#/private} · main" ]]' || { desk header; return 1; }
  check wait_for 5 shows "$planner" "No agent Kanna knows is installed" || { kanna read "$planner"; return 1; }
  kanna send "$planner" 'echo "planner=${KANNA_PLANNER-none} context=$CLAUDE_CODE_AUTO_COMPACT_WINDOW"' >/dev/null || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 5 shows "$planner" "planner=none context=300000" || { kanna read "$planner"; return 1; }
  (cd "$repo" && KANNA_TERMINAL="$planner" kanna task new "First" "hello" && KANNA_TERMINAL="$planner" kanna task new "Second") >/dev/null || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk shelf)" == "[\"$(tid 1)\", \"$(tid 2)\"]" && "$(desk page)" == True ]]' || { desk shelf; return 1; }
  sleep 0.5; snap d1 desk_shelf-planner >/dev/null
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 1)"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk shelf)" == "[\"$(tid 2)\"]" && "$(desk tasks.0.title)" == First ]]' || { desk tasks; return 1; }
  check test "$(desk running)" == 1 || { echo "desk_shelf: the planning terminal counts as a running task"; desk running; return 1; }
  check wait_for 5 shows First "mock-agent" || { echo "desk_shelf line $LINENO"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"task":"'"$(pane First id)"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$(pane First id)" && "$(desk lead)" == None && "$(desk page)" == False && "$(desk strip)" == True ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  check test "$(desk waiting)" == "1 on the Shelf" || { desk waiting; return 1; }
  is "$planner" state running || { echo "desk_shelf: the hidden planning terminal stopped"; return 1; }
  kanna task park First >/dev/null || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk lead)" == "$planner" && "$(desk page)" == True && "$(desk strip)" == False ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  check shows "$planner" "planner=none context=300000" || { echo "desk_shelf: the planning terminal lost its scrollback"; return 1; }
  [[ "$(desk header)" == "Planning · "*"${repo#/private} · main" ]] || { desk header; return 1; }
  kanna task pick $(tid 2) >/dev/null || { echo "desk_shelf line $LINENO"; return 1; }
  (cd "$repo" && kanna task new "Third" && kanna task new "Fourth") >/dev/null || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk running)" == 2 && "$(desk shelf)" == "[\"$(tid 3)\", \"$(tid 4)\"]" ]]' || { desk running; desk shelf; return 1; }
  strip() { desk stripLayout | python3 -c 'import json, sys; print(" ".join(i["task"] + ":" + str(i["key"]) for i in json.load(sys.stdin)["items"]))'; }
  docked() { [[ "$(desk window.dock.pane)" == "$1" ]]; }
  key cmd+2
  check wait_for 5 docked "$(pane Second id)" || { echo "desk_shelf: with two tasks ⌘2 didn't switch to the other one"; return 1; }
  check wait_for 5 eval '[[ "$(strip)" == "$(pane First id):⌘1" ]]' || { echo "desk_shelf: with two tasks the other shows ⌘1"; strip; return 1; }
  key cmd+2
  check wait_for 5 docked "$(pane First id)" || { echo "desk_shelf: with two tasks ⌘2 twice didn't go back"; return 1; }
  key cmd+.
  check wait_for 5 eval '[[ "$(desk page)" == True ]]' || { echo "desk_shelf line $LINENO"; return 1; }
  key cmd+t; key cmd+d
  check test "$(desk window.tabs)" == "[]" || { echo "desk_shelf: the Shelf got a tab"; desk window.tabs; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 3)"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk running)" == 3 && "$(desk shelf)" == "[\"$(tid 4)\"]" ]]' || { desk running; desk shelf; return 1; }
  check test "$(desk window.dock)" == None || { echo "desk_shelf: starting a Shelf task docked it"; return 1; }
  local first second third; first="$(pane First id)"; second="$(pane Second id)"; third="$(pane Third id)"
  rows() { desk rows | python3 -c 'import json, sys
print(" ".join(r["task"] + (":active" if r["active"] else "") + (":lit" if r["highlit"] else "") + (":" + r["go"] if r["goShown"] else "") for r in json.load(sys.stdin)))'; }
  check wait_for 5 eval '[[ "$(rows)" == "$first:active:lit $second $third" ]]' || { echo "desk_shelf: the Desk view lists its tasks oldest first, the active one in its place, with no hint until hovered"; rows; return 1; }
  key cmd+1
  check wait_for 5 docked "$third" || { echo "desk_shelf: ⌘1 isn't the task that came to the Desk last"; return 1; }
  check wait_for 5 eval '[[ "$(strip)" == "$first:⌘1 $second:⌘2" ]]' || { echo "desk_shelf: the strip lists the others in key order, ⌘1 first"; strip; return 1; }
  key cmd+1
  check wait_for 5 docked "$first" || { echo "desk_shelf: ⌘1 twice didn't go back to the first task"; return 1; }
  check wait_for 5 eval '[[ "$(strip)" == "$third:⌘1 $second:⌘2" ]]' || { echo "desk_shelf: the strip doesn't reorder by recency when you switch tasks"; strip; return 1; }
  key cmd+2
  check wait_for 5 docked "$second" || { echo "desk_shelf: ⌘2 (1)"; return 1; }
  key cmd+2
  check wait_for 5 docked "$third" || { echo "desk_shelf: ⌘2 (2)"; return 1; }
  key cmd+2
  check wait_for 5 docked "$first" || { echo "desk_shelf: with three tasks ⌘2 three times didn't visit every task"; return 1; }
  key cmd+3; sleep 0.5
  check docked "$first" || { echo "desk_shelf: ⌘3 made a task active"; return 1; }
  key cmd+.
  check wait_for 5 eval '[[ "$(desk page)" == True && "$(rows)" == "$first:active:lit $second $third" ]]' || { echo "desk_shelf: the active task moved in the Desk view"; rows; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 4)"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk note)" == "The Desk is full: move a task to the Shelf first (⇧⌘⌫ on its row)." ]]' || { desk note; return 1; }
  stop_app; start_app || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk lead)" == "$planner" ]]' || { echo "desk_shelf: the planning terminal didn't survive the app's restart"; kanna workspace list --all; return 1; }
  local refused; refused="$(raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"blank\",\"repo\":\"$repo\"}}")"
  [[ "$refused" == *'"ok":false'* && "$refused" == *"tasks needs an action"* ]] || { echo "desk_shelf: the Shelf still starts blank tasks: $refused"; return 1; }
  key cmd+shift+w
  check wait_for 5 eval '! kanna workspace list --all --json | grep -q "\"$planner\""' || { echo "desk_shelf: closing the window left its planning terminal running"; kanna workspace list --all; return 1; }
}

scenario move_to_shelf M4
move_to_shelf() {
  start_daemon && start_app || { echo "move_to_shelf line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"${2:-d1}\"}}" | field "$1"; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "move_to_shelf line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd "$repo" && kanna task new "Later" "hello there" --agent "$agent") >/dev/null || { echo "move_to_shelf line $LINENO"; return 1; }
  local started wt; started="$(kanna task pick $(tid 1))" || { echo "move_to_shelf line $LINENO"; return 1; }
  [[ "$started" =~ " in "([^ ]+)" (" ]] && wt="${BASH_REMATCH[1]/#\~/$HOME}" || { echo "move_to_shelf line $LINENO: $started"; return 1; }
  check wait_for 5 eval '[[ "$(pane Later id)" == term-1 ]]' || { echo "move_to_shelf line $LINENO"; return 1; }
  touch "$wt/work-in-progress"
  (cd "$repo" && kanna task new "Newer" "hello there" --agent "$agent") >/dev/null || { echo "move_to_shelf line $LINENO"; return 1; }
  local pid; pid="$(pane Later pid)"
  expected="moved term-1 to the Shelf as $(sid 1), still running: \`kanna task pick $(sid 1)\` brings it back"; says kanna task shelve Later || { echo "move_to_shelf line $LINENO"; return 1; }
  check test "$(ws $(tid 1) where)" == shelf || { echo "move_to_shelf: $(sid 1)'s workspace isn't on the Shelf"; state; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "move_to_shelf: the deferred agent stopped"; kanna workspace list --all; return 1; }
  (cd "$repo" && kanna task list) | placed | grep -qE '^'"$(sid 1)"' +Later +Shelf' || { (cd "$repo" && kanna task list); return 1; }
  (cd "$repo" && kanna task list) | placed | grep -qE '^'"$(sid 1)"' .* Desk ' && { echo "move_to_shelf: the deferred task is still on the Shelf"; (cd "$repo" && kanna task list); return 1; }
  check test "$( (cd "$repo" && kanna task list) | grep -oE '^task-[0-9a-f]+' | tr '\n' ' ')" == "$(sid 1) $(sid 2) " || { echo "move_to_shelf: the shelved task isn't listed first, though it's the oldest"; (cd "$repo" && kanna task list); return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^(opened|brought\ forward)\ (d[0-9]+) ]] || { echo "move_to_shelf line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[2]}"
  check wait_for 5 eval '[[ "$(desk empty "$w")" == True && "$(desk shelf "$w")" == "[\"$(tid 1)\", \"$(tid 2)\"]" ]]' || { echo "move_to_shelf: the empty Desk doesn't list the oldest first"; desk shelf "$w"; return 1; }
  (cd "$repo" && kanna task done $(tid 2)) >/dev/null || { echo "move_to_shelf line $LINENO"; return 1; }
  check test -f "$wt/work-in-progress" || { echo "move_to_shelf line $LINENO"; return 1; }
  started="$(kanna task pick $(tid 1))" || { echo "move_to_shelf line $LINENO"; return 1; }
  [[ "$started" == "moved $(sid 1) to the Desk: term-1 in ${wt/#$HOME/\~} ("* ]] || { echo "move_to_shelf: it didn't come back as it was: $started"; return 1; }
  check test "$(ws $(tid 1) where)" == '{"desk": "mac"}' || { echo "move_to_shelf line $LINENO"; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "move_to_shelf: the agent was started again"; kanna workspace list --all; return 1; }
  check test "$(ls "$repo/.kanna-worktrees" | wc -l | tr -d ' ')" == 1 || { echo "move_to_shelf line $LINENO"; return 1; }
  kanna open --name mine --window new >/dev/null || { echo "move_to_shelf line $LINENO"; return 1; }
  local refused; refused="$(kanna task shelve mine 2>&1)" && { echo "move_to_shelf: a pane that isn't a task was shelved: $refused"; return 1; }
  [[ "$refused" == *'no running task "mine"'* ]] || { echo "move_to_shelf: shelving a pane that isn't a task should say why: $refused"; return 1; }
  [[ "$(kanna task shelve nothing 2>&1)" == *'no running task "nothing"'* ]] || { echo "move_to_shelf line $LINENO"; return 1; }
}

scenario drag_to_shelf M4
drag_to_shelf() {
  start_daemon && start_app || return 1
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  drag_row() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"task\":\"$1\",\"window\":\"d1\",\"drop\":\"$2\"}}" | grep -q '"ok":true'; sleep 0.5; }
  desk_task first && desk_task second || return 1
  local front back pid
  front="$(pane first id)" back="$(pane second id)" pid="$(pane second pid)"
  kanna task activate first >/dev/null && kanna task park first >/dev/null || return 1
  check wait_for 5 eval '[[ "$(desk page)" == True && "$(desk active)" == "$front" && "$(desk tasks.1.task)" == "$back" ]]' || { echo "drag_to_shelf: the Desk view doesn't show both tasks"; raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  drag_row "$back" desk || { echo "drag_to_shelf: the drag onto the Desk didn't run"; return 1; }
  check test "$(ws $(tid 2) where)" == '{"desk": "mac"}' || { echo "drag_to_shelf: a drag dropped on the Desk moved the task"; state; return 1; }
  check test "$(desk active)" == "$front" || { echo "drag_to_shelf: a drag dropped on the Desk made the task active"; return 1; }
  drag_row "$back" shelf || { echo "drag_to_shelf: the drag onto the Shelf didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(ws $(tid 2) where)" == shelf ]]' || { echo "drag_to_shelf: the dragged task isn't on the Shelf"; state; return 1; }
  check test "$(pane second pid)" == "$pid" || { echo "drag_to_shelf: the dragged task's agent stopped"; kanna workspace list --all; return 1; }
  check wait_for 5 eval '[[ "$(desk shelf)" == "[\"$(tid 2)\"]" && "$(desk tasks.0.task)" == "$front" && "$(desk tasks.1)" == "" ]]' || { echo "drag_to_shelf: the Desk view didn't move the row to the Shelf"; raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  check test "$(desk active)" == "$front" || { echo "drag_to_shelf: shelving another task changed the active one"; return 1; }
  check test "$(desk page)" == True || { echo "drag_to_shelf: the Desk view stopped showing"; return 1; }
  drag_row "$front" shelf || { echo "drag_to_shelf: the drag of the active task didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(ws $(tid 1) where)" == shelf && "$(desk active)" == None && "$(desk page)" == True ]]' || { echo "drag_to_shelf: shelving the active task should leave none active, the Desk view showing"; raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
}

scenario shelf_row_status M4
shelf_row_status() {
  start_daemon && start_app || { echo "shelf_row_status line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_row_status line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  in_repo task new "Runs" "hello there" --agent "$agent" >/dev/null && in_repo task new "Waits" "hello there" --agent "$agent" --after "$(tid 1)" >/dev/null && in_repo task new "Stays" "hello there" --agent "$agent" >/dev/null || { echo "shelf_row_status line $LINENO"; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "shelf_row_status line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  dot() { local n; for n in 0 1 2; do [[ "$(desk scroll.dots.$n.task)" == "$1" ]] && desk scroll.dots.$n.dot; done; }
  check wait_for 10 eval '[[ "$(dot $(tid 1))" == circle && "$(dot $(tid 2))" == dashed && "$(dot $(tid 3))" == circle ]]' || { echo "shelf_row_status: stopped and blocked rows should show circles"; desk scroll; return 1; }
  kanna task start "$(tid 1)" >/dev/null || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(pane Runs id)" == term-* ]]' || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(dot $(tid 1))" == idle || "$(dot $(tid 1))" == working ]]' || { echo "shelf_row_status: a running Shelf task has no dot"; desk scroll; return 1; }
  check wait_for 10 is Runs status idle || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dot $(tid 1))" == idle ]]' || { echo "shelf_row_status: an idle Shelf task's dot isn't idle"; desk scroll; return 1; }
  kanna send Runs turn >/dev/null || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dot $(tid 1))" == working ]]' || { echo "shelf_row_status: a working Shelf task's dot isn't working"; desk scroll; return 1; }
  check wait_for 10 eval '[[ "$(dot $(tid 1))" == needs-you ]]' || { echo "shelf_row_status: the Shelf task's dot doesn't show it needs you"; desk scroll; return 1; }
  sleep 0.5; snap "$w" shelf_row_status-needs >/dev/null
  check test "$(dot $(tid 2))" == dashed || { echo "shelf_row_status: the blocked row changed"; desk scroll; return 1; }
  check test "$(dot $(tid 3))" == circle || { echo "shelf_row_status: the stopped row changed"; desk scroll; return 1; }
  kanna task stop "$(tid 1)" >/dev/null || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(in_repo task list | placed | grep -cE "Runs +Shelf +stopped")" == 1 ]]' || { echo "shelf_row_status: the task didn't stop"; in_repo task list; return 1; }
  sleep 0.5
  check test "$(dot $(tid 1))" == needs-you || { echo "shelf_row_status: a task stopped while it needed you lost its dot"; desk scroll; return 1; }
  stop_app; start_app || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(dot $(tid 1))" == needs-you ]]' || { echo "shelf_row_status: the dot didn't survive restarting the app"; desk scroll; return 1; }
  kanna task start "$(tid 1)" >/dev/null || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 10 is Runs status idle || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dot $(tid 1))" == idle ]]' || { echo "shelf_row_status: a restarted task doesn't show its status"; desk scroll; return 1; }
  kanna task stop "$(tid 1)" >/dev/null || { echo "shelf_row_status line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(in_repo task list | placed | grep -cE "Runs +Shelf +stopped")" == 1 ]]' || { echo "shelf_row_status: the task didn't stop"; in_repo task list; return 1; }
  sleep 0.5
  check test "$(dot $(tid 1))" == idle || { echo "shelf_row_status: a task stopped while idle lost its dot"; desk scroll; return 1; }
}

scenario shelf_and_back M4
shelf_and_back() {
  start_daemon && start_app || { echo "shelf_and_back line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_and_back line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  local w
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  (cd "$repo" && kanna task new "Later" "hello there" --agent "$agent") >/dev/null || { echo "shelf_and_back line $LINENO"; return 1; }
  kanna task pick $(tid 1) >/dev/null || { echo "shelf_and_back line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(pane Later id)" == term-* ]]' || { echo "shelf_and_back line $LINENO"; return 1; }
  local pulled; pulled="$(kanna task activate Later)" && [[ "$pulled" =~ " active on "(d[0-9]+)$ ]] || { echo "shelf_and_back line $LINENO: $pulled"; return 1; }
  w="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk window.dock.pane)" == "$(pane Later id)" ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"'"$w"'"}}'; return 1; }
  check test "$(ws $(tid 1) where)" == '{"desk": "mac"}' || { echo "shelf_and_back: $(sid 1)'s workspace isn't on the app's Shelf"; state; return 1; }
  check wait_for 5 shows Later "mock-agent" || { echo "shelf_and_back line $LINENO"; return 1; }
  local task pid; task="$(pane Later id)" pid="$(pane Later pid)"
  KANNA_TERMINAL="$task" kanna open --name side --cmd 'echo SIDE-READY; exec cat' >/dev/null || { echo "shelf_and_back line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin "$w" tabs)" == *"$(pane side id)"* ]]' || { echo "shelf_and_back: the side terminal isn't on the task's Desk"; kanna workspace list --all --json; return 1; }
  local tabs; tabs="$(dwin "$w" tabs)"
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Move to Shelf"}}' | grep -q '"ok":true' || { echo "shelf_and_back: Move to Backlog didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(desk empty)" == True && "$(desk shelf)" == "[\"$(tid 1)\"]" && "$(desk tasks)" == "[]" && "$(desk running)" == 0 ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"'"$w"'"}}'; return 1; }
  check test "$(desk window.dock)" == None || { echo "shelf_and_back: the task is still on the Desk"; return 1; }
  check test "$(ws $(tid 1) where)" == shelf || { echo "shelf_and_back line $LINENO"; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "shelf_and_back: Move to Backlog stopped the agent"; return 1; }
  kanna send Later "work 1" >/dev/null || { echo "shelf_and_back line $LINENO"; return 1; }
  check wait_for 10 shows Later "WORK_DONE" || { echo "shelf_and_back: the agent didn't keep working on the Shelf"; kanna read Later; return 1; }
  is side state running || { echo "shelf_and_back: the task's tab stopped on the Shelf"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "shelf_and_back line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk shelf)" == "[\"$(tid 1)\"]" && "$(desk tasks)" == "[]" ]]' || { echo "shelf_and_back: the Shelf task didn't survive a restart"; raw '{"id":1,"cmd":"debug.desk","args":{"window":"'"$w"'"}}'; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "shelf_and_back: the restart stopped the agent"; return 1; }
  check test "$(ws $(tid 1) where)" == shelf || { echo "shelf_and_back: $(sid 1)'s workspace didn't come back on the Shelf"; state; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 1)"'","window":"'"$w"'"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk shelf)" == "[]" && "$(desk tasks.0.task)" == "$task" ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"'"$w"'"}}'; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "shelf_and_back: Move to Shelf started the agent again"; return 1; }
  kanna task activate Later >/dev/null || { echo "shelf_and_back line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$task" ]]' || { echo "shelf_and_back line $LINENO"; return 1; }
  check test "$(dwin "$w" tabs)" == "$tabs" || { echo "shelf_and_back: the task's tabs didn't come back as they were"; dwin "$w" tabs; return 1; }
  check shows side "SIDE-READY" || { echo "shelf_and_back: the side terminal lost its screen"; return 1; }
}

scenario shelf_search M4
shelf_search() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_search line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "shelf_search line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  search() { desk search."$1"; }
  type() { raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$1\"}}" >/dev/null; sleep 0.3; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "shelf_search: no planning terminal"; kanna workspace list --all; return 1; }
  check test "$(search shown)" == False || { echo "shelf_search: a search field over an empty Shelf"; return 1; }
  (cd "$repo" && kanna task new "Fix flaky status test" "agent_status in the e2e waits for working; follow the status" --agent "$agent" \
    && kanna task new "Resume every agent's session" "Give codex its own session ID and resume it" --agent "$agent" \
    && kanna task new "Shelf strip shows parked time" "Show how long ago each item was parked" --agent "$agent" \
    && kanna task new "Start tasks past the Shelf" "Let a fourth task wait for a slot" --agent "$agent") >/dev/null || { echo "shelf_search line $LINENO"; return 1; }
  for i in $(seq 5 12); do (cd "$repo" && kanna task new "Task $i" "hello" --agent "$agent") >/dev/null || { echo "shelf_search line $LINENO"; return 1; }; done
  check wait_for 5 eval '[[ "$(desk shelf)" == *"$(tid 12)"* && "$(search shown)" == True ]]' || { desk shelf; desk search; return 1; }
  key cmd+f
  check test "$(search focused)" == True || { echo "shelf_search: ⌘F didn't reach the Shelf search"; desk responder; return 1; }
  local at; at="$(search left),$(search right)"
  type shelf
  check test "$(search left),$(search right)" == "$at" || { echo "shelf_search: the search field moved as the count changed"; desk search; return 1; }
  check test "$(search found)" == '["'"$(tid 3)"'", "'"$(tid 4)"'"]' || { desk search; return 1; }
  check test "$(search selected)" == 0 || { desk search; return 1; }
  type " parked"
  check test "$(search found)" == '["'"$(tid 3)"'"]' || { desk search; return 1; }
  key escape
  check test "$(search query)" == "" || { echo "shelf_search: esc didn't clear the search"; desk search; return 1; }
  check test "$(search found | grep -o '"t' | wc -l | tr -d ' ')" == 12 || { desk search; return 1; }
  check test "$(search focused)" == True || { desk search; return 1; }
  key escape
  check test "$(search focused)" == False || { echo "shelf_search: a second esc stayed in the search"; desk search; return 1; }
  check test "$(desk responder)" == TerminalSurfaceView || { echo "shelf_search: a second esc didn't go back to the planning terminal"; desk responder; return 1; }
  key cmd+f; type "RESUME codex"
  check test "$(search found)" == '["'"$(tid 2)"'"]' || { echo "shelf_search: no match on a first message, ignoring case"; desk search; return 1; }
  type " nothing"
  check test "$(search found)" == '[]' || { desk search; return 1; }
  key escape; type "$(sid 12 | tr a-z A-Z)"
  check test "$(search found)" == '["'"$(tid 12)"'"]' || { echo "shelf_search: ⌘F didn't find a task by its ID"; desk search; return 1; }
  type " hello"
  check test "$(search found)" == '["'"$(tid 12)"'"]' || { echo "shelf_search: ⌘F didn't find a task by its ID and a word"; desk search; return 1; }
  key escape; type shelf
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.3; snap d1 shelf_search-light >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.3; snap d1 shelf_search-dark >/dev/null
  key down
  check test "$(search selected)" == 1 || { desk search; return 1; }
  key down
  check test "$(search selected)" == 1 || { echo "shelf_search: the selection ran past the last match"; desk search; return 1; }
  key up
  check test "$(search selected)" == 0 || { desk search; return 1; }
  key down; key return
  check wait_for 10 eval '[[ "$(desk shelf)" != *"$(tid 4)"* && "$(desk tasks.0.title)" == "Start tasks past the Shelf" ]]' || { desk shelf; desk tasks; desk note; return 1; }
  check wait_for 5 eval '[[ "$(search query)" == "" ]]' || { echo "shelf_search: the search stayed after starting its task"; desk search; return 1; }
  local listed; listed="$(cd "$repo" && kanna task list shelf | placed)"
  check test "$(wc -l <<<"$listed" | tr -d ' ')" == 2 || { echo "$listed"; return 1; }
  [[ "$listed" == *"Start tasks past the Shelf"*"Desk"* && "$listed" == *"$(sid 3)"*"Shelf"* ]] || { echo "shelf_search: kanna task list QUERY"; echo "$listed"; return 1; }
  listed="$(cd "$repo" && kanna task list "$(sid 12)")"
  [[ "$(wc -l <<<"$listed" | tr -d ' ')" == 2 && "$listed" == "Shelf"*"$(sid 12)"*"Task 12"* ]] || { echo "shelf_search: kanna task list TASK"; echo "$listed"; return 1; }
  local done; done="$(sid 11)"
  (cd "$repo" && kanna task done "$done") >/dev/null || { echo "shelf_search line $LINENO"; return 1; }
  listed="$(cd "$repo" && kanna task list "$done")"
  [[ "$listed" == "$done \"Task 11\" is done (just now)" ]] || { echo "shelf_search: kanna task list on a done task's ID"; echo "$listed"; return 1; }
}

scenario task_start_stop M4
task_start_stop() {
  local logged; logged="$(cat "$out/daemon.log" 2>/dev/null | wc -l)"
  start_daemon && start_app || { echo "task_start_stop line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" || { echo "task_start_stop line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_start_stop line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  local n listed
  for n in 1 2 3 4 5; do (cd "$repo" && kanna task new "Task $n" "hello $n" --agent "$agent") >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }; done
  for n in 1 2 3 4 5; do
    [[ "$(kanna task start "$(tid $n)")" == "started $(sid $n) on the Shelf: term-"* ]] || { echo "task_start_stop: $(sid $n) didn't start on the Shelf"; return 1; }
  done
  check wait_for 10 eval '[[ "$(cd "$repo" && kanna task list | placed | grep -cE "^task-[0-9a-f]{4,8} +Task [1-5] +Shelf +(idle|working|running)")" == 5 ]]' || { (cd "$repo" && kanna task list); return 1; }
  for n in 1 2 3 4 5; do check wait_for 5 shows "Task $n" "hello $n" || { echo "task_start_stop: Task $n's agent didn't get its first message"; return 1; }; done
  [[ "$(kanna task start $(tid 1) 2>&1)" == *"$(sid 1) is running"* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  for n in 1 2 3; do kanna task pick "$(tid $n)" >/dev/null || { echo "task_start_stop: $(sid $n) didn't move to the Shelf"; return 1; }; done
  [[ "$(kanna task pick $(tid 4) 2>&1)" == *"has 3 tasks on the Desk"* ]] || { echo "task_start_stop: a fourth task went to the Shelf"; return 1; }
  listed="$(cd "$repo" && kanna task list | placed)"
  [[ "$(grep -cE ' Desk ' <<<"$listed")" == 3 && "$(grep -cE ' Shelf ' <<<"$listed")" == 2 ]] || { echo "$listed"; return 1; }
  local task session; task="$(pane "Task 1" id)"
  session="$(kanna read "Task 1" | tr -d '\r\n' | sed -n 's/.*--session-id \([0-9a-f-]\{36\}\).*/\1/p')"
  [[ "$session" =~ ^[0-9a-f-]{36}$ ]] || { echo "task_start_stop: Task 1 started without a session"; kanna read "Task 1"; return 1; }
  KANNA_TERMINAL="$task" kanna open --name side --cmd 'exec cat' >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }
  expected="stopped $(sid 1); \`kanna task start $(sid 1)\` starts it again"; says kanna task stop "Task 1" || { echo "task_start_stop line $LINENO"; return 1; }
  check is "Task 1" state exited || { echo "task_start_stop: Task 1's agent is still running"; return 1; }
  unwrapped "Task 1" "claude --model mock-model --dangerously-skip-permissions --resume $session" || { echo "task_start_stop: Task 1's agent didn't say its session as it quit"; kanna read "Task 1"; return 1; }
  ! tail -n +"$((logged + 1))" "$out/daemon.log" | grep -E "claude's box in |claude in [^ ]+ didn't exit" || { echo "task_start_stop: Task 1's agent wasn't quit by /exit"; return 1; }
  ! tail -n +"$((logged + 1))" "$out/daemon.log" | grep -E "in $task (has no quit keys|didn't quit)|quit keys in $task " || { echo "task_start_stop: Task 1's agent was hung up"; return 1; }
  check is side state exited || { echo "task_start_stop: Task 1's terminal tab didn't stay, hung up"; kanna workspace list --all; return 1; }
  (cd "$repo" && kanna task list) | placed | grep -qE "^$(sid 1) +Task 1 +Desk +stopped" || { echo "task_start_stop: Task 1 left the Shelf"; (cd "$repo" && kanna task list); return 1; }
  [[ "$(kanna task stop "Task 1" 2>&1)" == *"is stopped"* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  [[ "$(kanna task pick $(tid 4) 2>&1)" == *"has 3 tasks on the Desk"* ]] || { echo "task_start_stop: a stopped task stopped counting on the Shelf"; return 1; }
  [[ "$(kanna task start "Task 1")" == "started $(sid 1) again: term-"* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(kanna read "Task 1" | tr -d "\r\n")" == *"--resume $session"* ]]' || { echo "task_start_stop: Task 1 didn't resume its conversation"; kanna read "Task 1"; return 1; }
  check wait_for 5 is side state running || { echo "task_start_stop: Task 1's terminal tab didn't run again"; kanna workspace list --all; return 1; }
  (cd "$repo" && kanna task list) | placed | grep -qE "^$(sid 1) +Task 1 +Desk +(idle|working|running)" || { (cd "$repo" && kanna task list); return 1; }
  [[ "$(kanna task stop $(tid 5))" == "stopped $(sid 5) on the Shelf; "* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  (cd "$repo" && kanna task list) | placed | grep -qE '^'"$(sid 5)"' +Task 5 +Shelf +stopped' || { (cd "$repo" && kanna task list); return 1; }
  [[ "$(kanna task start $(tid 5))" == "started $(sid 5) again on the Shelf: term-"* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  check wait_for 5 shows "Task 5" "--resume" || { echo "task_start_stop: $(sid 5) didn't resume"; kanna read "Task 5"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "task_start_stop line $LINENO"; return 1; }
  KANNA_TERMINAL="$(pane "Task 4" id)" kanna open --name side4 --cmd 'exec cat' >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }
  kanna task stop $(tid 4) >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "task_start_stop line $LINENO"; return 1; }
  (cd "$repo" && kanna task list) | placed | grep -qE '^'"$(sid 4)"' +Task 4 +Shelf +stopped' || { echo "task_start_stop: a stopped task didn't survive a restart"; (cd "$repo" && kanna task list); return 1; }
  kanna task start $(tid 4) >/dev/null && check wait_for 5 shows "Task 4" "--resume" || { echo "task_start_stop: $(sid 4) didn't resume after a restart"; return 1; }
  check wait_for 5 is side4 state running || { echo "task_start_stop: a hung-up terminal didn't run again after a restart"; kanna workspace list --all; return 1; }
}

scenario task_quit_at_rest M4
task_quit_at_rest() {
  local logged; logged="$(cat "$out/daemon.log" 2>/dev/null | wc -l)"
  start_daemon && start_app || { echo "task_quit_at_rest line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude"
  mkdir -p "$repo/.kanna" "${agent%/*}" && ln -s "$mock_agent" "$agent" || { echo "task_quit_at_rest line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_quit_at_rest line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  logs() { tail -n +"$((logged + 1))" "$out/daemon.log" | grep -F -- "$1"; }
  status() { raw "{\"id\":1,\"cmd\":\"status\",\"args\":{\"action\":\"explain\",\"pane\":\"$1\"}}" | field status; }
  local id pane
  id="$(cd "$repo" && kanna task new "Ends itself" "say hi" --agent "$agent" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
  kanna task start "$id" >/dev/null && check wait_for 10 shows "Ends itself" SAID || { echo "task_quit_at_rest line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(status "$(pane "Ends itself" id)")" == idle ]]' || { echo "task_quit_at_rest: the agent didn't report idle"; return 1; }
  kanna send "Ends itself" "run $bin/kanna task done && sleep 1" || { echo "task_quit_at_rest line $LINENO"; return 1; }
  check wait_for 10 eval 'logs "ends its own task, so it quits when its turn ends" >/dev/null' || { echo "task_quit_at_rest: its own Done didn't wait for its turn"; return 1; }
  pane="$(logs "ends its own task" | sed -n 's/.* in \(term-[0-9]*\) ends its own task.*/\1/p' | tail -1)"
  check wait_for 15 eval '[[ "$(cd "$repo" && kanna task list "$id")" == *"is done"* ]]' || { echo "task_quit_at_rest: the task that ran its own Done isn't done"; (cd "$repo" && kanna task list "$id"); return 1; }
  ! logs "$pane isn't at rest" || { echo "task_quit_at_rest: its own Done interrupted it"; return 1; }
  ! logs "in $pane didn't" || { echo "task_quit_at_rest: its own Done didn't quit it by /exit"; return 1; }
  ! logs "box in $pane" || { echo "task_quit_at_rest: its own Done didn't quit it by /exit"; return 1; }
  id="$(cd "$repo" && kanna task new "Works" "say hi" --agent "$agent" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
  kanna task start "$id" >/dev/null && check wait_for 10 shows "Works" SAID || { echo "task_quit_at_rest line $LINENO"; return 1; }
  pane="$(pane "Works" id)"
  kanna send "Works" "work 60" && check wait_for 10 eval '[[ "$(status "$pane")" == working ]]' || { echo "task_quit_at_rest: it didn't start working"; return 1; }
  local started=$SECONDS
  [[ "$(kanna task stop "$id")" == "stopped $id on the Shelf; "* ]] || { echo "task_quit_at_rest: the working task didn't stop"; return 1; }
  check is "Works" state exited || { echo "task_quit_at_rest: the working agent is still running"; return 1; }
  (( SECONDS - started < 5 )) || { echo "task_quit_at_rest: stopping the working agent took $((SECONDS - started))s"; return 1; }
  shows "Works" INTERRUPTED && unwrapped "Works" "--resume" || { echo "task_quit_at_rest: Esc didn't bring it to rest before it quit"; kanna read "Works"; return 1; }
  logs "$pane isn't at rest, so it gets esc" >/dev/null && logs "in $pane is at rest, so it's told to exit" >/dev/null || { echo "task_quit_at_rest: the working agent wasn't brought to rest"; return 1; }
  ! logs "in $pane didn't" || { echo "task_quit_at_rest: the working agent got its keys"; return 1; }
  machine_runners 1
  id="$(cd "$repo" && kanna task new "Asks" "turn" --agent "$agent" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
  check wait_for 20 eval 'raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"events\",\"task\":\"$id\"}}" | grep -q "\"why\": *\"needs-you\""' || { echo "task_quit_at_rest: the asking task wasn't stopped as needs you"; return 1; }
  pane="$(pane "Asks" id)"
  check is "Asks" state exited || { echo "task_quit_at_rest: the asking agent is still running"; return 1; }
  shows "Asks" "Declined." && unwrapped "Asks" "--resume" || { echo "task_quit_at_rest: Esc didn't answer its question before it quit"; kanna read "Asks"; return 1; }
  logs "$pane isn't at rest, so it gets esc" >/dev/null && logs "in $pane is at rest, so it's told to exit" >/dev/null || { echo "task_quit_at_rest: the asking agent wasn't brought to rest"; return 1; }
  ! logs "in $pane didn't" || { echo "task_quit_at_rest: the asking agent got its keys"; return 1; }
}

scenario pick_at_once M4
pick_at_once() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" task seen took card
  mkdir -p "$repo" "${agent%/*}" && ln -s "$mock_agent" "$agent" || { echo "pick_at_once line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "pick_at_once line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "pick_at_once line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "pick_at_once: no planning terminal"; return 1; }
  (cd "$repo" && kanna task new "Line tables" "hello there" --agent "$agent") >/dev/null && kanna task start $(tid 1) >/dev/null || { echo "pick_at_once line $LINENO"; return 1; }
  check wait_for 5 shows "Line tables" "hello there" || { echo "pick_at_once: the task didn't start"; return 1; }
  task="$(pane "Line tables" id)"
  KANNA_TERMINAL="$task" kanna open --tab --name lines --cmd 'exec cat' >/dev/null && KANNA_TERMINAL="$task" kanna open --tab --name tables --cmd 'exec cat' >/dev/null || { echo "pick_at_once line $LINENO"; return 1; }
  kanna task stop $(tid 1) >/dev/null || { echo "pick_at_once line $LINENO"; return 1; }
  check wait_for 5 is "Line tables" state exited && check is tables state exited || { echo "pick_at_once: the task didn't stop"; return 1; }
  check wait_for 5 eval '[[ "$(desk shelf)" == "[\"$(tid 1)\"]" ]]' || { desk shelf; return 1; }
  seen="$(python3 -c 'import json, socket, sys, time
def ask(cmd, args):
    s = socket.socket(socket.AF_UNIX); s.settimeout(30); s.connect(sys.argv[1])
    s.sendall(json.dumps({"id": 1, "cmd": cmd, "args": args}).encode() + b"\n")
    return json.loads(s.makefile().readline())
start = time.monotonic()
ask("debug.click", {"shelf": sys.argv[2], "window": "d1"})
while time.monotonic() - start < 10:
    d = ask("debug.desk", {"window": "d1"})
    d = d.get("result", d)
    if not d["shelf"] and [t["title"] for t in d["tasks"]] == ["Line tables"]:
        print(round((time.monotonic() - start) * 1000), d["tasks"][0]["task"]); sys.exit(0)
    time.sleep(0.005)
print("never")' "$KANNA_SOCKET" "$(tid 1)")"
  took="${seen%% *}" card="${seen#* }"
  [[ "$took" =~ ^[0-9]+$ ]] || { echo "pick_at_once: the task never reached the Shelf"; desk tasks; desk note; return 1; }
  echo "pick_at_once: on the Shelf ${took}ms after the click"
  (( took <= 200 * stretch )) || { echo "pick_at_once: the task took ${took}ms to reach the Shelf"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"task":"'"$card"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$(pane "Line tables" id)" ]]' || { echo "pick_at_once: pulling it at once didn't put it on the Desk"; raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  check wait_for 5 shows "Line tables" "--resume" || { echo "pick_at_once: its agent didn't come back"; kanna read "Line tables"; return 1; }
  check wait_for 5 is lines state running && check is tables state running || { echo "pick_at_once: its tabs didn't run again"; kanna workspace list --all; return 1; }
  check test "$(desk note)" == "" || { echo "pick_at_once: $(desk note)"; return 1; }
  grep -qE '^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9:.]{12} kanna-daemon: ' "$out/daemon.log" || { echo "pick_at_once: daemon.log's lines have no time"; tail -3 "$out/daemon.log"; return 1; }
}

scenario pick_keeper_paused M4
pick_keeper_paused() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" keeper task seen took listed
  mkdir -p "$repo" "${agent%/*}" && ln -s "$mock_agent" "$agent" || { echo "pick_keeper_paused line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "pick_keeper_paused line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "pick_keeper_paused line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "pick_keeper_paused: no planning terminal"; return 1; }
  (cd "$repo" && kanna task new "Line tables" "hello there" --agent "$agent") >/dev/null && kanna task start $(tid 1) >/dev/null || { echo "pick_keeper_paused line $LINENO"; return 1; }
  check wait_for 5 shows "Line tables" "hello there" || { echo "pick_keeper_paused: the task didn't start"; return 1; }
  task="$(pane "Line tables" id)"
  KANNA_TERMINAL="$task" kanna open --tab --name lines --cmd 'exec cat' >/dev/null || { echo "pick_keeper_paused line $LINENO"; return 1; }
  kanna task stop $(tid 1) >/dev/null || { echo "pick_keeper_paused line $LINENO"; return 1; }
  check wait_for 5 is "Line tables" state exited && check is lines state exited || { echo "pick_keeper_paused: the task didn't stop"; return 1; }
  check wait_for 5 eval '[[ "$(desk shelf)" == "[\"$(tid 1)\"]" ]]' || { desk shelf; return 1; }
  keeper="$(cat "$KANNA_STATE_DIR/keeper.pid")"
  [[ "$keeper" =~ ^[0-9]+$ ]] && [[ "$(ps -o command= -p "$keeper")" == *"$KANNA_STATE_DIR"* ]] || { echo "pick_keeper_paused: $keeper isn't this run's keeper"; return 1; }
  kill -STOP "$keeper"
  seen="$(python3 -c 'import json, socket, subprocess, sys, time
def ask(cmd, args):
    s = socket.socket(socket.AF_UNIX); s.settimeout(30); s.connect(sys.argv[1])
    s.sendall(json.dumps({"id": 1, "cmd": cmd, "args": args}).encode() + b"\n")
    return json.loads(s.makefile().readline())
start = time.monotonic()
ask("debug.click", {"shelf": sys.argv[2], "window": "d1"})
while time.monotonic() - start < 10:
    d = ask("debug.desk", {"window": "d1"})
    d = d.get("result", d)
    if not d["shelf"] and [t["title"] for t in d["tasks"]] == ["Line tables"]:
        took = round((time.monotonic() - start) * 1000)
        asked = time.monotonic()
        try:
            listed = subprocess.run([sys.argv[3], "task", "list"], cwd=sys.argv[4], capture_output=True, text=True, timeout=2).stdout
        except subprocess.TimeoutExpired:
            listed = "no answer"
        place, row = "", []
        for line in listed.splitlines():
            place = line.strip() if line.strip() in ("Desk", "Shelf") else place
            row += [place + "  " + line] if "Line tables" in line else []
        print(took, round((time.monotonic() - asked) * 1000), *(row or [listed])); sys.exit(0)
    time.sleep(0.005)
print("never")' "$KANNA_SOCKET" "$(tid 1)" "$bin/kanna" "$repo")"
  sleep 0.5
  kill -CONT "$keeper"
  took="${seen%% *}" listed="${seen#* }"
  [[ "$took" =~ ^[0-9]+$ ]] || { echo "pick_keeper_paused: with the keeper paused, the task never reached the Desk"; desk tasks; desk note; return 1; }
  echo "pick_keeper_paused: on the Desk ${took}ms after the click, the keeper paused; task list (ms, row): $listed"
  (( took <= 200 * stretch )) || { echo "pick_keeper_paused: the task took ${took}ms to reach the Desk"; return 1; }
  [[ "$listed" =~ ^([0-9]+)\ Desk\ .*Line\ tables ]] && (( BASH_REMATCH[1] <= 200 * stretch )) || { echo "pick_keeper_paused: task list, asked while the keeper was paused, answered (ms, where) $listed"; return 1; }
  check wait_for 5 shows "Line tables" "--resume" || { echo "pick_keeper_paused: its agent didn't come back once the keeper went on"; kanna read "Line tables"; return 1; }
  check wait_for 5 is lines state running || { echo "pick_keeper_paused: its tab didn't run again"; kanna workspace list --all; return 1; }
  check test "$(desk note)" == "" || { echo "pick_keeper_paused: $(desk note)"; return 1; }
  kill -0 "$(cat "$KANNA_STATE_DIR/daemon.pid")" || { echo "pick_keeper_paused: the daemon stopped"; tail -5 "$out/daemon.log"; return 1; }
}

scenario task_restart_no_conversation M4
task_restart_no_conversation() {
  start_daemon && start_app || { echo "task_restart_no_conversation line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" wizard="$KANNA_STATE_DIR/mock-wizard"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" && touch "$wizard" || { echo "task_restart_no_conversation line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_restart_no_conversation line $LINENO"; return 1; }
  (cd "$repo" && kanna task new Wizard "say hi" --agent "$agent") >/dev/null || { echo "task_restart_no_conversation line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null && kanna task activate "$(tid 1)" >/dev/null || { echo "task_restart_no_conversation line $LINENO"; return 1; }
  check wait_for 10 shows Wizard "--session-id" || { kanna read Wizard; return 1; }
  [[ "$(pane Wizard cmd)" == *"--resume"* ]] || { echo "task_restart_no_conversation: the pane's command isn't the resuming one: $(pane Wizard cmd)"; return 1; }
  kanna restart Wizard >/dev/null
  check wait_for 5 shows Wizard "--session-id" || { echo "task_restart_no_conversation: restart didn't start it fresh"; kanna read Wizard; return 1; }
  rm "$wizard"
  kanna task stop Wizard >/dev/null && check wait_for 5 is Wizard state exited || { echo "task_restart_no_conversation: Wizard didn't stop"; return 1; }
  kanna task start Wizard >/dev/null && check wait_for 5 shows Wizard "SAID" || { echo "task_restart_no_conversation: starting it again didn't give the first message"; kanna read Wizard; return 1; }
  kanna read Wizard | grep -q -- "--session-id" || { echo "task_restart_no_conversation: task start didn't start it fresh"; kanna read Wizard; return 1; }
  kanna restart Wizard >/dev/null
  check wait_for 5 shows Wizard "--resume" || { echo "task_restart_no_conversation: with a conversation, restart didn't resume"; kanna read Wizard; return 1; }
  kanna read Wizard | grep -q "already in use" && { echo "task_restart_no_conversation: the session was started twice"; kanna read Wizard; return 1; }
  return 0
}

scenario task_restart_typing M4
task_restart_typing() {
  start_daemon && start_app || { echo "task_restart_typing line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" || { echo "task_restart_typing line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_restart_typing line $LINENO"; return 1; }
  (cd "$repo" && kanna task new Typed "hello there" --agent "$agent") >/dev/null || { echo "task_restart_typing line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null && kanna task activate "$(tid 1)" >/dev/null || { echo "task_restart_typing line $LINENO"; return 1; }
  check wait_for 10 shows Typed "hello there" || { kanna read Typed; return 1; }
  local responder
  raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"pane\":\"$(pane Typed id)\",\"text\":\"before\\r\"}}" >/dev/null
  check wait_for 5 shows Typed "echo: before" || { kanna read Typed; return 1; }
  kanna task stop Typed >/dev/null && check wait_for 5 is Typed state exited || { echo "task_restart_typing: Typed didn't stop"; return 1; }
  kanna task start Typed >/dev/null && check wait_for 5 shows Typed "--resume" || { echo "task_restart_typing: Typed didn't start again"; kanna read Typed; return 1; }
  responder="$(raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"after\r"}}' | field responder)"
  [[ "$responder" == TerminalSurfaceView ]] || { echo "task_restart_typing: the keyboard went to $responder, not the agent's terminal"; return 1; }
  check wait_for 5 shows Typed "echo: after" || { echo "task_restart_typing: keys typed after a restart didn't reach the agent"; kanna read Typed; return 1; }
  [[ "$(kanna read Typed | grep -c "echo: after")" == 1 ]] || { echo "task_restart_typing: the keys reached the agent twice"; kanna read Typed; return 1; }
  kanna task stop Typed >/dev/null && check wait_for 5 is Typed state exited || { echo "task_restart_typing: Typed didn't stop again"; return 1; }
  stop_daemon; start_daemon && check wait_for 10 app_attached || { echo "task_restart_typing line $LINENO"; return 1; }
  kanna task start Typed >/dev/null && check wait_for 5 eval '! shows Typed "echo: after" && shows Typed --resume' || { echo "task_restart_typing: Typed didn't start again after a daemon restart"; kanna read Typed; return 1; }
  responder="$(raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"again\r"}}' | field responder)"
  [[ "$responder" == TerminalSurfaceView ]] || { echo "task_restart_typing: after a daemon restart the keyboard went to $responder"; return 1; }
  check wait_for 5 shows Typed "echo: again" || { echo "task_restart_typing: keys typed after a daemon restart didn't reach the agent"; kanna read Typed; return 1; }
}

scenario task_pane_stop_start M4
task_pane_stop_start() {
  start_daemon && start_app || { echo "task_pane_stop_start line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" run side mark="$KANNA_STATE_DIR/ran-once"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" || { echo "task_pane_stop_start line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_pane_stop_start line $LINENO"; return 1; }
  (cd "$repo" && kanna task new OnDesk "hello desk" --agent "$agent" && kanna task new OnShelf "hello shelf" --agent "$agent") >/dev/null || { echo "task_pane_stop_start line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null && kanna task activate "$(tid 1)" >/dev/null || { echo "task_pane_stop_start line $LINENO"; return 1; }
  check wait_for 10 shows OnDesk "hello desk" || { kanna read OnDesk; return 1; }
  local desk; desk="$(pane OnDesk id)"
  KANNA_TERMINAL="$desk" kanna open --tab --name side --cmd "if [ -e '$mark' ]; then echo NEW-RUN; else touch '$mark'; printf '\\033[?2004h'; echo OLD-RUN; fi; exec cat" >/dev/null || { echo "task_pane_stop_start line $LINENO"; return 1; }
  check wait_for 5 shows side OLD-RUN || { kanna read side; return 1; }
  kanna focus side >/dev/null
  check wait_for 5 app_text side OLD-RUN || { raw '{"id":1,"cmd":"debug.text","args":{"pane":"side"}}'; return 1; }
  raw '{"id":1,"cmd":"debug.drop","args":{"pane":"side","text":"PASTED-ONE"}}' >/dev/null
  check wait_for 5 eval 'kanna read side | grep -qF "^[[200~PASTED-ONE^[[201~"' || { echo "task_pane_stop_start: the first run should have bracketed paste on"; kanna read side; return 1; }
  side="$(pane side id)"; run="$(pane side terminal)"

  kanna task stop OnDesk >/dev/null && check wait_for 5 is OnDesk state exited && check wait_for 5 is side state exited || { echo "task_pane_stop_start line $LINENO"; return 1; }
  check is OnDesk id "$desk" && check is side id "$side" || return 1
  kanna task start OnDesk >/dev/null && check wait_for 5 shows OnDesk "--resume" && check wait_for 5 shows side NEW-RUN || { echo "task_pane_stop_start line $LINENO"; return 1; }
  check is OnDesk id "$desk" && check is side id "$side" || { kanna workspace list --all --json; return 1; }
  [[ "$(pane side terminal)" != "$run" ]] || { echo "task_pane_stop_start: the new run needs its own terminal"; return 1; }
  [[ "$(pane OnDesk status)" != exited ]] || { echo "task_pane_stop_start: the new run shouldn't show the old one's status"; return 1; }
  check wait_for 5 app_text side NEW-RUN || { raw '{"id":1,"cmd":"debug.text","args":{"pane":"side"}}'; return 1; }
  ! app_text side OLD-RUN || { echo "task_pane_stop_start: the app kept the old run's scrollback"; raw '{"id":1,"cmd":"debug.text","args":{"pane":"side"}}'; return 1; }
  raw '{"id":1,"cmd":"debug.drop","args":{"pane":"side","text":"PASTED-TWO"}}' >/dev/null
  check wait_for 5 shows side PASTED-TWO || { kanna read side; return 1; }
  ! kanna read side | grep -qF "^[[200~PASTED-TWO" || { echo "task_pane_stop_start: a mode the old run left on came back"; kanna read side; return 1; }

  kanna task start "$(tid 2)" >/dev/null || { echo "task_pane_stop_start line $LINENO"; return 1; }
  check wait_for 10 shows OnShelf "hello shelf" || { kanna read OnShelf; return 1; }
  local shelf; shelf="$(pane OnShelf id)"
  run="$(pane OnShelf terminal)"
  kanna task stop "$(tid 2)" >/dev/null && check wait_for 5 is OnShelf state exited || { echo "task_pane_stop_start line $LINENO"; return 1; }
  check is OnShelf id "$shelf" || return 1
  [[ "$(kanna task start "$(tid 2)")" == *"again on the Shelf: $shelf"* ]] || { echo "task_pane_stop_start: the start should name the same pane"; return 1; }
  check wait_for 5 shows "$shelf" "--resume" || { kanna read "$shelf"; return 1; }
  check is OnShelf id "$shelf" || return 1
  [[ "$(pane OnShelf terminal)" != "$run" ]] || { echo "task_pane_stop_start: the new run needs its own terminal"; return 1; }
  ! shows "$shelf" "hello shelf" || { echo "task_pane_stop_start: the old run's screen came back"; kanna read "$shelf"; return 1; }
  kanna task pick "$(tid 2)" >/dev/null || { echo "task_pane_stop_start line $LINENO"; return 1; }
  check is OnShelf id "$shelf" || return 1
  kanna task shelve "$(tid 2)" >/dev/null || { echo "task_pane_stop_start line $LINENO"; return 1; }
  check is OnShelf id "$shelf" || return 1
}

scenario task_history M4
task_history() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" state="$KANNA_STATE_DIR"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" || { echo "task_history line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_history line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  printf '{"counter": 3, "tasks": [{"id": "t3", "name": "From JSON", "message": "hi", "repo": "%s", "madeAt": 1700000000, "agent": "%s"}], "running": []}' "$repo" "$agent" >"$state/backlog.json"
  start_daemon && start_app || { echo "task_history line $LINENO"; return 1; }
  [[ -f "$state/kanna.db" && -f "$state/backlog.json.imported" && ! -e "$state/backlog.json" ]] || { echo "task_history: backlog.json wasn't imported"; ls "$state"; return 1; }
  (cd "$repo" && kanna task list) | placed | grep -qE '^'"$(sid 1)"' +From JSON +Shelf' || { echo "task_history: the imported task isn't on the Shelf"; (cd "$repo" && kanna task list); return 1; }
  events() { raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"events\",\"task\":\"$1\"}}" | python3 -c 'import json, sys; print(" ".join(e["what"] + ("@" + e["by"] if e["by"] else "") for e in json.load(sys.stdin)["events"] if e["what"] != "status"))'; }
  [[ "$(events "$(tid 1)")" == "made renumbered@daemon" ]] || { echo "task_history: the imported task's events are $(events "$(tid 1)")"; return 1; }
  (cd "$repo" && kanna task new "Fresh" "hello fresh" --agent "$agent") | grep -qE '^added task-[0-9a-f]{4,8} ' || { echo "task_history: a new task didn't get an ID"; return 1; }
  kanna task pick $(tid 2) >/dev/null || { echo "task_history line $LINENO"; return 1; }
  local task window; task="$(pane Fresh id)"
  window="$(KANNA_TERMINAL="$task" kanna task activate "$task")" && window="${window##* active on }" && [[ "$window" =~ ^d[0-9]+$ ]] || { echo "task_history: pull said $window"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"deskView\":\"$window\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(dwin "$window" dock)" == None ]]' || { echo "task_history: the app's park didn't park"; return 1; }
  kanna task stop "$task" >/dev/null && kanna task start "$task" >/dev/null || { echo "task_history line $LINENO"; return 1; }
  kanna task name $(tid 2) "Fresher" >/dev/null || { echo "task_history line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(events $(tid 2))" == *renamed ]]' || { echo "task_history: the rename wasn't recorded: $(events $(tid 2))"; return 1; }
  kanna task shelve $(tid 2) >/dev/null && kanna task done $(tid 2) >/dev/null || { echo "task_history line $LINENO"; return 1; }
  local want="made@user picked@user activated@$task stopped@user started@user renamed shelved@user done@user"
  [[ "$(events $(tid 2))" == "$want" ]] || { echo "task_history: the task's events are \"$(events $(tid 2))\", not \"$want\""; return 1; }
  [[ "$(sqlite3 -readonly "$state/kanna.db" "select place from tasks where id = '$(tid 2)'")" == done ]] || { echo "task_history: a done task's row is gone or not done"; return 1; }
  stop_app; stop_daemon
  local lines; lines="$(wc -l <"$out/daemon.log")"
  rm -f "$state/kanna.db-wal" "$state/kanna.db-shm"
  printf 'this is not a database %.0s' {1..200} >"$state/kanna.db"
  start_daemon && start_app || { echo "task_history line $LINENO"; return 1; }
  tail -n +"$((lines + 1))" "$out/daemon.log" | grep -q "kanna.db can't be used" || { echo "task_history: the daemon didn't say the db was bad"; return 1; }
  tail -n +"$((lines + 1))" "$out/daemon.log" | grep -q "restored .*kanna.db from" || { echo "task_history: the daemon didn't say it restored the db"; return 1; }
  (cd "$repo" && kanna task list) | placed | grep -qE '^'"$(sid 1)"' +From JSON +Shelf' || { echo "task_history: a Shelf task was lost with the db"; (cd "$repo" && kanna task list); return 1; }
  ls "$state" | grep -q '^kanna.db.bad-' || { echo "task_history: the bad db wasn't kept"; return 1; }
}

scenario task_events M4
task_events() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" config user.email e@e && git -C "$repo" config user.name e || { echo "task_events line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  printf '/.kanna\n/.kanna-worktrees\n' >"$repo/.gitignore" && echo base >"$repo/shared" && echo base >"$repo/calm" && git -C "$repo" add . && git -C "$repo" commit -q -m first || { echo "task_events line $LINENO"; return 1; }
  machine_runners 1
  start_daemon && start_app || { echo "task_events line $LINENO"; return 1; }
  in_repo() { (cd "$repo" && kanna "$@"); }
  task_of() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0]
print("running" if t.get("running") else "stopped" if t.get("folder") else "waiting", t.get("held") or "-", t.get("folder") or "-")' "$1"; }
  in_repo task new "Busy" "work 12" --agent "$fake/codex" >/dev/null || { echo "task_events line $LINENO"; return 1; }
  local busy term folder said; busy="$(tid 1)"
  check wait_for 10 eval '[[ "$(task_of $busy)" == running* ]]' || { echo "task_events: the task didn't run"; return 1; }
  term="$(task_of $busy | cut -d' ' -f2)" folder="$(task_of $busy | cut -d' ' -f3)"
  check wait_for 10 is "$term" status working || { echo "task_events: the task never showed as working"; return 1; }
  [[ "$(in_repo task events "$busy")" == "hears: main-conflicts, base-merged, base-moved" ]] || { echo "task_events: its events start as $(in_repo task events "$busy")"; return 1; }
  echo ours >"$folder/shared" && git -C "$folder" commit -q -am ours || { echo "task_events line $LINENO"; return 1; }
  local main_saved='sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select value from meta where key like '"'"'main:%'"'"'"'
  check wait_for 10 eval '[[ -n "$('"$main_saved"')" ]]' || { echo "task_events: the daemon never saw main"; return 1; }

  echo theirs >"$repo/calm" && git -C "$repo" commit -q -am "no conflict" || { echo "task_events line $LINENO"; return 1; }
  local first; first="$(git -C "$repo" rev-parse HEAD)"
  check wait_for 10 eval '[[ "$('"$main_saved"')" == *"$first" ]]' || { echo "task_events: the daemon didn't see main move"; return 1; }
  in_repo task messages "$busy" | grep -q conflicts && { echo "task_events: told of a conflict that isn't one"; in_repo task messages "$busy"; return 1; }

  echo theirs >"$repo/shared" && git -C "$repo" commit -q -am conflict || { echo "task_events line $LINENO"; return 1; }
  check wait_for 10 eval 'in_repo task messages "$busy" | grep -q "main conflicts with your branch in shared"' || { echo "task_events: main moving into a conflict left no message"; in_repo task messages "$busy"; return 1; }
  check test "$(git -C "$folder" rev-parse --abbrev-ref HEAD)" != main || return 1
  check test "$(cat "$folder/shared")" == ours || { echo "task_events: the worktree was touched"; return 1; }
  git -C "$repo" commit -q --allow-empty -m again || { echo "task_events line $LINENO"; return 1; }
  sleep 1
  check test "$(in_repo task messages "$busy" | grep -c 'main conflicts')" -eq 1 || { echo "task_events: a second move left a second message"; in_repo task messages "$busy"; return 1; }
  shows "$term" "main conflicts with" && { echo "task_events: the message went in mid-turn"; kanna read "$term"; return 1; }
  check wait_for 30 unwrapped "$term" "peer: [message from kanna] main conflicts with your branch in shared: merge or rebase main, rerun your tests" || { echo "task_events: the message didn't arrive at the turn's end"; kanna read "$term"; in_repo task messages "$busy"; return 1; }
  check wait_for 5 eval 'in_repo task events "$busy" | grep -qE "main-conflicts +delivered +main conflicts with your branch in shared"' || { echo "task_events: events doesn't list what was sent"; in_repo task events "$busy"; return 1; }

  said="$(in_repo task subscribe main-moved "$busy")"
  [[ "$said" == *"hears main-moved" ]] || { echo "task_events: subscribe said $said"; return 1; }
  said="$(in_repo task unsubscribe main-conflicts "$busy")"
  [[ "$said" == *"no longer hears main-conflicts" ]] || { echo "task_events: unsubscribe said $said"; return 1; }
  git -C "$folder" merge -q main >/dev/null 2>&1; echo merged >"$folder/shared" && git -C "$folder" add shared && git -C "$folder" commit -q -m merged || { echo "task_events line $LINENO"; return 1; }
  echo ours2 >"$folder/shared" && git -C "$folder" commit -q -am ours2 || { echo "task_events line $LINENO"; return 1; }
  echo theirs2 >"$repo/shared" && git -C "$repo" commit -q -am "moved again" || { echo "task_events line $LINENO"; return 1; }
  check wait_for 10 eval 'in_repo task messages "$busy" | grep -q "main moved: 1 commit"' || { echo "task_events: a subscribed task didn't hear main move"; in_repo task messages "$busy"; return 1; }
  check test "$(in_repo task messages "$busy" | grep -c 'main conflicts')" -eq 1 || { echo "task_events: told of a conflict after unsubscribing"; in_repo task messages "$busy"; return 1; }
  [[ "$(in_repo task subscribe main-wobbled "$busy" 2>&1)" == *"main-conflicts, main-moved, base-merged, base-moved"* ]] || { echo "task_events: an unknown event wasn't refused with the list"; return 1; }
}

scenario blocked_tasks M4
blocked_tasks() {
  start_daemon && start_app || { echo "blocked_tasks line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "blocked_tasks line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  in_repo task new "Screenshot command" "hello there" --agent "$agent" >/dev/null && in_repo task new "Status fix" --agent "$agent" >/dev/null || { echo "blocked_tasks line $LINENO"; return 1; }
  local first added; first="$(tid 1)"; added="$(in_repo task new "Diff viewer" "hello there" --agent "$agent" --after "${first:5:4}" 2>&1)"
  [[ "$added" == "added $(sid 3) \"Diff viewer\" to the Shelf of $repo; it waits for $(sid 1)" ]] || { echo "blocked_tasks line $LINENO: $added"; return 1; }
  added="$(in_repo task new "Release notes" --agent "$agent" --after "$(tid 1),$(tid 2)" 2>&1)"
  [[ "$added" == "added $(sid 4) \"Release notes\" to the Shelf of $repo; it waits for $(sid 1), $(sid 2)" ]] || { echo "blocked_tasks line $LINENO: $added"; return 1; }
  [[ "$(in_repo task new "Nowhere" --after t9 2>&1)" == *'no task "t9"'* ]] || { echo "blocked_tasks: a blocker that isn't a task was taken"; return 1; }
  [[ "$(in_repo task edit $(tid 1) --after $(tid 3) 2>&1)" == *"$(sid 3) waits for $(sid 1): they'd wait for each other"* ]] || { echo "blocked_tasks: a task was let wait for itself"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 3)"' +Diff viewer +Shelf +blocked +waits for "Screenshot command"$' || { in_repo task list; return 1; }
  [[ "$(in_repo task start $(tid 3) 2>&1)" == *''"$(sid 3)"' waits for '"$(sid 1)"' "Screenshot command": it can start once it'"'"'s ready or done'* ]] || { echo "blocked_tasks: a blocked task started"; in_repo task list; return 1; }
  in_repo task pick $(tid 3) >/dev/null 2>&1 && { echo "blocked_tasks: a blocked task moved to the Shelf"; return 1; }
  check test ! -e "$repo/.kanna-worktrees" || { echo "blocked_tasks: a blocked task got a worktree"; ls "$repo/.kanna-worktrees"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "blocked_tasks line $LINENO"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 4)"' +Release notes +Shelf +blocked +waits for "Screenshot command", "Status fix"$' || { echo "blocked_tasks: blockers didn't survive a restart"; in_repo task list; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "blocked_tasks line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk scroll.blocked.0.task)" == $(tid 3) ]]' || { desk scroll; return 1; }
  check test "$(desk scroll.blocked.0.go)" == Blocked || { desk scroll.blocked; return 1; }
  check test "$(desk scroll.blocked.0.waits)" == '["Screenshot command"]' || { desk scroll.blocked; return 1; }
  check test "$(desk scroll.blocked.1.waits)" == '["Screenshot command", "Status fix"]' || { desk scroll.blocked; return 1; }
  sleep 0.5; snap "$w" blocked_tasks-blocked >/dev/null
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 3)"'","window":"'"$w"'"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk note)" == "“Diff viewer” waits for “Screenshot command”: it can move once that'"'"'s done." ]]' || { echo "blocked_tasks: clicking a blocked row didn't say why"; desk note; return 1; }
  check test "$(desk running)" == 0 || { echo "blocked_tasks: clicking a blocked row started it"; return 1; }
  local started pane; started="$(kanna task pick $(tid 1))" && [[ "$started" =~ ^"moved $(sid 1) to the Desk: "(term-[0-9]+) ]] || { echo "blocked_tasks line $LINENO: $started"; return 1; }
  pane="${BASH_REMATCH[1]}"
  check wait_for 5 is "$pane" status idle || { echo "blocked_tasks line $LINENO"; return 1; }
  local done; done="$(kanna task done "$pane")" || { echo "blocked_tasks line $LINENO"; return 1; }
  [[ "$done" == *$'\n'"$(sid 3)"' "Diff viewer" can start now: nothing it waits for is left' && "$done" != *$(sid 4)* ]] || { echo "blocked_tasks: done didn't name what it freed: $done"; return 1; }
  check wait_for 10 eval '[[ "$(desk scroll.blocked.0.task)" == $(tid 4) && "$(desk scroll.blocked.0.waits)" == "[\"Status fix\"]" ]]' || { echo "blocked_tasks: the freed row is still blocked"; desk scroll.blocked; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 3)"' +Diff viewer +Shelf +stopped$' || { in_repo task list; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 3)"'","window":"'"$w"'"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk running)" == 1 && "$(desk shelf)" == "[\"$(tid 2)\", \"$(tid 4)\"]" ]]' || { echo "blocked_tasks: the freed task didn't start"; desk shelf; desk note; return 1; }
  expected="$(sid 4) waits for nothing: it can start"; says in_repo task edit $(tid 4) --after none || { echo "blocked_tasks line $LINENO"; return 1; }
  in_repo task start $(tid 4) >/dev/null || { echo "blocked_tasks: a task with its blockers taken off didn't start"; return 1; }
  [[ "$(in_repo task edit $(tid 4) --after $(tid 2) 2>&1)" == *"$(sid 4) has started"* ]] || { echo "blocked_tasks: a started task's blockers changed"; return 1; }
}

scenario stacked_tasks M4
stacked_tasks() {
  start_daemon && start_app || { echo "stacked_tasks line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "stacked_tasks line $LINENO"; return 1; }
  git -C "$repo" config user.email e@e && git -C "$repo" config user.name e
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  in_repo task new A "hello there" --agent "$agent" >/dev/null || { echo "stacked_tasks line $LINENO"; return 1; }
  in_repo task new B "hello there" --agent "$agent" --after "$(tid 1)" >/dev/null && in_repo task new C "hello there" --agent "$agent" --after "$(tid 2)" >/dev/null || { echo "stacked_tasks line $LINENO"; return 1; }
  local a b c; a="$(tid 1)" b="$(tid 2)" c="$(tid 3)"
  folder() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
print(next(t["folder"] for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]))' "$1"; }
  of() { in_repo task list --json | python3 -c 'import json, sys
t = next(t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1])
names = lambda k: ",".join(x.get("name") or x["id"] for x in t.get(k) or [])
print(t.get("merge") or "-", "stale:" + names("stale"), "bases:" + names("bases"))' "$1"; }
  head() { git -C "$1" rev-parse HEAD; }
  [[ "$(in_repo task ready set "$a" 2>&1)" == *"\`kanna task ready set\` is gone: use \`kanna task ready [TASK]\`"* ]] || { echo "stacked_tasks: ready set didn't say it's gone"; return 1; }
  in_repo task start "$a" >/dev/null || { echo "stacked_tasks: A didn't start"; return 1; }
  [[ "$(in_repo task start "$b" 2>&1)" == *"waits for $(sid 1) \"A\": it can start once it's ready or done"* ]] || { echo "stacked_tasks: B started before A was claimed"; return 1; }
  local fa fb fc said; fa="$(folder "$a")"
  git -C "$fa" commit -q --allow-empty -m a1 || return 1
  said="$(in_repo task ready "$a" 2>&1)"
  [[ "$said" == "$(sid 1) is "*$'\n'"$(sid 2) \"B\" can start now: nothing it waits for is left" ]] || { echo "stacked_tasks: ready said $said"; return 1; }
  check wait_for 10 eval '[[ "$(of "$a")" == "ready stale: bases:" ]]' || { echo "stacked_tasks: A claimed and clean isn't ready"; of "$a"; return 1; }
  in_repo task start "$b" >/dev/null || { echo "stacked_tasks: B didn't start on A's claimed work"; return 1; }
  fb="$(folder "$b")"
  check test "$(head "$fb")" == "$(head "$fa")" || { echo "stacked_tasks: B didn't start from A's branch"; return 1; }
  check test "$(of "$b")" == "after stale: bases:A" || { of "$b"; return 1; }
  git -C "$fb" commit -q --allow-empty -m b1 && in_repo task ready "$b" >/dev/null || { echo "stacked_tasks line $LINENO"; return 1; }
  check test "$(of "$b")" == "after stale: bases:A" || { echo "stacked_tasks: a claimed stacked task isn't merges after"; of "$b"; return 1; }
  in_repo task start "$c" >/dev/null || { echo "stacked_tasks: C didn't start on B's claimed work"; return 1; }
  fc="$(folder "$c")"
  check test "$(head "$fc")" == "$(head "$fb")" || { echo "stacked_tasks: C didn't start from B's branch"; return 1; }
  in_repo task list | placed | grep -E '^'"$(sid 2)"' +B +Shelf ' | grep -v ' ready ' | grep -qE ' merges after "A"$' || { echo "stacked_tasks: B doesn't say it merges after A, or says ready"; in_repo task list; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "stacked_tasks line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  stack() { desk scroll.stacked | python3 -c 'import json, sys
print(next((r["stack"] for r in json.load(sys.stdin) if r["task"] == sys.argv[1]), "-"))' "$1"; }
  check wait_for 10 eval '[[ "$(stack "$b")" == "merges after “A”" ]]' || { echo "stacked_tasks: B's Shelf row doesn't show its stack"; desk scroll.stacked; return 1; }
  check wait_for 10 eval '[[ "$(stack "$a")" == "ready" ]]' || { echo "stacked_tasks: A's row doesn't say ready"; desk scroll.stacked; return 1; }

  git -C "$fa" commit -q --allow-empty -m a2 || return 1
  check wait_for 10 eval '[[ "$(of "$b")" == "stale stale:A bases:A" ]]' || { echo "stacked_tasks: a new commit on A didn't make B stale"; of "$b"; return 1; }
  check test "$(of "$a")" == "- stale: bases:" || { echo "stacked_tasks: A is still ready after its branch moved"; of "$a"; return 1; }
  check test "$(of "$c")" == "after stale: bases:B" || { echo "stacked_tasks: C went stale though B's head is in it"; of "$c"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 2)"' +B +Shelf +stale +behind "A"$' || { in_repo task list; return 1; }
  check wait_for 10 eval '[[ "$(stack "$b")" == "stale · behind “A”" ]]' || { echo "stacked_tasks: B's row doesn't show it's stale"; desk scroll.stacked; return 1; }
  sleep 0.5; snap "$w" stacked_tasks-stale >/dev/null
  in_repo task messages "$b" | grep -q "kanna .*\"A\" moved since this task was built on it" || { echo "stacked_tasks: B wasn't told to catch up"; in_repo task messages "$b"; return 1; }
  [[ "$(in_repo task ready "$b" 2>&1)" == *"\"A\" isn't in this branch yet: merge it first (git merge $(head "$fa"))"* ]] || { echo "stacked_tasks: B was let claim without A's new work"; return 1; }

  git -C "$fb" merge -q --no-edit "$(head "$fa")" || return 1
  check wait_for 10 eval '[[ "$(of "$b")" == "after stale: bases:A" ]]' || { echo "stacked_tasks: merging A didn't clear B's staleness"; of "$b"; return 1; }
  check wait_for 10 eval '[[ "$(of "$c")" == "stale stale:B bases:B" ]]' || { echo "stacked_tasks: B's new head didn't make C stale"; of "$c"; return 1; }
  [[ "$(in_repo task ready "$c" 2>&1)" == *"\"B\" isn't in this branch yet"* ]] || { echo "stacked_tasks: C was let claim without B's new work"; return 1; }
  in_repo task ready "$b" >/dev/null || { echo "stacked_tasks: B couldn't claim again"; return 1; }
  in_repo task messages "$c" | grep -q "kanna .*\"B\" is ready again at a new commit" || { echo "stacked_tasks: C wasn't told B is claimed again"; in_repo task messages "$c"; return 1; }
  git -C "$fc" merge -q --no-edit "$(head "$fb")" && in_repo task ready "$c" >/dev/null || { echo "stacked_tasks: C couldn't catch up"; return 1; }
  check wait_for 10 eval '[[ "$(of "$c")" == "after stale: bases:B" ]]' || { of "$c"; return 1; }
  check wait_for 10 eval '[[ "$(desk scroll.stacked)" != *"waits for"* ]]' || { echo "stacked_tasks: a started task's row says it waits"; desk scroll.stacked; return 1; }

  git -C "$repo" merge -q --no-edit "$(head "$fa")" || return 1
  check wait_for 10 eval '[[ "$(of "$b")" == "ready stale: bases:" ]]' || { echo "stacked_tasks: A in main didn't make B ready"; of "$b"; return 1; }
  in_repo task messages "$b" | grep -q "\"A\"'s work is in main now. Nothing you build on" || { echo "stacked_tasks: B wasn't told A landed"; in_repo task messages "$b"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 2)"' +B +Shelf +ready$' || { echo "stacked_tasks: B doesn't list as ready"; in_repo task list; return 1; }

  echo b >"$fb/f" && git -C "$fb" add f && git -C "$fb" commit -q -m b2 && in_repo task ready "$b" >/dev/null || { echo "stacked_tasks line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(of "$b")" == "ready stale: bases:" ]]' || { of "$b"; return 1; }
  local objects; objects="$(git -C "$repo" count-objects | cut -d' ' -f1)"
  echo main >"$repo/f" && git -C "$repo" add f && git -C "$repo" commit -q -m m1 || return 1
  check wait_for 10 eval '[[ "$(of "$b")" == "conflicts stale: bases:" ]]' || { echo "stacked_tasks: main conflicting with B didn't show"; of "$b"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 2)"' +B +Shelf +conflicts$' || { echo "stacked_tasks: B doesn't list as conflicts"; in_repo task list; return 1; }
  check wait_for 10 eval '[[ "$(stack "$b")" == "conflicts" ]]' || { echo "stacked_tasks: B's row doesn't show conflicts"; desk scroll.stacked; return 1; }
  check test "$(git -C "$repo" count-objects | cut -d' ' -f1)" -le "$((objects + 4))" || { echo "stacked_tasks: the conflict check wrote into the repo"; git -C "$repo" count-objects; return 1; }
  [[ "$(in_repo task ready "$b" 2>&1)" == *"conflicts with main in f"* ]] || { echo "stacked_tasks: claiming B didn't name the conflict"; return 1; }
  git -C "$fc" merge -q --no-edit "$(head "$fb")" && in_repo task ready "$c" >/dev/null || { echo "stacked_tasks: C couldn't go on because B conflicts with main"; return 1; }
  check wait_for 10 eval '[[ "$(of "$c")" == "after stale: bases:B" ]]' || { echo "stacked_tasks: C stopped because B conflicts with main"; of "$c"; return 1; }
  raw '{"id":1,"cmd":"tasks","args":{"action":"events","task":"'"$b"'"}}' | python3 -c 'import json, sys
seen = [e["what"] for e in json.load(sys.stdin)["events"]]
sys.exit(0 if all(w in seen for w in ("ready", "stale", "fresh", "base merged")) else print(seen) or 1)' || { echo "stacked_tasks: B's events don't record ready, stale, fresh and base merged"; return 1; }
  sleep 0.5; snap "$w" stacked_tasks >/dev/null
  stop_agents
}

scenario shelf_runners M4
shelf_runners() {
  start_daemon && start_app || { echo "shelf_runners line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_runners line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  runners() { machine_runners "$1"; }
  shelf() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
tasks = json.load(sys.stdin)["shelf"]
for t in tasks: print(t["id"], "running" if t.get("running") else "stopped:" + (t.get("stopped") or "-") if t.get("folder") else "waiting", t.get("held") or "-", t.get("session") or "-")'; }
  running() { shelf | awk '$2 == "running" { print $1 }' | xargs; }
  of() { shelf | awk -v id="$1" '$1 == id { print $'"$2"' }'; }
  runners 0
  in_repo task new "Work one" "work 3" --agent "$fake/codex" >/dev/null && in_repo task new "Asks" "turn" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  in_repo task new "Blocked" "work 1" --agent "$fake/codex" --after $(tid 1) >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  for name in Four Five Six; do in_repo task new "Work $name" "work 3" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }; done
  sleep 4
  check test -z "$(running)" || { echo "shelf_runners: shelf.runners 0 ran a task"; shelf; return 1; }
  check test ! -e "$repo/.kanna-worktrees" || { echo "shelf_runners: shelf.runners 0 made a worktree"; return 1; }
  runners 3
  check wait_for 10 eval '[[ "$(running)" == "$(tid 1) $(tid 2) $(tid 4)" ]]' || { echo "shelf_runners: three didn't start in order, skipping the blocked one"; shelf; return 1; }
  local seen="" most=0 now t
  for _ in $(seq 400); do
    now="$(running)"
    (( $(wc -w <<<"$now") > most )) && most=$(wc -w <<<"$now")
    for t in $now; do [[ " $seen " == *" $t "* ]] || seen="$seen $t"; done
    [[ "$(of $(tid 6) 2)" == stopped:* && "$(of $(tid 5) 2)" == stopped:* ]] && break
    sleep 0.1
  done
  check test "$most" -le 3 || { echo "shelf_runners: $most ran at once"; return 1; }
  check test "${seen# }" == "$(tid 1) $(tid 2) $(tid 4) $(tid 5) $(tid 6)" || { echo "shelf_runners: they started as $seen"; shelf; return 1; }
  check test "$(of $(tid 1) 2)" == stopped:finished || { shelf; return 1; }
  check test "$(of $(tid 2) 2)" == stopped:needs-you || { echo "shelf_runners: the task that asked didn't stop as needs you"; shelf; return 1; }
  check test "$(of $(tid 6) 2)" == stopped:finished || { shelf; return 1; }
  check test "$(of $(tid 3) 2)" == waiting || { echo "shelf_runners: the blocked task ran"; shelf; return 1; }
  local asks sid; asks="$(of $(tid 2) 3)" sid="$(of $(tid 1) 4)"
  [[ "$sid" =~ ^[0-9a-f-]{36}$ && "$(kanna read "$(of $(tid 1) 3)")" == *$'\n'"session $sid"$'\n'* ]] || { echo "shelf_runners: $(sid 1)'s session isn't the one it started ($sid)"; kanna read "$(of $(tid 1) 3)"; return 1; }
  sid="$(of $(tid 2) 4)"
  [[ "$sid" =~ ^[0-9a-f-]{36}$ ]] && unwrapped "$asks" "To continue this session, run: codex resume $sid" || { echo "shelf_runners: the session codex said as it quit wasn't recorded ($sid)"; shelf; kanna read "$asks"; return 1; }
  check is "$asks" state exited || { echo "shelf_runners: the stopped agent still runs"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 2)"' +Asks +Shelf +stopped( +codex)?$' || { echo "shelf_runners: task list shows how far a Shelf task got"; in_repo task list; return 1; }
  raw '{"id":1,"cmd":"tasks","args":{"action":"events","task":"'"$(tid 2)"'"}}' | grep -q '"why": *"needs-you"' || { echo "shelf_runners: why $(sid 2) stopped isn't in its events"; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "shelf_runners line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk shelf)" == "[\"$(tid 1)\", \"$(tid 2)\", \"$(tid 3)\", \"$(tid 4)\", \"$(tid 5)\", \"$(tid 6)\"]" ]]' || { echo "shelf_runners: the rows aren't oldest first"; desk shelf; return 1; }
  check test "$(desk scroll.dots.0.dot)" == idle || { echo "shelf_runners: a task paused at its turn's end lost its status"; desk scroll; return 1; }
  check test "$(desk scroll.dots.1.dot)" == needs-you || { echo "shelf_runners: a task paused asking lost its status"; desk scroll; return 1; }
  in_repo task done $(tid 1) >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(of $(tid 3) 2)" != waiting ]]' || { echo "shelf_runners: the blocked task didn't run once it was free"; shelf; return 1; }
  [[ "$(kanna task pick $(tid 2))" == "moved $(sid 2) to the Desk: "* ]] || { echo "shelf_runners: the stopped task didn't move to the Shelf"; return 1; }
  check is Asks id "$asks" || { echo "shelf_runners: the started task's pane should keep its ID"; return 1; }
  check wait_for 10 shows "$asks" "resumed $sid" || { echo "shelf_runners: the stopped task didn't resume its session on the Shelf"; kanna read "$asks"; return 1; }
  unwrapped "$asks" "args: resume $sid --remote unix://" || { kanna read "$asks"; return 1; }
  sleep 6
  check is "$asks" state running || { echo "shelf_runners: a task on the Shelf was stopped"; return 1; }
  [[ "$(kanna task shelve "$asks")" == *"to the Shelf as $(sid 2)"* ]] || { echo "shelf_runners: the resumed task didn't move back to the Shelf"; return 1; }
  check wait_for 15 eval '[[ "$(of $(tid 2) 2)" == stopped:finished ]]' || { echo "shelf_runners: a task moved to the Shelf idle didn't stop"; shelf; return 1; }
  runners 0
  in_repo task new "Seven" "work 1" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  sleep 4
  check test "$(of $(tid 7) 2)" == waiting || { echo "shelf_runners: shelf.runners 0 ran a task"; shelf; return 1; }
  in_repo task done $(tid 7) >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  cat >"$fake/present.sh" <<PRESENT
#!/bin/bash
set -e
exec 2>>"$KANNA_STATE_DIR/present.log"
printf '# Change\\n\\nWhat changed, and why.\\n' >notes.md
md="\$("$bin/kanna" open md notes.md | tee "$KANNA_STATE_DIR/opened" | sed -E 's/^opened ([a-z]+-[0-9]+).*/\1/')"
"$bin/kanna" comment notes.md:3 "Why it changed." >/dev/null
long="\$("$bin/kanna" open --tab --cmd 'echo LONG_RUNNING; exec sleep 600' | sed -E 's/^opened ([a-z]+-[0-9]+).*/\1/')"
short="\$("$bin/kanna" open --tab --cmd 'echo SHORT_DONE' | sed -E 's/^opened ([a-z]+-[0-9]+).*/\1/')"
until "$bin/kanna" workspace list --all --json | python3 -c 'import json, sys
def panes(v):
    if isinstance(v, dict):
        if v.get("id") == sys.argv[1]: yield v
        for x in v.values(): yield from panes(x)
    elif isinstance(v, list):
        for x in v: yield from panes(x)
sys.exit(not any(p.get("state") == "exited" for p in panes(json.load(sys.stdin))))' "\$short"; do sleep 0.1; done
"$bin/kanna" focus "\$md" >"$KANNA_STATE_DIR/focused"
"$bin/kanna" zoom "\$md" >/dev/null
echo "\$md \$long \$short" >"$KANNA_STATE_DIR/presented"
PRESENT
  chmod +x "$fake/present.sh"
  in_repo task new "Presents" "run $fake/present.sh" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  runners 1
  check wait_for 60 eval '[[ "$(of $(tid 8) 2)" == stopped:finished ]]' || { echo "shelf_runners: the presenting task didn't stop"; shelf; cat "$KANNA_STATE_DIR/present.log"; return 1; }
  local presenter md long short; presenter="$(of $(tid 8) 3)"
  read -r md long short <"$KANNA_STATE_DIR/presented" || { echo "shelf_runners: the agent didn't present"; kanna read "$presenter"; cat "$KANNA_STATE_DIR/present.log"; return 1; }
  check is "$md" type md || { echo "shelf_runners: stopping closed the markdown tab"; kanna workspace list --all; return 1; }
  [[ "$(cat "$KANNA_STATE_DIR/opened")" =~ ^"opened $md in ws-"[0-9]+" ($(sid 8), on the Shelf), tab-"[0-9]+": "[a-z]+$ ]] || { echo "shelf_runners: a held task's open said: $(cat "$KANNA_STATE_DIR/opened")"; return 1; }
  [[ "$(cat "$KANNA_STATE_DIR/focused")" =~ ^"focused $md in ws-"[0-9]+" ($(sid 8), on the Shelf)"$ ]] || { echo "shelf_runners: a held task's focus said: $(cat "$KANNA_STATE_DIR/focused")"; return 1; }
  check is "$long" state exited || { echo "shelf_runners: stopping didn't hang up the running terminal"; kanna workspace list --all; return 1; }
  check is "$short" state exited || { echo "shelf_runners line $LINENO"; return 1; }
  held() { raw '{"id":1,"cmd":"debug.saveState"}' | python3 -c 'import json, sys
w = next(w for w in json.load(sys.stdin)["state"]["parked"] if (w.get("dock") or {}).get("pane") == sys.argv[1])
print(len(w["tabs"]), w.get("focused"), w.get("zoomed"))' "$presenter"; }
  check test "$(held)" == "3 $md $md" || { echo "shelf_runners: the stopped task's layout changed: $(held)"; return 1; }
  runners 0
  local presenter_run long_run; presenter_run="$(pane "$presenter" terminal)" long_run="$(pane "$long" terminal)"
  [[ "$(kanna task pick $(tid 8))" == "moved $(sid 8) to the Desk: "* ]] || { echo "shelf_runners: the presenting task didn't move to the Shelf"; return 1; }
  check wait_for 10 eval '[[ "$(pane Presents terminal)" != "$presenter_run" ]]' || { echo "shelf_runners: the task's agent didn't start again"; return 1; }
  check is Presents id "$presenter" || { echo "shelf_runners: the task's pane should keep its ID"; return 1; }
  check wait_for 10 eval '[[ "$(pane "$long" terminal)" != "$long_run" ]]' || { echo "shelf_runners: the hung-up terminal didn't run again as a new terminal"; kanna workspace list --all; return 1; }
  check wait_for 10 is "$long" state running || { echo "shelf_runners: the hung-up terminal didn't run again"; kanna workspace list --all; return 1; }
  check wait_for 5 shows "$long" LONG_RUNNING || { kanna read "$long"; return 1; }
  check is "$short" state exited || { echo "shelf_runners: a terminal that had exited before the stop ran again"; return 1; }
  local desk8; desk8="$(kanna task activate "$presenter")" && desk8="${desk8##* active on }" && [[ "$desk8" =~ ^d[0-9]+$ ]] || { echo "shelf_runners line $LINENO: $desk8"; return 1; }
  shown() { echo "$(dwin "$desk8" tabs | count) $(dwin "$desk8" focused) $(dwin "$desk8" zoomed)"; }
  check wait_for 5 eval '[[ "$(shown)" == "3 $md $md" ]]' || { echo "shelf_runners: the pulled task doesn't show what it presented: $(shown)"; app_view; return 1; }
  [[ "$(kanna task pick $(tid 2))" == "moved $(sid 2) to the Desk: "* ]] || { echo "shelf_runners line $LINENO"; return 1; }
  kanna task activate $(tid 2) >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin "$desk8" dock.pane)" == "$(pane Asks id)" ]]' || { echo "shelf_runners: $(sid 2) didn't take $(sid 8)'s place on the Desk"; app_view; return 1; }
  [[ "$(ws $(tid 8) panes)" == *"\"$md\""* && "$(ws $(tid 2) panes)" != *"\"$md\""* ]] || { echo "shelf_runners: $(sid 8)'s tabs aren't in its workspace"; state; return 1; }
  [[ "$(KANNA_TERMINAL="$presenter" kanna open --name after --tab --no-wait)" == "opened "*" (after) in ws-"*" ($(sid 8), on the Desk), tab-"* ]] || { echo "shelf_runners: $(sid 8)'s open went elsewhere"; state; return 1; }
  runners 0
  for name in "Old stopped" "Old new" "New stopped" "New new"; do in_repo task new "$name" "work 1" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }; done
  for t in $(tid 9) $(tid 11); do
    in_repo task start $t >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
    check wait_for 10 eval '[[ "$(of $t 2)" == running ]]' || { echo "shelf_runners: $t didn't start"; shelf; return 1; }
    in_repo task stop $t >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
    check wait_for 15 eval '[[ "$(of $t 2)" == stopped:* ]]' || { echo "shelf_runners: $t didn't stop"; shelf; return 1; }
  done
  (cd "$repo" && KANNA_TERMINAL="$(of $(tid 9) 3)" kanna task send $(tid 11) "your turn, newer" >/dev/null && KANNA_TERMINAL="$(of $(tid 11) 3)" kanna task send $(tid 9) "your turn, older" >/dev/null) || { echo "shelf_runners line $LINENO"; return 1; }
  runners 1
  seen="" most=0
  for _ in $(seq 900); do
    now="$(running)"
    (( $(wc -w <<<"$now") > most )) && most=$(wc -w <<<"$now")
    for t in $now; do [[ " $seen " == *" $t "* ]] || seen="$seen $t"; done
    [[ "$(of $(tid 12) 2)" == stopped:* ]] && break
    sleep 0.1
  done
  check test "$most" -le 1 || { echo "shelf_runners: $most ran at once with one runner"; return 1; }
  check test "${seen# }" == "$(tid 9) $(tid 10) $(tid 11) $(tid 12)" || { echo "shelf_runners: they didn't take their turn by age: $seen, not $(tid 9) $(tid 10) $(tid 11) $(tid 12)"; shelf; return 1; }
  for t in $(tid 9) $(tid 11); do
    check wait_for 8 eval 'in_repo task messages '"$t"' | grep -q delivered' || { echo "shelf_runners: $t didn't get its message"; in_repo task messages $t; return 1; }
  done
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/claude"
  clock() { python3 -c 'import time; print(time.time())'; }
  waited() { raw '{"id":1,"cmd":"tasks","args":{"action":"events","task":"'"$1"'"}}' | python3 -c 'import json, sys
print(" ".join(p["command"] for e in json.load(sys.stdin)["events"] if e["what"] == "waiting" for p in (e.get("doc") or {}).get("processes", [])))'; }
  in_repo task new "Background" "background 4" --agent "$fake/claude" >/dev/null && in_repo task new "Wakes" "background 2 wake 4" --agent "$fake/claude" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  runners 3
  check wait_for 10 eval '[[ "$(of $(tid 13) 2)" == running && "$(of $(tid 13) 3)" == term-* ]]' || { echo "shelf_runners: the background task didn't start"; shelf; return 1; }
  local bg; bg="$(of $(tid 13) 3)"
  check wait_for 10 shows "$bg" BACKGROUND_STARTED || { kanna read "$bg"; return 1; }
  check wait_for 5 is "$bg" status idle || { echo "shelf_runners: the background task didn't report idle"; kanna read "$bg"; return 1; }
  sleep 1.5
  check test "$(of $(tid 13) 2)" == running || { echo "shelf_runners: a task idle with its background command running was stopped"; shelf; kanna read "$bg"; return 1; }
  check test "$(waited $(tid 13))" == "sleep 4" || { echo "shelf_runners: why it waits isn't in its events: $(waited $(tid 13))"; return 1; }
  check wait_for 10 shows "$bg" BACKGROUND_EXITED || { kanna read "$bg"; return 1; }
  local exited stopped; exited="$(clock)"
  sleep 5
  check test "$(of $(tid 13) 2)" == running || { echo "shelf_runners: the background task was stopped within the grace"; shelf; return 1; }
  check wait_for 20 eval '[[ "$(of $(tid 13) 2)" == stopped:finished ]]' || { echo "shelf_runners: the background task didn't stop once its command was done"; shelf; return 1; }
  stopped="$(clock)"
  python3 -c 'import sys; sys.exit(not float(sys.argv[2]) - float(sys.argv[1]) >= 8)' "$exited" "$stopped" || { echo "shelf_runners: the background task stopped before the grace passed"; return 1; }
  local wakes; wakes="$(of $(tid 14) 3)"
  check wait_for 30 eval '[[ "$(of $(tid 14) 2)" == stopped:finished ]]' || { echo "shelf_runners: the task woken by its background command didn't stop"; shelf; kanna read "$wakes"; return 1; }
  shows "$wakes" WOKE_DONE || { echo "shelf_runners: the task that worked again within the grace was stopped mid-turn"; kanna read "$wakes"; return 1; }
  local second="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/second"
  mkdir -p "$second/.kanna" && git -C "$second" init -q -b main && git -C "$second" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_runners line $LINENO"; return 1; }
  second="$(cd "$second" && pwd -P)"
  in_second() { (cd "$second" && kanna "$@"); }
  cap() { printf '{"version": 1, "shelf": {"runners": %s}}\n' "$1" >"$second/.kanna/config.json"; }
  sorted() { xargs -n1 | sort | xargs; }
  runners 0
  cap 5
  in_repo task new "First here" "work 3" --agent "$fake/codex" >/dev/null && in_second task new "First there" "work 3" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  in_repo task new "Next here" "work 3" --agent "$fake/codex" >/dev/null && in_second task new "Next there" "work 3" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  sleep 3
  check test -z "$(running)" || { echo "shelf_runners: a repo's 5 ran tasks past a machine total of 0"; shelf; return 1; }
  runners 2
  seen="" most=0
  for _ in $(seq 600); do
    now="$(running)"
    (( $(wc -w <<<"$now") > most )) && most=$(wc -w <<<"$now")
    for t in $now; do [[ " $seen " == *" $t "* ]] || seen="$seen $t"; done
    [[ "$(of $(tid 18) 2)" == stopped:* ]] && break
    sleep 0.1
  done
  check test "$most" -le 2 || { echo "shelf_runners: $most ran at once across two repos with a machine total of 2"; return 1; }
  check test "${seen# }" == "$(tid 15) $(tid 16) $(tid 17) $(tid 18)" || { echo "shelf_runners: two repos' tasks didn't start oldest first: $seen"; shelf; return 1; }
  runners 0
  cap 1
  for name in Three Four Five; do in_second task new "There $name" "work 3" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }; done
  for name in Three Four; do in_repo task new "Here $name" "work 3" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }; done
  runners 3
  local capped; capped="$(sorted <<<"$(tid 19) $(tid 22) $(tid 23)")"
  check wait_for 10 eval '[[ "$(running | sorted)" == "$capped" ]]' || { echo "shelf_runners: a repo at its cap didn't leave its places to the next repo's newer tasks"; shelf; return 1; }
  most=0
  for _ in $(seq 600); do
    now="$(running)"
    local there=0
    for t in $(tid 19) $(tid 20) $(tid 21); do [[ " $now " == *" $t "* ]] && there=$((there + 1)); done
    (( there > most )) && most=$there
    (( $(wc -w <<<"$now") > 3 )) && { echo "shelf_runners: more ran at once than the machine total: $now"; shelf; return 1; }
    [[ "$(of $(tid 21) 2)" == stopped:* ]] && break
    sleep 0.1
  done
  check test "$most" -eq 1 || { echo "shelf_runners: $most of a repo capped at 1 ran at once"; return 1; }
  check test "$(of $(tid 21) 2)" == stopped:finished || { echo "shelf_runners: the capped repo's last task didn't run"; shelf; return 1; }
  runners 0
  cap 0
  in_second task new "There off" "work 1" --agent "$fake/codex" >/dev/null && in_repo task new "Here on" "work 1" --agent "$fake/codex" >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  runners 2
  check wait_for 15 eval '[[ "$(of $(tid 25) 2)" == stopped:finished ]]' || { echo "shelf_runners: a repo set to 0 held up another repo's task"; shelf; return 1; }
  check test "$(of $(tid 24) 2)" == waiting || { echo "shelf_runners: a repo's 0 didn't hold"; shelf; return 1; }
}

scenario merge_queue M4
merge_queue() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/queue" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$fake/bin" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/bin/claude"
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$fake/bin/claude" >"$fake/claude" && chmod +x "$fake/claude"
  python3 -c 'import json, sys; p = sys.argv[1]; c = json.load(open(p)); c["pipelines"] = {"agents": {"claude": sys.argv[2]}}; json.dump(c, open(p, "w"))' "$KANNA_AGENTS_HOME/.kanna-v4/config.json" "$fake/claude" || { echo "merge_queue line $LINENO"; return 1; }
  start_daemon && start_app || { echo "merge_queue line $LINENO"; return 1; }
  mkdir -p "$repo/.kanna/prompts" && git -C "$repo" init -q -b main || { echo "merge_queue line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  git -C "$repo" config user.email e@e && git -C "$repo" config user.name e
  cat >"$repo/test.sh" <<'TEST'
if [ -e needs-a.txt ] && [ ! -e a.txt ]; then echo "FAILED: a.txt isn't here, so this wasn't tested on top of A"; exit 1; fi
if [ -e slow ] && [ ! -e .kanna/tmp/slept ]; then touch .kanna/tmp/testing; sleep 15; touch .kanna/tmp/slept; fi
mkdir -p target
if [ -e target/built ]; then echo "warm $(git rev-parse --short HEAD)" >>target/builds; else touch target/built; echo "cold $(git rev-parse --short HEAD)" >>target/builds; fi
exec sh bin/unit
TEST
  cat >"$repo/judge.sh" <<'JUDGE'
log="$(ls -t .kanna/tmp/stage-*.log | head -1)"
here="$(git branch --show-current)" mine="" others="" failing="$(tr -d '\r' <"$log" | sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p')"
[ -n "$failing" ] || { printf 'the tests fail before naming a test\n%s\n' "$(tail -3 "$log")" >"$KANNA_ANSWER"; exec "$KANNA_CLI" run done fail; }
if [ -e waits ] && [ ! -e .kanna/tmp/waited ]; then
  touch .kanna/tmp/waited
  KANNA_RUN_WAIT=0 "$KANNA_CLI" run command 'sleep 4; echo the suite on main'
  exit 0
fi
for t in $failing; do
  if sh bin/unit --exact "$t" >/dev/null 2>&1; then others="$others, $t passes alone"; continue; fi
  git checkout -q --detach HEAD^1
  said="$(KANNA_RUN_WAIT=1 "$KANNA_CLI" run command "sleep 2; sh bin/unit --exact $t")"
  while [ "$(printf '%s\n' "$said" | head -1)" = "still running" ]; do echo waited >>.kanna/tmp/waits; said="$(KANNA_RUN_WAIT=1 "$KANNA_CLI" run command --wait)"; done
  if [ "$(printf '%s\n' "$said" | head -1)" = "exit 0" ]; then mine="$mine $t"; else others="$others, $t fails on main too"; fi
  git checkout -q -f "$here"
done
if [ -n "$mine" ]; then printf '%s fails: the branch broke it\nIt passes on main and fails here, alone too.\n' "${mine# }" >"$KANNA_ANSWER"; exec "$KANNA_CLI" run done fail; fi
printf 'tests fail, none of them the branch'"'"'s: %s\n' "${others#, }" >"$KANNA_ANSWER"
exec "$KANNA_CLI" run done pass
JUDGE
  printf 'Judge the merge.\nmock: run sh judge.sh\n' >"$repo/.kanna/prompts/merge-judge.md"
  mkdir -p "$repo/bin"
  cat >"$repo/bin/unit" <<'UNIT'
#!/bin/sh
alone=""
[ "$1" = --exact ] && alone="$2"
t() { [ -z "$alone" ] || [ "$alone" = "$1" ]; }
failed=0 out=""
echo "running tests"
if t steady; then echo "test steady ... ok"; fi
if t known_broken && [ -e known-broken ]; then echo "test known_broken ... FAILED"; failed=1; fi
if t broken_thing && [ -e broken ]; then echo "test broken_thing ... FAILED"; failed=1; out="---- broken_thing stdout ----
FAILED: broken is here"; fi
if t flaky_thing && [ -e flaky ]; then if [ -z "$alone" ]; then echo "test flaky_thing ... FAILED"; failed=1; else echo "test flaky_thing ... ok"; fi; fi
[ -n "$out" ] && printf '\nfailures:\n\n%s\n\nfailures:\n    broken_thing\n\n' "$out"
if [ "$failed" = 1 ]; then echo "test result: FAILED."; exit 101; fi
echo "test result: ok."
UNIT
  chmod +x "$repo/bin/unit"
  printf 'target/\n.kanna/tmp/\n' >"$repo/.gitignore"
  printf '{"version": 1, "merge": {"test": "sh test.sh"}, "tasks": {"timeout": 5}}\n' >"$repo/.kanna/config.json"
  echo one >"$repo/shared.txt"
  git -C "$repo" add -A && git -C "$repo" commit -qm first || { echo "merge_queue line $LINENO"; return 1; }
  in_repo() { (cd "$repo" && kanna "$@"); }
  local n; for n in A B C D E F; do in_repo task new "$n" "say hi" --agent "$fake/claude" >/dev/null || { echo "merge_queue line $LINENO"; return 1; }; done
  for n in 1 2 3 4 5 6; do kanna task start "$(tid $n)" >/dev/null || { echo "merge_queue line $LINENO"; return 1; }; done
  local a b c d e f; a="$(tid 1)" b="$(tid 2)" c="$(tid 3)" d="$(tid 4)" e="$(tid 5)" f="$(tid 6)"
  wt() { echo "$repo/.kanna-worktrees/$1"; }
  commit() { local w; w="$(wt "$1")"; shift; (cd "$w" && "$@" && git add -A && git commit -qm "work") ; }
  check wait_for 20 eval '[[ -d "$(wt $d)" && -d "$(wt $e)" && -d "$(wt $f)" ]]' || { echo "merge_queue: the tasks didn't start"; return 1; }
  local opened dw; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "merge_queue line $LINENO: $opened"; return 1; }
  dw="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$dw\"}}" | field "$1"; }
  click() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{$1,\"window\":\"$dw\"}}" | grep -q '"ok":true'; }
  entries() { desk queue | python3 -c 'import json, sys; print(" ".join(e["task"] + ":" + e["state"] for e in json.load(sys.stdin)))'; }
  raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$dw\",\"size\":[1300,900]}}" >/dev/null
  check wait_for 10 eval '[[ "$(desk pages)" == '"'"'["Desk", "Machines", "Builds", "Stats", "Schedules", "Merge queue"]'"'"' ]]' || { echo "merge_queue: the Desk view's fixed tabs"; desk pages; return 1; }
  commit "$a" sh -c 'echo a >a.txt && touch slow' || { echo "merge_queue line $LINENO"; return 1; }
  commit "$b" sh -c 'echo b >b.txt && touch needs-a.txt' || { echo "merge_queue line $LINENO"; return 1; }
  commit "$c" sh -c 'echo c >c.txt && touch broken' || { echo "merge_queue line $LINENO"; return 1; }
  commit "$d" sh -c 'echo dee >shared.txt' || { echo "merge_queue line $LINENO"; return 1; }
  commit "$e" sh -c 'echo e >e.txt && touch flaky' || { echo "merge_queue line $LINENO"; return 1; }
  commit "$f" sh -c 'echo f >f.txt && touch waits' || { echo "merge_queue line $LINENO"; return 1; }
  echo main >"$repo/shared.txt" && git -C "$repo" commit -qam "main moves" || { echo "merge_queue line $LINENO"; return 1; }
  local said
  said="$(in_repo task merge "$d" 2>&1)" && { echo "merge_queue: a conflicting branch went into the queue: $said"; return 1; }
  [[ "$said" == *"conflicts with main in shared.txt"* ]] || { echo "merge_queue: the conflict's refusal said: $said"; return 1; }
  echo dirty >>"$(wt $c)/c.txt"
  said="$(in_repo task merge "$c" 2>&1)" && { echo "merge_queue: uncommitted changes went into the queue: $said"; return 1; }
  [[ "$said" == *"uncommitted changes to 1 file (c.txt)"* ]] || { echo "merge_queue: the uncommitted refusal said: $said"; return 1; }
  git -C "$(wt $c)" checkout -q c.txt
  local agent_a; agent_a="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from tasks where id = '$a'")"
  check wait_for 10 eval '[[ "$(pane "$agent_a" state)" == running ]]' || { echo "merge_queue: A's agent isn't running"; return 1; }
  said="$(in_repo task merge "$a" -m "a: add a" 2>&1)" || { echo "merge_queue: A didn't go into the queue: $said"; return 1; }
  [[ "$said" == *"is next in the merge queue"* ]] || { echo "merge_queue: merging A said: $said"; return 1; }
  said="$(in_repo task merge "$b" 2>&1)" || { echo "merge_queue: B didn't go into the queue: $said"; return 1; }
  [[ "$said" == *"is 2nd in the merge queue, after \"A\""* ]] || { echo "merge_queue: merging B said: $said"; return 1; }
  check wait_for 20 eval '[[ "$(pane "$agent_a" state)" != running ]]' || { echo "merge_queue: A's agent didn't quit at merge"; kanna read "$agent_a"; return 1; }
  local listed; listed="$(in_repo task list)"
  grep -qE "^Merging$" <<<"$listed" && grep -qE " A +(queued|testing) " <<<"$listed" && grep -qE " B +queued +2nd" <<<"$listed" || { echo "merge_queue: the list doesn't show the queue:"; echo "$listed"; return 1; }
  ! sed -n '/^Shelf/,$p' <<<"$listed" | grep -qE " (A|B) " || { echo "merge_queue: queued tasks are still on the Shelf:"; echo "$listed"; return 1; }
  local testing; testing="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select json_extract(doc, '$.worker') from merge_queue where task = '$a'")"
  check wait_for 30 test -e "$testing/.kanna/tmp/testing" || { echo "merge_queue: A's tests didn't start"; tail -30 "$out/daemon.log"; return 1; }
  check test "$(git -C "$repo" log -1 --format=%s main)" == "main moves" || { echo "merge_queue: main moved before A's tests passed"; return 1; }
  check wait_for 10 eval '[[ "$(desk queueCount)" == 2 ]]' || { echo "merge_queue: the Merge queue tab doesn't count 2 in line: $(desk queueCount)"; return 1; }
  check test "$(desk scroll.groups)" == '["Desk", "Shelf"]' || { echo "merge_queue: the Desk tab's groups are $(desk scroll.groups)"; return 1; }
  check test "$(desk shelf)" == "[\"$c\", \"$d\", \"$e\", \"$f\"]" || { echo "merge_queue: the Desk tab's Shelf has $(desk shelf)"; return 1; }
  click '"page":"Merge queue"' || { echo "merge_queue: no Merge queue tab to click"; return 1; }
  check wait_for 10 eval '[[ "$(desk queuePage)" == True && "$(entries)" == "$a:testing $b:queued" ]]' || { echo "merge_queue: the Merge queue tab lists: $(entries)"; return 1; }
  local tester; tester="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from run_stages where run = (select run from merge_queue where task = '$a') order by id desc limit 1")"
  check wait_for 10 eval '[[ -n "$tester" && "$(desk queueTerminal)" == "$tester" ]]' || { echo "merge_queue: the tab doesn't show A's tester's terminal $tester: $(desk queueTerminal), $(desk queueWaiting)"; return 1; }
  click "\"queue\":\"$b\"" || { echo "merge_queue: no row for B to click"; return 1; }
  check wait_for 5 eval '[[ "$(desk queuePicked)" == "$b" && "$(desk queueTerminal)" == None && "$(desk queueWaiting)" == *"once “A” lands or fails"* ]]' || { echo "merge_queue: clicking B shows: $(desk queuePicked), $(desk queueTerminal), $(desk queueWaiting)"; return 1; }
  click "\"queue\":\"$a\"" || { echo "merge_queue: no row for A to click"; return 1; }
  check wait_for 5 eval '[[ "$(desk queueTerminal)" == "$tester" ]]' || { echo "merge_queue: clicking A didn't show its tester's terminal again: $(desk queueTerminal)"; return 1; }
  stop_daemon; start_daemon || { echo "merge_queue: the daemon didn't come back"; return 1; }
  check wait_for 60 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select group_concat(state) from (select state from merge_queue order by queued_at)")" == landed,landed ]]' || { echo "merge_queue: A and B didn't land: $(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select task, state, doc from merge_queue")"; tail -40 "$out/daemon.log"; return 1; }
  local subjects; subjects="$(git -C "$repo" log --first-parent --format=%s main | head -3 | tr '\n' '|')"
  [[ "$subjects" == "Merge branch '$b': B|Merge branch '$a': a: add a|main moves|" ]] || { echo "merge_queue: main's history is $subjects"; return 1; }
  check test -e "$repo/a.txt" -a -e "$repo/b.txt" || { echo "merge_queue: the main checkout didn't fast-forward"; return 1; }
  check wait_for 20 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select group_concat(place) from tasks where id in ('"'$a'"', '"'$b'"')")" == done,done ]]' || { echo "merge_queue: landed tasks aren't done"; return 1; }
  check test -z "$(git -C "$repo" branch --list "queue/*")" || { echo "merge_queue: queue branches are left: $(git -C "$repo" branch --list "queue/*")"; return 1; }
  local runs_of="select run from merge_queue where task in ('$a', '$b')"
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select group_concat(distinct kind) || ' ' || count(*) from run_stages where run in ($runs_of) and verdict != 'interrupted'")" == "run 2" || { echo "merge_queue: A and B, whose tests pass, didn't land on a command stage each with no agent"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select run, stage, kind, verdict from run_stages"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*) from run_stages where kind = 'run' and verdict = 'pass' and ended_at - started_at >= 14 and run = (select run from merge_queue where task = '$a')")" -eq 1 || { echo "merge_queue: A's tests, 15s long with tasks.timeout at 5s, didn't run to their end"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select run, stage, verdict, ended_at - started_at, doc from run_stages"; return 1; }
  touch "$repo/known-broken" && git -C "$repo" add -A && git -C "$repo" commit -qm "main breaks a test" || { echo "merge_queue line $LINENO"; return 1; }
  said="$(in_repo task merge "$c" 2>&1)" || { echo "merge_queue: C didn't go into the queue: $said"; return 1; }
  said="$(in_repo task merge "$f" 2>&1)" || { echo "merge_queue: F didn't go into the queue: $said"; return 1; }
  said="$(in_repo task merge "$e" 2>&1)" || { echo "merge_queue: E didn't go into the queue: $said"; return 1; }
  check wait_for 60 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select state from merge_queue where task = '"'$c'"'")" == failed ]]' || { echo "merge_queue: C's failing tests didn't fail it"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id, task, state, run, doc from merge_queue"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id, state, outcome, doc from runs"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select run, stage, verdict, terminal, doc from run_stages"; in_repo task list; ls "$KANNA_STATE_DIR"; return 1; }
  ! git -C "$repo" merge-base --is-ancestor "$(git -C "$(wt $c)" rev-parse HEAD)" main || { echo "merge_queue: the failed entry landed on main"; return 1; }
  listed="$(in_repo task list)"
  grep -qE " C +merge-failed +(claude +)?broken_thing fails: the branch broke it" <<<"$listed" || { echo "merge_queue: C isn't back on the Shelf as merge-failed with its tester's reason:"; echo "$listed"; return 1; }
  ! sed -n '/^Merging/,/^$/p' <<<"$listed" | grep -qE " C " || { echo "merge_queue: the failed entry is still under Merging:"; echo "$listed"; return 1; }
  local failure; failure="$(in_repo task list --json | python3 -c 'import json, sys; t = [t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0]; f = t["mergeFailed"]; print(f["verdict"], "|", f["note"].splitlines()[-1])' "$c")"
  [[ "$failure" == "fail | It passes on main and fails here, alone too." ]] || { echo "merge_queue: C's mergeFailed says verdict | note: $failure"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*) from messages where recipient = '$c' and sender like 'queue-%' and text like '%didn''t merge: broken_thing fails: the branch broke it.%tests'' log: %test.log.%judge says it''s the branch''s doing:%It passes on main and fails here, alone too.%kanna task merge again%'")" -eq 1 || { echo "merge_queue: C's agent wasn't sent the judge's reasons and the log"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select sender, text from messages where recipient = '$c'"; return 1; }
  check grep -q "test broken_thing ... FAILED" "$(wt $c)/.kanna/tmp/$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id from merge_queue where task = '$c'")/test.log" || { echo "merge_queue: the failed tests' log wasn't kept in C's worktree"; return 1; }
  check wait_for 10 eval '[[ "$(entries)" == *"$c:failed $b:landed $a:landed" ]]' || { echo "merge_queue: the tab's recent entries are: $(entries)"; return 1; }
  check test -s "$repo/.kanna-worktrees/merge-queue/.kanna/tmp/waits" || { echo "merge_queue: C's judge never had to wait again for a command it asked Kanna to run"; return 1; }
  check wait_for 60 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select json_extract(doc, '"'"'$.needs'"'"') from merge_queue where task = '"'$f'"'")" == "judge stopped without reporting" ]]' || { echo "merge_queue: F's judge, whose turn ended without a verdict even after it was carried on to report, didn't leave F waiting on you"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id, task, state, run, doc from merge_queue where task = '$f'"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select what, by, doc from run_events where run = (select run from merge_queue where task = '$f')"; tail -40 "$out/daemon.log"; return 1; }
  local frun; frun="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select run from merge_queue where task = '$f'")"
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*) from run_events where run = '$frun' and what = 'resumed' and by = 'daemon' and json_extract(doc, '$.reply') like '%sleep 4; echo the suite on main%It exited 0, and all its output is in %command-2-1.log%'")" -eq 1 || { echo "merge_queue: F's judge ended its turn while its command ran, and wasn't carried on with the command's exit"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select what, by, doc from run_events where run = '$frun'"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*) from run_events where run = '$frun' and what = 'resumed' and by = 'daemon' and json_extract(doc, '$.reply') like 'Your turn ended, but judge isn''t over: only %run done pass%'")" -eq 1 || { echo "merge_queue: F's judge, whose turn ended with no verdict, wasn't carried on once to report before F needed you"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select what, by, doc from run_events where run = '$frun'"; return 1; }
  check grep -q "the suite on main" "$repo/.kanna-worktrees/merge-queue/.kanna/tmp/command-2-1.log" || { echo "merge_queue: the command F's judge asked for didn't run to its end after the judge's turn ended"; return 1; }
  listed="$(in_repo task list)"
  grep -qE " F +needs you +1st · judge stopped without reporting · kanna run resume $frun" <<<"$listed" && grep -qE " E +queued " <<<"$listed" || { echo "merge_queue: the list doesn't show F waiting on you, with E behind it:"; echo "$listed"; return 1; }
  [[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select json_extract(doc, '$.told') from merge_queue where task = '$f'")" == *"judge's turn ended without it reporting it was done or what it needs, again after Kanna carried it on once to report"*"Anything it left running in its own shell ended with it."*"kanna run resume $frun"* ]] || { echo "merge_queue: F's entry doesn't say what happened to its judge"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select doc from merge_queue where task = '$f'"; return 1; }
  click '"page":"Merge queue"' && check wait_for 10 eval '[[ "$(desk queue | python3 -c "import json, sys; print([e[\"said\"].split(\" · \")[:2] for e in json.load(sys.stdin) if e[\"needs\"]])")" == "[['"'"'needs you'"'"', '"'"'judge stopped without reporting'"'"']]" ]]' || { echo "merge_queue: the tab doesn't show F as needing you: $(desk queue)"; return 1; }
  click "\"queue\":\"$f\"" && check wait_for 5 eval '[[ "$(desk queueTerminal)" == None && "$(desk queueLog)" == *"Its screen ended:"*"Resume carries it on"* ]]' || { echo "merge_queue: picking the paused F shows: $(desk queueTerminal), $(desk queueLog)"; return 1; }
  check wait_for 20 eval 'said="$(in_repo run resume "$frun" "mock: run sh judge.sh" 2>&1)"' || { echo "merge_queue: F's run didn't resume: $said"; return 1; }
  check wait_for 60 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select state || '"' '"' || json_extract(doc, '"'"'$.result.note'"'"') || '"' '"' || ifnull(json_extract(doc, '"'"'$.needs'"'"'), '"'-'"') from merge_queue where task = '"'$f'"'")" == "landed tests fail, none of them the branch'"'"'s: known_broken fails on main too -" ]]' || { echo "merge_queue: F, resumed, didn't land on its judge's pass"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id, task, state, run, doc from merge_queue where task = '$f'"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select what, by, doc from run_events where run = '$frun'"; tail -40 "$out/daemon.log"; return 1; }
  click '"page":"Desk"' && check wait_for 5 eval '[[ "$(desk page)" == True && "$(desk queuePage)" == False ]]' || { echo "merge_queue: the Desk tab didn't come back"; return 1; }
  check wait_for 10 click "\"shelfLog\":\"$c\"" || { echo "merge_queue: C's Shelf row has no Log"; desk shelf; return 1; }
  check wait_for 10 eval '[[ "$(desk queuePage)" == True && "$(desk queuePicked)" == "$c" && "$(desk queueLog)" == "broken_thing fails: the branch broke it"*"test broken_thing ... FAILED"* ]]' || { echo "merge_queue: C's Log shows: $(desk queuePage), $(desk queuePicked), $(desk queueLog)"; return 1; }
  check wait_for 60 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select state from merge_queue where task = '"'$e'"'")" == landed ]]' || { echo "merge_queue: E, queued behind the failed C, didn't land"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id, task, state, run, doc from merge_queue"; tail -30 "$out/daemon.log"; return 1; }
  local note; note="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select json_extract(doc, '$.result.verdict') || ' | ' || json_extract(doc, '$.result.note') from merge_queue where task = '$e'")"
  [[ "$note" == "pass | tests fail, none of them the branch's: known_broken fails on main too, flaky_thing passes alone" ]] || { echo "merge_queue: E landed with verdict | note: $note"; return 1; }
  check test "$(git -C "$repo" log -1 --first-parent --format=%s main)" == "Merge branch '$e': E" || { echo "merge_queue: E isn't main's head"; return 1; }
  check wait_for 10 eval '[[ "$(entries)" == "$e:landed $f:landed $c:failed $b:landed $a:landed" && "$(desk queueCount)" == None ]]' || { echo "merge_queue: after E landed the tab lists: $(entries), counting $(desk queueCount)"; return 1; }
  click "\"queue\":\"$e\"" || { echo "merge_queue: no row for E to click"; return 1; }
  check wait_for 10 eval '[[ "$(desk queueLog)" == "tests fail, none of them the branch'"'"'s:"*"test known_broken ... FAILED"* ]]' || { echo "merge_queue: the landed E's log shows: $(desk queueLog)"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select outcome from runs where id = (select run from merge_queue where task = '$c')")" == failed || { echo "merge_queue: C's failed run isn't recorded as failed"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id, state, outcome, doc from runs"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select group_concat(stage || ':' || kind || ':' || verdict, ' ') from (select * from run_stages where run in (select run from merge_queue where task in ('$c', '$e')) order by id)")" == "test:run:fail judge:agent:fail test:run:fail judge:agent:pass" || { echo "merge_queue: C and E, whose tests fail, didn't each go from the tests to the judge"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select run, stage, kind, verdict from run_stages"; return 1; }
  local builds; builds="$(cat "$repo/.kanna-worktrees/merge-queue/target/builds")"
  [[ "$(grep -c '^cold' <<<"$builds")" == 1 && "$(grep -c '^warm' <<<"$builds")" -ge 3 ]] || { echo "merge_queue: the queue's worktree didn't keep its build between entries:"; echo "$builds"; return 1; }
  machine_runners 3
  check wait_for 30 eval 'shows "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from tasks where id = '"'$c'"'")" "message from queue-"' || { echo "merge_queue: C's agent didn't get the queue's message"; local ct; ct="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from tasks where id = '$c'")"; echo "terminal $ct"; kanna read "$ct"; in_repo task list; in_repo task messages "$c"; cat "$KANNA_AGENTS_HOME/.kanna-v4/config.json"; return 1; }
}

scenario task_scripts M4
task_scripts() {
  start_daemon && start_app || { echo "task_scripts line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/agent"
  mkdir -p "$repo/.kanna" && git -C "$repo" init -q -b main || { echo "task_scripts line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  printf 'target\nmade-by-setup\n' >"$repo/.gitignore"
  cat >"$repo/.kanna/setup.sh" <<SETUP
#!/bin/sh
echo "\$KANNA_TASK" >>"$KANNA_STATE_DIR/setups"
env | grep '^KANNA_' | grep -v '^KANNA_TERMINAL\|^KANNA_SOCKET\|^KANNA_STATE' | sort >made-by-setup
[ "\$KANNA_TASK_NAME" = Fails ] && { echo broken; exit 3; }
echo set up
SETUP
  cat >"$repo/.kanna/teardown.sh" <<TEARDOWN
#!/bin/sh
env | grep '^KANNA_TASK=' >"$KANNA_STATE_DIR/torn"
echo tearing down here
[ -e "$KANNA_STATE_DIR/teardown-fails" ] && { echo broken; exit 2; }
exit 0
TEARDOWN
  chmod +x "$repo/.kanna/setup.sh" "$repo/.kanna/teardown.sh"
  printf '{"version": 1, "tasks": {"setup": ".kanna/setup.sh", "teardown": ".kanna/teardown.sh", "timeout": 30}}\n' >"$repo/.kanna/config.json"
  git -C "$repo" add -A && git -C "$repo" -c user.email=e@e -c user.name=e commit -q -m first || { echo "task_scripts line $LINENO"; return 1; }
  printf '#!/bin/sh\necho "agent sees: $(tr "\\n" " " <made-by-setup 2>/dev/null)"\nexec sleep 600\n' >"$agent" && chmod +x "$agent"
  in_repo() { (cd "$repo" && kanna "$@"); }
  in_repo task new "Sets up" "go" --agent "$agent" >/dev/null && in_repo task new "Fails" "go" --agent "$agent" >/dev/null && in_repo task new "Kept" "go" --agent "$agent" >/dev/null || { echo "task_scripts line $LINENO"; return 1; }
  local started wt p1; started="$(kanna task pick $(tid 1))" || { echo "task_scripts line $LINENO"; return 1; }
  [[ "$started" =~ ^"moved $(sid 1) to the Desk: term-"[0-9]+" in "([^ ]+)" (" ]] || { echo "task_scripts line $LINENO: $started"; return 1; }
  p1="Sets up" wt="${BASH_REMATCH[1]}"
  check wait_for 15 shows "$p1" "agent sees:" || { echo "task_scripts: the agent didn't start after setup"; kanna read "$p1"; return 1; }
  shows "$p1" "set up" || { echo "task_scripts: setup's output isn't in the agent's terminal"; kanna read "$p1"; return 1; }
  local base; base="$(git -C "$repo" rev-parse HEAD)"
  for want in "KANNA_TASK=$(sid 1)" "KANNA_TASK_NAME=Sets up" "KANNA_REPO=$repo" "KANNA_WORKTREE=$wt" "KANNA_BRANCH=${wt##*/}" "KANNA_BASE=$base"; do
    unwrapped "$p1" "$want" || { echo "task_scripts: the agent's first command didn't see setup's $want"; kanna read "$p1"; return 1; }
  done
  check test "$(cat "$(git -C "$wt" rev-parse --absolute-git-dir)/kanna-setup")" == 0 || { echo "task_scripts line $LINENO"; return 1; }
  kanna task stop "$p1" >/dev/null && kanna task start "$p1" >/dev/null || { echo "task_scripts line $LINENO"; return 1; }
  check wait_for 10 is "$p1" state running && sleep 1 || { echo "task_scripts line $LINENO"; return 1; }
  check test "$(cat "$KANNA_STATE_DIR/setups" | xargs)" == "$(tid 1)" || { echo "task_scripts: setup ran again on a restart: $(cat "$KANNA_STATE_DIR/setups" | xargs)"; return 1; }
  local tab; tab="$(kanna task setup "$p1")" || { echo "task_scripts line $LINENO"; return 1; }
  [[ "$tab" =~ " runs again in "(term-[0-9]+)" (a Setup tab)"$ ]] || { echo "task_scripts line $LINENO: $tab"; return 1; }
  tab="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(cat "$KANNA_STATE_DIR/setups" | xargs)" == "$(tid 1) $(tid 1)" ]]' || { echo "task_scripts: the Setup tab didn't run setup"; return 1; }
  local elsewhere="$KANNA_STATE_DIR/volume/builds"
  mkdir -p "$elsewhere" && echo built >"$elsewhere/kanna" && ln -s "$elsewhere" "$wt/target" || { echo "task_scripts line $LINENO"; return 1; }
  touch "$KANNA_STATE_DIR/teardown-fails"
  [[ "$(kanna task done "$p1")" == "$(sid 1)'s teardown runs in term-"*": "* ]] || { echo "task_scripts: done didn't say teardown runs in the agent's pane"; return 1; }
  check wait_for 10 shows "$p1" "teardown failed (exit 2)" || { echo "task_scripts: the agent's pane doesn't show the failed teardown"; kanna read "$p1"; return 1; }
  shows "$p1" "tearing down here" || { echo "task_scripts: teardown's output isn't in the agent's pane"; kanna read "$p1"; return 1; }
  kanna read "$tab" >/dev/null 2>&1 && { echo "task_scripts: the task's tabs weren't closed for teardown"; return 1; }
  check test -d "$wt" || { echo "task_scripts: a failed teardown removed the worktree"; return 1; }
  check wait_for 5 eval 'in_repo task list | placed | grep -qE "^$(sid 1) +Sets up +Desk +teardown failed \(exit 2\)"' || { echo "task_scripts: task list doesn't say teardown failed"; in_repo task list; return 1; }
  rm -f "$KANNA_STATE_DIR/teardown-fails" "$KANNA_STATE_DIR/torn"
  kanna task done "$p1" >/dev/null || { echo "task_scripts line $LINENO"; return 1; }
  check wait_for 10 eval '[[ ! -e "$wt" ]]' || { echo "task_scripts: the merged task's worktree wasn't removed"; kanna read "$p1"; raw '{"id":1,"cmd":"tasks","args":{"action":"events","task":"'"$(tid 1)"'"}}'; return 1; }
  check test "$(cat "$KANNA_STATE_DIR/torn")" == "KANNA_TASK=$(tid 1)" || { echo "task_scripts: teardown didn't run again on Done"; return 1; }
  kanna read "$p1" >/dev/null 2>&1 && { echo "task_scripts: the agent's pane stayed after teardown succeeded"; return 1; }
  check test "$(cat "$elsewhere/kanna")" == built || { echo "task_scripts: cleanup removed what target/ links to"; return 1; }
  git -C "$repo" rev-parse --verify --quiet "refs/heads/${wt##*/}" >/dev/null && { echo "task_scripts: the merged branch is left"; return 1; }
  raw '{"id":1,"cmd":"tasks","args":{"action":"events","task":"'"$(tid 1)"'"}}' | grep -q '"cleaned up"' || { echo "task_scripts: cleanup isn't in the first task's events"; return 1; }
  rm -f "$KANNA_STATE_DIR/torn"
  started="$(kanna task pick $(tid 2))" || { echo "task_scripts line $LINENO"; return 1; }
  [[ "$started" =~ ^"moved $(sid 2) to the Desk: term-"[0-9]+" in "([^ ]+)" (" ]] || { echo "task_scripts line $LINENO: $started"; return 1; }
  local p2="Fails" wt2="${BASH_REMATCH[1]}"
  check wait_for 15 is "$p2" state exited || { echo "task_scripts: a failed setup didn't stop the task"; kanna read "$p2"; return 1; }
  shows "$p2" "setup failed (exit 3)" || { echo "task_scripts: the terminal doesn't say setup failed"; kanna read "$p2"; return 1; }
  shows "$p2" "agent sees:" && { echo "task_scripts: the agent started after a failed setup"; return 1; }
  check wait_for 5 eval 'in_repo task list | placed | grep -qE "^$(sid 2) +Fails +Desk +setup failed \(exit 3\)"' || { echo "task_scripts: task list doesn't say setup failed"; in_repo task list; return 1; }
  raw '{"id":1,"cmd":"tasks","args":{"action":"events","task":"'"$(tid 2)"'"}}' | grep -q '"setup failed"' || { echo "task_scripts: the failure isn't in t2's events"; return 1; }
  kanna task start "$p2" >/dev/null || { echo "task_scripts line $LINENO"; return 1; }
  check wait_for 10 shows "$p2" "agent sees:" || { echo "task_scripts: start didn't start the agent anyway"; kanna read "$p2"; return 1; }
  in_repo task list | placed | grep -qE "^$(sid 2) +Fails +Desk +setup failed" && { echo "task_scripts: a task started anyway still says setup failed"; return 1; }
  [[ "$(kanna task setup "$p2")" == "$(sid 2)'s setup runs again in "*"(a Setup tab)" ]] || { echo "task_scripts: task setup didn't open a Setup tab"; return 1; }
  check wait_for 10 eval '[[ "$(cat "$KANNA_STATE_DIR/setups" | xargs)" == "$(tid 1) $(tid 1) $(tid 2) $(tid 2)" ]]' || { echo "task_scripts: task setup didn't run it again: $(cat "$KANNA_STATE_DIR/setups" | xargs)"; return 1; }
  echo work >"$wt2/work.txt" && git -C "$wt2" add work.txt && git -C "$wt2" -c user.email=e@e -c user.name=e commit -q -m work || { echo "task_scripts line $LINENO"; return 1; }
  kanna task done "$p2" >/dev/null || { echo "task_scripts line $LINENO"; return 1; }
  check wait_for 10 eval 'raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"events\",\"task\":\"$(tid 2)\"}}" | grep -q "isn.t merged into main"' || { echo "task_scripts: an unmerged task wasn't kept, saying why"; raw '{"id":1,"cmd":"tasks","args":{"action":"events","task":"'"$(tid 2)"'"}}'; return 1; }
  check test "$(cat "$KANNA_STATE_DIR/torn")" == "KANNA_TASK=$(tid 2)" || { echo "task_scripts: teardown didn't run on t2's Done"; return 1; }
  check test -f "$wt2/work.txt" && git -C "$repo" rev-parse --verify --quiet "refs/heads/${wt2##*/}" >/dev/null || { echo "task_scripts: the unmerged worktree or branch went"; return 1; }
  rm -f "$KANNA_STATE_DIR/torn"
  started="$(kanna task pick $(tid 3))" || { echo "task_scripts line $LINENO"; return 1; }
  [[ "$started" =~ ^"moved $(sid 3) to the Desk: term-"[0-9]+" in "([^ ]+)" (" ]] || { echo "task_scripts line $LINENO: $started"; return 1; }
  local p3="Kept" wt3="${BASH_REMATCH[1]}"
  check wait_for 15 shows "$p3" "agent sees:" || { echo "task_scripts line $LINENO"; kanna read "$p3"; return 1; }
  [[ "$(kanna task done "$p3" --keep)" == "$(sid 3) is done: closed "* ]] || { echo "task_scripts: done --keep didn't end the task"; return 1; }
  check wait_for 10 eval 'raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"events\",\"task\":\"$(tid 3)\"}}" | grep -q "kanna task done --keep"' || { echo "task_scripts: done --keep isn't in the task's events"; return 1; }
  check test ! -e "$KANNA_STATE_DIR/torn" && check test -d "$wt3" || { echo "task_scripts: done --keep ran teardown or removed the worktree"; return 1; }
}

scenario shelf_rename M4
shelf_rename() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo/.kanna" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_rename line $LINENO"; return 1; }
  printf '{"version": 1, "shelf": {"runners": 0}}\n' >"$repo/.kanna/config.json"
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "shelf_rename line $LINENO"; return 1; }
  in_repo() { (cd "$repo" && kanna "$@"); }
  named() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys; t = json.load(sys.stdin)["shelf"][0]; print(t["name"], "|", t["message"])'; }
  in_repo task new "Old name" "work 1" --agent "$agent" >/dev/null || { echo "shelf_rename line $LINENO"; return 1; }
  check test "$(in_repo task name $(tid 1) "New name")" == 'named '"$(sid 1)"' "New name"' || { echo "shelf_rename: task name on a Shelf task"; return 1; }
  check test "$(named)" == "New name | work 1" || { echo "shelf_rename: the entry kept its old name, or its first message changed"; named; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  check wait_for 10 eval '[[ "$(desk shelf)" == "[\"$(tid 1)\"]" ]]' || { desk shelf; return 1; }
  in_repo task pick $(tid 1) >/dev/null || { echo "shelf_rename line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk tasks.0.title)" == "New name" ]]' || { echo "shelf_rename: the Shelf doesn't show the new name"; desk tasks; return 1; }
  kanna workspace list --all --json | python3 -c 'import json, sys; sys.exit(not any(p.get("name") == "New name" for p in json.load(sys.stdin)["panes"]))' || { echo "shelf_rename: the started task's pane isn't named from its entry"; kanna workspace list --all; return 1; }
}
