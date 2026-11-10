scenario cli_terminal M0
cli_terminal() {
  start_daemon || return 1
  kanna open --window new --name cli_terminal >/dev/null || return 1
  kanna send cli_terminal 'echo cli_terminal-$((40+2))' || return 1
  check wait_for 5 shows cli_terminal cli_terminal-42
}

scenario app_typing M0
app_typing() {
  start_daemon && start_app || return 1
  kanna open --window new --name app_typing >/dev/null || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"pane":"app_typing","text":"echo app_typing-$((40+2))\r"}}' >/dev/null
  check wait_for 5 shows app_typing app_typing-42 || return 1
  local drawn
  drawn="$(raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"app_typing.png","marker":"app_typing-42"}}')"
  cp "$KANNA_STATE_DIR/app_typing.png" "$out/app_typing.png" 2>/dev/null
  check python3 -c 'import json, sys
m = [p["marker"] for p in json.loads(sys.argv[1])["panes"] if p.get("marker", {}).get("found")]
assert m and all(x["inked"] == x["glyphs"] > 0 for x in m)' "$drawn"
}

scenario without_app M1
without_app() {
  start_daemon && start_app || return 1
  kanna open --window new --name without_app --cmd 'echo without_app-READY; exec sleep 600' >/dev/null || return 1
  stop_app
  check wait_for 5 shows without_app without_app-READY || return 1
  check is without_app state running
}

scenario survives_relaunch M1
survives_relaunch() {
  start_daemon && start_app || return 1
  kanna open --window new --name survives_relaunch --cmd 'printf "\033[2J\033[3;5Hsurvives_relaunch TOP\033[10;20Hsurvives_relaunch MIDDLE"; exec sleep 600' >/dev/null || return 1
  wait_for 5 shows survives_relaunch 'survives_relaunch MIDDLE' || { echo "survives_relaunch never drew"; return 1; }
  local pid before after
  pid="$(pane survives_relaunch pid)" before="$(kanna read survives_relaunch)"
  stop_app; start_app || return 1
  check is survives_relaunch pid "$pid" || return 1
  after="$(raw '{"id":1,"cmd":"debug.text","args":{"pane":"survives_relaunch"}}' | field text)"
  check test "$after" == "$before"
}

scenario instance_stop M4
instance_stop() {
  start_daemon && start_app || return 1
  kanna open --window new --name left --cmd 'echo LEFT-READY; exec sleep 600' >/dev/null && kanna open --window win-1 --name right --split right --size 1/3 >/dev/null || return 1
  wait_for 5 shows left LEFT-READY || { echo "instance_stop: left never drew"; return 1; }
  local before shell keeper app_was daemon_was saved
  before="$(layout)" shell="$(pane left pid)" keeper="$(cat "$KANNA_STATE_DIR/keeper.pid")" app_was="$app_pid" daemon_was="$daemon_pid"
  saved="$(stat -f %m "$KANNA_STATE_DIR/app-state.json" 2>/dev/null || echo 0)"
  sleep 1
  check test "$(kanna instances stop)" == "stopped default ($KANNA_SOCKET)" || return 1
  check test ! -S "$KANNA_SOCKET" -a ! -e "$KANNA_STATE_DIR/daemon.pid" || return 1
  check wait_for 5 eval '! kill -0 $app_was && ! kill -0 $daemon_was && ! kill -0 $keeper && ! kill -0 $shell' || { echo "instance_stop: still running: $(ps -o pid=,command= -p "$app_was,$daemon_was,$keeper,$shell")"; return 1; }
  wait "$app_pid" "$daemon_pid" 2>/dev/null; app_pid="" daemon_pid=""
  check test "$(stat -f %m "$KANNA_STATE_DIR/app-state.json")" -gt "$saved" || { echo "instance_stop: the app didn't save app-state.json as it quit"; return 1; }
  python3 -c 'import json, sys; sys.exit("win-1" not in json.load(open(sys.argv[1]))["frames"])' "$KANNA_STATE_DIR/app-state.json" || { echo "instance_stop: app-state.json has no frame for win-1"; cat "$KANNA_STATE_DIR/app-state.json"; return 1; }
  python3 -c 'import json, sys; sys.exit(json.load(open(sys.argv[1])).get("order", [None])[0] != "win-1")' "$KANNA_STATE_DIR/app-state.json" || { echo "instance_stop: app-state.json doesn't have win-1 in front"; cat "$KANNA_STATE_DIR/app-state.json"; return 1; }
  start_daemon && start_app || return 1
  check test "$(layout)" == "$before" || return 1
  check is left state exited || return 1
  check test "$(app_view | field key)" == win-1 || return 1
}

scenario layout_restored M1
layout_restored() {
  start_daemon && start_app || return 1
  kanna open --window new --name left >/dev/null && kanna open --window win-1 --name right --split right --size 1/3 >/dev/null || return 1
  desk_task agent && KANNA_TERMINAL=agent kanna open --name own --window new >/dev/null || return 1
  kanna task activate agent >/dev/null || return 1
  local before agent
  before="$(layout)" agent="$(pane agent id)"
  check test "$(ws agent windows)" == '["win-2"]' || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$agent" ]]' || return 1
  stop_app; start_app || return 1
  check test "$(layout)" == "$before" || return 1
  check wait_for 5 eval '[[ "$(ws agent windows)" == "[\"win-2\"]" && "$(ws agent where)" == "{\"desk\": \"mac\"}" && "$(dwin d1 dock.pane)" == "$agent" ]]'
}

