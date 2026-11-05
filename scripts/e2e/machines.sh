machine_vm() {
  export KANNA_MACHINE_MDNS=0 KANNA_MACHINE_PORT=0
  start_daemon || { echo "$1 line $LINENO"; return 1; }
  vm="$KANNA_STATE_DIR/machine-vm"
  mkdir -p "$vm/state" && chmod 700 "$vm"
  vmk() { KANNA_SOCKET="$vm/kanna.sock" KANNA_STATE_DIR="$vm/state" "$bin/kanna" "$@"; }
  vraw() { KANNA_SOCKET="$vm/kanna.sock" raw "$@"; }
  KANNA_SOCKET="$vm/kanna.sock" KANNA_STATE_DIR="$vm/state" KANNA_KEEPER="$bin/kanna-keeper" "$bin/kanna-daemon" >>"$out/daemon-vm.log" 2>&1 &
  vm_pid=$!
  wait_for 5 test -S "$vm/kanna.sock" || { echo "$1: the second daemon didn't open its socket"; return 1; }
  kanna machine name mbp >/dev/null && vmk machine name vm >/dev/null || { echo "$1 line $LINENO"; return 1; }
  python3 - "$KANNA_SOCKET" "$(sed -n 's/^pub const PROTOCOL: u32 = \([0-9]*\);/\1/p' "$repo_root/crates/kanna-build/src/lib.rs")" >"$KANNA_STATE_DIR/confirmed" <<'PY' &
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f = s.makefile("rw")
f.write(json.dumps({"id": 0, "cmd": "app.hello", "args": {"pid": 1, "protocol": int(sys.argv[2])}}) + "\n"); f.flush()
for line in f:
    m = json.loads(line)
    if m.get("cmd") == "app.machine.confirm":
        f.write(json.dumps({"id": m["id"], "ok": True, "confirmed": True}) + "\n"); f.flush()
        print(m["args"]["name"], flush=True)
        break
PY
  local confirmer=$!
  vmk machine pair --json >"$KANNA_STATE_DIR/offer" &
  local waiting=$!
  check wait_for 5 test -s "$KANNA_STATE_DIR/offer" || { echo "$1: no pairing code"; return 1; }
  local code peer
  code="$(head -1 "$KANNA_STATE_DIR/offer" | field code)"
  peer="$(head -1 "$KANNA_STATE_DIR/offer" | python3 -c 'import json, sys; o = json.load(sys.stdin); print("%s@127.0.0.1:%s" % (o["key"], o["port"]))')"
  expected="added vm to your machines"; says kanna machine pair "$code" --peer "$peer" || return 1
  wait "$waiting" "$confirmer"
  check test "$(cat "$KANNA_STATE_DIR/confirmed")" == vm || { echo "$1: the pairing wasn't confirmed in an app"; return 1; }
  start_app || { echo "$1 line $LINENO"; return 1; }
  machines() { raw '{"id":1,"cmd":"debug.machines"}' | field "$1"; }
  check wait_for 20 eval '[[ "$(machines links.0.attached)" == True ]]' || { echo "$1: the app didn't link to vm"; raw '{"id":1,"cmd":"debug.machines"}'; return 1; }
  check test "$(machines links.0.machine) $(machines links.0.daemon.machine) $(machines links.0.path.path)" == "vm vm direct" || { raw '{"id":1,"cmd":"debug.machines"}'; return 1; }
}

