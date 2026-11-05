scenario scheduler M4
scheduler() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "scheduler line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  printf '{"version": 1, "shelf": {"runners": 1}}\n' >"$repo/.kanna/config.json"
  start_daemon && start_app || { echo "scheduler line $LINENO"; return 1; }
  schedule() { (cd "$repo" && kanna schedule "$@"); }
  schedule list | grep -qE '^s1 +quiz +repo +daily 09:00 +planner +next ' || { echo "scheduler: the repo has no quiz schedule"; schedule list; return 1; }
  local refused; refused="$(schedule add dialy 09:00 planner hi 2>&1)"; [[ "$refused" == *'try "daily 09:00"'* ]] || { echo "scheduler: a bad rule wasn't refused with an example: $refused"; return 1; }
  task_state() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0]
print("running" if t.get("running") else "stopped" if t.get("folder") else "waiting", t.get("held") or "-")' "$1"; }
  (cd "$repo" && kanna task new "Busy" "work 12" --agent "$fake/codex") >/dev/null || { echo "scheduler line $LINENO"; return 1; }
  local busy term; busy="$(tid 1)"
  check wait_for 10 eval '[[ "$(task_state $busy)" == running* ]]' || { echo "scheduler: the task didn't run"; return 1; }
  term="$(task_state $busy | cut -d' ' -f2)"
  check wait_for 10 is "$term" status working || { echo "scheduler: the task never showed as working"; return 1; }
  local at added; at="$(date -v+2M +%H:%M)"
  added="$(schedule add daily "$at" "$(sid 1)" hello from the scheduler --name hello)" || { echo "scheduler line $LINENO"; return 1; }
  [[ "$added" == "s2 hello: daily $at → $(sid 1), next "*" $at" ]] || { echo "scheduler: add said \"$added\""; return 1; }
  raw '{"id":1,"cmd":"debug.clock","args":{"advance":180}}' | grep -q '"ok":true' || { echo "scheduler: the daemon has no test clock"; return 1; }
  check wait_for 5 eval '(cd "$repo" && kanna task messages "$busy") | grep -q "schedule hello"' || { echo "scheduler: the schedule left no message on the moved clock"; schedule list; return 1; }
  shows "$term" "hello from the scheduler" && { echo "scheduler: the message went in mid-turn"; kanna read "$term"; return 1; }
  check wait_for 20 shows "$term" "peer: \[message from schedule hello\] hello from the scheduler" || { echo "scheduler: the message didn't arrive at the turn's end"; kanna read "$term"; (cd "$repo" && kanna task messages); return 1; }
  sleep 2
  check test "$(kanna read "$term" | grep -c 'message from schedule hello')" -eq 1 || { echo "scheduler: the message came more than once"; kanna read "$term"; return 1; }
  schedule list | grep -E '^s2 ' | grep -q waiting && { echo "scheduler: still waiting after it was delivered"; schedule list; return 1; }
  [[ "$(schedule remove hello)" == "removed s2 hello" ]] || { echo "scheduler line $LINENO"; return 1; }
}

scenario schedules_tab M4
schedules_tab() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" other="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/aaa" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$other" "$fake" && git -C "$repo" init -q -b main && git -C "$other" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "schedules_tab line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"; other="$(cd "$other" && pwd -P)"
  printf '{"version": 1, "shelf": {"runners": 1}}\n' >"$repo/.kanna/config.json"
  start_daemon && start_app || { echo "schedules_tab line $LINENO"; return 1; }
  schedule() { (cd "$repo" && kanna schedule "$@"); }
  local opened d; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "schedules_tab line $LINENO: $opened"; return 1; }
  d="${BASH_REMATCH[1]}"
  terminal_of() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