scenario daemon_restart M2
daemon_restart() {
  start_daemon && start_app || return 1
  kanna open --window new --name daemon_restart >/dev/null || return 1
  kanna open --window win-1 --name daemon_restartb --split right >/dev/null || return 1
  local pid before
  pid="$(pane daemon_restart pid)" before="$(layout)"
  kanna send daemon_restart 'echo daemon_restart-BEFORE; sleep 2; echo daemon_restart-AFTER' || return 1
  check wait_for 5 shows daemon_restart daemon_restart-BEFORE || return 1
  stop_daemon
  sleep 3
  start_daemon || return 1
  check is daemon_restart pid "$pid" || return 1
  check shows daemon_restart daemon_restart-BEFORE || return 1
  check wait_for 5 shows daemon_restart daemon_restart-AFTER || return 1
  check wait_for 10 app_attached || return 1
  check test "$(layout)" == "$before" || return 1
  app_shows() { raw '{"id":1,"cmd":"debug.text","args":{"pane":"daemon_restart"}}' | field text | grep -q daemon_restart-AFTER; }
  check wait_for 5 app_shows
}

scenario exit_code M2
exit_code() {
  start_daemon || return 1
  kanna open --window new --name exit_code --cmd 'exit 3' >/dev/null || return 1
  check wait_for 5 is exit_code state exited || return 1
  check is exit_code exitCode 3
}

scenario cli_starts_app M1
cli_starts_app() {
  local home="$KANNA_STATE_DIR/home" socket folder instance
  folder="$("$rust_dir/kanna-build" | sed -n 's/^folder=//p')"
  instance="$("$rust_dir/kanna-build" | sed -n 's/^instance=//p')"
  [[ "$instance" == default ]] || folder="$folder-$instance"
  socket="$home/Library/Application Support/$folder/kanna.sock"
  mkdir -p "$(dirname "$socket")"
  KANNA_SOCKET="$socket" start_daemon || return 1
  dev() { env -u KANNA_SOCKET HOME="$home" KANNA_MUX=kanna-dev KANNA_APP="$repo_root/target/Kanna.app" KANNA_BG=1 "$bin/kanna" "$@"; }
  dev open --window new --name left >/dev/null || return 1
  KANNA_SOCKET="$socket" "$bin/kanna" open --window win-1 --name right --split right >/dev/null || return 1
  KANNA_SOCKET="$socket" "$bin/kanna" raw '{"cmd":"debug.app"}' >/dev/null 2>"$KANNA_STATE_DIR/pinned.err"
  check test $? -eq 6 || return 1
  check grep -q "doesn't start Kanna.app" "$KANNA_STATE_DIR/pinned.err" || return 1
  dev raw '{"cmd":"debug.app"}' >/dev/null
  local status=$?
  app_pid="$(sed -n 's/.*the app attached (pid \([0-9]*\)).*/\1/p' "$out/daemon.log" | tail -1)"
  check test "$status" -eq 0 || return 1
  check test "$(dev workspace list --all --json | field windows.0.tabs.0.layout)" == '{"children": [{"pane": "left", "size": "1/2"}, {"pane": "right", "size": "1/2"}], "split": "row"}'
}

scenario app_starts_daemon M3
app_starts_daemon() {
  env -u KANNA_NO_INITIAL_WINDOW "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not start a daemon and attach to it"; return 1; }
  local first
  first="$(cat "$KANNA_STATE_DIR/daemon.pid")"
  check grep -q "Kanna.app/Contents/Helpers/kanna-daemon" <(ps -o command= -p "$first") || return 1
  wait_for 5 eval '[[ "$(kanna workspace list --all --json | field panes.0.type)" == term ]]' || { echo "the app did not open a first terminal"; return 1; }
  kanna open --window new --name live --split right --cmd 'echo LIVE-READY; exec cat' >/dev/null || return 1
  app_shows_live() { raw '{"id":1,"cmd":"debug.text","args":{"pane":"live"}}' | field text | grep -q LIVE-READY; }
  check wait_for 5 app_shows_live || return 1
  local home="$KANNA_AGENTS_HOME"
  mkdir -p "$home/.claude" "$home/.codex" && echo '{"model":"opus"}' >"$home/.claude/settings.json" && echo '{"hooks":{}}' >"$home/.codex/hooks.json"
  check grep -q "you said no" <(kanna agents) || return 1
  check test -z "$(echo y | kanna agents setup --ask)" || { echo "app_starts_daemon: setup --ask asked after you said no"; return 1; }
  check grep -q "Kanna's skills are installed" <(kanna agents setup) || return 1
  check grep -q "kanna task name" "$home/.claude/skills/kanna-tasks/SKILL.md" || return 1
  check test -f "$home/.agents/skills/kanna-planning/SKILL.md" || return 1
  check grep -qF "$KANNA_SOCKET" "$home/.claude/settings.json" || return 1
  ! grep -q SessionStart "$home/.claude/settings.json" || { echo "app_starts_daemon: setup added a hook to claude's settings"; return 1; }
  check test "$(tr -d ' \n' <"$home/.codex/hooks.json")" == '{"hooks":{}}' || { echo "app_starts_daemon: setup changed codex's hooks"; return 1; }
  check grep -q "up to date" <(kanna agents) || return 1
  check test -z "$(echo y | kanna agents update --ask)" || { echo "app_starts_daemon: update --ask asked with nothing new"; return 1; }
  kanna agents remove >/dev/null || return 1
  check test ! -e "$home/.agents/skills/kanna-tasks" || return 1
  check test "$(tr -d ' \n' <"$home/.claude/settings.json")" == '{"model":"opus"}' || return 1
  check test "$(tr -d ' \n' <"$home/.codex/hooks.json")" == '{"hooks":{}}' || return 1
  kill "$first"
  wait_for 15 eval '[[ "$(cat "$KANNA_STATE_DIR/daemon.pid" 2>/dev/null)" != "$first" ]] && app_attached' || { echo "the app did not start a new daemon"; return 1; }
  check test "$(kanna workspace list --all --json | field panes.0.type)" == term || return 1
  stop_app
  check kill -0 "$(cat "$KANNA_STATE_DIR/daemon.pid")" || return 1
}