scenario machine_remote_task M4
machine_remote_task() {
  local vm vm_pid
  machine_vm machine_remote_task || return 1
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/team/notes"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first && git -C "$repo" remote add origin https://example.com/team/notes.git || { echo "machine_remote_task line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  local opened; opened="$(cd / && vmk "$repo")" && [[ "$opened" == "opened d1 on vm:$repo" ]] || { echo "machine_remote_task: a Desk on vm's project: $opened"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"${2:-d1}\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-*@vm ]]' || { echo "machine_remote_task: the Desk's planner isn't on vm"; desk lead; vmk workspace list --all; return 1; }
  local planner; planner="$(desk lead)"
  check test "$(vmk workspace list --all --json | python3 -c 'import json, sys; print(sum(1 for w in json.load(sys.stdin)["workspaces"] if w.get("where") == {"planner": "mbp"}))')" == 1 || { echo "machine_remote_task: vm's planner isn't claimed by mbp"; vmk workspace list --all --json; return 1; }
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys; print(len(json.load(sys.stdin)["workspaces"]))')" == 0 || { echo "machine_remote_task: the planner ran on this Mac"; return 1; }
  check wait_for 10 eval '[[ "$(desk header)" == "Planning · $repo · main" ]]' || { echo "machine_remote_task: header"; desk header; return 1; }

  (cd "$repo" && KANNA_TERMINAL="${planner%@vm}" vmk task new "Remote work" "Change the notes") >/dev/null || { echo "machine_remote_task line $LINENO"; return 1; }
  local task; task="$(sqlite3 -readonly "$vm/state/kanna.db" "select id from tasks order by rowid limit 1")@vm"
  check wait_for 10 eval '[[ "$(desk shelf)" == "[\"$task\"]" ]]' || { echo "machine_remote_task: the Desk view's Shelf doesn't list vm's task"; desk shelf; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$task"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk tasks.0.title)" == "Remote work" ]]' || { echo "machine_remote_task: starting vm's task didn't put it on the Shelf"; desk tasks; desk note; return 1; }
  local agent; agent="$(desk tasks.0.task)"
  [[ "$agent" == term-*@vm ]] || { echo "machine_remote_task: the task's agent isn't vm's: $agent"; return 1; }
  check test "$(desk places.0)" == "vm · Direct · LAN" || { echo "machine_remote_task: the Shelf's tooltips"; desk places; return 1; }
  check test "$(vmk task list --all --json | python3 -c 'import json, sys; print([t.get("where") for t in json.load(sys.stdin)["tasks"]])')" == "[{'desk': 'mbp'}]" || { vmk task list --json; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"task":"'"$agent"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$agent" ]]' || { echo "machine_remote_task: pulling vm's task didn't dock its agent"; raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  check wait_for 10 eval 'vmk read "${agent%@vm}" | grep -q mock-agent' || { vmk read "${agent%@vm}"; return 1; }

  vraw '{"id":1,"cmd":"debug.type","args":{"pane":"'"${agent%@vm}"'","text":"typed from the mac\r"}}' >/dev/null
  check wait_for 5 eval 'vmk read "${agent%@vm}" | grep -q "typed from the mac"' || { echo "machine_remote_task: typing into vm's agent didn't reach it"; vmk read "${agent%@vm}"; return 1; }
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.text\",\"args\":{\"pane\":\"$agent\"}}" | field text | grep -q "typed from the mac"' || { echo "machine_remote_task: the app's view of vm's agent didn't show its output"; raw "{\"id\":1,\"cmd\":\"debug.text\",\"args\":{\"pane\":\"$agent\"}}" | cut -c1-600; return 1; }

  local worktree="$repo/.kanna-worktrees/${task%@vm}"
  echo "remote change" >"$worktree/notes.txt"
  local diffopen; diffopen="$(cd "$worktree" && KANNA_TERMINAL="${agent%@vm}" vmk open diff --tab --name remote-diff --json)" || { echo "machine_remote_task line $LINENO"; return 1; }
  local diff; diff="$(echo "$diffopen" | field pane.id)@vm"
  diffpane() { raw "{\"id\":1,\"cmd\":\"debug.diff\",\"args\":{\"pane\":\"$diff\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(diffpane files)" == *notes.txt* ]]' || { echo "machine_remote_task: the app doesn't show vm's diff"; raw "{\"id\":1,\"cmd\":\"debug.diff\",\"args\":{\"pane\":\"$diff\"}}"; return 1; }
  check test "$(desk window.dock.pane)" == "$agent" || { echo "machine_remote_task: the diff moved the agent"; return 1; }
  local difftab; difftab="$(echo "$diffopen" | field tab)"
  raw '{"id":1,"cmd":"debug.rename","args":{"tab":"'"$difftab"'@vm","title":"term-1@vm"}}' | grep -q '"ok":true' || { echo "machine_remote_task line $LINENO"; return 1; }
  check wait_for 5 eval 'vmk workspace list --all --json | grep -q "\"term-1@vm\""' || { echo "machine_remote_task: a remote tab's title that looks like an ID lost its suffix through the app"; vmk workspace list --all --json; return 1; }
  sleep 0.5; snap d1 machine_remote_task >/dev/null

  check test "$(desk pages)" == '[]' || { echo "machine_remote_task: a task's Desk shows the Shelf or Machines tab"; desk pages; return 1; }
  check test "$(desk headerTip)" == "vm · Direct · LAN" || { echo "machine_remote_task: the agent header's tooltip"; desk headerTip; return 1; }
  check test "$(desk header)" == "$repo/.kanna-worktrees/${task%@vm} · ${task%@vm}" || { echo "machine_remote_task: the agent header"; desk header; return 1; }
  local first last; first="$(desk window.tabs.0.id)"; last="$(desk window.tabs | python3 -c 'import json, sys; print(json.load(sys.stdin)[-1]["id"])')"
  raw '{"id":1,"cmd":"debug.click","args":{"tab":"'"$first"'"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.tab)" == "$first" ]]' || { echo "machine_remote_task line $LINENO"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Previous Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.tab)" == "$last" ]]' || { echo "machine_remote_task: Previous Tab from a task's first tab didn't wrap to its last"; desk window.tab; return 1; }
  check test "$(desk page) $(desk machinesPage)" == "False False" || { echo "machine_remote_task: Previous Tab on a task's Desk showed a fixed page"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Next Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.tab)" == "$first" ]]' || { echo "machine_remote_task: Next Tab from a task's last tab didn't wrap to its first"; desk window.tab; return 1; }
  check test "$(desk page) $(desk machinesPage)" == "False False" || { echo "machine_remote_task: Next Tab on a task's Desk showed a fixed page"; return 1; }

  raw '{"id":1,"cmd":"debug.click","args":{"deskView":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk empty)" == True && "$(desk page)" == True ]]' || { echo "machine_remote_task: parking didn't show the Desk view"; desk empty; desk page; return 1; }
  check test "$(desk pages)" == '["Desk", "Machines", "Stats", "Schedules"]' || { echo "machine_remote_task: the empty Desk's fixed tabs"; desk pages; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"page":"Machines","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk machinesPage)" == True && "$(desk page)" == False ]]' || { echo "machine_remote_task: the Machines tab didn't show the Machines page"; return 1; }
  check python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
