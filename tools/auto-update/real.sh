good_feed
stop_instance
rm -f "$T/ticks"
reinstall
export KANNA_AGENTS_HOME="$T/home"
repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"; agents="$T/agents"; harness="$repo_root/tools/agent-harness"
rm -rf "$KANNA_AGENTS_HOME" "$agents"; mkdir -p "$repo" "$agents"
echo '{"version": 2, "setup": false}' >"$KANNA_AGENTS_HOME/.kanna-v4/agents.json"
printf '{"version": 1, "shelf": {"runners": 0}, "agents": {"prompts": "skip"}}\n' >"$KANNA_AGENTS_HOME/.kanna-v4/config.json"
git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first
repo="$(cd "$repo" && pwd -P)"
python3 -I "$harness/mock_server.py" --log "$agents/mock.jsonl" >"$agents/port" 2>"$agents/mock.log" &
mock=$!
trap 'kill $server $mock 2>/dev/null || true' EXIT
wait_for 10 test -s "$agents/port" || fail "the mock model didn't start"
python3 -I "$harness/isolate.py" --agent claude --real "$(command -v claude)" --home "$agents/claude-home" --port "$(cat "$agents/port")" --trust "$repo" --out "$agents/bin/claude"
launch
k() { "$install/Contents/Helpers/kanna" "$@"; }
in_repo() { (cd "$repo" && k "$@"); }
pane() { k workspace list --all --json | python3 -I -c 'import json, sys
p = [p for p in json.load(sys.stdin)["panes"] if sys.argv[1] in (p.get("name"), p.get("id"))][0]
print(p.get(sys.argv[2]))' "$1" "$2"; }
shows() { k read "$1" | grep -q -- "$2"; }
asked() { python3 -I -c 'import json, sys
lines = [json.loads(l) for l in open(sys.argv[1])]
print(sum(1 for l in lines if l.get("turn") and sys.argv[2] in l["prompt"] and (len(sys.argv) < 4 or any(sys.argv[3] in h for h in l["history"]))))' "$agents/mock.jsonl" "$@"; }
asked_is() { [[ "$(asked "$1" ${3:+"$3"} 2>/dev/null)" -ge "$2" ]]; }
mock_agent="$repo_root/tools/mock-agent/mock-agent"
wait_for 20 test -S "$KANNA_SOCKET" || fail "no daemon socket"
first="UPDATE_FIRST claude"; second="UPDATE_SECOND claude"
id="$(in_repo task new "claude task" "$first" --agent "$agents/bin/claude" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
desk="$(in_repo task new "Desk waiting" "say hi" --agent "$mock_agent" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
shelf="$(in_repo task new "On the Shelf" "say hi" --agent "$mock_agent" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
[[ -n "$id" && -n "$desk" && -n "$shelf" ]] || fail "tasks weren't added"
k task pick "$id" >/dev/null; k task pick "$desk" >/dev/null; k task activate "claude task" >/dev/null; k task start "$shelf" >/dev/null || true
wait_for 60 asked_is "$first" 1 || fail "claude's first message didn't reach the mock model: $(k read 'claude task' | tail -5)"
wait_for 30 shows "claude task" "Agent status probe" || fail "claude didn't show the answer"
in_repo open --window new --name updshell --tab >/dev/null
k send updshell 'echo UPDATE-BEFORE-$((6*7))' >/dev/null
wait_for 5 shows updshell UPDATE-BEFORE-42 || fail "the shell didn't answer"
in_repo open --window new --name ticker --tab --cmd "while :; do date +%s >>'$T/ticks'; echo tick; sleep 0.2; done" >/dev/null
wait_for 5 test -s "$T/ticks" || fail "the ticker didn't start"
keeper="$(cat "$state/keeper.pid")"; old_daemon="$(cat "$state/daemon.pid" 2>/dev/null || pgrep -f "$install/Contents/Helpers/kanna-daemon" | head -1)"
panes=("claude task" "Desk waiting" "On the Shelf" updshell ticker); pids=()
for p in "${panes[@]}"; do pids+=("$(pane "$p" pid)"); done
note "before: keeper $keeper, daemon $old_daemon, pids ${pids[*]}"
keeper_build="$(shasum -a 256 <"$install/Contents/Helpers/kanna-keeper")"
wait_for 40 titled "is available" || fail "no toast"
note "toast: $(toast_field title) / $(toast_field detail) / warning $(toast_field warning)"
lines="$(wc -l <"$state/daemon.log" 2>/dev/null || echo 0)"
started="$(date +%s)"
press "Install and Relaunch"
wait_for 120 eval '[[ "$(installed_build)" == "$build_b" ]]' || fail "B never installed: $(tail -5 "$T/app.log")"
note "installed B after $(( $(date +%s) - started ))s"
newapp() { pgrep -f "^$install/Contents/MacOS/Kanna" | grep -vx "$app_pid" | head -1; }
wait_for 60 eval '[[ -n "$(newapp)" ]]' || fail "B wasn't relaunched"
new="$(newapp)"; note "relaunched as pid $new"
wait_for 60 eval 'k workspace list --all --json >/dev/null 2>&1' || fail "no daemon after the relaunch"
wait_for 60 eval '! ps -p $old_daemon >/dev/null' || fail "the old daemon is still running"
wait_for 30 eval 'tail -n +"$((lines + 1))" "$state/daemon.log" | grep -q "took back"' || fail "the new daemon took nothing back"
log="$(tail -n +"$((lines + 1))" "$state/daemon.log")"
grep "upgraded the keeper (pid $keeper)" <<<"$log" || fail "the keeper wasn't upgraded: $log"
grep "took back" <<<"$log"
[[ "$(cat "$state/keeper.pid")" == "$keeper" ]] || fail "the keeper's pid changed"
[[ "$(ps -o command= -p "$keeper")" == "$install/Contents/Helpers/kanna-keeper"* ]] || fail "the keeper runs $(ps -o command= -p "$keeper")"
i=0
for p in "${panes[@]}"; do
  [[ "$(pane "$p" pid)" == "${pids[$i]}" ]] || fail "$p has another pid"
  [[ "$(pane "$p" state)" == running ]] || fail "$p isn't running"
  [[ "$(ps -o ppid= -p "${pids[$i]}" | tr -d ' ')" == "$keeper" ]] || fail "$p's parent isn't the keeper"
  i=$((i + 1))
done
note "every pid kept, under keeper $keeper"
shows updshell UPDATE-BEFORE-42 || fail "the shell's screen is gone"
shows "claude task" "Agent status probe" || fail "claude's screen is gone"
before="$(wc -l <"$T/ticks")"; sleep 2; [[ "$(wc -l <"$T/ticks")" -gt "$before" ]] || fail "the ticker stopped"
python3 -I -c 'import sys
t = [int(l) for l in open(sys.argv[1])]
gap = max(b - a for a, b in zip(t, t[1:]))
print("longest gap in the ticker:", gap, "s"); sys.exit(gap > 3)' "$T/ticks" || fail "the ticker paused"
k send "claude task" "$second" >/dev/null; k send-key "claude task" return >/dev/null
wait_for 60 asked_is "$second" 1 "$first" || fail "claude's next message didn't carry its conversation"
[[ "$(asked "$first")" == 1 ]] || fail "claude started over"
note "claude carried on its conversation"
k send updshell 'exit 7' >/dev/null
wait_for 10 eval '[[ "$(pane updshell state)" == exited ]]' || fail "the shell didn't exit"
[[ "$(pane updshell exitCode)" == 7 ]] || fail "exit code $(pane updshell exitCode)"
note "exit 7 reported 7"
[[ -z "$(defaults read "$bundle" KannaUpdatePending 2>/dev/null)" ]] || note "pending still set"
k --version
for f in Contents/MacOS/Kanna Contents/Helpers/kanna-keeper; do codesign --verify --strict "$install/$f"; done
spctl -a -t exec -vv "$install" 2>&1 | head -3 || true
echo "$new" >"$T/new-app.pid"