scenario instances M4
instances() {
  local home="$KANNA_STATE_DIR/home" folder support
  support="$home/Library/Application Support"
  folder="$support/kanna-dev-e2e"
  mkdir -p "$home"
  inst() { env -u KANNA_SOCKET -u KANNA_STATE_DIR -u KANNA_INSTANCE HOME="$home" KANNA_APP="$repo_root/target/Kanna.app" "$bin/kanna" "$@"; }
  check test "$(inst instances)" == "No Kanna instances are running. Any command starts one, e.g. kanna . or kanna --instance work open --window new." || return 1
  inst --instance e2e --bg open --window new --name instances --cmd 'echo "instances $KANNA_INSTANCE"; exec cat' >/dev/null || return 1
  check test -S "$folder/kanna.sock" || return 1
  check grep -q "Kanna.app/Contents/Helpers/kanna-daemon" <(ps -o command= -p "$(cat "$folder/daemon.pid")") || return 1
  check wait_for 5 eval 'inst --instance e2e read instances | grep -q "instances e2e"' || return 1
  check test "$(inst --instance e2e capabilities --json | field instance)" == e2e || return 1
  check wait_for 10 eval '[[ "$(inst --instance e2e capabilities --json | field app)" == True ]]' || return 1
  check wait_for 5 eval 'inst --instance e2e workspace list --all --json | field windows.0.tabs.0.layout | grep -q instances' || return 1
  local listed
  listed="$(inst instances)"
  check grep -q "^\* e2e  dev  1 window, [12] panes\?\( .*\)\?$" <<<"$(inst --instance e2e instances)" || { inst --instance e2e instances; return 1; }
  check grep -q "^  e2e  dev  1 window, [12] panes\?\( .*\)\?$" <<<"$listed" || { echo "$listed"; return 1; }
  check test "$(inst --instance e2e instances --json | field instances.0.current)" == True || return 1
  inst --instance e2e open --window win-1 --name instancesb --split right >/dev/null || return 1
  check test "$(inst --instance e2e workspace list --all --json | field windows.0.tabs.0.layout)" == '{"children": [{"pane": "instances", "size": "1/2"}, {"pane": "instancesb", "size": "1/2"}], "split": "row"}' || return 1
  local daemon keeper
  daemon="$(cat "$folder/daemon.pid")" keeper="$(cat "$folder/keeper.pid")"
  check test "$(inst --instance e2e instances stop)" == "stopped e2e ($folder/kanna.sock)" || return 1
  check test ! -e "$folder/daemon.pid" || return 1
  check wait_for 5 eval '! kill -0 $daemon && ! kill -0 $keeper' || { echo "instances: the stopped instance's daemon or keeper is still running"; return 1; }
  check test "$(inst instances)" == "No Kanna instances are running. Any command starts one, e.g. kanna . or kanna --instance work open --window new."
}

scenario daemon_upgrade M4
daemon_upgrade() {
  local old="$KANNA_STATE_DIR/old"
  mkdir -p "$old" && cp "$bin/kanna-daemon" "$old/kanna-daemon" || { echo "daemon_upgrade line $LINENO"; return 1; }
  codesign -f -s - -i dev.kanna.e2e.old-daemon "$old/kanna-daemon" 2>/dev/null || { echo "daemon_upgrade: can't re-sign a copy of the daemon as an older build"; return 1; }
  KANNA_KEEPER="$bin/kanna-keeper" "$old/kanna-daemon" >>"$out/daemon.log" 2>&1 &
  daemon_pid=$!
  wait_for 5 test -S "$KANNA_SOCKET" || { echo "the old daemon did not open its socket"; return 1; }
  local new_build old_build
  new_build="$(shasum -a 256 "${app%/MacOS/Kanna}/Helpers/kanna-daemon" | cut -d' ' -f1)"
  old_build="$(raw '{"id":1,"cmd":"capabilities"}' | field build)"
  [[ "$old_build" == "$(shasum -a 256 "$old/kanna-daemon" | cut -d' ' -f1)" && "$old_build" != "$new_build" ]] || { echo "daemon_upgrade: the old daemon's build is $old_build"; return 1; }
  kanna open --window new --name daemon_upgrade --cmd 'echo daemon_upgrade-BEFORE; exec cat' >/dev/null || return 1
  check wait_for 5 shows daemon_upgrade daemon_upgrade-BEFORE || return 1
  local pid; pid="$(pane daemon_upgrade pid)"
  [[ "$(kanna send nosuch x 2>&1)" == *"from another build than this kanna"* ]] || { echo "daemon_upgrade: the CLI didn't note the old daemon"; return 1; }
  "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 eval "! kill -0 $daemon_pid" || { echo "daemon_upgrade: the old daemon didn't stop for the new app"; return 1; }
  wait "$daemon_pid" 2>/dev/null; daemon_pid=""
  wait_for 15 app_attached || { echo "daemon_upgrade: the app did not start its daemon and attach"; return 1; }
  check grep -q "Kanna.app/Contents/Helpers/kanna-daemon" <(ps -o command= -p "$(cat "$KANNA_STATE_DIR/daemon.pid")") || return 1
  check test "$(raw '{"id":1,"cmd":"capabilities"}' | field build)" == "$new_build" || return 1
  check is daemon_upgrade pid "$pid" || return 1
  kanna send daemon_upgrade 'daemon_upgrade-AFTER' || return 1
  check wait_for 5 shows daemon_upgrade daemon_upgrade-AFTER || return 1
  kanna open --window win-1 --name daemon_upgradeb --split right >/dev/null || return 1
  check app_text daemon_upgrade daemon_upgrade-BEFORE || return 1
  [[ "$(kanna send nosuch x 2>&1)" == *"another build"* ]] && { echo "daemon_upgrade: the CLI notes a daemon of its own build"; return 1; }
  return 0
}