assert [(r["name"], r["path"]) for r in rows] == [("mbp", "This Mac"), ("vm", "Direct · LAN")], rows
assert rows[0]["detail"].startswith("This Mac · macOS") and rows[1]["detail"].startswith("macOS") and "cores" not in rows[1]["detail"], rows
assert "local network" in rows[1]["tip"], rows' "$(desk machines)" || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"page":"Desk","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk page)" == True && "$(desk machinesPage)" == False ]]' || { echo "machine_remote_task: the Shelf tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Next Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk machinesPage)" == True && "$(desk page)" == False ]]' || { echo "machine_remote_task: Next Tab from the Shelf tab didn't show the Machines tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Next Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk statsPage)" == True && "$(desk machinesPage)" == False ]]' || { echo "machine_remote_task: Next Tab from the Machines tab didn't show the Stats tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Next Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk schedulesPage)" == True && "$(desk statsPage)" == False ]]' || { echo "machine_remote_task: Next Tab from the Stats tab didn't show the Schedules tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Next Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk page)" == True && "$(desk schedulesPage)" == False ]]' || { echo "machine_remote_task: Next Tab from the Schedules tab didn't wrap to the Shelf tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Previous Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk schedulesPage)" == True && "$(desk page)" == False ]]' || { echo "machine_remote_task: Previous Tab from the Shelf tab didn't show the Schedules tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Previous Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk statsPage)" == True && "$(desk schedulesPage)" == False ]]' || { echo "machine_remote_task: Previous Tab from the Schedules tab didn't show the Stats tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Previous Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk machinesPage)" == True && "$(desk statsPage)" == False ]]' || { echo "machine_remote_task: Previous Tab from the Stats tab didn't show the Machines tab"; return 1; }
  sleep 0.5; snap d1 machine_remote_machines >/dev/null

  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Show Active Task"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$agent" ]]' || { echo "machine_remote_task: Show Active Task from the Machines page didn't show vm's task"; return 1; }
  check wait_for 5 eval '[[ "$(desk pages)" == "[]" && "$(desk machinesPage)" == False && "$(desk page)" == False ]]' || { echo "machine_remote_task: pulling a task from the Machines page left a fixed tab or page"; desk pages; return 1; }

  kill "$vm_pid"; wait_for 10 eval '! kill -0 '"$vm_pid" || { echo "machine_remote_task: vm's daemon didn't stop"; return 1; }
  check wait_for 10 eval '[[ "$(machines links.0.attached)" == False ]]' || { echo "machine_remote_task: the app didn't see vm go"; return 1; }
  check wait_for 5 eval '[[ "$(desk headerTip)" == "vm · Offline" && "$(desk header)" == "Last received"*"input is unavailable"* ]]' || { echo "machine_remote_task: an offline task's header"; desk header; desk headerTip; return 1; }
  check test "$(desk window.dock.pane)" == "$agent" || { echo "machine_remote_task: going offline moved the task"; return 1; }
  raw '{"id":1,"cmd":"debug.type","args":{"pane":"'"$agent"'","text":"lost while offline\r"}}' >/dev/null
  stop_app; start_app || { echo "machine_remote_task: the app didn't start again with vm's Desk saved and vm offline"; return 1; }
  sleep 1; check app_attached || { echo "machine_remote_task: the app didn't survive restoring vm's Desk while vm is offline"; tail -5 "$out/app.log"; return 1; }
  KANNA_MACHINE_PORT="$(head -1 "$KANNA_STATE_DIR/offer" | field port)" KANNA_SOCKET="$vm/kanna.sock" KANNA_STATE_DIR="$vm/state" KANNA_KEEPER="$bin/kanna-keeper" "$bin/kanna-daemon" >>"$out/daemon-vm.log" 2>&1 &
  vm_pid=$!
  check wait_for 30 eval '[[ "$(machines links.0.attached)" == True ]]' || { echo "machine_remote_task: the app didn't link to vm again"; raw '{"id":1,"cmd":"debug.machines"}'; return 1; }
  check wait_for 5 eval '[[ "$(desk headerTip)" == "vm · Direct · LAN" && "$(desk header)" != "Last received"* ]]' || { echo "machine_remote_task: the task didn't come back online"; desk header; return 1; }
  check test "$(desk window.dock.pane) $(desk tasks)" == "$agent []" || { echo "machine_remote_task: reconnecting changed the Desk"; desk tasks; return 1; }
  ! vmk read "${agent%@vm}" | grep -q "lost while offline" || { echo "machine_remote_task: input typed while offline was queued"; return 1; }
  vraw '{"id":1,"cmd":"debug.type","args":{"pane":"'"${agent%@vm}"'","text":"back online\r"}}' >/dev/null
  check wait_for 5 eval 'vmk read "${agent%@vm}" | grep -q "back online"' || { echo "machine_remote_task: input after reconnecting"; return 1; }

  local clone="$KANNA_STATE_DIR/clone"
  git clone -q "$repo" "$clone" && git -C "$clone" remote set-url origin git@example.com:team/notes.git || { echo "machine_remote_task line $LINENO"; return 1; }
  clone="$(cd "$clone" && pwd -P)"
  opened="$(cd / && kanna "$clone" 2>&1)"
  [[ "$opened" == *"kanna project import $clone"* ]] || { echo "machine_remote_task: a clone of vm's project outside repos on this Mac: $opened"; return 1; }

  kanna task list --all >/dev/null || return 1
  check test "$(kanna task list --all --json | python3 -c 'import json, sys; print(len(json.load(sys.stdin)["tasks"]))')" == 0 || { echo "machine_remote_task: vm's task leaked into this Mac's Shelf"; return 1; }
}

