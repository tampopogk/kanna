scenario agent_status M4
agent_status() {
  start_daemon && start_app || return 1
  kanna open --window new --name agent_statusshell >/dev/null || return 1
  kanna open --window win-1 --name agent_status --tab --cmd "$repo_root/tools/mock-agent/mock-agent" >/dev/null || return 1
  agent_status_status() { raw '{"id":1,"cmd":"debug.status"}' | field "panes.$(pane agent_status id).$1"; }
  check wait_for 5 is agent_status status idle || return 1
  check test "$(pane agent_statusshell status)" == None || return 1
  kanna focus agent_statusshell >/dev/null || return 1
  kanna send agent_status turn || return 1
  check wait_for 5 is agent_status status working || return 1
  check wait_for 10 is agent_status status needs-you || return 1
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys; print(next(p.get("status") for p in json.load(sys.stdin)["panes"] if p["name"] == "agent_status"))')" == needs-you || return 1
  kanna status explain agent_status | grep -qF 'record: state=blocked kind=permission' || { kanna status explain agent_status; return 1; }
  check wait_for 5 eval '[[ "$(agent_status_status status)" == needs-you ]]' || return 1
  kanna send agent_status 1 || return 1
  check wait_for 5 is agent_status status working || return 1
  check wait_for 10 is agent_status status idle || return 1
  check wait_for 5 eval '[[ "$(agent_status_status status)" == idle && "$(agent_status_status unread)" == True ]]' || { raw '{"id":1,"cmd":"debug.status"}'; return 1; }
  kanna focus agent_status >/dev/null || return 1
  check wait_for 5 eval '[[ "$(agent_status_status unread)" == False ]]' || return 1
  kanna send agent_status exit || return 1
  check wait_for 5 is agent_status status exited || return 1
  check wait_for 5 eval '[[ "$(agent_status_status status)" == exited ]]'
}