scenario login_shell M4
login_shell() {
  local zdot="$KANNA_STATE_DIR/zdot" nvm="$KANNA_STATE_DIR/nvm/bin" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$zdot" "$nvm" "$repo" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$nvm/codex" || { echo "login_shell line $LINENO"; return 1; }
  printf 'export login_shell_ZSHRC=from-zshrc\npath=(%q $path)\necho login_shell zshrc says hello\n' "$nvm" >"$zdot/.zshrc"
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "login_shell line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  SHELL=/bin/zsh ZDOTDIR="$zdot" start_daemon && start_app || { echo "login_shell line $LINENO"; return 1; }
  kanna open --window new --name login_shell --cmd 'echo "login_shell:$login_shell_ZSHRC:$(command -v codex)"; exec cat' >/dev/null || return 1
  check wait_for 5 shows login_shell "login_shell:from-zshrc:$nvm/codex" || { echo "login_shell: a command pane didn't read .zshrc"; kanna read login_shell; return 1; }
  local id; id="$(cd "$repo" && kanna task new "Codex from nvm" "login_shell first message" --agent codex | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
  [[ "$id" =~ ^task-[0-9a-f]{4,8}$ ]] || { echo "login_shell: the task wasn't added"; return 1; }
  kanna task start "$id" >/dev/null || { echo "login_shell: the task didn't start"; return 1; }
  check wait_for 10 unwrapped "Codex from nvm" "Your task: login_shell first message" || { echo "login_shell: codex from .zshrc's PATH didn't start"; kanna read "Codex from nvm"; return 1; }
  kanna read "Codex from nvm" | grep -q "command not found" && { echo "login_shell: the task's shell didn't find codex"; return 1; }
  return 0
}