scenario project_import M4
project_import() {
  start_daemon && start_app || { echo "project_import line $LINENO"; return 1; }
  local outside="$KANNA_STATE_DIR/work/notes" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/app" agent="$KANNA_STATE_DIR/bin/claude"
  mkdir -p "$outside" "$repo" "${agent%/*}" && ln -s "$mock_agent" "$agent" || { echo "project_import line $LINENO"; return 1; }
  for dir in "$outside" "$repo"; do git -C "$dir" init -q -b main && git -C "$dir" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "project_import line $LINENO"; return 1; }; done
  outside="$(cd "$outside" && pwd -P)"; repo="$(cd "$repo" && pwd -P)"
  local refused; refused="$(cd "$outside" && kanna task new "Outside" "hi" 2>&1)" && { echo "project_import: a task was made outside repos"; return 1; }
  [[ "$refused" == *"isn't in "*"import a copy there with \`kanna project import $outside\` (to "*"/.kanna-v4/repos/local/notes)"* ]] || { echo "project_import: task new outside repos: $refused"; return 1; }
  refused="$(cd / && kanna "$outside" 2>&1)" && { echo "project_import: a Desk opened outside repos"; return 1; }
  [[ "$refused" == *"kanna project import $outside"* ]] || { echo "project_import: kanna PATH outside repos: $refused"; return 1; }

  (cd "$repo" && kanna task new "Importing" "say importing" --agent "$agent") >/dev/null || { echo "project_import line $LINENO"; return 1; }
  [[ "$(kanna task start "$(tid 1)")" == "started $(sid 1) on the Shelf: term-"* ]] || { echo "project_import: the task didn't start"; return 1; }
  check wait_for 5 shows "Importing" "say importing" || { echo "project_import: the agent didn't get its first message"; kanna read "Importing"; return 1; }
  local task worktree session; task="$(pane "Importing" id)"; worktree="$repo/.kanna-worktrees/$(tid 1)"
  session="$(kanna read "Importing" | tr -d '\r\n' | sed -n 's/.*--session-id \([0-9a-f-]\{36\}\).*/\1/p')"
  [[ "$session" =~ ^[0-9a-f-]{36}$ ]] || { echo "project_import: the task started without a session"; kanna read "Importing"; return 1; }
  local claude="$KANNA_AGENTS_HOME/.claude/projects" encoded; encoded() { tr -c 'A-Za-z0-9\n' - <<<"$1"; }
  mkdir -p "$claude/$(encoded "$worktree")" "$claude/$(encoded "$repo")/memory" || { echo "project_import line $LINENO"; return 1; }
  echo '{"said":"importing"}' >"$claude/$(encoded "$worktree")/$session.jsonl" && echo remember >"$claude/$(encoded "$repo")/memory/MEMORY.md" || { echo "project_import line $LINENO"; return 1; }
  KANNA_TERMINAL="$task" kanna open --name side --cmd 'pwd -P; exec cat' >/dev/null || { echo "project_import line $LINENO"; return 1; }
  kanna open --window new --name plain --cwd "$repo" --cmd 'pwd -P; exec cat' >/dev/null || { echo "project_import line $LINENO"; return 1; }
  check wait_for 5 unwrapped side "$worktree" && check wait_for 5 unwrapped plain "$repo" || { kanna read side; kanna read plain; return 1; }

  local g=(git -c user.email=e@e -c user.name=e -C "$repo")
  echo one >"$repo/kept.txt" && "${g[@]}" add kept.txt && "${g[@]}" commit -q -m kept || { echo "project_import line $LINENO"; return 1; }
  echo stashed >"$repo/kept.txt" && "${g[@]}" stash -q && echo uncommitted >"$repo/kept.txt" && echo untracked >"$repo/new.txt" && echo "task work" >"$worktree/wip.txt" || { echo "project_import line $LINENO"; return 1; }
  git -C "$repo" remote add origin https://example.com/team/app.git || return 1
  refused="$(cd "$repo" && kanna task new "Late" "hi" 2>&1)" && { echo "project_import: a task was made in a project that's out of its place"; return 1; }
  [[ "$refused" == *"isn't at its place in "*"kanna project import "*"/.kanna-v4/repos/local/app"*"/.kanna-v4/repos/team/app)"* ]] || { echo "project_import: a new origin is a new place: $refused"; return 1; }
  refused="$(KANNA_TERMINAL="$(pane plain id)" kanna project import "$repo" 2>&1)" && { echo "project_import: the import ran from a terminal it stops"; return 1; }
  [[ "$refused" == *"run kanna project import from a terminal outside it"* ]] || { echo "project_import: $refused"; return 1; }

  local summed; summed() { (cd "$1" && git status --porcelain=v2 --branch && git rev-parse HEAD 'stash@{0}' && git stash list && find . | LC_ALL=C sort && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum); }
  local before; before="$(summed "$repo")" || { echo "project_import line $LINENO"; return 1; }
  local imported there="$KANNA_AGENTS_HOME/.kanna-v4/repos/team/app"
  there="$(cd "${there%/team/app}" && pwd -P)/team/app"
  imported="$(cd / && kanna project import "$repo")" || { echo "project_import: the import failed: $imported"; return 1; }
  [[ "$imported" == "imported $repo into $there"* && "$imported" == *"copied with it: 1 worktree, 2 Claude folders"* && "$imported" == *"started again there: $(sid 1), term-"* && "$imported" == *"$repo is as it was, and Kanna won't open it: delete it once you're happy with the copy."* ]] || { echo "project_import: $imported"; return 1; }
  local newtree="$there/.kanna-worktrees/$(tid 1)"
  check wait_for 10 eval '[[ "$(kanna read Importing | tr -d "\r\n")" == *"resumed $session"* ]]' || { echo "project_import: the agent didn't resume its conversation"; kanna read Importing; return 1; }
  check test "$(cd "$(pane Importing cwd)" && pwd -P)" == "$newtree" || { echo "project_import: the agent isn't in the copy's worktree"; pane Importing cwd; return 1; }
  check test "$(summed "$repo")" == "$before" || { echo "project_import: the original changed"; diff <(echo "$before") <(summed "$repo"); return 1; }
  check test "$(cat "$there/kept.txt") $(cat "$there/new.txt") $(cat "$newtree/wip.txt")" == "uncommitted untracked task work" || { echo "project_import: the copy lacks the uncommitted or untracked work"; return 1; }
  check test "$(git -C "$there" rev-parse HEAD 'stash@{0}')" == "$(git -C "$repo" rev-parse HEAD 'stash@{0}')" || { echo "project_import: the copy's HEAD or stash"; return 1; }
  check test "$(git -C "$there" status --porcelain)" == "$(git -C "$repo" status --porcelain)" || { echo "project_import: the copy's git status"; return 1; }
  check test "$(git -C "$newtree" rev-parse --path-format=absolute --git-common-dir)" == "$there/.git" || { echo "project_import: the copy's worktree isn't the copy's"; return 1; }
  check test "$(git -C "$worktree" rev-parse --path-format=absolute --git-common-dir)" == "$repo/.git" || { echo "project_import: the original's worktree isn't the original's any more"; return 1; }
  check test -f "$claude/$(encoded "$newtree")/$session.jsonl" -a -f "$claude/$(encoded "$worktree")/$session.jsonl" || { echo "project_import: Claude's conversation wasn't copied"; ls "$claude"; return 1; }
  check test "$(cat "$claude/$(encoded "$there")/memory/MEMORY.md") $(cat "$claude/$(encoded "$repo")/memory/MEMORY.md")" == "remember remember" || { echo "project_import: Claude's memory wasn't copied"; ls -R "$claude"; return 1; }
  check wait_for 5 is side state running && check wait_for 5 unwrapped side "$newtree" || { echo "project_import: the task's terminal didn't start again in the copy"; kanna read side; return 1; }
  check wait_for 5 is plain state running && check wait_for 5 unwrapped plain "$there" || { echo "project_import: the shell didn't start again in the copy"; kanna read plain; return 1; }
  (cd "$there" && kanna task list) | placed | grep -qE "^$(sid 1) +Importing +Shelf +(idle|working|running)" || { (cd "$there" && kanna task list); return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select repo || ' ' || json_extract(doc, '$.folder') from tasks")" == "$there $newtree" || { echo "project_import: kanna.db still names the original"; return 1; }
  local opened; opened="$(cd / && kanna "$there")" && [[ "$opened" == "opened d"*" on $there" ]] || { echo "project_import: a Desk on the copy: $opened"; return 1; }
  refused="$(cd / && kanna "$repo" 2>&1)" && { echo "project_import: a Desk opened on the original"; return 1; }
  (cd "$there" && kanna task new "Next" "hi") >/dev/null || { echo "project_import: a task in the copy"; return 1; }
  refused="$(cd / && kanna project import "$there" 2>&1)"; [[ "$refused" == *"is in its place already"* ]] || { echo "project_import: $refused"; return 1; }
  refused="$(cd / && kanna project import "$repo" 2>&1)"; [[ "$refused" == *"is already there"* || "$refused" == *"both clones of example.com/team/app"* ]] || { echo "project_import: importing the original again: $refused"; return 1; }
}

