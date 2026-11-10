scenario perf_intervals M4
perf_intervals() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" card shown i
  mkdir -p "$repo" "${agent%/*}" && ln -s "$mock_agent" "$agent" || { echo "perf_intervals line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "perf_intervals line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "perf_intervals line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field "$1"; }
  perf() { raw '{"id":1,"cmd":"debug.perf"}' | python3 -c 'import json, sys
r = json.load(sys.stdin); r = r.get("result", r)
names = {i["name"]: i for i in r["intervals"]}
for want in sys.argv[1:]:
    if want not in names: print("missing", want, sorted(names)); sys.exit(1)
    print(want, names[want]["last"], "ms")' "$@"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "perf_intervals: no planning terminal"; return 1; }
  (cd "$repo" && kanna task new "Timed" "hello there" --agent "$agent") >/dev/null && kanna task start $(tid 1) >/dev/null || { echo "perf_intervals line $LINENO"; return 1; }
  check wait_for 5 shows "Timed" "hello there" || { echo "perf_intervals: the task didn't start"; return 1; }
  kanna task stop $(tid 1) >/dev/null && check wait_for 5 is "Timed" state exited || { echo "perf_intervals: the task didn't stop"; return 1; }
  for i in $(seq 2 50); do
    (cd "$repo" && kanna task new "Shelf task $i" "$(python3 -c 'import sys
n = int(sys.argv[1])
words = "the daemon moves a stopped task to the desk and its card shows at once while the agent and tabs start behind it".split()
print(" ".join(words[(n + k) % len(words)] for k in range(450)))' "$i")") >/dev/null || { echo "perf_intervals line $LINENO"; return 1; }
  done
  check wait_for 10 eval '(( $(desk shelf | python3 -c "import json, sys; print(len(json.load(sys.stdin)))") == 50 ))' || { desk shelf; return 1; }
  sleep 1
  raw '{"id":1,"cmd":"debug.perf","args":{"reset":true}}' >/dev/null
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(tid 1)"'","window":"d1"}}' >/dev/null
  check wait_for 5 perf click.pick.wait click.pick.sent click.pick.shown message.layout refresh render DeskView.lay command.debug.click >/dev/null \
    || { echo "perf_intervals: the click's intervals weren't recorded"; perf click.pick.shown; return 1; }
  echo "perf_intervals: $(perf click.pick.wait click.pick.sent click.pick.shown | tr '\n' ' ')"
  shown="$(perf click.pick.shown | awk '{print $2}')"
  python3 -c 'import sys; sys.exit(float(sys.argv[1]) > 100 * int(sys.argv[2]))' "$shown" "$stretch" || { echo "perf_intervals: with 50 tasks on the Shelf the card showed ${shown}ms after the click"; raw '{"id":1,"cmd":"debug.perf"}'; return 1; }
  card="$(desk tasks.0.task)"
  raw '{"id":1,"cmd":"debug.click","args":{"task":"'"$card"'","window":"d1"}}' >/dev/null
  check wait_for 5 perf click.activate.shown >/dev/null || { echo "perf_intervals: pulling it wasn't timed"; perf click.activate.shown; return 1; }
  raw '{"id":1,"cmd":"debug.hang","args":{"ms":300}}' >/dev/null
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.perf\"}" | python3 -c "import json, sys
r = json.load(sys.stdin); r = r.get(\"result\", r)
sys.exit(not any(h[\"ms\"] >= 300 and \"command.debug.hang\" in h[\"open\"] for h in r[\"hangs\"]))"' \
    || { echo "perf_intervals: debug.perf has no 300ms hang in command.debug.hang"; raw '{"id":1,"cmd":"debug.perf"}'; return 1; }
  check wait_for 5 grep -qE 'kanna: the main thread hung [0-9]{3}ms; open: .*command\.debug\.hang' "$out/app.log" || { echo "perf_intervals: the hang isn't in the app's log"; tail -5 "$out/app.log"; return 1; }
}

scenario task_list_speed M4
task_list_speed() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" n=40 i started ms worst=0
  mkdir -p "$repo" "${agent%/*}" && ln -s "$mock_agent" "$agent" || { echo "task_list_speed line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_list_speed line $LINENO"; return 1; }
  start_daemon || { echo "task_list_speed line $LINENO"; return 1; }
  for ((i = 1; i <= n; i++)); do (cd "$repo" && kanna task new "Task $i" "hello there" --agent "$agent") >/dev/null || { echo "task_list_speed line $LINENO"; return 1; }; done
  for ((i = 1; i <= n; i++)); do kanna task start "$(tid $i)" >/dev/null || { echo "task_list_speed: task $i didn't start"; return 1; }; done
  for ((i = 1; i <= n; i++)); do kanna task stop "$(tid $i)" >/dev/null || { echo "task_list_speed: task $i didn't stop"; return 1; }; done
  started="$(find "$repo/.kanna-worktrees" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
  (( started == n )) || { echo "task_list_speed: $started of $n tasks have a worktree"; return 1; }
  raw '{"id":1,"cmd":"debug.daemon.perf","args":{"reset":true}}' >/dev/null
  for ((i = 1; i <= 20; i++)); do
    ms="$(cd "$repo" && { /usr/bin/time -p kanna task list --json | grep -q "Task $n"; } 2>&1 | awk '/^real/ {printf "%d", $2 * 1000}')"
    (( ms > worst )) && worst=$ms
  done
  echo "task_list_speed: $n stopped tasks with worktrees; kanna task list took at most ${worst}ms; daemon $(raw '{"id":1,"cmd":"debug.daemon.perf"}' | python3 -c 'import json, sys
r = json.load(sys.stdin); r = r.get("result", r)
i = {i["name"]: i for i in r["intervals"]}
print("loop.async max %sms, tasks.list p50 %sms max %sms" % (i.get("loop.async", {}).get("max"), i.get("tasks.list", {}).get("p50"), i.get("tasks.list", {}).get("max")))')"
  (( worst < 300 * stretch )) || { echo "task_list_speed: a list of $n tasks took ${worst}ms"; return 1; }
}

scenario loop_latency M4
loop_latency() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" n=40 rounds="${KANNA_LOOP_ROUNDS:-20}" i r t mark readings
  local codex="$KANNA_STATE_DIR/bin/codex" big="$KANNA_STATE_DIR/work/big" shipped="$KANNA_STATE_DIR/skills" sessions day
  mkdir -p "$repo" "${agent%/*}" && ln -s "$mock_agent" "$agent" && ln -s "$mock_agent" "$codex" || { echo "loop_latency line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "loop_latency line $LINENO"; return 1; }
  cp -R "$KANNA_AGENT_SKILLS" "$shipped" && export KANNA_AGENT_SKILLS="$shipped" CODEX_HOME="$KANNA_STATE_DIR/codex-home" || { echo "loop_latency line $LINENO"; return 1; }
  sessions="$CODEX_HOME/sessions/2026/10/01"; mkdir -p "$sessions" || { echo "loop_latency line $LINENO"; return 1; }
  line="$(python3 -c 'import json; print(json.dumps({"payload": {"cwd": "/nowhere", "id": "other", "filler": "x" * 65536}}))')"
  for ((i = 0; i < 400; i++)); do printf '%s\n' "$line" >"$sessions/rollout-2026-10-01T00-00-$i.jsonl"; done
  mkdir -p "$big" && git -C "$big" init -q -b main || { echo "loop_latency line $LINENO"; return 1; }
  for ((i = 0; i < 3000; i++)); do echo "$i" >"$big/$i.txt"; done
  git -C "$big" add . && git -C "$big" -c user.email=e@e -c user.name=e commit -q -m big || { echo "loop_latency line $LINENO"; return 1; }
  start_daemon && start_app || { echo "loop_latency line $LINENO"; return 1; }
  for ((i = 1; i <= n; i++)); do (cd "$repo" && kanna task new "Task $i" "hello there" --agent "$agent") >/dev/null || { echo "loop_latency line $LINENO"; return 1; }; done
  for ((i = 1; i <= n; i++)); do kanna task start "$(tid $i)" >/dev/null && kanna task stop "$(tid $i)" >/dev/null || { echo "loop_latency: task $i didn't start and stop"; return 1; }; done
  mark="$(wc -l <"$out/daemon.log")"
  raw '{"id":1,"cmd":"debug.daemon.perf","args":{"reset":true}}' >/dev/null
  for ((r = 0; r < rounds; r++)); do
    kanna task list --json >/dev/null || { echo "loop_latency line $LINENO"; return 1; }
    for i in 1 2 3; do kanna task pick "$(tid $((r * 3 % 39 + i)))" >/dev/null || { echo "loop_latency: round $r, pick $i failed"; return 1; }; done
    (cd "$repo" && kanna task list >/dev/null) || { echo "loop_latency line $LINENO"; return 1; }
    for i in 1 2 3; do kanna task shelve "$(tid $((r * 3 % 39 + i)))" >/dev/null || { echo "loop_latency: round $r, shelve $i failed"; return 1; }; done
  done
  local installed="$KANNA_AGENTS_HOME/.agents/skills/kanna-tasks/SKILL.md"
  kanna agents setup >/dev/null && [[ -f "$installed" ]] || { echo "loop_latency line $LINENO"; return 1; }
  echo "yours" >>"$installed" && echo "ours" >>"$shipped/kanna-tasks/SKILL.md" || { echo "loop_latency line $LINENO"; return 1; }
  for ((i = 0; i < 10; i++)); do kanna agents >"$KANNA_STATE_DIR/agents-status" || { echo "loop_latency line $LINENO"; return 1; }; done
  grep -qi "conflict\|both\|yours" "$KANNA_STATE_DIR/agents-status" || { echo "loop_latency: agents didn't reach a merge:"; cat "$KANNA_STATE_DIR/agents-status"; return 1; }
  (cd "$repo" && kanna task new "Codex" "hello there" --agent "$codex") >/dev/null || { echo "loop_latency line $LINENO"; return 1; }
  t="$(tid $((n + 1)))"
  for i in 1 2 3 4 5; do kanna task start "$t" >/dev/null && kanna task stop "$t" >/dev/null || { echo "loop_latency: the codex task didn't start and stop, $i"; return 1; }; done
  (cd / && kanna project import "$big" >"$KANNA_STATE_DIR/import-said" 2>&1; echo "$?" >"$KANNA_STATE_DIR/import-exit") &
  local importing=$! listed=0 slowest=0 ms
  while kill -0 "$importing" 2>/dev/null; do
    ms="$(cd "$repo" && { /usr/bin/time -p kanna task list --json >/dev/null; } 2>&1 | awk '/^real/ {printf "%d", $2 * 1000}')"
    listed=$((listed + 1)); (( ms > slowest )) && slowest=$ms
  done
  wait "$importing"
  [[ "$(cat "$KANNA_STATE_DIR/import-exit")" == 0 ]] || { echo "loop_latency: the import failed:"; cat "$KANNA_STATE_DIR/import-said"; return 1; }
  echo "loop_latency: during the import, $listed lists took at most ${slowest}ms; $(uptime | sed 's/.*load/load/')"
  readings="$(raw '{"id":1,"cmd":"debug.daemon.perf"}' | python3 -c 'import json, sys
r = json.load(sys.stdin); r = r.get("result", r)
loop = [i for i in r["intervals"] if i["name"].startswith("loop.")]
for i in r["intervals"]: print("%s p50 %sms p99 %sms max %sms (%s)" % (i["name"], i["p50"], i["p99"], i["max"], i["count"]))
held = r["loop"]; print("holds %s, over 20ms %s" % (held["holds"], held["over"]))
print("worst", 100 * held["over"] / max(held["holds"], 1))')"
  echo "loop_latency: $n tasks, $rounds rounds of list, pick 3, list, shelve 3:"; grep -v '^worst' <<<"$readings" | sed 's/^/  /'
  tail -n +"$((mark + 1))" "$out/daemon.log" | grep 'kanna-daemon: slow: ' | sed 's/^/  /'
  python3 -c 'import sys; sys.exit(float(sys.argv[1]) > int(sys.argv[2]))' "$(grep '^worst' <<<"$readings" | cut -d' ' -f2)" "$stretch" || { echo "loop_latency: more than 1% of the loop's holds are over 20ms: the loop's p99 is over its bound"; return 1; }
}

scenario typing_latency M4
typing_latency() {
  local keys="${KANNA_LATENCY_KEYS:-60}" floods="${KANNA_LATENCY_FLOODS:-1}" hidden="${KANNA_LATENCY_HIDDEN:-4}" agents="${KANNA_LATENCY_AGENTS:-0}" burners="${KANNA_LATENCY_CPU:-0}"
  local bound="${KANNA_LATENCY_BOUND:-$((100 * stretch))}" flood="exec yes 'typing_latency flood ........................................................................'" i burner_pids=()
  local agent="exec python3 $KANNA_STATE_DIR/redraw.py"
  cat >"$KANNA_STATE_DIR/redraw.py" <<'PY'
import random, sys, time
while True:
    rows = "".join("%3d %s\033[K\r\n" % (n, "x" * random.randrange(20, 100)) for n in range(40))
    sys.stdout.write("\033[?2026h\033[H" + rows + "\033[?2026l")
    sys.stdout.flush()
    time.sleep(1 / 30)
PY
  start_daemon && start_app || { echo "typing_latency line $LINENO"; return 1; }
  kanna open --window new --name typer --cmd 'echo TYPER-READY; exec cat' >/dev/null || { echo "typing_latency line $LINENO"; return 1; }
  for ((i = 1; i <= floods; i++)); do kanna open --window win-1 --name "flood$i" --split right --cmd "$flood" >/dev/null || return 1; done
  for ((i = 1; i <= agents; i++)); do kanna open --window win-1 --name "agent$i" --split down --cmd "$agent" >/dev/null || return 1; done
  for ((i = 1; i <= hidden; i++)); do kanna open --window win-1 --name "hidden$i" --tab --cmd "$flood" >/dev/null || return 1; done
  kanna focus typer >/dev/null || { echo "typing_latency line $LINENO"; return 1; }
  check wait_for 10 shows typer TYPER-READY || return 1
  for ((i = 1; i <= burners; i++)); do yes >/dev/null & burner_pids+=($!); done
  sleep 2
  raw '{"id":1,"cmd":"debug.perf","args":{"reset":true}}' >/dev/null
  raw '{"id":1,"cmd":"debug.daemon.perf","args":{"reset":true}}' >/dev/null
  for i in $(seq "$keys"); do
    raw '{"id":1,"cmd":"debug.type","args":{"pane":"typer","text":"k"}}' >/dev/null
    sleep 0.05
  done
  sleep 1
  (( ${#burner_pids[@]} )) && kill "${burner_pids[@]}" 2>/dev/null
  local readings
  readings="$( { raw '{"id":1,"cmd":"debug.perf"}'; raw '{"id":1,"cmd":"debug.daemon.perf"}'; } | python3 -c 'import json, sys
want = ["key.wait", "key.sent", "key.echo", "key.drawn", "key.lost", "input.handled", "input.written", "echo.read", "echo.parsed", "echo.taken", "echo.sent",
        "loop.chores", "loop.output", "loop.app.message", "loop.status", "loop.request", "loop.async", "message.output", "message.status", "render"]
rows = {}
for line in sys.stdin:
    r = json.loads(line); r = r.get("result", r)
    if "intervals" not in r: print("no readings:", line.strip()[:200])
    for i in r.get("intervals", []): rows.setdefault(i["name"], i)
for name in want:
    i = rows.get(name)
    if i: print("%-16s n=%-6d p50=%7.2f p95=%7.2f p99=%7.2f max=%7.2f" % (name, i["count"], i["p50"], i["p95"], i.get("p99", i["p95"]), i["max"]))')"
  echo "typing_latency: $keys keys into cat; $floods flooding beside it, $hidden flooding in hidden tabs, $agents redrawing at 30 fps, $burners CPU burners"
  echo "$readings"
  local drawn
  drawn="$(raw '{"id":1,"cmd":"debug.perf"}' | python3 -c 'import json, sys
r = json.load(sys.stdin); r = r.get("result", r)
i = {i["name"]: i for i in r.get("intervals", [])}.get("key.drawn")
print("%d %s" % (i["count"], i["p95"]) if i else "0 0")')"
  (( ${drawn%% *} >= keys * 9 / 10 )) || { echo "typing_latency: only ${drawn%% *} of $keys keys were drawn"; return 1; }
  python3 -c 'import sys; sys.exit(float(sys.argv[1]) > float(sys.argv[2]))' "${drawn#* }" "$bound" \
    || { echo "typing_latency: a keystroke's echo took ${drawn#* }ms to draw (p95), over ${bound}ms"; return 1; }
}