scenario keeper_upgrade M4
keeper_upgrade() {
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}"
  [[ -x "$claude" ]] || { echo "keeper_upgrade: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  need "$bin/kanna-daemon" && need "$bin/kanna-keeper" || return 1
  local old="$KANNA_STATE_DIR/old" agents="$KANNA_STATE_DIR/agents" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" harness="$repo_root/tools/agent-harness"
  mkdir -p "$old" "$agents" "$repo" && cp "$bin/kanna-keeper" "$old/kanna-keeper" || { echo "keeper_upgrade line $LINENO"; return 1; }
  codesign -f -s - -i dev.kanna.e2e.old-keeper "$old/kanna-keeper" 2>/dev/null || { echo "keeper_upgrade: can't re-sign a copy of the keeper as an older build"; return 1; }
  [[ "$(shasum -a 256 "$old/kanna-keeper" | cut -d' ' -f1)" != "$(shasum -a 256 "$bin/kanna-keeper" | cut -d' ' -f1)" ]] || { echo "keeper_upgrade: the old keeper is the same build"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "keeper_upgrade line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  KANNA_KEEPER="$old/kanna-keeper" "$bin/kanna-daemon" >>"$out/daemon.log" 2>&1 &
  daemon_pid=$!
  wait_for 5 test -S "$KANNA_SOCKET" || { echo "keeper_upgrade: the daemon did not open its socket"; return 1; }
  start_app || { echo "keeper_upgrade line $LINENO"; return 1; }
  local keeper; keeper="$(cat "$KANNA_STATE_DIR/keeper.pid")"
  [[ "$(ps -o command= -p "$keeper")" == "$old/kanna-keeper"* ]] || { echo "keeper_upgrade: the old keeper isn't running: $(ps -o command= -p "$keeper")"; return 1; }
  python3 -I "$harness/mock_server.py" --log "$agents/mock.jsonl" >"$agents/port" 2>"$agents/mock.log" &
  check wait_for 10 eval '[[ -s "$agents/port" ]]' || { echo "keeper_upgrade: the mock model didn't start"; return 1; }
  python3 -I "$harness/isolate.py" --agent claude --real "$claude" --home "$agents/claude-home" --port "$(cat "$agents/port")" --trust "$repo" --out "$agents/bin/claude" || { echo "keeper_upgrade line $LINENO"; return 1; }
  asked() { python3 -c 'import json, sys
lines = [json.loads(l) for l in open(sys.argv[1])]
print(sum(1 for l in lines if l.get("turn") and sys.argv[2] in l["prompt"] and (len(sys.argv) < 4 or any(sys.argv[3] in h for h in l["history"]))))' "$agents/mock.jsonl" "$@"; }
  local first="UPGRADE_FIRST claude" second="UPGRADE_SECOND claude" id parked
  id="$(in_repo task new "claude task" "$first" --agent "$agents/bin/claude" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
  parked="$(in_repo task new "Parked" "say hi" --agent "$repo_root/tools/mock-agent/mock-agent" | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
  [[ "$id" =~ ^task-[0-9a-f]{4,8}$ && "$parked" =~ ^task-[0-9a-f]{4,8}$ ]] || { echo "keeper_upgrade: the tasks weren't added"; return 1; }
  kanna task pick "$id" >/dev/null && kanna task activate "claude task" >/dev/null || { echo "keeper_upgrade: the claude task didn't reach the Desk"; return 1; }
  kanna task pick "$parked" >/dev/null || { echo "keeper_upgrade: the parked task didn't go to the Shelf"; return 1; }
  check wait_for 60 eval '[[ "$(asked "$first")" == 1 ]]' || { echo "keeper_upgrade: claude's first message didn't reach the mock model"; kanna read "claude task"; return 1; }
  check wait_for 30 shows "claude task" "Agent status probe" || { echo "keeper_upgrade: claude didn't show the mock's answer"; kanna read "claude task"; return 1; }
  in_repo open --window new --name upgradeshell --tab >/dev/null || { echo "keeper_upgrade line $LINENO"; return 1; }
  kanna send upgradeshell 'echo UPGRADE-BEFORE-$((6*7))' >/dev/null || return 1
  check wait_for 5 shows upgradeshell UPGRADE-BEFORE-42 || return 1
  check wait_for 5 is Parked state running || return 1
  local pane pids=()
  for pane in "claude task" Parked upgradeshell; do pids+=("$(pane "$pane" pid)"); done
  stop_app; stop_daemon
  local lines; lines="$(wc -l <"$out/daemon.log")"
  start_daemon && start_app || { echo "keeper_upgrade line $LINENO"; return 1; }
  local log; log="$(tail -n +"$((lines + 1))" "$out/daemon.log")"
  grep -q "upgraded the keeper (pid $keeper) from build" <<<"$log" || { echo "keeper_upgrade: the daemon didn't upgrade the keeper"; echo "$log"; return 1; }
  grep -qE "took back [3-9] terminal\(s\) from the keeper \(0 the disk didn't know\), 0 that had exited; 0 gone" <<<"$log" || { echo "keeper_upgrade: the daemon didn't take every terminal back"; echo "$log"; return 1; }
  check test "$(cat "$KANNA_STATE_DIR/keeper.pid")" == "$keeper" || return 1
  [[ "$(ps -o command= -p "$keeper")" == "$(cd "$bin" && pwd -P)/kanna-keeper"* ]] || { echo "keeper_upgrade: the keeper runs $(ps -o command= -p "$keeper")"; return 1; }
  local i=0
  for pane in "claude task" Parked upgradeshell; do
    check is "$pane" pid "${pids[$i]}" || return 1
    check is "$pane" state running || return 1
    [[ "$(ps -o ppid= -p "${pids[$i]}" | tr -d ' ')" == "$keeper" ]] || { echo "keeper_upgrade: $pane's parent isn't the keeper"; return 1; }
    i=$((i + 1))
  done
  check app_text upgradeshell UPGRADE-BEFORE-42 || return 1
  check shows "claude task" "Agent status probe" || return 1
  kanna send "claude task" "$second" && kanna send-key "claude task" return || { echo "keeper_upgrade line $LINENO"; return 1; }
  check wait_for 60 eval '[[ "$(asked "$second" "$first")" -ge 1 ]]' || { echo "keeper_upgrade: claude's next message didn't carry its conversation"; kanna read "claude task"; return 1; }
  [[ "$(asked "$first")" == 1 ]] || { echo "keeper_upgrade: claude started over instead of carrying on"; return 1; }
  kanna send upgradeshell 'exit 7' >/dev/null || return 1
  check wait_for 5 is upgradeshell state exited || return 1
  check is upgradeshell exitCode 7 || return 1
  stop_agents
}

scenario keeper_lost M4
keeper_lost() {
  local me=keeper_lost repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" ran="$KANNA_STATE_DIR/merge-ran"
  mkdir -p "$repo/.kanna" "${agent%/*}" && ln -s "$mock_agent" "$agent" || { echo "$me line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" config user.email e@e && git -C "$repo" config user.name e || { echo "$me line $LINENO"; return 1; }
  printf '{"version": 1, "merge": {"test": "if [ -e %s ]; then exit 0; fi; touch %s; sleep 900"}}\n' "$ran" "$ran" >"$repo/.kanna/config.json"
  printf '.kanna/tmp/\n' >"$repo/.gitignore"
  git -C "$repo" add -A && git -C "$repo" commit -qm first || { echo "$me line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon && start_app || { echo "$me line $LINENO"; return 1; }
  (cd "$repo" && kanna .) >/dev/null || { echo "$me line $LINENO"; return 1; }
  in_repo() { (cd "$repo" && kanna "$@"); }
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  row() { in_repo task list | awk -v name="$1" '$2 == name { print $3 }'; }
  session() { pane "$1" cmd | grep -oE -- "--resume '[0-9a-f-]{36}'" | grep -oE '[0-9a-f-]{36}'; }
  resumed() {
    local name="$1" id="$2" was="$3"
    check wait_for 20 eval 'is "$name" state running && shows "$name" "resumed $was"' || { echo "$me: $name didn't resume its session $was"; kanna read "$name"; return 1; }
    check test "$(pane "$name" id) $(session "$name")" == "$id $was" || { echo "$me: $name is $(pane "$name" id) with session $(session "$name"), not $id with $was"; return 1; }
    kanna read "$name" | tr -d '\r\n' | grep -q -- "args: --dangerously-skip-permissions --resume $was" || { echo "$me: $name's agent didn't start with its resuming command"; kanna read "$name"; return 1; }
    ! kanna read "$name" | grep -qE "^session |already in use|No conversation found" || { echo "$me: $name started a new session"; kanna read "$name"; return 1; }
    check wait_for 10 is "$name" status idle || { echo "$me: $name, resumed, is $(pane "$name" status), not idle"; return 1; }
  }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { kanna workspace list --all; return 1; }
  local name n
  for name in Ready Busy Active Stopped Exited Queued; do
    in_repo task new "$name" "$([[ "$name" == Busy ]] && echo "work 900" || echo "say hi")" --agent "$agent" >/dev/null || { echo "$me line $LINENO"; return 1; }
  done
  for n in 1 2 3; do kanna task pick "$(tid $n)" >/dev/null || { echo "$me line $LINENO"; return 1; }; done
  kanna task activate "$(tid 3)" >/dev/null || { echo "$me line $LINENO"; return 1; }
  for n in 4 5 6; do kanna task start "$(tid $n)" >/dev/null || { echo "$me line $LINENO"; return 1; }; done
  for name in Ready Active Stopped Exited Queued; do check wait_for 15 eval 'shows "$name" SAID && is "$name" status idle' || { echo "$me: $name didn't start"; kanna read "$name"; return 1; }; done
  check wait_for 10 is Busy status working || { echo "$me: Busy isn't working"; return 1; }
  (cd "$repo/.kanna-worktrees/$(tid 1)" && echo ready >ready.txt && git add -A && git commit -qm ready) && kanna task ready "$(tid 1)" >/dev/null || { echo "$me line $LINENO"; return 1; }
  kanna task stop "$(tid 4)" >/dev/null && check wait_for 10 is Stopped state exited || { echo "$me: Stopped didn't stop"; return 1; }
  kanna send Exited exit >/dev/null && check wait_for 10 is Exited state exited || { echo "$me: Exited's agent didn't exit"; kanna read Exited; return 1; }
  (cd "$repo/.kanna-worktrees/$(tid 6)" && echo queued >queued.txt && git add -A && git commit -qm queued) && in_repo task merge "$(tid 6)" -m "queued: add it" >/dev/null || { echo "$me line $LINENO"; return 1; }
  check wait_for 30 test -e "$ran" || { echo "$me: the queue's tests didn't start"; in_repo task list; return 1; }
  machine_runners 1
  in_repo task new Runner "work 900" --agent "$agent" >/dev/null || { echo "$me line $LINENO"; return 1; }
  check wait_for 15 eval 'is Runner state running && is Runner status working' || { echo "$me: a runner didn't start Runner"; in_repo task list; return 1; }
  check test "$(row Ready) $(row Busy) $(row Active) $(row Stopped) $(row Exited) $(row Runner) $(row Queued)" == "ready working idle stopped stopped working testing" || { echo "$me: before the keeper is lost"; in_repo task list; return 1; }
  local planner keeper active ids="" sessions=""; planner="$(desk window.lead.pane)" active="$(desk window.active)" keeper="$(cat "$KANNA_STATE_DIR/keeper.pid")"
  for name in Ready Busy Active Runner; do ids="$ids $(pane "$name" id)" sessions="$sessions $(session "$name")"; done
  [[ "$(wc -w <<<"$sessions")" -eq 4 && "$active" == "$(pane Active id)" ]] || { echo "$me: the tasks' sessions are [$sessions], the active task $active"; return 1; }
  [[ "$keeper" =~ ^[0-9]+$ ]] && [[ "$(ps -o command= -p "$keeper")" == *"$KANNA_STATE_DIR"* ]] || { echo "$me: $keeper isn't this run's keeper"; return 1; }
  stop_app; stop_daemon
  kill "$keeper"
  check wait_for 5 eval '! kill -0 $keeper' || { echo "$me: this run's keeper didn't stop"; return 1; }
  start_daemon && start_app || { echo "$me line $LINENO"; return 1; }
  local i=1
  for name in Ready Busy Active Runner; do
    resumed "$name" "$(cut -d' ' -f$((i + 1)) <<<"$ids")" "$(cut -d' ' -f$((i + 1)) <<<"$sessions")" || return 1
    i=$((i + 1))
  done
  check test "$(row Ready) $(row Busy) $(row Active) $(row Runner)" == "ready idle idle idle" || { echo "$me: the resumed tasks' rows"; in_repo task list; return 1; }
  check test "$(desk window.active)" == "$active" || { echo "$me: the active task was $active and is $(desk window.active)"; return 1; }
  grep -q "$(tid 7)'s agent ended with the keeper, so it runs on the Shelf again (1 of 1 here" "$out/daemon.log" || { echo "$me: Runner wasn't started again by a runner"; grep "$(tid 7)" "$out/daemon.log" | tail -5; return 1; }
  check test "$(pane Stopped state) $(pane Exited state) $(row Stopped) $(row Exited)" == "exited exited stopped stopped" || { echo "$me: a task that was stopped, or whose agent had exited, started"; in_repo task list; return 1; }
  check wait_for 60 git -C "$repo" cat-file -e main:queued.txt || { echo "$me: the queued merge didn't land after its tests ran again"; in_repo task list --include-done; grep -n "interrupted\|$(tid 6)" "$out/daemon.log" | tail -8; return 1; }
  grep -q "test visit lost its terminal, so it's recorded interrupted and runs again" "$out/daemon.log" || { echo "$me: the lost test run wasn't recorded interrupted"; return 1; }
  kanna task park Active >/dev/null || { echo "$me line $LINENO"; return 1; }
  check wait_for 15 eval '[[ "$(desk lead)" == term-* ]] && is "$(desk lead)" state running' || { echo "$me: the Desk got no running planner: $(desk lead)"; kanna workspace list --all; return 1; }
  check test "$(desk lead)" != "$planner" || { echo "$me: the lost planner pane came back"; return 1; }
  check test "$(pane Stopped state) $(pane Exited state)" == "exited exited" || { echo "$me: a stopped task started later"; return 1; }
}

shows_file() { kanna workspace list --all --json | python3 -c 'import json, sys; sys.exit(not any(p.get("path") == sys.argv[1] for p in json.load(sys.stdin)["panes"]))' "$1"; }

scenario crash_app_draw M4
crash_app_draw() {
  start_daemon && start_app || { echo "crash_app_draw line $LINENO"; return 1; }
  kanna open --window new --name crashing --cmd 'exec /bin/sh' >/dev/null || { echo "crash_app_draw line $LINENO"; return 1; }
  local pane; pane="$(pane crashing id)"
  check wait_for 10 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.app\"}" | field windows)" == *win-* ]]' || { echo "crash_app_draw: the window didn't open"; return 1; }
  local crashed="$app_pid"
  check test "$(raw '{"id":1,"cmd":"debug.crash"}' | field crashing)" == True || return 1
  local record=""
  check wait_for 20 eval 'record="$(ls "$KANNA_STATE_DIR"/crashes/*-app.json 2>/dev/null | head -1)"; [[ -n "$record" ]]' || { echo "crash_app_draw: no crash record"; tail -5 "$out/app.log"; return 1; }
  python3 - "$record" "$crashed" "$pane" <<'PY' || { echo "crash_app_draw: the record"; cat "$record"; return 1; }
import json, sys
r = json.load(open(sys.argv[1]))
site = [f for f in r["stack"]]
while site and (site[0]["image"] in ("CoreFoundation", "libobjc.A.dylib") or "Kanna.CrashHandlers." in site[0]["symbol"]):
    site.pop(0)
assert (r["process"], r["kind"], r["pid"]) == ("app", "exception", int(sys.argv[2])), r
assert r["reason"] == "NSInternalInconsistencyException: debug.crash in a draw", r["reason"]
assert "CrashView.draw" in site[0]["symbol"], site[:3]
assert any("drawRect" in f["symbol"] or "display" in f["symbol"].lower() for f in site[:40]), "no drawing frames"
assert r["build"]["channel"] == "dev" and len(r["build"]["id"] or "") == 64, r["build"]
assert sys.argv[3] in [p["id"] for p in r["panes"]], r["panes"]
assert len(r["signature"]) == 16
PY
  check wait_for 30 eval '! kill -0 '"$crashed"' 2>/dev/null || [[ "$(ps -o stat= -p '"$crashed"')" == Z* ]]' || { echo "crash_app_draw: the app didn't die"; return 1; }
  wait "$crashed" 2>/dev/null
  check wait_for 40 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.app\"}" | field pid)" =~ ^[0-9]+$ && "$(raw "{\"id\":1,\"cmd\":\"debug.app\"}" | field pid)" != "$crashed" ]]' || { echo "crash_app_draw: the app wasn't relaunched"; grep "kanna-daemon: the app" "$out/daemon.log" | tail -5; return 1; }
  grep -q "kanna-daemon: the app crashed: NSInternalInconsistencyException: debug.crash in a draw ($record); relaunching it in 1s" "$out/daemon.log" || { echo "crash_app_draw: the daemon didn't log the crash"; grep "kanna-daemon: the app" "$out/daemon.log" | tail -5; return 1; }
  local relaunched; relaunched="$(raw '{"id":1,"cmd":"debug.app"}' | field pid)"
  check wait_for 10 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.app\"}" | field windows)" == *win-* ]]' || { echo "crash_app_draw: the relaunched app didn't restore its window"; return 1; }
  banner() { raw '{"id":1,"cmd":"debug.toasts"}' | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin)["toasts"] if t.get("record")]
print(t[0][sys.argv[1]] if t else "")' "$1"; }
  check wait_for 10 eval '[[ "$(banner title)" == "Kanna crashed: NSInternalInconsistencyException: debug.crash in a draw" ]]' || { echo "crash_app_draw: no banner: $(raw '{"id":1,"cmd":"debug.toasts"}')"; return 1; }
  check test "$(banner record)" == "$record" -a "$(banner sticky)" == True || return 1
  [[ "$(banner detail)" == "NSInternalInconsistencyException: debug.crash in a draw · Kanna.app at "* ]] || { echo "crash_app_draw: the banner's detail: $(banner detail)"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"png":"crash-banner.png"}}' >/dev/null && cp "$KANNA_STATE_DIR/crash-banner.png" "$out/crash-banner.png"
  local listed ips=""
  if wait_for 60 eval 'listed="$(kanna crashes)"; [[ "$listed" == *"  report  "*".ips"* ]]'; then
    ips="$(sed -n 's/^  report  //p' <<<"$listed" | head -1)"; ips="${ips/#\~/$HOME}"
    [[ "$(python3 -c 'import json, sys; print(json.loads(open(sys.argv[1]).read().split("\n", 1)[1])["pid"])' "$ips")" == "$crashed" ]] || { echo "crash_app_draw: $ips isn't the crashed app's"; return 1; }
  else
    echo "crash_app_draw: note: macOS wrote no .ips for pid $crashed within 60s (ReportCrash drops some when it's busy), so the report isn't checked"
  fi
  [[ "$listed" == *"  app (pid $crashed, dev "*")  NSInternalInconsistencyException: debug.crash in a draw"* && "$listed" == *"  record  "*"/crashes/$(basename "$record")"* && "$listed" == *"CrashView.draw"* ]] || { echo "crash_app_draw: kanna crashes: $listed"; return 1; }
  local told; told="$(raw '{"id":1,"cmd":"tasks","args":{"action":"messages"}}' | python3 -c 'import json, sys