scenario machine_remote_files M4
machine_remote_files() {
  local vm vm_pid
  machine_vm machine_remote_files || return 1
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/team/specs"
  mkdir -p "$repo/docs" "$repo/src" && git -C "$repo" init -q -b main || return 1
  printf '# Spec\n\nThe keeper hands terminals over.\n\n![the stack](stack.png)\n\nSee [the other doc](other.md).\n' >"$repo/docs/spec.md"
  printf '# Other\n\nAnother doc.\n' >"$repo/docs/other.md"
  printf 'fn main() {\n    println!("remote");\n}\n' >"$repo/src/main.rs"
  python3 - "$repo/docs/stack.png" <<'PY'
import struct, sys, zlib
chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
rows = b"".join(b"\0" + b"\xff\x3b\x30" * 8 for _ in range(8))
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 8, 8, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
PY
  git -C "$repo" add -A && git -C "$repo" -c user.email=e@e -c user.name=e commit -q -m first && git -C "$repo" remote add origin https://example.com/team/specs.git || { echo "machine_remote_files line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd / && vmk "$repo") >/dev/null || { echo "machine_remote_files line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-*@vm ]]' || { echo "machine_remote_files: the Desk's planner isn't on vm"; return 1; }
  local planner; planner="$(desk lead)"
  (cd "$repo" && KANNA_TERMINAL="${planner%@vm}" vmk task new "Remote spec" "Review the spec") >/dev/null || { echo "machine_remote_files line $LINENO"; return 1; }
  local task; task="$(sqlite3 -readonly "$vm/state/kanna.db" "select id from tasks order by rowid limit 1")@vm"
  check wait_for 10 eval '[[ "$(desk shelf)" == "[\"$task\"]" ]]' || { echo "machine_remote_files: the Desk view's Shelf doesn't list vm's task"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$task"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk tasks.0.title)" == "Remote spec" ]]' || { echo "machine_remote_files: starting vm's task"; desk tasks; return 1; }
  local agent; agent="$(desk tasks.0.task)"
  raw '{"id":1,"cmd":"debug.click","args":{"task":"'"$agent"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$agent" ]]' || { echo "machine_remote_files: pulling vm's task"; return 1; }
  local worktree="$repo/.kanna-worktrees/${task%@vm}"
  check wait_for 10 test -f "$worktree/docs/spec.md" || { echo "machine_remote_files: no worktree"; return 1; }
  printf '# Spec\n\nThe keeper hands terminals over. SENDME to the app.\n\n![the stack](stack.png)\n\nSee [the other doc](other.md).\n' >"$worktree/docs/spec.md"
  rv() { (cd "$worktree" && KANNA_TERMINAL="${agent%@vm}" vmk "$@"); }

  local spec; spec="$(rv open md docs/spec.md --tab --name spec --json | field pane.id)@vm" || { echo "machine_remote_files line $LINENO"; return 1; }
  rv markup spec | grep -qF "comments on in" || { echo "machine_remote_files: markup on vm's doc"; return 1; }
  md() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$spec\"${1:-}}}"; }
  check wait_for 10 eval 'md | field text | grep -qF "[image]"' || { echo "machine_remote_files: vm's image didn't show"; md | field text; return 1; }
  check wait_for 5 eval '[[ "$(md | field name) $(md | field markup) $(md | field agent)" == "docs/spec.md True $agent" ]]' || { echo "machine_remote_files: the doc's name, comments and agent"; md; return 1; }
  thread() { md | field threads | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin) if t["id"] == sys.argv[1]]