r = json.load(sys.stdin)
t = [t for t in r["desk"] + r["shelf"] if t["id"] == sys.argv[1]][0]
print(t.get("pane") or t.get("held") or "")' "$1"; }
  local busy term; busy="$(cd "$repo" && kanna --json task new "Sched target" "work 1" --agent "$fake/codex" | field task.id)" || { echo "schedules_tab line $LINENO"; return 1; }
  kanna task pick "$busy" >/dev/null || { echo "schedules_tab line $LINENO"; return 1; }
  check wait_for 10 eval '[[ -n "$(terminal_of $busy)" ]]' || { echo "schedules_tab: the task didn't run"; return 1; }
  term="$(terminal_of $busy)"
  check wait_for 15 is "$term" status idle || { echo "schedules_tab: the task never went idle"; return 1; }
  (cd "$other" && kanna schedule list) >/dev/null || { echo "schedules_tab line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  shown() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$d\"}}" | python3 -c "$1"; }
  rowis() { shown 'import json, sys
rows = {r["name"]: r for r in json.load(sys.stdin)["schedules"] if r["repo"] == "'"$repo"'"}
r = rows.get("ping")
sys.exit(0 if r is not None and ('"$1"') else 1)'; }
  click() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"schedule\":\"$1\",\"window\":\"$d\"}}" | grep -q '"ok":true'; }
  schedule add every 5m "$busy" "hello from the tab" --name ping >/dev/null || { echo "schedules_tab line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk pages "$d")" == '"'"'["Desk", "Machines", "Stats", "Schedules"]'"'"' ]]' || { echo "schedules_tab: the Desk view's fixed tabs"; desk pages "$d"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$d\",\"size\":[1300,900]}}" >/dev/null
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"Schedules\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(desk schedulesPage "$d")" == True ]]' || { echo "schedules_tab: the Schedules tab"; return 1; }
  check wait_for 5 rowis 'r["rule"] == "every 5m" and r["target"] == "Sched target" and not r["waiting"] and not r["firings"]' || { echo "schedules_tab: no row for ping"; desk schedules "$d"; return 1; }
  check test "$(desk scheduleGroups "$d")" == "[\"$repo\", \"$other\"]" || { echo "schedules_tab: the groups' order"; desk scheduleGroups "$d"; return 1; }
  raw '{"id":1,"cmd":"debug.clock","args":{"advance":301}}' | grep -q '"ok":true' || { echo "schedules_tab: the daemon has no test clock"; return 1; }
  check wait_for 10 rowis 'len(r["firings"]) == 1 and r["firings"][0]["state"] in ("waiting", "delivered")' || { echo "schedules_tab: a firing didn't appear, live"; desk schedules "$d"; schedule list; return 1; }
  check wait_for 20 rowis 'r["firings"][0]["state"] == "delivered" and not r["waiting"]' || { echo "schedules_tab: the delivery didn't appear, live"; desk schedules "$d"; schedule list; return 1; }
  check wait_for 15 is "$term" status idle || { echo "schedules_tab: the turn didn't end"; return 1; }
  raw "{\"id\":1,\"cmd\":\"schedule\",\"args\":{\"action\":\"reply\",\"schedule\":\"ping\",\"repo\":\"$repo\",\"text\":\"All green.\",\"by\":\"$term\"}}" | grep -q '"ok":true' || { echo "schedules_tab: the target couldn't reply"; return 1; }
  check wait_for 5 rowis 'r["firings"][0]["note"] == "All green."' || { echo "schedules_tab: the reply didn't appear"; desk schedules "$d"; return 1; }
  click "$(schedule list | awk '$2 == "ping" {print $1}'):pause" || { echo "schedules_tab: no Pause button"; return 1; }
  check wait_for 5 rowis 'r["paused"]' || { echo "schedules_tab: Pause from the tab"; desk schedules "$d"; return 1; }
  schedule list | grep -E ' ping ' | grep -q ' paused ' || { echo "schedules_tab: list doesn't say paused"; schedule list; return 1; }
  raw '{"id":1,"cmd":"debug.clock","args":{"advance":301}}' >/dev/null
  sleep 2
  check rowis 'len(r["firings"]) == 1' || { echo "schedules_tab: a paused schedule fired"; desk schedules "$d"; return 1; }
  [[ "$(schedule resume ping)" == *"so it fires once now" ]] || { echo "schedules_tab: resume didn't say it missed a time"; return 1; }
  check wait_for 70 rowis 'len(r["firings"]) == 2 and not r["paused"]' || { echo "schedules_tab: resume, from the CLI, didn't fire the missed time once"; desk schedules "$d"; schedule list; return 1; }
  check wait_for 20 rowis 'r["firings"][0]["state"] == "delivered" and r["runs"]' || { echo "schedules_tab: the missed time's message wasn't delivered"; desk schedules "$d"; return 1; }
  check wait_for 15 is "$term" status idle || { echo "schedules_tab: the second turn didn't end"; return 1; }
  local id; id="$(schedule list | awk '$2 == "ping" {print $1}')"
  click "$id:run" || { echo "schedules_tab: no Run now button"; return 1; }
  check wait_for 5 rowis 'len(r["firings"]) == 3' || { echo "schedules_tab: Run now from the tab"; desk schedules "$d"; return 1; }
  click "$id:prompt" || { echo "schedules_tab: no prompt to expand"; return 1; }
  check wait_for 5 rowis 'r["open"]' || { echo "schedules_tab: the prompt didn't expand"; return 1; }
  click "$id:remove" || { echo "schedules_tab: no Remove button"; return 1; }
  sleep 1
  check rowis 'True' || { echo "schedules_tab: Remove went without its second click"; return 1; }
  click "$id:remove" || { echo "schedules_tab: no Remove? button"; return 1; }
  check wait_for 5 eval '! rowis True' || { echo "schedules_tab: Remove from the tab"; desk schedules "$d"; return 1; }
}