m = [m for m in json.load(sys.stdin)["messages"] if m["from"] == "kanna" and m["to"] == "planner"]
print(len(m)); print(m[0]["repo"] if m else ""); print(m[0]["text"] if m else "")')"
  [[ "$(head -1 <<<"$told")" == 1 && "$(sed -n 2p <<<"$told")" == "$(cd "$(git -C "$repo_root" rev-parse --git-common-dir)/.." && pwd -P)" ]] || { echo "crash_app_draw: the planner's message: $told"; return 1; }
  [[ "$told" == *"Kanna crashed: app (pid $crashed): NSInternalInconsistencyException: debug.crash in a draw"* && "$told" == *"record: $record"* && "$told" == *"CrashView.draw"* ]] || { echo "crash_app_draw: the message says: $told"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"click":"'"$(banner id)"'"}}' >/dev/null
  check wait_for 10 shows_file "$record" || { echo "crash_app_draw: clicking the banner didn't open the record"; kanna workspace list --all; return 1; }
  check wait_for 5 eval '[[ -z "$(banner id)" ]]' || { echo "crash_app_draw: the banner stayed"; return 1; }
  check wait_for 5 eval '[[ "$(kanna crashes)" == "no crashes"* ]]' || { echo "crash_app_draw: the clicked banner didn't dismiss the record"; kanna crashes; return 1; }
  stop_pid "$relaunched"
  check wait_for 10 eval 'grep -q "kanna-daemon: the app quit" "$out/daemon.log"' || { echo "crash_app_draw: quitting the relaunched app wasn't a clean quit"; return 1; }
  rm -f "$record" ${ips:+"$ips"}
}