v = t[0][sys.argv[2]] if t else None
print(len(v) if isinstance(v, list) else v)' "$1" "$2"; }
  local at; at="$(md | field text | python3 -c 'import sys; print(sys.stdin.read().index("SENDME"))')"
  md ",\"select\":[$at,6]" >/dev/null
  key cmd+return; check wait_for 2 eval '[[ "$(md | field box)" == new ]]' || { echo "machine_remote_files: no comment box"; md; return 1; }
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Please fix"}}' >/dev/null
  key cmd+return
  check wait_for 5 eval '[[ "$(thread c1 status)" == sent ]]' || { echo "machine_remote_files: the comment wasn't sent"; md | field threads; return 1; }
  check test -f "$worktree/.kanna/docs/docs__spec.md.json" || { echo "machine_remote_files: the thread wasn't saved on vm"; return 1; }
  check wait_for 5 eval 'vmk read "${agent%@vm}" | tr -d "\\n" | grep -qF "kanna comment c1 · docs/spec.md:3"' || { echo "machine_remote_files: the comment didn't reach vm's agent"; vmk read "${agent%@vm}"; return 1; }
  rv markup reply c1 --doc "$worktree/docs/spec.md" "Fixed on vm." >/dev/null || { echo "machine_remote_files line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(thread c1 messages)" == 2 ]]' || { echo "machine_remote_files: vm's reply didn't reach the app"; md | field threads; return 1; }
  sleep 0.5; snap d1 machine_remote_files >/dev/null
  local shot; shot="$(rv screenshot spec -o "$vm/spec.png")" || { echo "machine_remote_files: screenshot from vm: $shot"; return 1; }
  [[ "$shot" == *"$vm/spec.png"* && "$(head -c 4 "$vm/spec.png" | tail -c 3)" == PNG ]] || { echo "machine_remote_files: vm's screenshot wasn't written where it asked: $shot"; return 1; }
  ! rv screenshot spec -o "$vm/spec.png" >/dev/null 2>&1 || { echo "machine_remote_files: a screenshot replaced a file without --force"; return 1; }
  md ',"follow":"other.md"' >/dev/null
  check wait_for 5 eval '[[ "$(md | field name)" == docs/other.md ]] && md | field text | grep -q "Another doc"' || { echo "machine_remote_files: following a link in vm's doc"; md | field name; return 1; }

  local diffed; diffed="$(rv open md docs/spec.md --diff main --tab --name spec-diff --json | field pane.id)@vm" || { echo "machine_remote_files line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$diffed\"}}" | field changes)" == 1 ]]' || { echo "machine_remote_files: vm's doc against main"; raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$diffed\"}}"; return 1; }
  local code; code="$(rv open code src/main.rs --tab --name code --json | field pane.id)@vm" || { echo "machine_remote_files line $LINENO"; return 1; }
  check wait_for 10 eval 'raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$code\"}}" | field text | grep -qF "println!(\"remote\")"' || { echo "machine_remote_files: vm's code"; return 1; }
  rv comment src/main.rs:2 "The print." >/dev/null || { echo "machine_remote_files: a comment on vm's code needs the app to snap it"; rv comment src/main.rs:2 "The print."; return 1; }
}