scenario pipeline_run M4
pipeline_run() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/factory" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$fake/bin" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/bin/claude"
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$fake/bin/claude" >"$fake/claude" && chmod +x "$fake/claude"
  python3 -c 'import json, sys; p = sys.argv[1]; c = json.load(open(p)); c["pipelines"] = {"agents": {"claude": sys.argv[2]}}; json.dump(c, open(p, "w"))' "$KANNA_AGENTS_HOME/.kanna-v4/config.json" "$fake/claude" || { echo "pipeline_run line $LINENO"; return 1; }
  start_daemon && start_app || { echo "pipeline_run line $LINENO"; return 1; }
  mkdir -p "$repo/.kanna/pipelines" "$repo/.kanna/prompts" && git -C "$repo" init -q -b main || { echo "pipeline_run line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  git -C "$repo" config user.email e@e && git -C "$repo" config user.name e
  printf '{"version": 1, "shelf": {"runners": 1}}\n' >"$repo/.kanna/config.json"
  cat >"$repo/.kanna/pipelines/demo.kojo" <<'PIPE'
budget = 4 sessions, 30m
build  = claude
check  = run sh check.sh

build -> check
check -[fail, max 2]-> build
check -> done
PIPE
  cat >"$repo/.kanna/prompts/build.md" <<'PROMPT'
Build the change.
mock: run echo visit >> built.txt && git add built.txt && git commit -qm build && echo 'Built it.' >"$KANNA_ANSWER" && "$KANNA_CLI" run done
PROMPT
  printf 'needy = claude\nquiet = claude\nneedy -> quiet -> done\n' >"$repo/.kanna/pipelines/needy.kojo"
  printf 'Ask first.\nmock: run "$KANNA_CLI" run need "Which colour?"\n' >"$repo/.kanna/prompts/needy.md"
  printf 'Stop without reporting.\nsay: Not sure what to do.\n' >"$repo/.kanna/prompts/quiet.md"
  cat >"$repo/check.sh" <<'CHECK'
n=$(cat .kanna/tmp/checks 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" >.kanna/tmp/checks
if [ "$n" -eq 1 ]; then echo "FAILED: first try"; exit 1; fi
touch .kanna/tmp/checking; sleep 8
grep -c visit built.txt
CHECK
  git -C "$repo" add -A && git -C "$repo" commit -qm first || { echo "pipeline_run line $LINENO"; return 1; }
  local main; main="$(git -C "$repo" rev-parse main)"
  (cd "$repo" && kanna task new "Factory" "run $bin/kanna run start demo --request 'Count the visits'" --agent "$fake/claude") >/dev/null || { echo "pipeline_run line $LINENO"; return 1; }
  local task; task="$(tid 1)"
  rows() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
t = json.load(sys.stdin); print(len(t["shelf"]), len(t.get("desk") or []))'; }
  held() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
print([t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0].get("held") or "-")' "$task"; }
  runs() { kanna run list --all --json | python3 -c 'import json, sys
r = json.load(sys.stdin)["runs"]; print(" ".join(x[sys.argv[1]] if isinstance(x[sys.argv[1]], str) else json.dumps(x[sys.argv[1]]) for x in r))' "$1"; }
  check wait_for 20 eval '[[ "$(runs run)" == run-* ]]' || { echo "pipeline_run: the task's agent didn't start a run"; kanna read "$(held)"; tail -20 "$out/daemon.log"; return 1; }
  local run term; run="$(runs run)"; term="$(held)"
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select task from runs where id = '$run'")" == "$task" || { echo "pipeline_run: the run isn't the task's"; return 1; }
  local worker="$repo/.kanna-worktrees/$task-run-1"
  check test "$(git -C "$worker" branch --show-current)" == "run/$task-1" || { echo "pipeline_run: no worker on run/$task-1"; return 1; }
  where_is() { kanna workspace list --all --json | python3 -c 'import json, sys
pane = sys.argv[1]; view = json.load(sys.stdin)
def ids(node):
    if isinstance(node, dict):
        for k, v in node.items():
            if k in ("background", "panes"): continue
            if k in ("pane", "focused", "id") and v == pane: yield v
            yield from ids(v)
    elif isinstance(node, list):
        for v in node: yield from ids(v)
if pane not in [p["id"] for p in view.get("panes") or []]: print("closed"); sys.exit()
background = [w["id"] for w in view.get("workspaces") or [] if pane in (w.get("background") or [])]
view.pop("panes", None)
print("drawn" if any(ids(view)) else "background " + background[0] if background else "nowhere")' "$1"; }
  local taskws; taskws="$(kanna workspace list --all --json | python3 -c 'import json, sys
print([w["id"] for w in json.load(sys.stdin)["workspaces"] if w["dock"]["id"] == sys.argv[1]][0])' "$term")" || { echo "pipeline_run: the task has no workspace"; return 1; }
  workers() { sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from run_stages where run = '$run' and terminal is not null" | tr '\n' ' '; }
  local bad=""
  watch_rows() {
    [[ "$(rows)" == "1 0" ]] || bad="the Shelf and Desk showed $(rows) rows during the run"
    local w at; for w in $(workers); do at="$(where_is "$w")"; [[ "$at" == "background $taskws" || "$at" == closed ]] || bad="worker $w is $at, not in $taskws's background"; done
    [[ -n "$bad" || -f "$worker/.kanna/tmp/checking" ]]
  }
  check wait_for 60 watch_rows || { echo "pipeline_run: the second check never started"; kanna run show "$run"; for w in $(workers) "$term"; do echo "== $w"; kanna read "$w"; kanna status explain "$w"; done; tail -30 "$out/daemon.log"; return 1; }
  [[ -z "$bad" ]] || { echo "pipeline_run: $bad"; return 1; }
  check test "$(held)" == "$term" || { echo "pipeline_run: the task's docked pane changed from $term to $(held)"; return 1; }
  local checking; checking="$(workers | awk '{print $NF}')"
  local peek; peek="$(kanna move "$checking" --tab)" || { echo "pipeline_run: the worker couldn't be shown: $peek"; return 1; }
  check test "$(where_is "$checking")" == drawn || { echo "pipeline_run: the shown worker is $(where_is "$checking"): $peek"; return 1; }
  in_tab() { kanna workspace list --all --json | python3 -c 'import json, sys
ws = [w for w in json.load(sys.stdin)["workspaces"] if w["id"] == sys.argv[1]][0]
print(any(sys.argv[2] in t["panes"] for t in ws["tabs"]))' "$taskws" "$checking"; }
  check test "$(in_tab)" == True || { echo "pipeline_run: the shown worker isn't in a tab of $taskws: $peek"; return 1; }
  check test "$(rows)" == "1 0" || { echo "pipeline_run: $(rows) rows with a worker shown"; return 1; }
  local back; back="$(kanna move "$checking" --background)" || { echo "pipeline_run: the worker couldn't go back: $back"; return 1; }
  [[ "$back" == "$checking is in the background of $taskws (${task:0:9}, on the Shelf)" ]] || { echo "pipeline_run: putting the worker back said: $back"; return 1; }
  check test "$(where_is "$checking")" == "background $taskws" || { echo "pipeline_run: the worker put back is $(where_is "$checking")"; return 1; }
  check test "$(in_tab)" == False || { echo "pipeline_run: the worker put back is still in a tab"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from run_stages where run = '$run' and terminal is not null order by rowid desc limit 1")" == "$checking" || { echo "pipeline_run: the run's records changed"; return 1; }
  grep -q "# Feedback from check" "$worker/.kanna/tmp/stage-3.md" && grep -q "FAILED: first try" "$worker/.kanna/tmp/stage-3.md" || { echo "pipeline_run: build's second visit didn't get check's failure"; cat "$worker/.kanna/tmp/stage-3.md"; return 1; }
  grep -q "Count the visits" "$worker/.kanna/tmp/stage-1.md" || { echo "pipeline_run: the request isn't in the stage's prompt"; return 1; }
  stop_daemon; start_daemon || { echo "pipeline_run: the daemon didn't come back"; return 1; }
  check wait_for 60 eval '[[ "$(runs state)" == done ]]' || { echo "pipeline_run: the run didn't hand back after the restart: $(runs state)"; kanna run show "$run"; tail -30 "$out/daemon.log"; return 1; }
  check test "$(rows)" == "1 0" || { echo "pipeline_run: $(rows) rows after the run"; return 1; }
  local shown; shown="$(kanna run show "$run" --json)"
  python3 -c 'import json, sys
v = json.loads(sys.argv[1])["visits"]
got = [(x["stage"], x["verdict"]) for x in v]
want = [("build", "done"), ("check", "fail"), ("build", "done"), ("check", "pass")]
assert got == want, got
assert v[0]["doc"]["answer"] == "Built it.\n", v[0]["doc"]
assert all(x["machine"] for x in v)' "$shown" || { echo "pipeline_run: the visits were: $shown"; return 1; }
  check test "$(git -C "$repo" rev-parse main)" == "$main" || { echo "pipeline_run: the run changed main"; git -C "$repo" log --oneline -3 main; return 1; }
  local folder="$repo/.kanna-worktrees/$task"
  check test "$(git -C "$folder" rev-parse HEAD)" == "$main" || { echo "pipeline_run: the hand-back moved the task's branch"; return 1; }
  check test ! -e "$folder/built.txt" || { echo "pipeline_run: the hand-back changed the task's worktree"; return 1; }
  check test "$(git -C "$repo" show "run/$task-1:built.txt" | grep -c visit)" -eq 2 || { echo "pipeline_run: the run's branch doesn't have its work"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*) from messages where sender = '$run' and text like 'handed back: 4 visits of 2 stages%run/$task-1 is ready: take it with \`git merge --ff-only run/$task-1\` in your worktree%'")" -eq 1 || { echo "pipeline_run: the task wasn't told how to take the run's branch"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select text from messages where sender = '$run'"; return 1; }
  git -C "$folder" merge -q --ff-only "run/$task-1" || { echo "pipeline_run: the task's agent couldn't take the run's branch"; return 1; }
  check test ! -d "$worker" || { echo "pipeline_run: the worker's worktree is still there"; return 1; }
  check wait_for 30 eval 'term="$(held)"; [[ "$term" != - ]] && shows "$term" "peer: \[message from $run\] handed back: 4 visits of 2 stages"' || { echo "pipeline_run: the task didn't get the run's message"; kanna read "$(held)"; (cd "$repo" && kanna task messages "$task"); return 1; }
  local events; events="$(kanna run log "$run" --json | python3 -c 'import json, sys; print(" ".join(e["what"] for e in json.load(sys.stdin)["events"]))')"
  [[ "$events" == "started stage verdict stage verdict stage verdict stage verdict handed_back" ]] || { echo "pipeline_run: the run's events were: $events"; return 1; }
  local said; said="$(cd "$folder" && KANNA_TERMINAL="$(held)" kanna run done 2>&1)" && { echo "pipeline_run: the task's own terminal reported a stage done"; return 1; }
  [[ "$said" == *"for a stage's agent, in its worker's terminal"* ]] || { echo "pipeline_run: run done outside a stage said: $said"; return 1; }
  local again; again="$(cd "$folder" && KANNA_TERMINAL="$(held)" kanna run start needy 2>&1)" || { echo "pipeline_run: a second run didn't start: $again"; return 1; }
  [[ "$again" == run-*" runs needy for "task-*" on run/$task-2 "* ]] || { echo "pipeline_run: the second run said: $again"; return 1; }
  local second; second="${again%% *}"
  check wait_for 20 eval '[[ "$(runs state)" == "done paused" ]]' || { echo "pipeline_run: a stage's need didn't pause the run: $(runs state)"; return 1; }
  [[ "$(runs why)" == *"needy needs: Which colour?"* ]] || { echo "pipeline_run: the paused run says: $(runs why)"; return 1; }
  check wait_for 30 eval 'term="$(held)"; [[ "$term" != - ]] && shows "$term" "needy needs you: Which colour?"' || { echo "pipeline_run: the task wasn't told what the stage needs"; kanna read "$(held)"; return 1; }
  local answered=""
  resumed() { answered="$(cd "$folder" && KANNA_TERMINAL="$(held)" kanna run resume "$second" "$1" 2>&1)"; }
  check wait_for 20 resumed 'mock: run echo blue > colour.txt && git add colour.txt && git commit -qm colour && "$KANNA_CLI" run done' || { echo "pipeline_run: the needy stage didn't resume: $answered"; tail -20 "$out/daemon.log"; return 1; }
  check wait_for 20 eval '[[ "$(runs why)" == *"quiet stopped without reporting"* ]]' || { echo "pipeline_run: a turn that ended without a report didn't pause the run: $(runs state) $(runs why)"; kanna run show "$second"; return 1; }
  check wait_for 30 eval 'term="$(held)"; [[ "$term" != - ]] && shows "$term" "kanna run resume $second"' || { echo "pipeline_run: the task wasn't told the quiet stage stopped"; kanna read "$(held)"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*) from messages where sender = '$second' and text like 'quiet''s turn ended without it reporting%Not sure what to do.%'")" -eq 1 || { echo "pipeline_run: the task wasn't told the quiet stage stopped, with its screen"; sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select text from messages where sender = '$second'"; return 1; }
  check wait_for 20 resumed 'mock: run "$KANNA_CLI" run done' || { echo "pipeline_run: the quiet stage didn't resume: $answered"; return 1; }
  check wait_for 30 eval '[[ "$(runs state)" == "done done" ]]' || { echo "pipeline_run: the resumed run didn't hand back: $(runs state) $(runs why)"; kanna run show "$second"; tail -20 "$out/daemon.log"; return 1; }
  check test "$(git -C "$repo" show "run/$task-2:colour.txt")" == blue || { echo "pipeline_run: the resumed stage's work isn't on the run's branch"; return 1; }
  check test ! -e "$folder/colour.txt" || { echo "pipeline_run: the second hand-back changed the task's worktree"; return 1; }
  python3 -c 'import json, sys
v = json.loads(sys.argv[1])["visits"]
assert [(x["stage"], x["verdict"]) for x in v] == [("needy", "done"), ("quiet", "done")], v
assert v[0]["doc"]["replies"] == 1 and "paused" not in v[0]["doc"], v[0]["doc"]' "$(kanna run show "$second" --json)" || { echo "pipeline_run: the resumed run's visits: $(kanna run show "$second" --json)"; return 1; }
  events="$(kanna run log "$second" --json | python3 -c 'import json, sys; print(" ".join(e["what"] for e in json.load(sys.stdin)["events"]))')"
  [[ "$events" == "started stage paused resumed verdict stage paused resumed verdict handed_back" ]] || { echo "pipeline_run: the resumed run's events were: $events"; return 1; }
  again="$(cd "$folder" && KANNA_TERMINAL="$(held)" kanna run start needy 2>&1)" || { echo "pipeline_run: a third run didn't start: $again"; return 1; }
  check wait_for 20 eval '[[ "$(runs state)" == "done done paused" ]]' || { echo "pipeline_run: the third run didn't pause: $(runs state)"; return 1; }
  (cd "$folder" && KANNA_TERMINAL="$(held)" kanna run stop) >/dev/null || { echo "pipeline_run line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(runs state)" == "done done stopped" ]]' || { echo "pipeline_run: the paused run wasn't stopped: $(runs state)"; return 1; }
  printf 'wait = run sleep 300\nwait -> done\n' >"$folder/.kanna/pipelines/slow.kojo"
  again="$(cd "$folder" && KANNA_TERMINAL="$(held)" kanna run start slow 2>&1)" || { echo "pipeline_run: the slow run didn't start: $again"; return 1; }
  local slow="${again%% *}" sleeper=""
  slept() { sleeper="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from run_stages where run = '$slow' and terminal is not null")"; [[ -n "$sleeper" ]]; }
  check wait_for 20 slept || { echo "pipeline_run: the slow run opened no worker"; return 1; }
  local slow_worker; slow_worker="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select json_extract(doc, '$.worker') from runs where id = '$slow'")"
  check test -d "$slow_worker" || { echo "pipeline_run: no worktree for the slow run"; return 1; }
  check wait_for 10 eval '[[ "$(where_is "$sleeper")" == "background $taskws" ]]' || { echo "pipeline_run: the slow worker is $(where_is "$sleeper")"; return 1; }
  again="$(cd "$folder" && KANNA_TERMINAL="$(held)" kanna run start slow 2>&1)" || { echo "pipeline_run: a second live run of the task didn't start: $again"; return 1; }
  local other="${again%% *}" other_sleeper=""
  slept_too() { other_sleeper="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from run_stages where run = '$other' and terminal is not null")"; [[ -n "$other_sleeper" ]]; }
  check wait_for 20 slept_too || { echo "pipeline_run: the second live run opened no worker"; return 1; }
  check test "$(runs state)" == "done done stopped running running" || { echo "pipeline_run: two runs of one task aren't both live: $(runs state)"; return 1; }
  check wait_for 10 eval '[[ "$(where_is "$other_sleeper")" == "background $taskws" ]]' || { echo "pipeline_run: the second slow worker is $(where_is "$other_sleeper")"; return 1; }
  said="$(cd "$folder" && KANNA_TERMINAL="$(held)" kanna run stop 2>&1)" && { echo "pipeline_run: run stop picked one of two live runs"; return 1; }
  [[ "$said" == *"2 live runs: say which"*"$slow"*"$other"* ]] || { echo "pipeline_run: run stop with two live runs said: $said"; return 1; }
  kanna task done "$task" >/dev/null || { echo "pipeline_run: the task with live runs couldn't be done"; return 1; }
  check wait_for 20 eval '[[ "$(runs state)" == "done done stopped stopped stopped" ]]' || { echo "pipeline_run: closing the task didn't stop its runs: $(runs state)"; return 1; }
  [[ "$(runs why)" == *"the task was closed the task was closed" ]] || { echo "pipeline_run: the closed task's runs say: $(runs why)"; return 1; }
  check wait_for 10 eval '[[ "$(where_is "$sleeper") $(where_is "$other_sleeper")" == "closed closed" ]]' || { echo "pipeline_run: the closed task's workers are $(where_is "$sleeper") $(where_is "$other_sleeper")"; return 1; }
  check wait_for 10 test ! -d "$slow_worker" || { echo "pipeline_run: the closed task's run kept its worktree"; return 1; }
  check test ! -d "$repo/.kanna-worktrees/$task-run-5" || { echo "pipeline_run: the closed task's second run kept its worktree"; return 1; }
  check test -n "$(git -C "$repo" branch --list "run/$task-4")" -a -n "$(git -C "$repo" branch --list "run/$task-5")" || { echo "pipeline_run: the closed task's run branches are gone"; return 1; }
}