scenario real_agent_tasks M4
real_agent_tasks() {
  local logged; logged="$(cat "$out/daemon.log" 2>/dev/null | wc -l)"
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}" codex="${KANNA_E2E_CODEX:-$(command -v codex)}"
  [[ -x "$claude" ]] || { echo "real_agent_tasks: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  [[ -x "$codex" ]] || { echo "real_agent_tasks: codex isn't installed (or set KANNA_E2E_CODEX)"; return 1; }
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" harness="$repo_root/tools/agent-harness"
  CODEX_HOME="$agents/codex-home/.codex" start_daemon && start_app || { echo "real_agent_tasks line $LINENO"; return 1; }
  mkdir -p "$agents" "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "real_agent_tasks line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  python3 -I "$harness/mock_server.py" --log "$agents/mock.jsonl" >"$agents/port" 2>"$agents/mock.log" &
  check wait_for 10 eval '[[ -s "$agents/port" ]]' || { echo "real_agent_tasks: the mock model didn't start"; return 1; }
  local port; port="$(cat "$agents/port")"
  local agent real
  for agent in claude codex; do
    [[ $agent == claude ]] && real="$claude" || real="$codex"
    python3 -I "$harness/isolate.py" --agent $agent --real "$real" --home "$agents/$agent-home" --port "$port" --trust "$repo" --out "$agents/bin/$agent" || { echo "real_agent_tasks line $LINENO"; return 1; }
  done
  asked() { python3 -c 'import json, sys
lines = [json.loads(l) for l in open(sys.argv[1])]
print(sum(1 for l in lines if l.get("turn") and sys.argv[2] in l["prompt"] and (len(sys.argv) < 4 or any(sys.argv[3] in h for h in l["history"]))))' "$agents/mock.jsonl" "$@"; }
  local id task first second said
  for agent in claude codex; do
    first="RESUME_FIRST $agent" second="RESUME_SECOND $agent"
    id="$(cd "$repo" && kanna task new "$agent task" "$first" --agent "$agents/bin/$agent" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
    [[ "$id" =~ ^task-[0-9a-f]{4,8}$ ]] || { echo "real_agent_tasks: $agent's task wasn't added"; return 1; }
    kanna task start "$id" >/dev/null || { echo "real_agent_tasks: $agent's task didn't start"; return 1; }
    check wait_for 60 eval '[[ "$(asked "$first")" == 1 ]]' || { echo "real_agent_tasks: $agent's first message didn't reach the mock model"; kanna read "$agent task"; return 1; }
    [[ "$(asked "You're a task's agent in Kanna (terminal $(pane "$agent task" terminal)), in your task's worktree. Your task: $first")" == 1 ]] || { echo "real_agent_tasks: $agent wasn't told it's a task's agent"; tail -1 "$agents/mock.jsonl" | cut -c1-300; return 1; }
    check wait_for 30 shows "$agent task" "Agent status probe" || { echo "real_agent_tasks: $agent didn't show the mock's answer"; kanna read "$agent task"; return 1; }
    check wait_for 10 grep -rqs "Agent status probe" "$agents/$agent-home" || { echo "real_agent_tasks: $agent didn't save its answer"; return 1; }
    if [[ $agent == codex ]]; then
      local session; session="$(grep -rhos '"session_meta","payload":{[^}]*' "$agents/codex-home/.codex/sessions" | sed -n 's/.*"id":"\([0-9a-f-]*\)".*/\1/p' | head -1)"
      [[ -n "$session" ]] || { echo "real_agent_tasks: codex wrote no session"; return 1; }
      check wait_for 10 eval '[[ "$(pane "$agent task" cmd)" == *" resume '"'"'$session'"'"'" ]]' || { echo "real_agent_tasks: codex's pane doesn't resume its session $session: $(pane "$agent task" cmd)"; return 1; }
    fi
    kanna restart "$agent task" >/dev/null || { echo "real_agent_tasks: $agent's pane didn't restart"; return 1; }
    check wait_for 60 eval 'shows "$agent task" "$first" && shows "$agent task" "Agent status probe"' || { echo "real_agent_tasks: $agent didn't restart into its conversation"; kanna read "$agent task"; return 1; }
    sleep 3
    [[ "$(asked "$first")" == 1 ]] || { echo "real_agent_tasks: $agent restarted with its first message instead of resuming"; return 1; }
    kanna task pick "$id" >/dev/null || { echo "real_agent_tasks: $agent's task didn't move to the Shelf"; return 1; }
    task="$(pane "$agent task" id)"
    [[ "$(kanna task shelve "$task")" == *"to the Shelf as $id"* ]] || { echo "real_agent_tasks: $agent's task didn't move back to the Shelf as $id"; return 1; }
    [[ "$(kanna task stop "$id")" == "stopped $id on the Shelf; "* ]] || { echo "real_agent_tasks: $agent's task didn't stop"; return 1; }
    check is "$agent task" state exited || { echo "real_agent_tasks: $agent is still running"; return 1; }
    said="$(kanna read "$agent task" | tr -d '\r\n' | grep -oE "$agent( --?[^ ]+( [^- ][^ ]*)?)* (--)?resume [0-9a-f-]{36}" | tail -1 | grep -oE '[0-9a-f-]{36}$')"
    [[ -n "$said" ]] || { echo "real_agent_tasks: $agent didn't say its session as it quit"; kanna read "$agent task" --lines 60; return 1; }
    ! tail -n +"$((logged + 1))" "$out/daemon.log" | grep -E "in $task (has no quit keys|didn't quit)|quit keys in $task " || { echo "real_agent_tasks: $agent was hung up"; return 1; }
    [[ "$(raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys; print(next(t for t in json.load(sys.stdin)["shelf"] if t["id"].startswith(sys.argv[1])).get("session"))' "$id")" == "$said" ]] || { echo "real_agent_tasks: the session $agent said ($said) wasn't kept"; return 1; }
    [[ "$(kanna task start "$id")" == "started $id again on the Shelf: "* ]] || { echo "real_agent_tasks: $agent's task didn't start again"; return 1; }
    [[ "$(raw '{"id":1,"cmd":"list"}' | python3 -c 'import json, sys; print(next(p.get("cmd") for p in json.load(sys.stdin)["panes"] if p.get("name") == sys.argv[1]))' "$agent task")" == *"resume '$said'"* ]] || { echo "real_agent_tasks: $agent didn't start again with the session it said"; raw '{"id":1,"cmd":"list"}'; return 1; }
    check wait_for 60 eval 'shows "$agent task" "$first" && shows "$agent task" "Agent status probe"' || { echo "real_agent_tasks: $agent didn't come back to its conversation"; kanna read "$agent task"; return 1; }
    sleep 3
    [[ "$(asked "$first")" == 1 ]] || { echo "real_agent_tasks: $agent started again with its first message instead of resuming"; return 1; }
    kanna send "$agent task" "$second" || { echo "real_agent_tasks line $LINENO"; return 1; }
    check wait_for 60 eval '[[ "$(asked "$second" "$first")" -ge 1 ]]' || { echo "real_agent_tasks: $agent's next message didn't carry its conversation"; kanna read "$agent task"; tail -3 "$agents/mock.jsonl" | cut -c1-300; return 1; }
    said="$(cd "$repo" && kanna task send "$id" "MESSAGE_PROBE $agent")" || { echo "real_agent_tasks line $LINENO"; return 1; }
    [[ "$said" != *warning* ]] || { echo "real_agent_tasks: $agent can't take a message: $said"; return 1; }
    check wait_for 60 eval '[[ "$(asked "] MESSAGE_PROBE $agent" "$first")" -ge 1 ]]' || { echo "real_agent_tasks: the message to $agent didn't reach it"; kanna read "$agent task"; grep -o 'MESSAGE_PROBE.\{0,200\}' "$agents/mock.jsonl" | tail -3; tail -2 "$agents/mock.jsonl" | cut -c1-600; kanna task messages "$id"; tail -n +"$((logged + 1))" "$out/daemon.log" | grep -i message | tail -5; return 1; }
    sleep 3
    check wait_for 30 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"status\",\"args\":{\"action\":\"explain\",\"pane\":\"$agent task\"}}" | field status)" == idle ]]' || { echo "real_agent_tasks: $agent didn't go idle after the message"; return 1; }
    local before; before="$(wc -l <"$out/daemon.log")"
    [[ "$(kanna task stop "$id")" == "stopped $id on the Shelf; "* ]] || { echo "real_agent_tasks: $agent's task didn't stop at its prompt"; return 1; }
    check is "$agent task" state exited || { echo "real_agent_tasks: $agent at its prompt is still running"; return 1; }
    ! tail -n +"$((before + 1))" "$out/daemon.log" | grep -E "$agent's box in |$agent in [^ ]+ didn't (exit|quit)" || { echo "real_agent_tasks: $agent wasn't quit by /exit"; return 1; }
    ! grep -rqsE --include='*.jsonl' 'Request interrupted|turn_aborted' "$agents/$agent-home" || { echo "real_agent_tasks: $agent's transcript has an interruption"; return 1; }
    [[ "$(kanna task start "$id")" == "started $id again on the Shelf: "* ]] || { echo "real_agent_tasks: $agent's task didn't start after /exit"; return 1; }
    check wait_for 60 eval 'shows "$agent task" "Agent status probe"' || { echo "real_agent_tasks: $agent didn't come back after /exit"; kanna read "$agent task"; return 1; }
    sleep 3
    check wait_for 30 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"status\",\"args\":{\"action\":\"explain\",\"pane\":\"$agent task\"}}" | field status)" == idle ]]' || { echo "real_agent_tasks: $agent didn't go idle after it came back"; return 1; }
    raw "{\"id\":1,\"cmd\":\"send\",\"args\":{\"pane\":\"$agent task\",\"text\":\"DRAFT_PROBE $agent\",\"enter\":false}}" >/dev/null || { echo "real_agent_tasks line $LINENO"; return 1; }
    check wait_for 10 shows "$agent task" "DRAFT_PROBE" || { echo "real_agent_tasks: $agent's draft didn't show"; kanna read "$agent task"; return 1; }
    [[ "$(kanna task stop "$id")" == "stopped $id on the Shelf; "* ]] || { echo "real_agent_tasks: $agent's task with a draft didn't stop"; return 1; }
    check is "$agent task" state exited || { echo "real_agent_tasks: $agent with a draft is still running"; return 1; }
    tail -n +"$((before + 1))" "$out/daemon.log" | grep -q "$agent's box in " || { echo "real_agent_tasks: $agent's draft wasn't seen"; tail -20 "$out/daemon.log"; return 1; }
    [[ "$(asked "DRAFT_PROBE")" == 0 ]] || { echo "real_agent_tasks: $agent's draft was sent"; return 1; }
    ! tail -n +"$((before + 1))" "$out/daemon.log" | grep -E "$agent in [^ ]+ didn't quit" || { echo "real_agent_tasks: $agent with a draft was hung up"; return 1; }
  done
  stop_agents
}

scenario real_agent_program_status M4
real_agent_program_status() {
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}" codex="${KANNA_E2E_CODEX:-$(command -v codex)}"
  [[ -x "$claude" ]] || { echo "real_agent_program_status: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  [[ -x "$codex" ]] || { echo "real_agent_program_status: codex isn't installed (or set KANNA_E2E_CODEX)"; return 1; }
  start_daemon && start_app || { echo "real_agent_program_status line $LINENO"; return 1; }
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" harness="$repo_root/tools/agent-harness"
  mkdir -p "$agents" "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "real_agent_program_status line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  python3 -I "$harness/mock_server.py" --log "$agents/mock.jsonl" >"$agents/port" 2>"$agents/mock.log" &
  check wait_for 10 eval '[[ -s "$agents/port" ]]' || { echo "real_agent_program_status: the mock model didn't start"; return 1; }
  reported() { raw "{\"id\":1,\"cmd\":\"status\",\"args\":{\"action\":\"explain\",\"pane\":\"$1\"}}" | python3 -c 'import json, sys; r = json.load(sys.stdin); print((r.get("reported") or {}).get("state"), r.get("status"))'; }
  local agent real pane into=new
  for agent in claude codex; do
    [[ $agent == claude ]] && real="$claude" || real="$codex"
    pane="$agent reporting"
    python3 -I "$harness/isolate.py" --agent $agent --real "$real" --home "$agents/$agent-home" --port "$(cat "$agents/port")" --trust "$repo" --out "$agents/bin/$agent" || { echo "real_agent_program_status line $LINENO"; return 1; }
    kanna open --window "$into" --name "$pane" --cwd "$repo" --cmd "$agents/bin/$agent" >/dev/null || { echo "real_agent_program_status line $LINENO"; return 1; }
    into=win-1
    check wait_for 60 shows "$pane" "for shortcuts\\|shift+tab" || { echo "real_agent_program_status: $agent didn't start"; kanna read "$pane"; return 1; }
    kanna send "$pane" "PLAIN_TURN" || { echo "real_agent_program_status line $LINENO"; return 1; }
    check wait_for 30 eval '[[ "$(reported "$pane")" == "working working" ]]' || { echo "real_agent_program_status: $agent's turn wasn't working from OSC 7501 but $(reported "$pane")"; kanna status explain "$pane"; kanna read "$pane"; tail -2 "$agents/mock.jsonl" | cut -c1-300; return 1; }
    check wait_for 60 eval '[[ "$(reported "$pane")" =~ ^(idle|done)" idle"$ ]]' || { echo "real_agent_program_status: $agent's turn didn't end idle from OSC 7501 but $(reported "$pane")"; kanna status explain "$pane"; kanna read "$pane"; return 1; }
    shows "$pane" "slow streamed answer" || { echo "real_agent_program_status: $agent didn't show the mock's answer"; kanna read "$pane"; return 1; }
    if [[ $agent == claude ]]; then
      rm -f "$repo/background-done.txt"
      kanna send "$pane" "SHELL_IN_BACKGROUND" || { echo "real_agent_program_status line $LINENO"; return 1; }
      check wait_for 60 eval 'shows "$pane" "SHELL_IN_BACKGROUND_DONE" && [[ "$(reported "$pane")" =~ ^(idle|done)" idle"$ ]]' || { echo "real_agent_program_status: claude's turn didn't end idle with its background shell running but $(reported "$pane")"; kanna read "$pane"; return 1; }
      [[ ! -e "$repo/background-done.txt" ]] || { echo "real_agent_program_status: claude's background shell ended before its turn did"; return 1; }
      check wait_for 30 eval '[[ -e "$repo/background-done.txt" ]]' || { echo "real_agent_program_status: claude's background shell didn't finish"; return 1; }
      check wait_for 10 eval '[[ "$(reported "$pane")" == working* ]]' || { echo "real_agent_program_status: claude wasn't working within Shelf runners' 10s grace once its background shell ended but $(reported "$pane")"; kanna read "$pane"; return 1; }
      check wait_for 60 eval '[[ "$(reported "$pane")" =~ ^(idle|done)" idle"$ ]]' || { echo "real_agent_program_status line $LINENO"; return 1; }
    fi
    [[ $agent == codex ]] || continue
    kanna send "$pane" "BACKGROUND_TURN" || { echo "real_agent_program_status line $LINENO"; return 1; }
    check wait_for 60 eval 'shows "$pane" "BACKGROUND_DONE" && shows "$pane" "Worked for" && shows "$pane" "background terminal running"' || { echo "real_agent_program_status: codex's turn didn't end with a background terminal running"; kanna read "$pane"; return 1; }
    sleep 3
    [[ "$(reported "$pane")" == "working working" ]] || { echo "real_agent_program_status: codex with a background terminal running wasn't working but $(reported "$pane")"; kanna status explain "$pane"; kanna read "$pane"; return 1; }
    kanna send "$pane" "/stop" || { echo "real_agent_program_status line $LINENO"; return 1; }
    check wait_for 30 eval '[[ "$(reported "$pane")" == "idle idle" ]]' || { echo "real_agent_program_status: codex wasn't idle once /stop closed its background terminal but $(reported "$pane")"; kanna status explain "$pane"; kanna read "$pane"; return 1; }
  done
  stop_agents
}

scenario send_submits M4
send_submits() {
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}" codex="${KANNA_E2E_CODEX:-$(command -v codex)}"
  [[ -x "$claude" ]] || { echo "send_submits: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  start_daemon || return 1
  local shell name into=new
  for shell in "zsh -f" "bash --norc --noprofile"; do
    name="send_${shell%% *}"
    kanna open --window "$into" --name "$name" --cmd "exec $shell" >/dev/null || return 1
    into=win-1
    kanna send "$name" "echo $name-\$((40+2))" || return 1
    check wait_for 5 shows "$name" "^$name-42" || { kanna read "$name"; return 1; }
  done
  kanna open --window win-1 --name send_cat --cmd "exec cat" >/dev/null || return 1
  kanna send send_cat send_cat-LINE || return 1
  check wait_for 5 eval '[[ "$(kanna read send_cat | grep -c send_cat-LINE)" == 2 ]]' || { kanna read send_cat; return 1; }
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" harness="$repo_root/tools/agent-harness"
  mkdir -p "$agents" "$repo" && git -C "$repo" init -q -b main || return 1
  repo="$(cd "$repo" && pwd -P)"
  python3 -I "$harness/mock_server.py" --log "$agents/mock.jsonl" >"$agents/port" 2>"$agents/mock.log" &
  check wait_for 10 eval '[[ -s "$agents/port" ]]' || { echo "send_submits: the mock model didn't start"; return 1; }
  sent() { python3 -c 'import json, sys
print(sum(1 for l in map(json.loads, open(sys.argv[1])) if l.get("turn") and any(sys.argv[2] in h for h in l["history"])))' "$agents/mock.jsonl" "$1"; }
  local agent real i rounds="${KANNA_E2E_SEND_ROUNDS:-10}" long
  long="$(printf 'a long message, as a planner writes one, %.0s' $(seq 12))"
  for agent in claude codex; do
    [[ $agent == claude ]] && real="$claude" || real="$codex"
    [[ -x "$real" ]] || { echo "send_submits: $agent isn't installed, so only the others were checked"; continue; }
    python3 -I "$harness/isolate.py" --agent $agent --real "$real" --home "$agents/$agent-home" --port "$(cat "$agents/port")" --trust "$repo" --out "$agents/bin/$agent" || return 1
    kanna open --window win-1 --name "send_$agent" --cwd "$repo" --cmd "$agents/bin/$agent" >/dev/null || return 1
    check wait_for 60 is "send_$agent" status idle || { kanna read "send_$agent"; return 1; }
    sleep 2
    for i in $(seq "$rounds"); do
      kanna send "send_$agent" "SEND_IDLE $agent $i: $long" || return 1
      kanna send "send_$agent" "SEND_BUSY $agent $i: $long" || return 1
      check wait_for 30 eval '[[ "$(sent "SEND_IDLE $agent $i:")" -ge 1 ]]' || { echo "send_submits: a message sent to idle $agent wasn't submitted"; kanna read "send_$agent"; return 1; }
      check wait_for 30 eval '[[ "$(sent "SEND_BUSY $agent $i:")" -ge 1 ]]' || { echo "send_submits: a message sent to busy $agent wasn't submitted"; kanna read "send_$agent"; return 1; }
      check wait_for 30 is "send_$agent" status idle || { kanna read "send_$agent"; return 1; }
    done
  done
  stop_agents
}

scenario codex_task M4
codex_task() {
  start_daemon && start_app || { echo "codex_task line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents" w folder
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "codex_task line $LINENO"; return 1; }
  ln -s "$mock_agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  printf '{"version": 1, "shelf": {"runners": 0}, "tasks": {"agent": "%s"}, "agents": {"prompts": "skip"}}\n' "$fake/codex" >"$KANNA_AGENTS_HOME/.kanna-v4/config.json"
  printf '{"tasks": {"agent": "touch %s/owned; claude"}}\n' "$KANNA_STATE_DIR" >"$repo/.kanna/config.json"
  in_repo task new "Codex task" "run git branch -m codex-mock-work" >/dev/null || { echo "codex_task line $LINENO"; return 1; }
  check test "$(in_repo --json task list | field shelf.0.agent)" == "$fake/codex" || { echo "codex_task: tasks.agent in your config wasn't the task's agent, or the repo's command was taken"; in_repo --json task list; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 1)"' +Codex task +Shelf +stopped$' || { echo "codex_task: task list names the default agent"; in_repo task list; return 1; }
  w="$(cd / && kanna "$repo")" && [[ "$w" =~ ^opened\ (d[0-9]+) ]] && w="${BASH_REMATCH[1]}" || { echo "codex_task line $LINENO: $w"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk agents)" == "[\"codex\"]" ]]' || { echo "codex_task: the app doesn't keep the Shelf task's agent"; desk agents; return 1; }
  kanna task start $(tid 1) >/dev/null || { echo "codex_task: the task didn't start"; return 1; }
  check wait_for 10 shows "Codex task" "RUN_DONE 0" || { echo "codex_task: the first message didn't run"; kanna read "Codex task"; return 1; }
  spaceless "Codex task" "args: -c approval_policy=\"never\" -c sandbox_mode=\"danger-full-access\" You're a task's agent in Kanna (terminal $(pane "Codex task" id)), in your task's worktree. Your task: run git branch -m codex-mock-work --remote unix://" || { echo "codex_task: codex wasn't started as a task's agent"; kanna read "Codex task"; return 1; }
  folder="$(pane "Codex task" cwd)"
  check test "$(git -C "$folder" branch --show-current)" == codex-mock-work || { echo "codex_task: the branch wasn't renamed"; git -C "$folder" branch; return 1; }
  check wait_for 10 is "Codex task" status idle || { echo "codex_task: not idle after its turn"; pane "Codex task" status; return 1; }
  kanna send "Codex task" "work 3" && check wait_for 5 is "Codex task" status working || { echo "codex_task: not working on kanna send"; return 1; }
  check wait_for 10 is "Codex task" status idle && shows "Codex task" WORK_DONE || { echo "codex_task: not idle after kanna send's turn"; return 1; }
  kanna send "Codex task" ask && check wait_for 5 is "Codex task" status needs-you || { echo "codex_task: not needs you at codex's question"; kanna read "Codex task"; return 1; }
  kanna send-key "Codex task" return && check wait_for 10 shows "Codex task" TURN_DONE && check wait_for 10 is "Codex task" status idle || { echo "codex_task: the answer didn't go through"; return 1; }
  [[ "$(kanna task pick $(tid 1))" == "moved $(sid 1) to the Desk: "* ]] || { echo "codex_task: the task didn't move to the Shelf"; return 1; }
  check wait_for 10 eval '[[ "$(desk tasks.0.agent)" == codex ]]' || { echo "codex_task: the app doesn't keep the Shelf task's agent"; desk tasks; return 1; }
  in_repo task list | placed | grep -qE "^[^ ]+ +Codex task +Desk +idle +just now$" || { echo "codex_task: task list names the default agent"; in_repo task list; return 1; }
  kanna task done $(tid 1) >/dev/null || { echo "codex_task: done failed"; return 1; }
  check wait_for 10 eval '! kanna workspace list --all --json | grep -q "\"Codex task\""' || { echo "codex_task: the agent outlived done"; return 1; }
  check test ! -e "$KANNA_STATE_DIR/owned" || { echo "codex_task: a repo's tasks.agent ran a command"; return 1; }
}

scenario planner_choice M4
planner_choice() {
  local fake="$KANNA_STATE_DIR/fake" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$fake" "$repo" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/claude" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex" || { echo "planner_choice line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "planner_choice line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  local mine="$KANNA_AGENTS_HOME/.kanna-v4/config.json"
  local kept; kept="$(jq 'del(.agents)' "$mine")" && printf '%s\n' "$kept" >"$mine" || { echo "planner_choice line $LINENO"; return 1; }
  KANNA_PLANNING_AGENTS="$fake/claude $fake/codex" start_daemon || { echo "planner_choice line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead d1)" == term-* ]]' || { echo "planner_choice: no planning terminal"; kanna workspace list --all; return 1; }
  local planner; planner="$(desk lead d1)"
  check wait_for 10 shows "$planner" "Choose an agent for planning and task creation" || { echo "planner_choice: the first Desk didn't ask which agent"; kanna read "$planner"; return 1; }
  check shows "$planner" "1  claude" && check shows "$planner" "2  codex" || { kanna read "$planner"; return 1; }
  ! shows "$planner" "Folder, repo or name" || { echo "planner_choice: a Desk on a repo asked where"; return 1; }
  kanna send "$planner" 2 >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 5 shows "$planner" "codex plans repo from now on" || { kanna read "$planner"; return 1; }
  check wait_for 5 spaceless "$planner" "Let Kanna's agents work without permission prompts?" || { echo "planner_choice: the first Desk didn't ask about permission prompts"; kanna read "$planner"; return 1; }
  ! shows "$planner" "args:" || { echo "planner_choice: the agent started before the question was answered"; return 1; }
  kanna send "$planner" y >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 5 spaceless "$planner" "Kanna's agents work without permission prompts from now on" || { kanna read "$planner"; return 1; }
  check wait_for 10 spaceless "$planner" "args: -c approval_policy=\"never\" -c sandbox_mode=\"danger-full-access\" -c model_auto_compact_token_limit=300000 You are the Kanna planning agent. --remote unix://" || { echo "planner_choice: codex didn't start as the planner"; kanna read "$planner"; return 1; }
  check grep -Eq '"prompts" ?: "skip"' "$mine" && check grep -Eq '"runners" ?: 0' "$mine" || { echo "planner_choice: the answer wasn't saved with the file's other keys"; cat "$mine"; return 1; }
  check grep -Eq '"agent" ?: "codex"' "$repo/.kanna/config.json" || { cat "$repo/.kanna/config.json"; return 1; }
  kanna send "$planner" exit >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 5 shows "$planner" "The planning agent quit. Start it again:" || { kanna read "$planner"; return 1; }
  check is "$planner" state running || { echo "planner_choice: quitting the agent ended the planning terminal"; return 1; }
  local session; session="$(kanna read "$planner" | tr -d '\r\n' | grep -oE 'codex resume [0-9a-f-]{36}' | head -1 | grep -oE '[0-9a-f-]{36}$')"
  [[ -n "$session" ]] || { echo "planner_choice: codex didn't say its session"; kanna read "$planner"; return 1; }
  kanna read "$planner" | tr -d ' \r\n' | grep -q "Startitagain:.*resume'$session'" || { echo "planner_choice: the way to start it again isn't its session"; kanna read "$planner"; return 1; }
  close_desk d1 || { echo "planner_choice line $LINENO"; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "planner_choice line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk lead "$w")" == term-* ]]' || { echo "planner_choice: the next Desk has no planning terminal"; return 1; }
  planner="$(desk lead "$w")"
  check wait_for 10 shows "$planner" "^resumed $session" || { echo "planner_choice: the next Desk didn't come back to the project's planner"; kanna read "$planner"; return 1; }
  spaceless "$planner" "Back to repo's planning conversation." && ! spaceless "$planner" "You are the Kanna planning agent." || { echo "planner_choice: the next Desk's planner was told again"; kanna read "$planner"; return 1; }
  ! shows "$planner" "Choose an agent" || { echo "planner_choice: the next Desk asked again"; return 1; }
  ! spaceless "$planner" "permission prompts?" || { echo "planner_choice: the next Desk asked about permission prompts again"; return 1; }
  key cmd+n
  check wait_for 10 eval '[[ "$(desk lead d3)" == term-* ]]' || { echo "planner_choice: ⌘N opened no Desk"; kanna workspace list --all; return 1; }
  planner="$(desk lead d3)"
  check wait_for 10 shows "$planner" "Folder, repo, name or number \[1\]:" || { echo "planner_choice: a Desk in ~ didn't ask where"; kanna read "$planner"; return 1; }
  local typo="${repo%?}q${repo: -1}" keys=() i
  for ((i = 0; i < ${#typo}; i++)); do keys+=("${typo:i:1}"); done
  kanna send-key "$planner" "${keys[@]}" left backspace >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(kanna read "$planner" | tr -d "\n")" == *"Folder, repo, name or number [1]: $repo" ]]' || { echo "planner_choice: ← and Backspace didn't fix the answer"; kanna read "$planner"; return 1; }
  ! kanna read "$planner" | tr -d '\n' | grep -qF '^[[' || { echo "planner_choice: the prompt printed raw escapes"; kanna read "$planner"; return 1; }
  kanna send-key "$planner" return >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk header d3)" == "Planning · "*"${repo#/private} · main" ]]' || { echo "planner_choice: the Desk didn't follow to the folder"; desk header d3; return 1; }
  check wait_for 10 spaceless "$planner" "args: -c approval_policy=\"never\" -c sandbox_mode=\"danger-full-access\" -c model_auto_compact_token_limit=300000 You are the Kanna planning agent. --remote unix://" || { echo "planner_choice: the Desk didn't start the folder's planner"; kanna read "$planner"; return 1; }
  spaceless "$planner" "The planner in another window is in repo's planning conversation, so this one starts a new one." || { echo "planner_choice: a second Desk on the project didn't say why it starts a new conversation"; kanna read "$planner"; return 1; }
}

scenario real_agent_planners M4
real_agent_planners() {
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}" codex="${KANNA_E2E_CODEX:-$(command -v codex)}"
  [[ -x "$claude" ]] || { echo "real_agent_planners: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  [[ -x "$codex" ]] || { echo "real_agent_planners: codex isn't installed (or set KANNA_E2E_CODEX)"; return 1; }
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" harness="$repo_root/tools/agent-harness"
  mkdir -p "$agents" "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "real_agent_planners line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  python3 -I "$harness/mock_server.py" --log "$agents/mock.jsonl" >"$agents/port" 2>"$agents/mock.log" &
  check wait_for 10 eval '[[ -s "$agents/port" ]]' || { echo "real_agent_planners: the mock model didn't start"; return 1; }
  local port agent real; port="$(cat "$agents/port")"
  for agent in claude codex; do
    [[ $agent == claude ]] && real="$claude" || real="$codex"
    python3 -I "$harness/isolate.py" --agent $agent --real "$real" --home "$agents/$agent-home" --port "$port" --trust "$repo" --out "$agents/bin/$agent" || { echo "real_agent_planners line $LINENO"; return 1; }
  done
  told() { python3 -c 'import json, sys
lines = [json.loads(l) for l in open(sys.argv[1])]
print(sum(1 for l in lines if l.get("turn") and l["path"].startswith(sys.argv[2]) and any(h.rstrip().endswith("You are the Kanna planning agent.") for h in l["history"])))' "$agents/mock.jsonl" "$1"; }
  KANNA_PLANNING_AGENTS="$agents/bin/claude $agents/bin/codex" start_daemon || { echo "real_agent_planners line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  local w=d1 choice path planner opened
  for agent in claude codex; do
    [[ $agent == claude ]] && choice=1 path=/v1/messages || choice=2 path=/v1/responses
    check wait_for 10 eval '[[ "$(desk lead "$w")" == term-* ]]' || { echo "real_agent_planners: no planning terminal"; kanna workspace list --all; return 1; }
    planner="$(desk lead "$w")"
    check wait_for 10 shows "$planner" "Choose \[1\]:" || { kanna read "$planner"; return 1; }
    kanna send "$planner" "$choice" >/dev/null && check wait_for 5 shows "$planner" "$agent plans .* from now on" || { echo "real_agent_planners line $LINENO"; kanna read "$planner"; return 1; }
    check wait_for 60 eval '[[ "$(told "$path")" -ge 1 ]]' || { echo "real_agent_planners: $agent wasn't told it's the planning agent"; kanna read "$planner"; tail -3 "$agents/mock.jsonl" | cut -c1-300; return 1; }
    check wait_for 30 shows "$planner" "Agent status probe" || { echo "real_agent_planners: $agent didn't show the mock's answer"; kanna read "$planner"; return 1; }
    close_desk "$w" && rm "$repo/.kanna/config.json" || { echo "real_agent_planners line $LINENO"; return 1; }
    opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "real_agent_planners line $LINENO: $opened"; return 1; }
    w="${BASH_REMATCH[1]}"
  done
  stop_agents
}

scenario planner_resumes M4
planner_resumes() {
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}" codex="${KANNA_E2E_CODEX:-$(command -v codex)}"
  [[ -x "$claude" ]] || { echo "planner_resumes: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  [[ -x "$codex" ]] || { echo "planner_resumes: codex isn't installed (or set KANNA_E2E_CODEX)"; return 1; }
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" harness="$repo_root/tools/agent-harness"
  mkdir -p "$agents" "$repo/.kanna" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "planner_resumes line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  python3 -I "$harness/mock_server.py" --log "$agents/mock.jsonl" >"$agents/port" 2>"$agents/mock.log" &
  check wait_for 10 eval '[[ -s "$agents/port" ]]' || { echo "planner_resumes: the mock model didn't start"; return 1; }
  local port agent real; port="$(cat "$agents/port")"
  for agent in claude codex; do
    [[ $agent == claude ]] && real="$claude" || real="$codex"
    python3 -I "$harness/isolate.py" --agent $agent --real "$real" --home "$agents/$agent-home" --port "$port" --trust "$repo" --out "$agents/bin/$agent" || { echo "planner_resumes line $LINENO"; return 1; }
  done
  asked() { python3 -c 'import json, sys
lines = [json.loads(l) for l in open(sys.argv[1])]
print(sum(1 for l in lines if l.get("turn") and l["path"].startswith(sys.argv[2]) and sys.argv[3] in l["prompt"] and (len(sys.argv) < 5 or any(sys.argv[4] in h for h in l["history"]))))' "$agents/mock.jsonl" "$@"; }
  KANNA_PLANNING_AGENTS="$agents/bin/claude $agents/bin/codex" CODEX_HOME="$agents/codex-home/.codex" start_daemon && start_app || { echo "planner_resumes line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  windows() { app_view | python3 -c 'import json, sys; print(" ".join(w["id"] for w in json.load(sys.stdin)["windows"]))'; }
  local role="You are the Kanna planning agent." path w planner old opened note how n
  for agent in claude codex; do
    [[ $agent == claude ]] && path=/v1/messages || path=/v1/responses
    printf '{"version": 1, "planning": {"agent": "%s"}}\n' "$agent" >"$repo/.kanna/config.json"
    opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "planner_resumes line $LINENO: $opened"; return 1; }
    w="${BASH_REMATCH[1]}"
    check wait_for 10 eval '[[ "$(desk lead "$w")" == term-* ]]' || { echo "planner_resumes: no planning terminal"; kanna workspace list --all; return 1; }
    planner="$(desk lead "$w")"
    check wait_for 60 eval '[[ "$(asked "$path" "$role")" == 1 ]]' || { echo "planner_resumes: $agent wasn't told it's the planning agent"; kanna read "$planner"; return 1; }
    check wait_for 30 shows "$planner" "Agent status probe" || { echo "planner_resumes: $agent didn't answer"; kanna read "$planner"; return 1; }
    note="PLANNER_NOTE $agent"
    kanna send "$planner" "$note" >/dev/null && check wait_for 60 eval '[[ "$(asked "$path" "$note" "$role")" -ge 1 ]]' || { echo "planner_resumes: $agent didn't take the note"; kanna read "$planner"; return 1; }
    sleep 3
    for how in closed restarted "a new Desk"; do
      old="$planner"
      case $how in
        closed) kanna close "$planner" >/dev/null || { echo "planner_resumes line $LINENO"; return 1; } ;;
        restarted) kanna restart "$planner" >/dev/null || { echo "planner_resumes line $LINENO"; return 1; } ;;
        *)
          close_desk "$w" || { echo "planner_resumes line $LINENO"; return 1; }
          opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "planner_resumes line $LINENO: $opened"; return 1; }
          w="${BASH_REMATCH[1]}" ;;
      esac
      check wait_for 10 eval '[[ "$(desk lead "$w")" == term-* ]] && { [[ $how == restarted ]] || [[ "$(desk lead "$w")" != "$old" ]]; }' || { echo "planner_resumes: no planning terminal after it was $how"; kanna workspace list --all; return 1; }
      planner="$(desk lead "$w")"
      check wait_for 60 shows "$planner" "$note" || { echo "planner_resumes: $agent's conversation isn't on screen once $how"; kanna read "$planner"; return 1; }
      sleep 3
      kanna send "$planner" "PLANNER_BACK $agent $how" >/dev/null || { echo "planner_resumes line $LINENO"; return 1; }
      check wait_for 60 eval '[[ "$(asked "$path" "PLANNER_BACK $agent $how" "$note")" -ge 1 ]]' || { echo "planner_resumes: $agent's next message once $how didn't carry its conversation"; kanna read "$planner"; tail -2 "$agents/mock.jsonl" | cut -c1-400; return 1; }
      [[ "$(asked "$path" "$role")" == 1 ]] || { echo "planner_resumes: $agent was told it's the planner again once $how"; return 1; }
    done
    n="$(windows | wc -w)"
    key cmd+n
    check wait_for 10 eval '[[ "$(windows | wc -w)" -gt $n ]]' || { echo "planner_resumes: ⌘N opened no Desk"; return 1; }
    local second; second="$(windows | tr ' ' '\n' | grep -vxF "$w" | tail -1)"
    check wait_for 10 eval '[[ "$(desk lead "$second")" == term-* ]]' || { echo "planner_resumes: the second Desk has no planning terminal"; return 1; }
    check wait_for 10 shows "$(desk lead "$second")" "Folder, repo" || { kanna read "$(desk lead "$second")"; return 1; }
    kanna send "$(desk lead "$second")" "$repo" >/dev/null || { echo "planner_resumes line $LINENO"; return 1; }
    check wait_for 60 eval '[[ "$(asked "$path" "$role")" == 2 ]]' || { echo "planner_resumes: a second window on the project didn't start a new conversation"; kanna read "$(desk lead "$second")"; return 1; }
    close_desk "$second" && close_desk "$w" || { echo "planner_resumes line $LINENO"; return 1; }
  done
  stop_agents
}