scenario crash_daemon_panic M4
crash_daemon_panic() {
  start_daemon && start_app || { echo "crash_daemon_panic line $LINENO"; return 1; }
  local crashed="$daemon_pid"
  raw '{"id":1,"cmd":"debug.panic"}' >/dev/null 2>&1
  wait "$crashed" 2>/dev/null; daemon_pid=""
  local record; record="$(ls "$KANNA_STATE_DIR"/crashes/*-daemon.json 2>/dev/null | head -1)"
  [[ -n "$record" ]] || { echo "crash_daemon_panic: no crash record"; tail -5 "$out/daemon.log"; return 1; }
  python3 - "$record" "$crashed" <<'PY' || { echo "crash_daemon_panic: the record"; cat "$record"; return 1; }
import json, sys
r = json.load(open(sys.argv[1]))
assert (r["process"], r["kind"], r["pid"], r["executable"]) == ("daemon", "panic", int(sys.argv[2]), "kanna-daemon"), r
assert r["reason"].startswith("debug.panic in the daemon's main loop at crates/kanna-daemon/src/crashes.rs:"), r["reason"]
assert any("kanna_daemon::crashes" in f["symbol"] for f in r["stack"]), r["stack"]
PY
  check wait_for 30 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.app\"}" | field attached)" == True ]]' || { echo "crash_daemon_panic: the app didn't start another daemon"; return 1; }
  check test "$(raw '{"id":1,"cmd":"debug.app"}' | field pid)" == "$app_pid" || { echo "crash_daemon_panic: the app was restarted"; return 1; }
  banner() { raw '{"id":1,"cmd":"debug.toasts"}' | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin)["toasts"] if t.get("record")]
