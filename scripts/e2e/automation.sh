scenario scheduler M4
scheduler() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "scheduler line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  machine_runners 1
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
  machine_runners 1
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
  check wait_for 10 eval '[[ "$(desk pages "$d")" == '"'"'["Desk", "Machines", "Builds", "Stats", "Schedules", "Merge queue"]'"'"' ]]' || { echo "schedules_tab: the Desk view's fixed tabs"; desk pages "$d"; return 1; }
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
  machine_runners 1
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

scenario pipeline_fanout M4
pipeline_fanout() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/fanout" fake="$KANNA_STATE_DIR/agents" host; host="$(hostname -s)"
  mkdir -p "$fake/bin" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/bin/claude"
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s KANNA_MOCK_REPORTS=1 exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$fake/bin/claude" >"$fake/claude" && chmod +x "$fake/claude"
  python3 -c 'import json, sys; p = sys.argv[1]; c = json.load(open(p)); c["pipelines"] = {"agents": {"claude": sys.argv[2]}}; json.dump(c, open(p, "w"))' "$KANNA_AGENTS_HOME/.kanna-v4/config.json" "$fake/claude" || { echo "pipeline_fanout line $LINENO"; return 1; }
  start_daemon && start_app || { echo "pipeline_fanout line $LINENO"; return 1; }
  mkdir -p "$repo/.kanna/pipelines" "$repo/.kanna/prompts" && git -C "$repo" init -q -b main || { echo "pipeline_fanout line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  git -C "$repo" config user.email e@e && git -C "$repo" config user.name e
  printf '{"version": 1, "pipelines": {"workers": 2}}\n' >"$repo/.kanna/config.json"
  machine_runners 1
  cat >"$repo/.kanna/pipelines/fanout.kojo" <<PIPE
budget = 12 sessions, 30m
plan   = claude on here
build  = claude each plan.parts
join   = merge build
fix    = claude
tidy   = claude guard score
best   = claude x2 by score
score  = run sh score.sh on $host

plan -> build -> join
join -[fail, max 1]-> fix -> join
join -> tidy -> best -> score -> done
PIPE
  printf 'far = run true on mini\nfar -> done\n' >"$repo/.kanna/pipelines/far.kojo"
  printf 'Plan the parts.\nmock: run cp plan-answer.md "$KANNA_ANSWER" && "$KANNA_CLI" run done\n' >"$repo/.kanna/prompts/plan.md"
  printf 'Build your part.\n' >"$repo/.kanna/prompts/build.md"
  printf 'Resolve the conflicts.\nmock: run printf "a\\nb\\n" > shared.txt && git add shared.txt && git commit -q --no-edit && echo Resolved > "$KANNA_ANSWER" && "$KANNA_CLI" run done\n' >"$repo/.kanna/prompts/fix.md"
  printf 'Tidy up.\nmock: run echo 1 > score.txt && git commit -qam tidy && echo Tidied > "$KANNA_ANSWER" && "$KANNA_CLI" run done\n' >"$repo/.kanna/prompts/tidy.md"
  printf 'Try it.\nmock: run if [ "$KANNA_INSTANCE" = attempt-1 ]; then n=3; else n=7; fi; echo $n > score.txt && echo "$KANNA_INSTANCE" > best.txt && git add -A && git commit -qm "$KANNA_INSTANCE" && echo "Tried $n" > "$KANNA_ANSWER" && "$KANNA_CLI" run done\n' >"$repo/.kanna/prompts/best.md"
  printf 'sed "s/^/score: /" score.txt\n' >"$repo/score.sh"
  printf 'base\n' >"$repo/shared.txt"; printf '5\n' >"$repo/score.txt"
  python3 -c 'import json, sys
part = lambda p: f"Part {p}\nmock: run mkdir -p \"$KANNA_REPO/.kanna/tmp\" && touch \"$KANNA_REPO/.kanna/tmp/building-$KANNA_INSTANCE\" && i=0; while [ $(ls \"$KANNA_REPO\"/.kanna/tmp/building-* | wc -l) -lt 2 ] && [ $i -lt 60 ]; do sleep 0.5; i=$((i+1)); done; sleep 4; echo {p} > shared.txt && echo {p} > {p}.txt && git add -A && git commit -qm {p} && echo \"Built {p}\" > \"$KANNA_ANSWER\" && \"$KANNA_CLI\" run done"
open(sys.argv[1], "w").write("Two parts.\n\n```json name=parts\n" + json.dumps([part("a"), part("b")]) + "\n```\n")' "$repo/plan-answer.md"
  git -C "$repo" add -A && git -C "$repo" commit -qm first || { echo "pipeline_fanout line $LINENO"; return 1; }
  local opened w; opened="$(kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "pipeline_fanout: kanna PATH didn't open the project's Desk: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  (cd "$repo" && kanna task new "Factory" "run $bin/kanna run start fanout --request 'Build both parts'" --agent "$fake/claude") >/dev/null || { echo "pipeline_fanout line $LINENO"; return 1; }
  local task; task="$(tid 1)"
  rows() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
t = json.load(sys.stdin); print(len(t["shelf"]), len(t.get("desk") or []))'; }
  held() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
print([t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0].get("held") or "-")' "$task"; }
  runs() { kanna run list --all --json | python3 -c 'import json, sys
r = json.load(sys.stdin)["runs"]; print(" ".join(x[sys.argv[1]] if isinstance(x[sys.argv[1]], str) else json.dumps(x[sys.argv[1]]) for x in r))' "$1"; }
  listed() { (cd "$repo" && kanna task list) | placed | wc -l | tr -d ' '; }
  check wait_for 20 eval '[[ "$(runs run)" == run-* ]]' || { echo "pipeline_fanout: the task's agent didn't start a run"; kanna read "$(held)"; tail -20 "$out/daemon.log"; return 1; }
  local run term; run="$(runs run)"; term="$(held)"
  local taskws; taskws="$(kanna workspace list --all --json | python3 -c 'import json, sys
print([w["id"] for w in json.load(sys.stdin)["workspaces"] if w["dock"]["id"] == sys.argv[1]][0])' "$term")" || { echo "pipeline_fanout: the task has no workspace"; return 1; }
  background() { kanna workspace list --all --json | python3 -c 'import json, sys
print(" ".join(sorted([w for w in json.load(sys.stdin)["workspaces"] if w["id"] == sys.argv[1]][0].get("background") or [])))' "$taskws"; }
  placed_worker() { kanna workspace list --all --json | python3 -c 'import json, sys
view = json.load(sys.stdin); ws, pane = sys.argv[1:3]
held = [w for w in view["workspaces"] if w["id"] == ws][0].get("background") or []
print("ok" if pane in held or pane not in [p["id"] for p in view.get("panes") or []] else "elsewhere")' "$taskws" "$1"; }
  workers() { sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from run_stages where run = '$run' and terminal is not null and ended_at is null" | sort | tr '\n' ' ' | sed 's/ $//'; }
  strip() { desk stripLayout | python3 -c 'import json, sys; print(" ".join(i["task"] for i in json.load(sys.stdin)["items"]))'; }
  local bad="" building="$repo/.kanna/tmp"
  on_shelf() {
    [[ "$(rows)" == "1 0" ]] || bad="the Shelf and Desk showed $(rows) rows"
    [[ "$(desk shelf)" == "[\"$task\"]" ]] || bad="the Desk view's Shelf showed $(desk shelf)"
    [[ "$(listed)" == 1 ]] || bad="kanna task list showed $(listed) rows"
    local w; for w in $(workers); do [[ "$(placed_worker "$w")" == ok ]] || bad="worker $w isn't in $taskws's background: $(background)"; done
    [[ -n "$bad" || ( -e "$building/building-part-a" && -e "$building/building-part-b" ) ]]
  }
  check wait_for 60 on_shelf || { echo "pipeline_fanout: the two parts never built at once"; kanna run show "$run"; ls "$building"; tail -30 "$out/daemon.log"; return 1; }
  [[ -z "$bad" ]] || { echo "pipeline_fanout: on the Shelf, $bad"; return 1; }
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*) from run_stages where run = '$run' and stage = 'build' and ended_at is null and terminal is not null")" -eq 2 || { echo "pipeline_fanout: two build workers aren't open at once"; kanna run show "$run"; return 1; }
  local part; for part in a b; do
    check test "$(git -C "$repo/.kanna-worktrees/$task-run-1-part-$part" branch --show-current)" == "run/$task-1-part-$part" || { echo "pipeline_fanout: no worker for part $part"; return 1; }
  done
  grep -q "# Your part" "$repo/.kanna-worktrees/$task-run-1-part-a/.kanna/tmp/stage-2.md" && grep -q "^Part a$" "$repo/.kanna-worktrees/$task-run-1-part-a/.kanna/tmp/stage-2.md" || { echo "pipeline_fanout: part a's prompt doesn't give it its part"; cat "$repo/.kanna-worktrees/$task-run-1-part-a/.kanna/tmp/stage-2.md"; return 1; }
  kanna task pick "$task" >/dev/null || { echo "pipeline_fanout: the running task couldn't go to the Desk"; return 1; }
  (cd "$repo" && kanna task new "Other" --agent "exec \$SHELL #") >/dev/null || { echo "pipeline_fanout line $LINENO"; return 1; }
  check wait_for 10 eval 'kanna task pick "$(tid 2)" >/dev/null' || { echo "pipeline_fanout: the other task couldn't go to the Desk"; return 1; }
  check wait_for 10 eval 'kanna task activate "$(tid 2)" >/dev/null' || { echo "pipeline_fanout: the other task couldn't be made active"; return 1; }
  dock() { kanna workspace list --all --json | python3 -c 'import json, sys
print([w for w in json.load(sys.stdin)["workspaces"] if w["id"] == sys.argv[1]][0]["dock"]["id"])' "$taskws"; }
  check wait_for 10 eval '[[ "$(strip)" == "$(dock)" ]]' || { echo "pipeline_fanout: the strip shows $(strip), not the task"; desk stripLayout; return 1; }
  on_desk() {
    [[ "$(rows)" == "0 2" ]] || bad="the Shelf and Desk showed $(rows) rows"
    [[ "$(desk shelf)" == "[]" ]] || bad="the Desk view's Shelf showed $(desk shelf)"
    [[ "$(desk rows | python3 -c 'import json, sys; print(len(json.load(sys.stdin)))')" == 2 ]] || bad="the Desk view showed rows $(desk rows)"
    [[ "$(strip)" == "$(dock)" ]] || bad="the strip showed $(strip)"
    [[ "$(listed)" == 2 ]] || bad="kanna task list showed $(listed) rows"
    local w; for w in $(workers); do [[ "$(placed_worker "$w")" == ok ]] || bad="worker $w isn't in $taskws's background: $(background)"; done
    local f; f="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select json_extract(doc, '$.prompt') from run_stages where run = '$run' and stage = 'fix'")"
    [[ -n "$f" && -f "$f" && ! -e "$KANNA_STATE_DIR/fix.md" ]] && cp "$f" "$KANNA_STATE_DIR/fix.md"
    [[ -n "$bad" || "$(runs state)" != running ]]
  }
  check wait_for 120 on_desk || { echo "pipeline_fanout: the run didn't end"; kanna run show "$run"; tail -40 "$out/daemon.log"; return 1; }
  [[ -z "$bad" ]] || { echo "pipeline_fanout: on the Desk, $bad"; kanna run show "$run"; return 1; }
  check test "$(runs state)" == done || { echo "pipeline_fanout: the run ended $(runs state): $(runs why)"; kanna run show "$run"; tail -40 "$out/daemon.log"; return 1; }
  local shown; shown="$(kanna run show "$run" --json)"
  python3 -c 'import json, sys
v = json.loads(sys.argv[1])["visits"]
got = [(x["stage"], x["instance"], x["kind"], x["verdict"]) for x in v]
want = [("plan", None, "agent", "done"), ("build", "part-a", "agent", "done"), ("build", "part-b", "agent", "done"), ("join", None, "merge", "fail"), ("fix", None, "agent", "done"), ("join", None, "merge", "pass"),
        ("tidy", "before", "score", "pass"), ("tidy", None, "agent", "done"), ("tidy", "after", "score", "pass"),
        ("best", "attempt-1", "agent", "done"), ("best", "attempt-2", "agent", "done"), ("best", "attempt-1", "score", "pass"), ("best", "attempt-2", "score", "pass"), ("score", None, "run", "pass")]
assert sorted(got[1:3], key=str) == want[1:3] and sorted(got[9:13], key=str) == sorted(want[9:13], key=str) and got[:1] + got[3:9] + got[13:] == want[:1] + want[3:9] + want[13:], got
a, b = v[1], v[2]
assert a["startedAt"] < b["endedAt"] and b["startedAt"] < a["endedAt"], "the parts did not build at once"
assert v[3]["doc"]["conflicts"] == ["shared.txt"] and v[3]["doc"]["joined"] == ["part-a"], v[3]["doc"]
assert v[5]["doc"]["joined"] == ["part-b"], v[5]["doc"]
assert v[7]["doc"]["guard"] == {"stage": "score", "before": 5.0, "after": 1.0, "rolledBack": True}, v[7]["doc"]
scores = {x["instance"]: (x["doc"]["score"], x["doc"]["won"]) for x in v if x["stage"] == "best" and x["kind"] == "agent"}
assert scores == {"attempt-1": (3.0, False), "attempt-2": (7.0, True)}, scores
assert all(x["machine"] == sys.argv[2] for x in v), [x["machine"] for x in v]' "$shown" "$host" || { echo "pipeline_fanout: the visits were: $shown"; return 1; }
  grep -q "^# Feedback from join$" "$KANNA_STATE_DIR/fix.md" && grep -q "^- shared.txt$" "$KANNA_STATE_DIR/fix.md" || { echo "pipeline_fanout: fix wasn't told what conflicts"; cat "$KANNA_STATE_DIR/fix.md"; return 1; }
  local worker="$repo/.kanna-worktrees/$task-run-1"
  check test ! -d "$worker" || { echo "pipeline_fanout: the run's worktree is still there"; return 1; }
  local branch="run/$task-1"
  check test "$(git -C "$repo" show "$branch:shared.txt")" == $'a\nb' || { echo "pipeline_fanout: the conflict's fix isn't on the run's branch"; git -C "$repo" show "$branch:shared.txt"; return 1; }
  check test "$(git -C "$repo" show "$branch:a.txt")$(git -C "$repo" show "$branch:b.txt")" == ab || { echo "pipeline_fanout: both parts aren't on the run's branch"; return 1; }
  [[ "$(git -C "$repo" log --format=%s "$branch" | grep '^Merge build')" == $'Merge build · part-b\nMerge build · part-a' ]] || { echo "pipeline_fanout: the parts weren't joined in item order"; git -C "$repo" log --format=%s "$branch"; return 1; }
  for part in a b; do
    check test ! -d "$repo/.kanna-worktrees/$task-run-1-part-$part" -a -z "$(git -C "$repo" branch --list "run/$task-1-part-$part")" || { echo "pipeline_fanout: joined part $part's worktree or branch is still there"; return 1; }
  done
  check test "$(git -C "$repo" log --format=%s "$branch" | grep -c '^tidy$')" -eq 0 || { echo "pipeline_fanout: the guard didn't roll tidy back"; git -C "$repo" log --format=%s "$branch"; return 1; }
  check test "$(git -C "$repo" log -1 --format=%s "$branch-guard-1")" == tidy || { echo "pipeline_fanout: tidy's rolled-back work isn't kept"; git -C "$repo" branch; return 1; }
  check test "$(git -C "$repo" show "$branch:best.txt")$(git -C "$repo" show "$branch:score.txt")" == attempt-27 || { echo "pipeline_fanout: the best attempt isn't the one kept"; git -C "$repo" log --format=%s "$branch"; return 1; }
  check test -z "$(git -C "$repo" branch --list "run/$task-1-attempt-*")" -a -z "$(ls -d "$repo/.kanna-worktrees/$task-run-1-attempt-"* 2>/dev/null)" || { echo "pipeline_fanout: the attempts' worktrees or branches are still there"; git -C "$repo" branch; return 1; }
  check test "$(rows)" == "0 2" -a "$(listed)" == 2 || { echo "pipeline_fanout: $(rows) rows after the run"; return 1; }
  [[ -z "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select 1 from run_stages where run = '$run' and ended_at is null")" ]] || { echo "pipeline_fanout: a visit is still open"; return 1; }
  local folder="$repo/.kanna-worktrees/$task" said
  said="$(cd "$folder" && KANNA_TERMINAL="$(dock)" kanna run start far 2>&1)" && { echo "pipeline_fanout: a stage on another machine started: $said"; return 1; }
  [[ "$said" == *"far.kojo:1: far is on mini: no such machine"* ]] || { echo "pipeline_fanout: a stage on another machine said: $said"; return 1; }
  check test "$(kanna run list --all --json | python3 -c 'import json, sys; print(len(json.load(sys.stdin)["runs"]))')" -eq 1 || { echo "pipeline_fanout: the task started another run"; kanna run list --all; return 1; }
}

scenario pipeline_tab M4
pipeline_tab() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/tabbed" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$fake/bin" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/bin/claude"
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$fake/bin/claude" >"$fake/claude" && chmod +x "$fake/claude"
  python3 -c 'import json, sys; p = sys.argv[1]; c = json.load(open(p)); c["pipelines"] = {"agents": {"claude": sys.argv[2]}}; json.dump(c, open(p, "w"))' "$KANNA_AGENTS_HOME/.kanna-v4/config.json" "$fake/claude" || { echo "pipeline_tab line $LINENO"; return 1; }
  start_daemon && start_app || { echo "pipeline_tab line $LINENO"; return 1; }
  mkdir -p "$repo/.kanna/pipelines" "$repo/.kanna/prompts" && git -C "$repo" init -q -b main || { echo "pipeline_tab line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  git -C "$repo" config user.email e@e && git -C "$repo" config user.name e
  machine_runners 1
  cat >"$repo/.kanna/pipelines/demo.kojo" <<'PIPE'
budget = 6 sessions, 30m
build  = claude
check  = run sh check.sh
build -> check
check -[fail, max 2]-> build
check -> done
PIPE
  cat >"$repo/.kanna/prompts/build.md" <<'PROMPT'
Build the change.
mock: run echo visit >> built.txt && git add built.txt && git commit -qm build && echo 'Built the counter.' >"$KANNA_ANSWER" && "$KANNA_CLI" run done
PROMPT
  cat >"$repo/check.sh" <<CHECK
n=\$(cat .kanna/tmp/checks 2>/dev/null || echo 0); n=\$((n + 1)); echo "\$n" >.kanna/tmp/checks
until [ -e "$KANNA_STATE_DIR/go\$n" ]; do sleep 0.2; done
[ "\$n" -eq 1 ] && { echo "FAILED: counter_counts"; exit 1; }
echo ok
CHECK
  printf 'needy = claude\nneedy -> done\n' >"$repo/.kanna/pipelines/needy.kojo"
  printf 'Ask first.\nmock: run "$KANNA_CLI" run need "Which colour?"\n' >"$repo/.kanna/prompts/needy.md"
  printf 'wait = run sleep 300\nwait -> done\n' >"$repo/.kanna/pipelines/slow.kojo"
  printf 'ok = ask "Ship it?"\nok -> done\n' >"$repo/.kanna/pipelines/asky.kojo"
  git -C "$repo" add -A && git -C "$repo" commit -qm first || { echo "pipeline_tab line $LINENO"; return 1; }
  (cd "$repo" && kanna task new "Counter" "run $bin/kanna run start demo --request 'Count the visits'" --agent "$fake/claude") >/dev/null || { echo "pipeline_tab line $LINENO"; return 1; }
  local task; task="$(tid 1)"
  local folder="$repo/.kanna-worktrees/$task"
  runs() { kanna run list --all --json | python3 -c 'import json, sys
r = json.load(sys.stdin)["runs"]; print(" ".join(x[sys.argv[1]] if isinstance(x[sys.argv[1]], str) else json.dumps(x[sys.argv[1]]) for x in r))' "$1"; }
  check wait_for 20 eval '[[ "$(runs run)" == run-* ]]' || { echo "pipeline_tab: the task's agent didn't start a run"; tail -20 "$out/daemon.log"; return 1; }
  local run term; run="$(runs run)"
  term="$(raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
print([t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0].get("held") or "-")' "$task")"
  agent_of() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
l = json.load(sys.stdin)
print(next((t.get("held") or t.get("pane") for t in (l["shelf"] + (l.get("desk") or [])) if t["id"] == sys.argv[1] and (t.get("held") or t.get("pane"))), "-"))' "$task"; }
  here() { (cd "$folder" && KANNA_TERMINAL="$(agent_of)" kanna "$@"); }
  local opened; opened="$(here open run)" || { echo "pipeline_tab: kanna open run failed: $opened"; return 1; }
  [[ "$opened" =~ ^opened\ (runview-[0-9]+)\ in\ .*\ tab-[0-9]+:\ running$ ]] || { echo "pipeline_tab: kanna open run said: $opened"; return 1; }
  local tab="${BASH_REMATCH[1]}"
  local taskws; taskws="$(kanna workspace list --all --json | python3 -c 'import json, sys
print([w["id"] for w in json.load(sys.stdin)["workspaces"] if w["dock"]["id"] == sys.argv[1]][0])' "$term")"
  in_tab() { kanna workspace list --all --json | python3 -c 'import json, sys
ws = [w for w in json.load(sys.stdin)["workspaces"] if w["id"] == sys.argv[1]][0]
print(next((t["id"] for t in ws["tabs"] if sys.argv[2] in t["panes"]), "-"))' "$taskws" "$1"; }
  check test "$(in_tab "$tab")" != - || { echo "pipeline_tab: the Run tab isn't a tab of the task's workspace"; return 1; }
  shown() { raw "{\"id\":1,\"cmd\":\"debug.run\",\"args\":{\"pane\":\"$tab\"${2:-}}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(shown run)" == "$run" ]]' || { echo "pipeline_tab: the Run tab shows no run"; raw "{\"id\":1,\"cmd\":\"debug.run\",\"args\":{\"pane\":\"$tab\"}}"; return 1; }
  python3 -c 'import json, sys; assert json.loads(sys.argv[1]) == ["agent session", "command", "passed", "failed", "running", "waiting on you", "\u00d7N visits"]' "$(shown legend)" || { echo "pipeline_tab: the legend is $(shown legend)"; return 1; }
  check test "$(shown nodes | python3 -c 'import json, sys; print(" ".join(n["id"] for n in json.load(sys.stdin)))')" == "build check done" || { echo "pipeline_tab: the graph is $(shown nodes)"; return 1; }
  check wait_for 20 eval '[[ "$(shown badge)" == 2/2 ]]' || { echo "pipeline_tab: the badge while check runs is $(shown badge)"; return 1; }
  [[ "$(cd "$repo" && kanna task list)" == *"Counter  "*" · run: check 2/2"* ]] || { echo "pipeline_tab: task list doesn't say the run's stage"; (cd "$repo" && kanna task list); return 1; }
  local tiles; tiles="$(shown tiles)"
  python3 -c 'import json, sys
t = dict((l.split(" ")[0], v) for v, l in json.loads(sys.argv[1]))
assert t["agent"] == "—" and t["tokens"] == "—", ("turns and tokens are not recorded yet", t)
assert t["rework"] == "0" and t["your"] == "0", t' "$tiles" || { echo "pipeline_tab: the tiles were $tiles"; return 1; }
  check test "$(shown enabled.watch)" == True -a "$(shown enabled.stop)" == True || { echo "pipeline_tab: the buttons are $(shown enabled)"; return 1; }
  workers() { sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from run_stages where run = '$run' and terminal is not null order by id" | tr '\n' ' '; }
  (cd "$repo" && kanna task pick "$task") >/dev/null || { echo "pipeline_tab line $LINENO"; return 1; }
  local w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "pipeline_tab line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  (cd "$repo" && kanna task activate "$task") >/dev/null || { echo "pipeline_tab line $LINENO"; return 1; }
  local first; first="$(workers | awk '{print $2}')"
  local watched; watched="$(here open run --watch)" || { echo "pipeline_tab: kanna open run --watch failed: $watched"; return 1; }
  [[ "$watched" == "$first shows $run's worker in "*"; the tab follows the run to the next stage" ]] || { echo "pipeline_tab: --watch said: $watched"; return 1; }
  local watch_tab; watch_tab="$(in_tab "$first")"
  check test "$watch_tab" != - || { echo "pipeline_tab: the watched worker isn't in a tab"; return 1; }
  check test "$(ws "$taskws" focused)" == "$first" || { echo "pipeline_tab: the watched worker isn't focused"; return 1; }
  touch "$KANNA_STATE_DIR/go1"
  check wait_for 30 eval '[[ "$(workers | wc -w | tr -d " ")" == 4 ]]' || { echo "pipeline_tab: the second check never started"; kanna run show "$run"; return 1; }
  local second third; second="$(workers | awk '{print $3}')"; third="$(workers | awk '{print $4}')"
  check wait_for 10 eval '[[ "$(in_tab "$third")" == "$watch_tab" ]]' || { echo "pipeline_tab: the next stage's worker $third isn't in the watched tab $watch_tab: $(in_tab "$third")"; kanna workspace list --all --json; tail -20 "$out/daemon.log"; return 1; }
  check test "$(in_tab "$first")" == - -a "$(in_tab "$second")" == - || { echo "pipeline_tab: earlier workers are still in tabs"; return 1; }
  grep -q "next worker $second takes the tab $first was shown in" "$out/daemon.log" || { echo "pipeline_tab: build's worker didn't take the watched tab"; tail -30 "$out/daemon.log"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"pane\":\"$third\",\"text\":\" \"}}" >/dev/null
  check wait_for 10 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*) from run_events where run = '"'$run'"' and what = '"'typed'"'")" == 1 ]]' || { echo "pipeline_tab: typing into the worker wasn't recorded"; return 1; }
  check wait_for 10 eval '[[ "$(shown tiles | python3 -c "import json, sys; print(json.load(sys.stdin)[5][0])")" == 1 ]]' || { echo "pipeline_tab: the interventions tile is $(shown tiles)"; return 1; }
  check test "$(shown tiles | python3 -c 'import json, sys; print(json.load(sys.stdin)[4][0])')" == 1 || { echo "pipeline_tab: one fail arrow was taken, but rework loops is $(shown tiles)"; return 1; }
  python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
assert [r[1] for r in rows] == ["build", "check", "build ·2", "check ·2"], rows
assert [r[4] for r in rows] == ["—", "fail", "—", "running"], rows
assert rows[0][7] == "Built the counter." and rows[1][7] == "FAILED: counter_counts", rows
assert rows[0][5] == "—" and rows[0][6] == "—" and rows[1][5] == "", rows' "$(shown rows)" || { echo "pipeline_tab: the stages table is $(shown rows)"; return 1; }
  check test "$(shown workers | python3 -c 'import json, sys; print(json.load(sys.stdin)[0].split(" · ")[:3])')" == "['check ·2', 'check.sh', '$third']" || { echo "pipeline_tab: the workers are $(shown workers)"; return 1; }
  kanna focus "$tab" >/dev/null || { echo "pipeline_tab line $LINENO"; return 1; }
  sleep 1; snap "$w" pipeline_tab-run >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 1; snap "$w" pipeline_tab-run-light >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  touch "$KANNA_STATE_DIR/go2"
  check wait_for 30 eval '[[ "$(runs state)" == done ]]' || { echo "pipeline_tab: the run didn't hand back"; kanna run show "$run"; return 1; }
  check wait_for 10 eval '[[ "$(shown badge)" == done ]]' || { echo "pipeline_tab: the badge after hand-back is $(shown badge)"; return 1; }
  check test "$(shown enabled.stop) $(shown enabled.watch)" == "False False" || { echo "pipeline_tab: a handed-back run's buttons are $(shown enabled)"; return 1; }
  check wait_for 10 eval '[[ "$(in_tab "$third")" == - ]]' || { echo "pipeline_tab: the last worker is still shown"; return 1; }
  [[ "$(cd "$repo" && kanna task list)" != *"run:"* ]] || { echo "pipeline_tab: task list still says a run after it ended"; return 1; }

  local again; again="$(here run start needy 2>&1)" || { echo "pipeline_tab: the needy run didn't start: $again"; return 1; }
  local needy="${again%% *}"
  opened="$(here open run "$needy")" && [[ "$opened" =~ ^opened\ (runview-[0-9]+) ]] || { echo "pipeline_tab: open run RUN said: $opened"; return 1; }
  tab="${BASH_REMATCH[1]}"
  check wait_for 30 eval '[[ "$(shown card)" == "needy needs you: Which colour?" && "$(shown cardShown)" == True ]]' || { echo "pipeline_tab: the paused run shows no card: $(shown card)"; return 1; }
  check test "$(shown badge)" == 1/1 || { echo "pipeline_tab: the paused badge is $(shown badge)"; return 1; }
  kanna focus "$tab" >/dev/null && sleep 1 && snap "$w" pipeline_tab-gate >/dev/null
  local note; note="$(python3 -c 'import json; print(json.dumps({"pane": "'"$tab"'", "press": "resume", "note": "mock: run \"$KANNA_CLI\" run done"}))')"
  resume_card() { raw "{\"id\":1,\"cmd\":\"debug.run\",\"args\":$note}" >/dev/null; sleep 1; [[ "$(shown flash)" == None ]]; }
  check wait_for 20 resume_card || { echo "pipeline_tab: Resume on the card was refused: $(shown flash)"; return 1; }
  check wait_for 30 eval '[[ "$(runs state)" == "done done" ]]' || { echo "pipeline_tab: the resumed run didn't hand back: $(runs state) $(runs why)"; return 1; }
  [[ "$(kanna run log "$needy" --json | python3 -c 'import json, sys; print([e["by"] for e in json.load(sys.stdin)["events"] if e["what"] == "resumed"])')" == "['user']" ]] || { echo "pipeline_tab: the card's answer wasn't recorded as yours"; kanna run log "$needy"; return 1; }

  again="$(here run start asky 2>&1)" || { echo "pipeline_tab: the asky run didn't start: $again"; return 1; }
  local asky="${again%% *}"
  opened="$(here open run "$asky")" && [[ "$opened" =~ ^opened\ (runview-[0-9]+) ]] || { echo "pipeline_tab: open run said: $opened"; return 1; }
  tab="${BASH_REMATCH[1]}"
  check wait_for 30 eval '[[ "$(shown card)" == "Ship it?" && "$(shown cardShown)" == True ]]' || { echo "pipeline_tab: the ask shows no card: $(shown card)"; return 1; }
  note="$(python3 -c 'import json; print(json.dumps({"pane": "'"$tab"'", "press": "yes", "note": "Looks right."}))')"
  check wait_for 20 resume_card || { echo "pipeline_tab: Yes on the card was refused: $(shown flash)"; return 1; }
  check wait_for 30 eval '[[ "$(runs state)" == "done done done" ]]' || { echo "pipeline_tab: the answered run didn't hand back: $(runs state) $(runs why)"; return 1; }
  [[ "$(kanna run log "$asky" --json | python3 -c 'import json, sys; print([(e["by"], e["doc"]["answer"], e["doc"]["note"]) for e in json.load(sys.stdin)["events"] if e["what"] == "answered"])')" == "[('user', 'yes', 'Looks right.')]" ]] || { echo "pipeline_tab: the card's yes wasn't recorded as yours"; kanna run log "$asky"; return 1; }

  again="$(here run start slow 2>&1)" || { echo "pipeline_tab: the slow run didn't start: $again"; return 1; }
  local slow="${again%% *}"
  opened="$(here open run "$slow")" && [[ "$opened" =~ ^opened\ (runview-[0-9]+) ]] || { echo "pipeline_tab: open run said: $opened"; return 1; }
  tab="${BASH_REMATCH[1]}"
  check wait_for 20 eval '[[ "$(shown enabled.stop)" == True ]]' || { echo "pipeline_tab: the slow run's Stop is $(shown enabled.stop)"; return 1; }
  check test "$(shown flash ',"press":"pause"')" == None || { echo "pipeline_tab: Pause was refused: $(shown flash)"; return 1; }
  check wait_for 10 eval '[[ "$(kanna run log "$slow" --json | python3 -c '"'"'import json, sys; print([e["by"] for e in json.load(sys.stdin)["events"] if e["what"] == "paused"])'"'"')" == "['"'"'user'"'"']" ]]' || { echo "pipeline_tab: Pause wasn't recorded as yours"; kanna run log "$slow"; return 1; }
  shown state ',"press":"stop"' >/dev/null
  check wait_for 10 eval '[[ "$(runs state)" == "done done done stopped" ]]' || { echo "pipeline_tab: Stop on the Run tab didn't stop the run: $(runs state)"; return 1; }
  [[ "$(kanna run log "$slow" --json | python3 -c 'import json, sys; print([e["by"] for e in json.load(sys.stdin)["events"] if e["what"] == "stopped"])')" == "['user']" ]] || { echo "pipeline_tab: the Stop wasn't recorded as yours"; return 1; }
  check wait_for 10 eval '[[ "$(shown state)" == stopped ]]' || { echo "pipeline_tab: the stopped run's tab says $(shown state)"; return 1; }
}

scenario pipeline_gates M4
pipeline_gates() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/gates" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$fake/bin" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/bin/claude"
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$fake/bin/claude" >"$fake/claude" && chmod +x "$fake/claude"
  python3 -c 'import json, sys; p = sys.argv[1]; c = json.load(open(p)); c["pipelines"] = {"agents": {"claude": sys.argv[2]}}; json.dump(c, open(p, "w"))' "$KANNA_AGENTS_HOME/.kanna-v4/config.json" "$fake/claude" || { echo "pipeline_gates line $LINENO"; return 1; }
  start_daemon && start_app || { echo "pipeline_gates line $LINENO"; return 1; }
  mkdir -p "$repo/.kanna/pipelines" "$repo/.kanna/prompts" && git -C "$repo" init -q -b main || { echo "pipeline_gates line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  git -C "$repo" config user.email e@e && git -C "$repo" config user.name e
  machine_runners 1
  cat >"$repo/.kanna/pipelines/gates.kojo" <<'PIPE'
plan  = claude
ok    = ask "Is the plan right?"
build = claude
check = run sh check.sh
wrap  = run touch .kanna/tmp/wrapped

plan -> ok
ok -[fail, max 2]-> plan
ok -> build -> check -> wrap -> done
PIPE
  printf 'Plan it.\nmock: run sleep 6 && echo plan >> plan.txt && git add plan.txt && git commit -qm plan && "$KANNA_CLI" run done\n' >"$repo/.kanna/prompts/plan.md"
  printf 'Build it.\nsay: Not sure how.\n' >"$repo/.kanna/prompts/build.md"
  cat >"$repo/check.sh" <<'CHECK'
n=$(cat .kanna/tmp/checks 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" >.kanna/tmp/checks
touch ".kanna/tmp/check-$n"
while [ ! -e .kanna/tmp/go ]; do sleep 0.2; done
CHECK
  printf 'work = claude\ntail = run touch .kanna/tmp/tailed\nwork -> tail -> done\n' >"$repo/.kanna/pipelines/slowpoke.kojo"
  printf 'Work slowly.\nmock: run sleep 60\n' >"$repo/.kanna/prompts/work.md"
  git -C "$repo" add -A && git -C "$repo" commit -qm first || { echo "pipeline_gates line $LINENO"; return 1; }
  (cd "$repo" && kanna task new "Gates" "run $bin/kanna run start gates --request 'Gate it'" --agent "$fake/claude") >/dev/null || { echo "pipeline_gates line $LINENO"; return 1; }
  local task; task="$(tid 1)"
  task_json() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
t = json.load(sys.stdin); task = [x for x in t["shelf"] if x["id"] == sys.argv[1]]
print(json.dumps(task[0] if task else {}))' "$task"; }
  task_field() { task_json | python3 -c 'import json, sys; v = json.load(sys.stdin).get(sys.argv[1]); print(v if isinstance(v, str) else json.dumps(v))' "$1"; }
  runs() { kanna run list --all --json | python3 -c 'import json, sys
r = json.load(sys.stdin)["runs"]; print(" ".join(x[sys.argv[1]] if isinstance(x[sys.argv[1]], str) else json.dumps(x[sys.argv[1]]) for x in r))' "$1"; }
  visits() { kanna run show "$1" --json | python3 -c 'import json, sys
print(" ".join(v["stage"] + ":" + (v["verdict"] or "open") + ("(" + v["doc"]["by"] + ")" if v["doc"].get("by") else "") for v in json.load(sys.stdin)["visits"]))'; }
  check wait_for 20 eval '[[ "$(runs run)" == run-* ]]' || { echo "pipeline_gates: the task's agent didn't start a run"; (cd "$repo" && kanna task list); task_json; kanna read "$(task_field held)"; return 1; }
  local run worker; run="$(runs run)"; worker="$repo/.kanna-worktrees/$task-run-1"
  check wait_for 15 eval '[[ "$(task_field running) $(runs state)" == "false running" ]]' || { echo "pipeline_gates: the Shelf runner didn't stop the task's agent while the run went on: $(task_field running) $(runs state)"; return 1; }
  check wait_for 30 eval '[[ "$(runs state)" == waiting ]]' || { echo "pipeline_gates: the ask didn't make the run wait: $(runs state) $(runs why)"; kanna run show "$run"; tail -20 "$out/daemon.log"; return 1; }
  [[ "$(runs why)" == "ok asks: Is the plan right?" ]] || { echo "pipeline_gates: the waiting run says: $(runs why)"; return 1; }
  check wait_for 30 eval 'shows "$(task_field held)" "peer: \[message from $run\] ok asks: Is the plan right?"' || { echo "pipeline_gates: the run's ask didn't wake the stopped task with its question"; kanna read "$(task_field held)"; (cd "$repo" && kanna task messages "$task"); return 1; }
  check wait_for 10 eval '[[ "$(task_field status)" == needs-you ]]' || { echo "pipeline_gates: the asking task's status is $(task_field status)"; return 1; }
  check wait_for 20 eval '[[ "$(task_field running) $(task_field status)" == "false needs-you" ]]' || { echo "pipeline_gates: the stopped asking task is $(task_field running) $(task_field status)"; return 1; }
  (cd "$repo" && kanna task list) | grep -q "Gates .*needs you" || { echo "pipeline_gates: task list doesn't say the task needs you"; (cd "$repo" && kanna task list); return 1; }
  stop_daemon; start_daemon || { echo "pipeline_gates: the daemon didn't come back"; return 1; }
  check test "$(runs state)" == waiting || { echo "pipeline_gates: the ask didn't survive a restart: $(runs state)"; return 1; }
  check wait_for 10 eval '[[ "$(task_field status)" == needs-you ]]' || { echo "pipeline_gates: after a restart the asking task is $(task_field status)"; return 1; }
  local said; said="$(kanna run answer "$run" no "Split it in two." 2>&1)" || { echo "pipeline_gates: answering from the CLI failed: $said"; return 1; }
  [[ "$said" == "$run: ok is answered no" ]] || { echo "pipeline_gates: answer said: $said"; return 1; }
  check wait_for 10 eval '[[ "$(task_field status)" != needs-you ]]' || { echo "pipeline_gates: the task still needs you after the answer"; return 1; }
  check wait_for 30 eval 'grep -q "Split it in two." "$worker/.kanna/tmp/stage-3.md" 2>/dev/null' || { echo "pipeline_gates: plan's second visit didn't get the note"; cat "$worker/.kanna/tmp/stage-3.md"; return 1; }
  grep -q "# Feedback from ok" "$worker/.kanna/tmp/stage-3.md" || { echo "pipeline_gates: the note isn't under ok's feedback heading"; cat "$worker/.kanna/tmp/stage-3.md"; return 1; }
  check wait_for 30 eval '[[ "$(runs state)" == waiting ]]' || { echo "pipeline_gates: the second ask didn't come: $(runs state)"; return 1; }
  local picked; picked="$(kanna task pick "$task")" || { echo "pipeline_gates: the task couldn't go to the Desk: $picked"; return 1; }
  [[ "$picked" =~ (term-[0-9]+) ]] || { echo "pipeline_gates: pick said: $picked"; return 1; }
  local agent="${BASH_REMATCH[1]}"
  check wait_for 15 is "$agent" state running || { echo "pipeline_gates: the task's agent didn't start on the Desk"; return 1; }
  check wait_for 15 shows "$agent" "❯" || { echo "pipeline_gates: the task's agent isn't ready"; kanna read "$agent"; return 1; }
  kanna send "$agent" "run $bin/kanna run answer $run yes Go ahead." >/dev/null || { echo "pipeline_gates line $LINENO"; return 1; }
  check wait_for 20 eval '[[ "$(runs why)" == *"build stopped without reporting"* ]]' || { echo "pipeline_gates: the agent's answer didn't carry the run to build: $(runs state) $(runs why)"; kanna read "$agent"; kanna run log "$run"; return 1; }
  steer() { said="$(kanna run "$1" "$run" 2>&1)"; }
  check wait_for 20 steer skip || { echo "pipeline_gates: skip failed: $said"; return 1; }
  [[ "$said" == "$run: build is skipped, as a pass" ]] || { echo "pipeline_gates: skip said: $said"; return 1; }
  check wait_for 20 test -e "$worker/.kanna/tmp/check-1" || { echo "pipeline_gates: check didn't start after the skip: $(runs state) $(runs why)"; return 1; }
  said="$(kanna run retry "$run" 2>&1)" || { echo "pipeline_gates: retry failed: $said"; return 1; }
  check wait_for 20 test -e "$worker/.kanna/tmp/check-2" || { echo "pipeline_gates: check didn't run again: $said"; kanna run show "$run"; return 1; }
  said="$(kanna run pause "$run" 2>&1)" || { echo "pipeline_gates: pause failed: $said"; return 1; }
  [[ "$said" == "$run pauses once check ends; \`kanna run resume\` carries it on" ]] || { echo "pipeline_gates: pause said: $said"; return 1; }
  touch "$worker/.kanna/tmp/go"
  check wait_for 20 eval '[[ "$(runs state)" == paused ]]' || { echo "pipeline_gates: the run didn't pause after check: $(runs state) $(runs why)"; return 1; }
  [[ "$(runs why)" == "paused by you before wrap" ]] || { echo "pipeline_gates: the paused run says: $(runs why)"; return 1; }
  sleep 2
  check test ! -e "$worker/.kanna/tmp/wrapped" || { echo "pipeline_gates: a paused run started its next stage"; return 1; }
  said="$(kanna run resume "$run" 2>&1)" || { echo "pipeline_gates: resume failed: $said"; return 1; }
  check wait_for 20 eval '[[ "$(runs state)" == done ]]' || { echo "pipeline_gates: the resumed run didn't hand back: $(runs state) $(runs why)"; return 1; }
  check test "$(git -C "$repo" show "run/$task-1:plan.txt" | grep -c plan)" -eq 2 || { echo "pipeline_gates: the run's branch doesn't have both plans"; return 1; }
  local got; got="$(visits "$run")"
  [[ "$got" == "plan:done ok:fail(user) plan:done ok:pass(agent) build:skipped(user) check:retried check:pass wrap:pass" ]] || { echo "pipeline_gates: the visits were: $got"; return 1; }
  python3 -c 'import json, sys
r = json.loads(sys.argv[1])["run"]
assert r["interventions"] == {"answered": 2, "skipped": 1, "retried": 1, "paused": 1, "resumed": 1}, r["interventions"]' "$(kanna run show "$run" --json)" || { echo "pipeline_gates: the interventions counted"; return 1; }
  local events; events="$(kanna run log "$run" --json | python3 -c 'import json, sys; print(" ".join(e["what"] + ":" + (e["by"] or "-") for e in json.load(sys.stdin)["events"] if e["what"] in ("answered", "skipped", "retried", "paused", "resumed")))')"
  [[ "$events" == "answered:user answered:agent paused:daemon skipped:user retried:user paused:user resumed:user" ]] || { echo "pipeline_gates: the steering events were: $events"; return 1; }
  kanna run show "$run" | grep -q "interventions: 2 answered" || { echo "pipeline_gates: run show doesn't list the interventions"; kanna run show "$run"; return 1; }
  local folder="$repo/.kanna-worktrees/$task" second
  second="$(cd "$folder" && KANNA_TERMINAL="$agent" kanna run start slowpoke 2>&1)" || { echo "pipeline_gates: the slowpoke run didn't start: $second"; return 1; }
  second="${second%% *}"
  local slow_worker="$repo/.kanna-worktrees/$task-run-2" sleeper=""
  working() { sleeper="$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select terminal from run_stages where run = '$second' and stage = 'work' and terminal is not null order by rowid desc limit 1")"; [[ -n "$sleeper" ]] && shows "$sleeper" "session "; }
  check wait_for 20 working || { echo "pipeline_gates: the work stage didn't start"; return 1; }
  stop_daemon; start_daemon || { echo "pipeline_gates: the daemon didn't come back mid-stage"; return 1; }
  check wait_for 10 is "$sleeper" state running || { echo "pipeline_gates: the stage's agent didn't outlive the daemon"; return 1; }
  check test "$(runs state)" == "done running" || { echo "pipeline_gates: after a restart mid-stage the runs are $(runs state)"; return 1; }
  kanna task stop "$task" >/dev/null || { echo "pipeline_gates: the task couldn't be stopped"; return 1; }
  check wait_for 20 eval '[[ "$(runs state)" == "done paused" ]]' || { echo "pipeline_gates: stopping the task didn't pause its run: $(runs state)"; return 1; }
  [[ "$(runs why)" == *"the task is stopped" ]] || { echo "pipeline_gates: the held run says: $(runs why)"; return 1; }
  check wait_for 20 eval '! is "$sleeper" state running' || { echo "pipeline_gates: the stage's terminal kept running with its task stopped"; return 1; }
  check wait_for 10 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select json_extract(doc, \"$.paused.task\") from run_stages where run = \"$second\" order by rowid desc limit 1")" == 1 ]]' || { echo "pipeline_gates: the stopped stage wasn't kept to carry on"; return 1; }
  kanna task start "$task" >/dev/null || { echo "pipeline_gates: the task couldn't start again"; return 1; }
  check wait_for 20 eval '[[ "$(runs state)" == "done running" ]] || [[ "$(runs why)" == *"work stopped without reporting" ]]' || { echo "pipeline_gates: starting the task didn't carry its run on: $(runs state) $(runs why)"; return 1; }
  check wait_for 20 test -e "$slow_worker/.kanna/tmp/reply-1-1.md" || { echo "pipeline_gates: the stage wasn't resumed in its session"; return 1; }
  check test "$(cat "$slow_worker/.kanna/tmp/reply-1-1.md")" == "Carry on." || { echo "pipeline_gates: the resumed stage was told: $(cat "$slow_worker/.kanna/tmp/reply-1-1.md")"; return 1; }
  check wait_for 30 eval '[[ "$(runs why)" == *"work stopped without reporting" ]]' || { echo "pipeline_gates: the resumed stage didn't run: $(runs state) $(runs why)"; return 1; }
  local answered=""
  resumed() { answered="$(kanna run resume "$second" "$1" 2>&1)"; }
  check wait_for 20 resumed 'mock: run "$KANNA_CLI" run done' || { echo "pipeline_gates: the slowpoke run didn't resume: $answered"; return 1; }
  check wait_for 30 eval '[[ "$(runs state)" == "done done" ]]' || { echo "pipeline_gates: the slowpoke run didn't hand back: $(runs state) $(runs why)"; kanna run show "$second"; return 1; }
  events="$(kanna run log "$second" --json | python3 -c 'import json, sys; print(" ".join(e["what"] + ":" + (e["by"] or "-") for e in json.load(sys.stdin)["events"] if e["what"] in ("paused", "resumed")))')"
  [[ "$events" == "paused:task resumed:task paused:daemon resumed:user" ]] || { echo "pipeline_gates: the slowpoke run's events were: $events"; return 1; }
}