scenario agent_messaging M4
agent_messaging() {
  start_daemon && start_app || { echo "agent_messaging line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "agent_messaging line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  printf '{"version": 1, "shelf": {"runners": 1}}\n' >"$repo/.kanna/config.json"
  task_state() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0]
print("running" if t.get("running") else "stopped" if t.get("folder") else "waiting", t.get("held") or "-")' "$1"; }
  times() { kanna read "$1" | grep -c -- "$2"; }
  in_repo task new "Busy" "work 4" --agent "$fake/codex" >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  local busy; busy="$(tid 1)"
  check wait_for 10 eval '[[ "$(task_state $busy)" == running* ]]' || { echo "agent_messaging: the task didn't run"; return 1; }
  local term; term="$(task_state $busy | cut -d' ' -f2)"
  check wait_for 10 is "$term" status working || { echo "agent_messaging: the task never showed as working"; kanna status explain "$term"; return 1; }
  local said; said="$(in_repo task send "$busy" "hello while busy")"
  [[ "$said" == "queued for $(sid 1): it's working, so it gets it when its turn is over" ]] || { echo "agent_messaging: send said: $said"; return 1; }
  sleep 1
  check test "$(times "$term" "hello while busy")" -eq 0 || { echo "agent_messaging: the message was typed mid-turn"; kanna read "$term"; return 1; }
  check wait_for 15 shows "$term" "peer: \[message from user\] hello while busy" || { echo "agent_messaging: the busy task didn't get its message at its turn's end"; kanna read "$term"; return 1; }
  check wait_for 15 eval '[[ "$(task_state $busy)" == stopped* ]]' || { echo "agent_messaging: the task wasn't stopped after the turn its message started"; task_state "$busy"; return 1; }
  check test "$(times "$term" "peer: \[message from user\] hello while busy")" -eq 1 || { echo "agent_messaging: the message came more than once"; kanna read "$term"; return 1; }
  [[ "$(in_repo task send "$busy" "wake up")" == *"it's stopped"* ]] || { echo "agent_messaging line $LINENO"; return 1; }
  check wait_for 15 eval '[[ "$(task_state $busy)" == running* ]]' || { echo "agent_messaging: a message didn't wake the stopped Shelf task"; task_state "$busy"; return 1; }
  term="$(task_state $busy | cut -d' ' -f2)"
  check wait_for 15 shows "$term" "peer: \[message from user\] wake up" || { echo "agent_messaging: the woken task didn't get its message"; kanna read "$term"; return 1; }
  shows "$term" "^resumed " || { echo "agent_messaging: the woken task didn't resume its session"; kanna read "$term"; return 1; }
  check wait_for 15 eval '[[ "$(task_state $busy)" == stopped* ]]' || { echo "agent_messaging: the woken task wasn't stopped again"; return 1; }
  in_repo task new "Other" "work 1" --agent "$fake/codex" >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  local other; other="$(tid 2)"
  check wait_for 15 eval '[[ "$(task_state $other)" == stopped* ]]' || { echo "agent_messaging: the second task didn't run and stop"; task_state "$other"; return 1; }
  (cd "$repo" && KANNA_TERMINAL="$term" kanna task send "$other" "from one task to another") >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  check wait_for 15 eval '[[ "$(task_state $other)" == running* ]]' || { echo "agent_messaging line $LINENO"; return 1; }
  local oterm; oterm="$(task_state $other | cut -d' ' -f2)"
  check wait_for 15 shows "$oterm" "peer: \[message from $(sid 1) \"Busy\"\] from one task to another" || { echo "agent_messaging: a task's message didn't name it"; kanna read "$oterm"; return 1; }
  check wait_for 15 eval '[[ "$(task_state $other)" == stopped* ]]' || { echo "agent_messaging line $LINENO"; return 1; }
  local opened planner; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "agent_messaging line $LINENO: $opened"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"${BASH_REMATCH[1]}\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { desk lead; return 1; }
  planner="$(desk lead)"
  wait_for 10 shows "$planner" "Install Kanna's skills? \[Y/n\]" && kanna send "$planner" "n" >/dev/null
  check wait_for 10 shows "$planner" "No agent Kanna knows is installed" || { kanna read "$planner"; return 1; }
  kanna send "$planner" "$bin/kanna-daemon --codex-remote $KANNA_STATE_DIR $fake/codex" >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  check wait_for 10 shows "$planner" "mock-agent:" || { kanna read "$planner"; return 1; }
  (cd "$repo" && KANNA_TERMINAL="$oterm" kanna task send planner "I'm done") >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  check wait_for 15 shows "$planner" "peer: \[message from $(sid 2) \"Other\"\] I'm done" || { echo "agent_messaging: the planner didn't get the task's message"; kanna read "$planner"; in_repo task messages; return 1; }
  (cd "$repo" && KANNA_TERMINAL="$planner" kanna task send "$busy" "from the planner") >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  term="$(task_state $busy | cut -d' ' -f2)"
  check wait_for 20 eval 'term="$(task_state $busy | cut -d" " -f2)"; shows "$term" "peer: \[message from planner\] from the planner"' || { echo "agent_messaging: the task didn't get the planner's message"; kanna read "$term"; return 1; }
  local list; list="$(in_repo task messages "$busy")"
  [[ "$(grep -c delivered <<<"$list")" == 4 && "$list" == *"user"*"hello while busy"* ]] || { echo "agent_messaging: task messages shows: $list"; return 1; }
  in_repo task done "$other" >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  said="$(in_repo task send "$other" "too late" 2>&1)" && { echo "agent_messaging: a done task took a message"; return 1; }
  [[ "$said" == *"is done, so it takes no messages"* ]] || { echo "agent_messaging: send to a done task said: $said"; return 1; }
  mkdir -p "$fake/bin" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/bin/claude"
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$fake/bin/claude" >"$fake/claude" && chmod +x "$fake/claude"
  in_repo task new "Claude" "work 4" --agent "$fake/claude" >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  local claude; claude="$(tid 3)"
  check wait_for 15 eval '[[ "$(task_state $claude)" == running* ]]' || { echo "agent_messaging: the claude task didn't run"; task_state "$claude"; return 1; }
  local cterm; cterm="$(task_state $claude | cut -d' ' -f2)"
  in_repo task send "$claude" "over the socket" >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  check wait_for 15 shows "$cterm" "peer: \[message from user\] over the socket" || { echo "agent_messaging: the claude task didn't get its message on its socket"; kanna read "$cterm"; tail -5 "$out/daemon.log"; return 1; }
  check test "$(times "$cterm" "over the socket")" -eq 1 || { echo "agent_messaging: the claude task's message was typed too"; kanna read "$cterm"; return 1; }
  check wait_for 15 eval '[[ "$(task_state $claude)" == stopped* ]]' || { echo "agent_messaging: the claude task wasn't stopped after the turn its message started"; task_state "$claude"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/agy"
  in_repo task new "Agy" "work 1" --agent "$fake/agy" >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  said="$(in_repo task send "$(tid 4)" "can you hear me")"
  [[ "$said" == *$'\n'"warning: it waits: agy can't take messages: only claude and codex can" ]] || { echo "agent_messaging: send to an agy task said: $said"; return 1; }
  waits() { raw '{"id":1,"cmd":"debug.toasts"}' | python3 -c 'import json, sys
print(" ".join(t["about"] + ":" + (t.get("detail") or "") for t in json.load(sys.stdin)["toasts"] if t["title"] == "Messages wait for Agy"))'; }
  check wait_for 15 eval '[[ "$(waits)" == "Agy:agy can'"'"'t take messages: only claude and codex can" ]]' || { echo "agent_messaging: the waiting message raised no toast about the agy task: $(waits)"; return 1; }
  sleep 2
  check test "$(waits)" == "Agy:agy can't take messages: only claude and codex can" || { echo "agent_messaging: the same reason raised another toast: $(waits)"; return 1; }
  check wait_for 15 eval '[[ "$(task_state "$(tid 4)")" == stopped* ]]' || { echo "agent_messaging: the agy task wasn't stopped at its turn's end"; task_state "$(tid 4)"; return 1; }
  sleep 3
  check test "$(task_state "$(tid 4)" | cut -d' ' -f1)" == stopped || { echo "agent_messaging: the agy task was woken for a message it can't take"; return 1; }
  [[ "$(in_repo task messages "$(tid 4)")" == *"waiting  can you hear me"* ]] || { echo "agent_messaging: task messages shows: $(in_repo task messages "$(tid 4)")"; return 1; }
}

scenario agent_too_old M4
agent_too_old() {
  start_daemon && start_app || { echo "agent_too_old line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake/bin" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "agent_too_old line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  sed 's/"KANNA_MOCK_VERSION", "99.0.0"/"KANNA_MOCK_VERSION", "2.1.290"/' "$repo_root/tools/mock-agent/mock-agent" >"$fake/bin/claude" && chmod +x "$fake/bin/claude" || { echo "agent_too_old line $LINENO"; return 1; }
  printf '{"version": 1, "shelf": {"runners": 1}}\n' >"$repo/.kanna/config.json"
  task_state() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0]
print("running" if t.get("running") else "stopped" if t.get("folder") else "waiting", t.get("stopped") or "-")' "$1"; }
  (cd "$repo" && kanna task new "Old" "work 30" --agent "$fake/bin/claude") >/dev/null || { echo "agent_too_old line $LINENO"; return 1; }
  check wait_for 20 eval '[[ "$(task_state $(tid 1))" == "stopped too-old" ]]' || { echo "agent_too_old: the old claude's task wasn't stopped as too old: $(task_state $(tid 1))"; return 1; }
  refused() { raw '{"id":1,"cmd":"debug.toasts"}' | python3 -c 'import json, sys
print(" ".join(t.get("detail") or "" for t in json.load(sys.stdin)["toasts"] if t["title"].startswith("Kanna can'"'"'t run")))'; }
  check wait_for 10 eval '[[ "$(refused)" == "claude 2.1.290 is too old for Kanna: it needs 2.1.295 or newer"* ]]' || { echo "agent_too_old: no toast said why: $(refused)"; raw '{"id":1,"cmd":"debug.toasts"}'; return 1; }
  sleep 3
  check test "$(task_state $(tid 1))" == "stopped too-old" || { echo "agent_too_old: a runner started it again"; return 1; }
  kanna open --window new --name plain --cwd "$repo" --cmd "$fake/bin/claude" >/dev/null || { echo "agent_too_old line $LINENO"; return 1; }
  check wait_for 10 is plain status idle || { echo "agent_too_old: the old claude in a plain terminal didn't run with its reports"; kanna status explain plain; return 1; }
  sleep 2
  check test "$(pane plain state)" == running || { echo "agent_too_old: the plain terminal's agent was refused"; return 1; }
}