print(t[0][sys.argv[1]] if t else "")' "$1"; }
  check wait_for 10 eval '[[ "$(banner title)" == "Kanna crashed: debug.panic in the daemon"* && "$(banner record)" == "$record" ]]' || { echo "crash_daemon_panic: no banner: $(raw '{"id":1,"cmd":"debug.toasts"}')"; return 1; }
  [[ "$(banner detail)" == "debug.panic in the daemon's main loop at "*" · the daemon at "* ]] || { echo "crash_daemon_panic: the banner's detail: $(banner detail)"; return 1; }
  [[ "$(kanna crashes)" == *"  daemon (pid $crashed, dev "*"debug.panic in the daemon's main loop"*"  report  none"* ]] || { echo "crash_daemon_panic: kanna crashes: $(kanna crashes)"; return 1; }
  raw '{"id":1,"cmd":"tasks","args":{"action":"messages"}}' | grep -q "Kanna crashed: daemon (pid $crashed)" || { echo "crash_daemon_panic: the planner wasn't told"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"click":"'"$(banner id)"'","close":true}}' >/dev/null
  check wait_for 5 eval '[[ "$(kanna crashes)" == "no crashes"* && -z "$(banner id)" ]]' || { echo "crash_daemon_panic: × didn't dismiss it"; return 1; }
  shows_file "$record" && { echo "crash_daemon_panic: × opened the record"; return 1; }
  rm -f "$record"
}