scenario machine_remote_web M4
machine_remote_web() {
  local vm vm_pid
  machine_vm machine_remote_web || return 1
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/team/devserver"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first && git -C "$repo" remote add origin https://example.com/team/devserver.git || { echo "machine_remote_web line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd / && vmk "$repo") >/dev/null || { echo "machine_remote_web line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-*@vm ]]' || { echo "machine_remote_web: the Desk's planner isn't on vm"; return 1; }
  local planner; planner="$(desk lead)"
  (cd "$repo" && KANNA_TERMINAL="${planner%@vm}" vmk task new "Dev server" "Run the dev server") >/dev/null || { echo "machine_remote_web line $LINENO"; return 1; }
  local task; task="$(sqlite3 -readonly "$vm/state/kanna.db" "select id from tasks order by rowid limit 1")@vm"
  check wait_for 10 eval '[[ "$(desk shelf)" == "[\"$task\"]" ]]' || { echo "machine_remote_web: the Shelf doesn't list vm's task"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$task"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk tasks.0.title)" == "Dev server" ]]' || { echo "machine_remote_web: picking vm's task didn't put it on the Desk"; return 1; }
  local agent; agent="$(desk tasks.0.task)"
  raw '{"id":1,"cmd":"debug.click","args":{"task":"'"$agent"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$agent" ]]' || { echo "machine_remote_web: activating vm's task didn't dock its agent"; return 1; }

  local worktree="$repo/.kanna-worktrees/${task%@vm}" port
  port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
  echo "hello from vm" >"$worktree/message.txt"
  local opened; opened="$(cd "$worktree" && KANNA_TERMINAL="${agent%@vm}" vmk open web "http://localhost:$port/" --tab --name devpage --json)" || { echo "machine_remote_web: opening the dev server's page: $opened"; return 1; }
  local page; page="$(echo "$opened" | field pane.id)@vm"
  web() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$page\"}}" | field "$1"; }
  tunnel() { raw '{"id":1,"cmd":"debug.machines"}' | python3 -c 'import json, sys; print(json.load(sys.stdin)["links"][0]["ports"].get(sys.argv[1], ""))' "$port"; }
  check wait_for 10 eval '[[ "$(tunnel)" =~ ^[0-9]+$ ]]' || { echo "machine_remote_web: the app opened no tunnel to vm's port $port"; raw '{"id":1,"cmd":"debug.machines"}'; return 1; }
  check wait_for 10 eval '[[ "$(web notice)" == "Waiting for localhost:$port on vm…"* ]]' || { echo "machine_remote_web: a page before its dev server is up should wait for it"; web notice; return 1; }
  (cd "$worktree" && KANNA_TERMINAL="${agent%@vm}" vmk open --tab --name devserver --cmd "python3 '$repo_root/tools/hmr-server/hmr-server.py' $port '$worktree'") >/dev/null || { echo "machine_remote_web line $LINENO"; return 1; }
  check wait_for 10 eval 'vmk read devserver | grep -q "hmr-server on"' || { echo "machine_remote_web: the task's dev server didn't start"; vmk read devserver; return 1; }
  local local_port; local_port="$(tunnel)"
  [[ "$local_port" != "$port" ]] || { echo "machine_remote_web: the tunnel listens on the dev server's own port"; return 1; }
  check wait_for 10 eval 'web text | grep -q "hello from vm"' || { echo "machine_remote_web: the web pane didn't load vm's dev server"; web url; web text; return 1; }
  check test "$(web url)" == "http://127.0.0.1:$local_port/" || { echo "machine_remote_web: the web pane isn't on this Mac's loopback"; web url; return 1; }
  check web text | grep -q "served for 127.0.0.1:$local_port" || { echo "machine_remote_web: the dev server didn't see a localhost Host"; web text; return 1; }
  check wait_for 10 eval 'web text | grep -qx connected' || { echo "machine_remote_web: the page's HMR socket didn't connect"; web text; return 1; }
  vmurl() { KANNA_SOCKET="$vm/kanna.sock" pane devpage url; }
  check test "$(vmurl)" == "http://localhost:$port/" || { echo "machine_remote_web: vm's daemon doesn't keep the dev server's own URL"; vmurl; return 1; }
  sleep 0.5; snap d1 machine_remote_web >/dev/null

  echo "hot reloaded on vm" >"$worktree/message.txt"
  check wait_for 10 eval 'web text | grep -q "hot reloaded on vm"' || { echo "machine_remote_web: editing the file on vm didn't hot reload the page"; web text; return 1; }
  check web text | grep -q "hot updated" || { web text; return 1; }
  check web text | grep -q "loads 1" || { echo "machine_remote_web: the page reloaded instead of taking a hot update"; web text; return 1; }

  raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$page\",\"js\":\"location.href = '/next?x=2'; 1\"}}" >/dev/null
  check wait_for 10 eval 'web text | grep -q "loads 2"' || { echo "machine_remote_web: following a link on the page"; web text; return 1; }
  check wait_for 5 eval '[[ "$(vmurl)" == "http://localhost:$port/next?x=2" ]]' || { echo "machine_remote_web: a page followed in the app didn't reach vm's daemon with the dev server's own address"; vmurl; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$page\",\"js\":\"document.body.append('stale view'); 1\"}}" >/dev/null
  check wait_for 5 eval 'web text | grep -q "stale view"' || return 1
  local shown; shown="$(raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$page\",\"edit\":\"{shown}\"}}" | field editor)"
  [[ "$shown" == "http://localhost:$port/next?x=2" ]] || { echo "machine_remote_web: the URL editor shows the tunnel's address: $shown"; return 1; }
  check wait_for 10 eval 'web text | grep -q "hot reloaded on vm" && ! web text | grep -q "stale view"' || { echo "machine_remote_web: submitting the URL editor didn't load the page again"; web text; return 1; }
  check test "$(vmurl)" == "http://localhost:$port/next?x=2" || { echo "machine_remote_web: submitting the URL editor gave vm's daemon the tunnel's address"; vmurl; return 1; }

  kill "$vm_pid"; wait_for 10 eval '! kill -0 '"$vm_pid" || { echo "machine_remote_web: vm's daemon didn't stop"; return 1; }
  check wait_for 10 eval '[[ "$(machines links.0.attached)" == False ]]' || { echo "machine_remote_web: the app didn't see vm go"; return 1; }
  KANNA_MACHINE_PORT="$(head -1 "$KANNA_STATE_DIR/offer" | field port)" KANNA_SOCKET="$vm/kanna.sock" KANNA_STATE_DIR="$vm/state" KANNA_KEEPER="$bin/kanna-keeper" "$bin/kanna-daemon" >>"$out/daemon-vm.log" 2>&1 &
  vm_pid=$!
  check wait_for 30 eval '[[ "$(machines links.0.attached)" == True ]]' || { echo "machine_remote_web: the app didn't link to vm again"; return 1; }
  check test "$(tunnel)" == "$local_port" || { echo "machine_remote_web: vm coming back changed the page's origin"; tunnel; return 1; }
  echo "after vm came back" >"$worktree/message.txt"
  raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$page\",\"js\":\"location.reload(); 1\"}}" >/dev/null
  check wait_for 10 eval 'web text | grep -q "after vm came back"' || { echo "machine_remote_web: the page didn't load through the tunnel after vm came back"; web text; return 1; }

  stop_app; start_app || { echo "machine_remote_web line $LINENO"; return 1; }
  check wait_for 30 eval '[[ "$(machines links.0.attached)" == True ]]' || { echo "machine_remote_web: the app didn't link to vm again"; return 1; }
  check wait_for 15 eval 'web text | grep -q "after vm came back"' || { echo "machine_remote_web: the restored web pane didn't load vm's dev server again"; web url; web text; return 1; }
  echo "after the restart" >"$worktree/message.txt"
  check wait_for 10 eval 'web text | grep -q "after the restart"' || { echo "machine_remote_web: hot reload after the app restarted"; web text; return 1; }
}
