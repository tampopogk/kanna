#!/usr/bin/env bash
set -uo pipefail

usage() {
  cat <<'USAGE'
Kanna's end-to-end scenarios: the reference for docs/plans/mux-build.md.
Each scenario is one behaviour of the finished system; the plan makes them
pass milestone by milestone. Scenarios are named for their behaviour, never
numbered, so tasks adding them in parallel can't collide: a function NAME()
with `scenario NAME MILESTONE` on the line above it. Runs a private Kanna
(daemon, keeper, app in the background) on its own socket and folder, never
yours, and stops it by PID.

usage: scripts/e2e-kanna.sh [--through M0|M1|M2|M3|M4] [NAME …]
  --through M  run the scenarios due by milestone M (default: all);
               later ones are listed as not due
  NAME…        only these scenarios (e.g. cli_terminal desk_zoom)

A scenario that runs longer than $KANNA_E2E_TIMEOUT seconds (600) fails.
Every run stops what it started however it ends, and first stops what runs
it can't see anymore left behind (docs/specs/e2e-cleanup.md). Run it
unsandboxed: it needs ps, signals, Unix sockets and /tmp.

Uses target/Kanna.app (or $KANNA_E2E_APP) and the kanna, kanna-daemon and kanna-keeper inside it,
and target/PROFILE/kanna-build (debug in a worktree, release in the main checkout: docs/specs/build-store.md)
(scripts/build.sh). The private instance gets:
  KANNA_SOCKET     the daemon's socket (the CLI and the app connect to it)
  KANNA_STATE_DIR  where the daemon, keeper and app keep their files
                   (the keeper writes keeper.pid there)
  KANNA_KEEPER     the keeper binary the daemon starts (once it uses one)
real_agent_tasks, real_agent_planners, real_agent_program_status and keeper_upgrade run the real claude and codex (on PATH,
or $KANNA_E2E_CLAUDE / $KANNA_E2E_CODEX) against tools/agent-harness's mock
model, each in a HOME of its own. A planning terminal
offers no agent (KANNA_PLANNING_AGENTS=none) unless a scenario gives it some. Shelf tasks
don't run by themselves (shelf.runners is 0) unless a scenario's repo sets it.
USAGE
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="${KANNA_E2E_APP:-$repo_root/target/Kanna.app}/Contents/MacOS/Kanna"
bin="${app%/MacOS/Kanna}/Helpers"
rust_dir="$(. "$repo_root/scripts/env.sh" && kanna_rust_dir "$repo_root")"
out="$repo_root/target/e2e-kanna"
mkdir -p "$out"

scenarios=()
scenario() { scenarios+=("$1:$2"); }
due() {
  local s
  for s in "${scenarios[@]}"; do [[ "${s%%:*}" == "$1" ]] && { echo "${s#*:}"; return 0; }; done
  return 1
}

through=M4 only=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --through) through="$2"; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "e2e-kanna: unknown argument $1 (see --help)" >&2; exit 2 ;;
    *) only+=("$1") ;;
  esac
  shift
done

unset KANNA_PANE KANNA_TERMINAL KANNA_INSTANCE
ps -o pid= -p $$ >/dev/null 2>&1 && kill -0 $$ 2>/dev/null || { echo "e2e-kanna: can't list or signal processes here (a sandbox?); run it unsandboxed" >&2; exit 2; }
export KANNA_E2E_OWNER=$$
mock_agent="$repo_root/tools/mock-agent/mock-agent"
new_state_dir() {
  KANNA_STATE_DIR="$(mktemp -d /tmp/kanna-e2e.XXXXXX)" || exit 1
  echo $$ >"$KANNA_STATE_DIR/e2e-kanna.pid"
  export KANNA_AGENTS_HOME="$KANNA_STATE_DIR/home"
  [[ -n "$KANNA_STATE_DIR" && -d "$KANNA_STATE_DIR" ]] || { echo "e2e-kanna: mktemp gave no folder" >&2; exit 1; }
  export KANNA_STATE_DIR KANNA_SOCKET="$KANNA_STATE_DIR/kanna.sock"
  mkdir -p "$KANNA_AGENTS_HOME/.kanna-v4" && echo '{"version": 2, "setup": false}' >"$KANNA_AGENTS_HOME/.kanna-v4/agents.json"
  printf '{"version": 1, "shelf": {"runners": 0}, "tasks": {"agent": "%s"}, "agents": {"prompts": "skip"}}\n' "$mock_agent" >"$KANNA_AGENTS_HOME/.kanna-v4/config.json"
}
KANNA_STATE_DIR=""
export KANNA_IGNORE_OCCLUSION=1
export KANNA_DEBUG=1
export KANNA_TEST_EFFECTS=1
export KANNA_NO_INITIAL_WINDOW=1
export KANNA_PLANNING_AGENTS=none
export KANNA_AGENT_SKILLS="${app%/MacOS/Kanna}/Resources/kanna-skills"
daemon_pid="" app_pid="" scenario_pid=""

need() { [[ -x "$1" ]] || { echo "not built: ${1#$repo_root/}"; return 1; }; }
wait_for() {
  local tries=$(($1 * 10)); shift
  for _ in $(seq "$tries"); do
    kill -0 "$KANNA_E2E_OWNER" 2>/dev/null || exit 1
    "$@" 2>/dev/null && return 0; sleep 0.1
  done
  return 1
}
start_daemon() {
  need "$bin/kanna-daemon" || return 1
  if "$bin/kanna-daemon" --uses-keeper; then
    need "$bin/kanna-keeper" || return 1
    KANNA_KEEPER="$bin/kanna-keeper" "$bin/kanna-daemon" >>"$out/daemon.log" 2>&1 &
  else
    "$bin/kanna-daemon" >>"$out/daemon.log" 2>&1 &
  fi
  daemon_pid=$!
  wait_for 5 test -S "$KANNA_SOCKET" || { echo "the daemon did not open its socket"; return 1; }
}
stop_daemon() { stop_pid "$daemon_pid"; daemon_pid=""; }
stop_pid() {
  [[ -n "$1" ]] || return 0
  kill "$1" 2>/dev/null
  wait_for 5 eval "! kill -0 $1" || { echo "e2e-kanna: $(ps -o command= -p "$1" | cut -c1-120) (pid $1) didn't stop on TERM; killed" >&2; kill -9 "$1" 2>/dev/null; }
  wait "$1" 2>/dev/null
}
start_app() {
  need "$app" || return 1
  "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach to the daemon"; return 1; }
}
stop_app() { stop_pid "$app_pid"; app_pid=""; }
app_attached() { [[ "$(raw '{"id":1,"cmd":"debug.app"}' | field ok)" == True ]]; }
instance_sockets() {
  local socket
  for socket in "$KANNA_STATE_DIR/kanna.sock" "$KANNA_STATE_DIR"/machine-*/kanna.sock "$KANNA_STATE_DIR/home/Library/Application Support"/kanna-dev*/kanna.sock; do
    [[ -S "$socket" ]] && echo "$socket"
  done
}
stop_instances() {
  [[ -n "$KANNA_STATE_DIR" && -d "$KANNA_STATE_DIR" ]] || return 0
  local socket pids=() log="$out/stop.log"
  : 2>/dev/null >>"$log" || log=/dev/stderr
  while IFS= read -r socket; do
    env -u KANNA_INSTANCE KANNA_SOCKET="$socket" "$bin/kanna" instances stop >/dev/null 2>>"$log" &
    pids+=($!)
  done < <(instance_sockets)
  [[ ${#pids[@]} -eq 0 ]] || wait "${pids[@]}"
}
stop_jobs() {
  local pid command
  for pid in $(jobs -p); do
    kill -0 "$pid" 2>/dev/null || continue
    command="$(ps -o command= -p "$pid")"
    case "$command" in *kanna-daemon*|*Kanna.app*) echo "e2e-kanna: ${command:0:120} (pid $pid) outlived kanna instances stop" >&2 ;; esac
    stop_pid "$pid"
  done
}
run_scenario() {
  trap 'exit 143' TERM
  trap 'stop_instances; stop_agents; stop_jobs' EXIT
  "$1"
  exit
}
cleanup() {
  if [[ -n "$scenario_pid" ]]; then
    kill "$scenario_pid" 2>/dev/null
    wait_for 30 eval "! kill -0 $scenario_pid" || kill -9 "$scenario_pid" 2>/dev/null
    wait "$scenario_pid" 2>/dev/null
    scenario_pid=""
  fi
  [[ -n "$KANNA_STATE_DIR" ]] || return 0
  stop_instances
  stop_tied --last-resort "$KANNA_STATE_DIR"
  app_pid="" daemon_pid=""
  rm -rf "$KANNA_STATE_DIR"
}
stop_agents() { stop_tied "$KANNA_STATE_DIR/agents" "$KANNA_STATE_DIR/codex"; }
stop_tied() {
  python3 - "$$" "$@" <<'PY'
import os, re, signal, subprocess, sys, time
runner, paths = int(sys.argv[1]), sys.argv[2:]
last_resort = paths[:1] == ["--last-resort"]
paths = paths[1:] if last_resort else paths
if not paths or not all(paths):
    sys.exit(0)
sweeping = paths == ["--orphans"]
FOLDER = re.compile(r"/tmp/kanna-e2e\.[A-Za-z0-9]{6}(?=[/\s\"']|$)")
KEYS = r"(?:^|\s)(?:KANNA_STATE_DIR|KANNA_SOCKET|KANNA_AGENTS_HOME|HOME)="
norm = lambda text: text.replace("/private/tmp/", "/tmp/")

def ps(*args):
    rows = {}
    for line in subprocess.run(["ps", *args], capture_output=True, text=True).stdout.splitlines():
        fields = line.split(None, 3 if args[0] == "-axww" else 1)
        if len(fields) >= 2:
            rows[int(fields[0])] = fields[1:]
    return rows

def table():
    plain = ps("-axww", "-o", "pid=,ppid=,stat=,command=")
    env = ps("-E", "-axww", "-o", "pid=,command=")
    mine, pid = set(), os.getpid()
    while pid in plain and pid not in mine:
        mine.add(pid)
        pid = int(plain[pid][0])
    rows = []
    for pid, fields in plain.items():
        if len(fields) < 3 or pid in mine or pid == runner or fields[1].startswith("Z"):
            continue
        command = fields[2]
        if "e2e-kanna.sh" in command or command.startswith("/Applications/Kanna.app/"):
            continue
        whole = env.get(pid, [command])[0]
        rows.append((pid, norm(command), norm(whole[len(command):] if whole.startswith(command) else "")))
    return rows

def ties(path):
    path = re.escape(norm(path.rstrip("/")))
    used, set_ = re.compile(path + r"(?=[/\s\"']|$)"), re.compile(KEYS + path + r"(?=[/\s]|$)")
    return lambda command, env: bool(used.search(command) or set_.search(env))

def folders(command, env):
    found = set(FOLDER.findall(command))
    found |= {m.group(1) for m in re.finditer(KEYS + r"(/tmp/kanna-e2e\.[A-Za-z0-9]{6})(?=[/\s]|$)", env)}
    return found

def owned(folder):
    try:
        pid = int(open(folder + "/e2e-kanna.pid").read())
    except FileNotFoundError:
        try:
            return time.time() - os.stat(folder).st_mtime < 3600
        except FileNotFoundError:
            return False
    except ValueError:
        return False
    command = subprocess.run(["ps", "-o", "command=", "-p", str(pid)], capture_output=True, text=True).stdout
    return "e2e-kanna.sh" in command

def alive(pid):
    stat = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return bool(stat) and not stat.startswith("Z")

def signal_(pid, signo):
    try:
        os.kill(pid, signo)
    except ProcessLookupError:
        pass

def stop_scenario(folder):
    try:
        pid = int(open(folder + "/e2e-scenario.pid").read())
    except (OSError, ValueError):
        return
    row = ps("-axww", "-o", "pid=,ppid=,stat=,command=").get(pid)
    if not row or len(row) < 3 or "e2e-kanna.sh" not in row[2] or row[0] != "1":
        return
    signal_(pid, signal.SIGTERM)
    deadline = time.time() + 2
    while time.time() < deadline and alive(pid):
        time.sleep(0.1)
    if alive(pid):
        signal_(pid, signal.SIGKILL)

def stop(tied, why):
    for _ in range(4):
        pids = {pid: command for pid, command, env in table() if tied(command, env)}
        if not pids:
            return
        for pid, command in pids.items():
            if last_resort:
                print(f"e2e-kanna: bug: {command[:120]} (pid {pid}) was still running after kanna instances stop{why}", file=sys.stderr)
            signal_(pid, signal.SIGTERM)
        deadline = time.time() + 5
        while time.time() < deadline and any(alive(p) for p in pids):
            time.sleep(0.1)
        still = {pid: command for pid, command, env in table() if pid in pids and tied(command, env)}
        for pid, command in still.items():
            print(f"e2e-kanna: {command[:120]} (pid {pid}) didn't stop on TERM{why}; killed", file=sys.stderr)
            signal_(pid, signal.SIGKILL)
        deadline = time.time() + 2
        while time.time() < deadline and any(alive(p) for p in still):
            time.sleep(0.1)

if sweeping:
    seen = {f for _, command, env in table() for f in folders(command, env)}
    seen |= {"/tmp/" + name for name in os.listdir("/tmp") if FOLDER.fullmatch("/tmp/" + name)}
    for folder in sorted(seen):
        if owned(folder):
            continue
        tied = ties(folder)
        count = sum(1 for _, command, env in table() if tied(command, env))
        if count:
            print(f"e2e-kanna: stopping {count} processes an earlier run left in {folder}", file=sys.stderr)
        stop_scenario(folder)
        stop(tied, f" (left in {folder})")
        subprocess.run(["rm", "-rf", folder])
else:
    tests = [ties(p) for p in paths]
    stop(lambda command, env: any(t(command, env) for t in tests), "")
PY
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
stop_tied --orphans
new_state_dir

kanna() { "$bin/kanna" "$@"; }
raw() {
  python3 -c 'import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.settimeout(30); s.connect(sys.argv[1])
s.sendall(sys.argv[2].encode() + b"\n"); print(s.makefile().readline().strip())' "$KANNA_SOCKET" "$1"
}
field() { python3 -c 'import json, sys; v = json.load(sys.stdin)
for k in sys.argv[1].split("."): v = v[int(k)] if k.isdigit() else v[k]
print(json.dumps(v, sort_keys=True) if isinstance(v, (dict, list)) else v)' "$1"; }
pane() {
  kanna workspace list --all --json | python3 -c 'import json, sys
p = [p for p in json.load(sys.stdin)["panes"] if sys.argv[1] in (p.get("name"), p.get("id"))][0]
print(json.dumps(p.get(sys.argv[2])) if isinstance(p.get(sys.argv[2]), (dict, list)) else p.get(sys.argv[2]))' "$1" "$2"
}
shows() { local screen; screen="$(kanna read "$1")" && grep -q -- "$2" <<<"$screen"; }
unwrapped() { kanna read "$1" | tr -d '\r\n' | grep -qF -- "$2"; }
spaceless() { kanna read "$1" | tr -d ' \r\n' | grep -qF -- "$(tr -d ' ' <<<"$2")"; }
is() { [[ "$(pane "$1" "$2")" == "$3" ]]; }
check() { "$@" || { echo "failed: $*"; return 1; }; }
layout() { kanna workspace list --all --json | field windows.0.tabs.0.layout; }

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

scenario web_md_ios_panes M3
web_md_ios_panes() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR" before
  printf '# web_md_ios_panes DOC\n\nweb_md_ios_panes TEXT\n' >"$dir/web_md_ios_panes.md"
  printf '<title>web_md_ios_panes PAGE</title><p id="x">web_md_ios_panes WEB</p>\n' >"$dir/web_md_ios_panes.html"
  printf '<title>web_md_ios_panes NEXT</title><p>web_md_ios_panes NEXT</p>\n' >"$dir/web_md_ios_panesb.html"
  page_shows() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q "$2"; }
  doc_shows() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q "$2"; }
  kanna open --window new web --name web --history "file://$dir/web_md_ios_panes.html" >/dev/null || return 1
  kanna open --window win-1 md --name doc --split right "$dir/web_md_ios_panes.md" >/dev/null || return 1
  check wait_for 5 page_shows web "web_md_ios_panes WEB" || return 1
  check wait_for 5 doc_shows doc "web_md_ios_panes TEXT" || return 1
  kanna navigate web "file://$dir/web_md_ios_panesb.html" >/dev/null || return 1
  check wait_for 5 page_shows web "web_md_ios_panes NEXT" || return 1
  kanna navigate web --back >/dev/null || return 1
  check is web url "file://$dir/web_md_ios_panes.html" || return 1
  check wait_for 5 page_shows web "web_md_ios_panes WEB" || return 1
  before="$(layout)"
  raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc","zoom":2}}' >/dev/null
  sleep 1
  stop_app
  check is doc path "$dir/web_md_ios_panes.md" || return 1
  kanna navigate web "file://$dir/web_md_ios_panesb.html" >/dev/null || return 1
  start_app || return 1
  check test "$(layout)" == "$before" || return 1
  check is web url "file://$dir/web_md_ios_panesb.html" || return 1
  check wait_for 5 page_shows web "web_md_ios_panes NEXT" || return 1
  check python3 -c 'import sys; assert float(sys.argv[1]) > 1, sys.argv[1]' "$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field zoom)" || return 1
  kanna navigate web --back >/dev/null || return 1
  check wait_for 5 page_shows web "web_md_ios_panes WEB" || return 1
  check wait_for 5 doc_shows doc "web_md_ios_panes TEXT" || return 1
  stop_daemon; start_daemon || return 1
  check wait_for 10 app_attached || return 1
  check is web state running || return 1
  check test "$(layout)" == "$before" || return 1
  check page_shows web "web_md_ios_panes WEB" || return 1
  local udid
  udid="$(xcrun simctl list devices booted 2>/dev/null | sed -n 's/^ *iPhone.*(\([0-9A-F-]\{36\}\)) (Booted).*/\1/p' | head -1)"
  if [[ -z "$udid" ]]; then
    echo "(iOS part skipped: no booted iPhone simulator)"
    return 0
  fi
  kanna open --window win-1 ios --name sim --app com.apple.Preferences --device "$udid" --tab >/dev/null || return 1
  check is sim state running || return 1
  check test "$(raw '{"id":1,"cmd":"debug.ios","args":{"pane":"sim"}}' | field ok)" == True || return 1
  stop_app; start_app || return 1
  ios_shown() { [[ "$(raw '{"id":1,"cmd":"debug.ios","args":{"pane":"sim"}}' | field ok)" == True ]]; }
  check wait_for 60 ios_shown || return 1
  check is sim state running
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

says() { local got; got="$("$@" 2>&1)"; [[ "$got" == "$expected" ]] || { echo "kanna ${*:2}: expected \"$expected\", got \"$got\""; return 1; }; }
exits() { local want="$1"; shift; "$@" >/dev/null 2>&1; local got=$?; [[ $got == "$want" ]] || { echo "${*}: exit $got, expected $want"; return 1; }; }
app_text() { raw "{\"id\":1,\"cmd\":\"debug.text\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q -- "$2"; }
page_text() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q -- "$2"; }
doc_text() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q -- "$2"; }

scenario markdown_diff M3
markdown_diff() {
  start_daemon && start_app || return 1
  local repo="$KANNA_STATE_DIR/markdown_diff" git=(git -C "$KANNA_STATE_DIR/markdown_diff" -c user.name=e2e -c user.email=e2e@example.invalid)
  mkdir -p "$repo" && "${git[@]}" init -q -b main || return 1
  { printf '# Spec\n\nKeep this paragraph as it is.\n\nThe old wording of this sentence.\n\n- one\n- two\n'; for i in $(seq 60); do printf '\nFiller paragraph %s stays the same.\n' "$i"; done; printf '\nThe last paragraph.\n'; } >"$repo/spec.md"
  "${git[@]}" add spec.md && "${git[@]}" commit -qm base || return 1
  "${git[@]}" checkout -qb work || return 1
  perl -pi -e 's/The old wording/The new wording/; s/^- two$/- two\n- three/' "$repo/spec.md" && "${git[@]}" commit -qam work || return 1
  "${git[@]}" checkout -q main && printf '\nOn main only.\n' >>"$repo/other.md" && "${git[@]}" add other.md && "${git[@]}" commit -qm later && "${git[@]}" checkout -q work || return 1
  perl -pi -e 's/The last paragraph\./The last paragraph, edited./' "$repo/spec.md"
  mdd() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$1\"${3:-}}}" | field "$2"; }
  kanna open --window new md "$repo/spec.md" --name diffdoc --diff main --tab >/dev/null || return 1
  check is diffdoc diff main || return 1
  check wait_for 5 eval '[[ "$(mdd diffdoc changes)" == 3 ]]' || { echo "markdown_diff: $(mdd diffdoc changes) changes vs main, expected 3"; return 1; }
  check doc_text diffdoc "The oldnew wording" || return 1
  printf '\nA new closing paragraph.\n' >>"$repo/spec.md"
  check wait_for 5 eval '[[ "$(mdd diffdoc changes)" == 3 ]] && doc_text diffdoc "A new closing paragraph"' || { echo "markdown_diff: an edit next to a change: $(mdd diffdoc changes)"; return 1; }
  perl -pi -e 's/Filler paragraph 30 stays the same\./Filler paragraph 30 now differs./' "$repo/spec.md"
  check wait_for 5 eval '[[ "$(mdd diffdoc changes)" == 4 ]]' || { echo "markdown_diff: the count didn't follow the edit: $(mdd diffdoc changes)"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"diffdoc","at":[4,200]}}' >/dev/null; sleep 0.3
  mdd diffdoc scroll ',"scroll":0' >/dev/null
  key "]"; key c; key "]"; key c; key "]"; key c; key "]"; key c
  check python3 -c 'import sys; assert float(sys.argv[1]) > 300, sys.argv[1]' "$(mdd diffdoc scroll)" || { echo "markdown_diff: ]c didn't reach the last change"; return 1; }
  local far; far="$(mdd diffdoc scroll)"
  key "["; key c
  check python3 -c 'import sys; assert 0 < float(sys.argv[1]) < float(sys.argv[2]), sys.argv[1:]' "$(mdd diffdoc scroll)" "$far" || { echo "markdown_diff: [c didn't go back"; return 1; }
  kanna open --window win-1 md "$repo/spec.md" --name headdoc --diff HEAD --tab >/dev/null || return 1
  check wait_for 5 eval '[[ "$(mdd headdoc changes)" == 2 ]]' || { echo "markdown_diff: $(mdd headdoc changes) uncommitted changes, expected 2"; return 1; }
  printf '# Fresh\n\nAll of it is new.\n' >"$repo/fresh.md"
  kanna open --window win-1 md "$repo/fresh.md" --name fresh --diff --tab >/dev/null || return 1
  check is fresh diff main || return 1
  check wait_for 5 eval '[[ "$(mdd fresh changes)" == 1 ]]' || return 1
  local failed
  printf '# Loose\n' >"$KANNA_STATE_DIR/loose.md"
  failed="$(kanna open --window win-1 md "$KANNA_STATE_DIR/loose.md" --diff HEAD --tab 2>&1)"
  check grep -q "isn't in a git repo" <<<"$failed" || { echo "markdown_diff: outside a repo: $failed"; return 1; }
  failed="$(kanna open --window win-1 md "$repo/spec.md" --diff nope --tab 2>&1)"
  check grep -q 'no commit or branch "nope"' <<<"$failed" || { echo "markdown_diff: a bad ref: $failed"; return 1; }
  kanna open --window win-1 md "$repo/spec.md" --name rangedoc --diff main..work --tab >/dev/null || return 1
  check is rangedoc diff main..work || return 1
  check wait_for 5 eval '[[ "$(mdd rangedoc changes)" == 2 ]]' || { echo "markdown_diff: $(mdd rangedoc changes) changes in main..work, expected 2"; return 1; }
  check doc_text rangedoc "The last paragraph." || return 1
  doc_text rangedoc "now differs" && { echo "markdown_diff: main..work shows the file on disk, not work"; return 1; }
  printf '\nAnother edit on disk.\n' >>"$repo/spec.md"; sleep 2.5
  doc_text rangedoc "Another edit on disk" && { echo "markdown_diff: a range followed the file"; return 1; }
  check test "$(mdd rangedoc changes)" == 2 || return 1
  failed="$(kanna markup rangedoc 2>&1)"
  check grep -q "takes no comments" <<<"$failed" || { echo "markdown_diff: comments on a range: $failed"; return 1; }
  printf '# Added\n\nNew on work.\n' >"$repo/added.md" && "${git[@]}" add added.md && "${git[@]}" commit -qm added || return 1
  kanna open --window win-1 md "$repo/added.md" --name addeddoc --diff main..work --tab >/dev/null || return 1
  check wait_for 5 eval '[[ "$(mdd addeddoc changes)" == 1 ]]' || { echo "markdown_diff: a doc new in work: $(mdd addeddoc changes)"; return 1; }
  failed="$(kanna open --window win-1 md "$repo/fresh.md" --diff main..work --tab 2>&1)"
  check grep -q "fresh.md isn't in work" <<<"$failed" || { echo "markdown_diff: a doc missing at B: $failed"; return 1; }
}

scenario markdown_overview M4
markdown_overview() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR/mdov" pictures top
  mkdir -p "$dir/.git" "$dir/.kanna/docs"
  {
    printf '# Login\n\nHow sign-in works, from the button to the session.\n\n## The redirect\n\nThe app sends you to the provider and waits for the redirect back.\n\n'
    printf '| Step | Who | What |\n|---|---|---|\n| 1 | app | opens the sign-in sheet |\n| 2 | provider | asks for the password |\n| 3 | app | reads the redirect |\n\n'
    printf '```swift\nfunc signIn() async throws {\n    let url = try await provider.start()\n    try await session.open(url)\n}\n```\n\n'
    printf '> The redirect must land on an allowed host.\n\n- Check it against the allow list.\n- Refuse anything else.\n\n'
    printf '```mermaid\nflowchart LR\n    A[app] --> P[provider] --> R[redirect] --> S[session]\n```\n\n'
    for i in $(seq 70); do printf '## Section %s\n\nParagraph %s about the session, the token and how the redirect is checked before the app trusts it.\n\n' "$i" "$i"; done
  } >"$dir/login.md"
  cat >"$dir/.kanna/docs/login.md.json" <<JSON
{"file": "$dir/login.md", "agent": null, "socket": null, "next": 3, "threads": [
 {"id": "c1", "quote": "allow list", "prefix": "", "suffix": "", "line": 22, "state": "open", "status": "unsent", "messages": [{"who": "you", "text": "Where does the allow list live?", "at": "2026-10-10T12:00:00"}], "error": null},
 {"id": "c2", "quote": "Paragraph 40 about the session", "prefix": "", "suffix": "", "line": 200, "state": "resolved", "status": "replied", "messages": [{"who": "you", "text": "Fine.", "at": "2026-10-10T12:00:00"}], "error": null}]}
JSON
  printf '# Release\n\nA short note that fits.\n' >"$dir/release.md"
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  kanna open --window new md "$dir/login.md" --name longdoc >/dev/null || return 1
  mdo() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"${3:-longdoc}\"${2:-}}}" | field "$1"; }
  mdwin() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"win-1\",\"size\":[$1,700]}}" >/dev/null; }
  mdshot() { raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$1.png\"}}" >/dev/null; cp "$KANNA_STATE_DIR/$1.png" "$out/" 2>/dev/null; }
  mdwin 1200
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == True ]]' || { echo "no overview: wanted $(mdo overview.wanted), width $(mdo width)"; return 1; }
  check eval '(( $(mdo overview.box_height | cut -d. -f1) > 10 ))' || { echo "box $(mdo overview.box_y) $(mdo overview.box_height)"; return 1; }
  check eval '(( $(mdo pictures) >= 1 ))' || return 1
  pictures="$(mdo pictures)"
  mdo scroll ',"scroll":2000' >/dev/null
  check wait_for 5 eval '[[ "$(mdo overview.top | cut -d. -f1)" == 2000 ]]' || { echo "the box didn't follow the scroll: top $(mdo overview.top)"; return 1; }
  sleep 0.3
  check test "$(mdo pictures)" == "$pictures" || { echo "a scroll drew the picture again: $pictures, then $(mdo pictures)"; return 1; }
  mdo zoom ',"zoom":1' >/dev/null; sleep 0.3
  check test "$(mdo pictures)" == "$pictures" || { echo "a zoom drew the picture again: $pictures, then $(mdo pictures)"; return 1; }
  mdo zoom ',"zoom":0' >/dev/null
  mdo scroll ',"overview_click":3' >/dev/null
  check wait_for 5 eval '[[ "$(mdo scroll)" == 0* ]]' || { echo "a click at the top of the thumbnail left the scroll at $(mdo scroll)"; return 1; }
  mdo scroll ',"overview_marks_click":690' >/dev/null
  check wait_for 5 eval '(( $(mdo scroll | cut -d. -f1) > 2000 ))' || { echo "a click at the foot of the marks left the scroll at $(mdo scroll)"; return 1; }
  mdo scroll ',"overview_drag":[10,-400]' >/dev/null
  check wait_for 5 eval '[[ "$(mdo scroll)" == 0* ]]' || { echo "dragging the box up left the scroll at $(mdo scroll)"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"longdoc","at":[300,300]}}' >/dev/null; sleep 0.3
  key cmd+f
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"redirect"}}' >/dev/null
  check wait_for 5 eval '(( $(mdo overview.marks_by_kind.hit 2>/dev/null || echo 0) > 3 ))' || { echo "find hits not marked: $(mdo overview.marks_by_kind)"; return 1; }
  check test "$(mdo pictures)" == "$pictures" || { echo "a find drew the picture again: $pictures, then $(mdo pictures)"; return 1; }
  mdo scroll ',"scroll":0' >/dev/null
  mdo overview.shown ',"overview_hover":true' >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.5
  check wait_for 5 eval '(( $(mdo pictures) > pictures ))' || { echo "the appearance change didn't draw a picture"; return 1; }
  mdshot markdown_overview-light
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.5
  mdshot markdown_overview-dark
  mdo overview.shown ',"overview_hover":false' >/dev/null
  key escape
  check test "$(mdo overview.shown ',"overview_enabled":false')" == False || { echo "View › Overview off left it shown"; return 1; }
  check test "$(mdo overview.shown ',"overview_enabled":true')" == True || { echo "View › Overview on didn't bring it back"; return 1; }
  pictures="$(mdo pictures)"
  mdwin 1000
  check wait_for 5 eval '(( $(mdo pictures) > pictures ))' || { echo "a width change didn't draw a picture"; return 1; }
  mdwin 700
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == False && "$(mdo overview.wanted)" == True ]]' || { echo "at 700 pt: shown $(mdo overview.shown)"; return 1; }
  mdwin 1200
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == True ]]' || { echo "not back at 1200 pt"; return 1; }
  kanna open --window win-1 md "$dir/release.md" --name shortdoc --tab >/dev/null || return 1
  check wait_for 5 eval 'doc_text shortdoc "fits"' || return 1
  check test "$(mdo overview.wanted '' shortdoc)" == False || { echo "a doc that fits wants the overview"; return 1; }
  check test "$(mdo overview.shown '' shortdoc)" == False || return 1
  kanna close shortdoc >/dev/null
  kanna markup longdoc >/dev/null || return 1
  mdwin 1500
  check wait_for 5 eval '[[ "$(mdo markup)" == True && "$(mdo overview.shown)" == True && "$(mdo threads_shown)" == True ]]' || { echo "at 1500 pt with comments: overview $(mdo overview.shown), list $(mdo threads_shown)"; return 1; }
  check wait_for 5 eval '[[ "$(mdo overview.marks_by_kind.comment)" == 1 ]]' || { echo "comment marks: $(mdo overview.marks_by_kind)"; return 1; }
  mdo overview.shown ',"overview_hover":true' >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.5
  mdshot markdown_overview-threads-light
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.5
  mdshot markdown_overview-threads-dark
  mdo overview.shown ',"overview_hover":false' >/dev/null
  mdwin 1000
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == False && "$(mdo threads_shown)" == True ]]' || { echo "at 1000 pt the overview should go first: overview $(mdo overview.shown), list $(mdo threads_shown)"; return 1; }
  mdshot markdown_overview-narrow
  mdwin 500
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == False && "$(mdo threads_shown)" == False ]]' || { echo "at 500 pt: overview $(mdo overview.shown), list $(mdo threads_shown)"; return 1; }
}

scenario markdown_diff_overview M4
markdown_diff_overview() {
  start_daemon && start_app || return 1
  local repo="$KANNA_STATE_DIR/mddiffov" git=(git -C "$KANNA_STATE_DIR/mddiffov" -c user.name=e2e -c user.email=e2e@example.invalid) pictures
  mkdir -p "$repo" && "${git[@]}" init -q -b main || return 1
  {
    printf '# Login\n\nHow sign-in works.\n\n'
    for i in $(seq 25); do printf 'Opening paragraph %s stays as it is.\n\n' "$i"; done
    printf -- '- check the host\n- keep the old fallback\n- open the session\n\n'
    for i in $(seq 25); do printf 'Middle paragraph %s stays as it is.\n\n' "$i"; done
    printf '| Step | What |\n|---|---|\n| 1 | open the sheet |\n| 2 | read the token |\n\n'
    for i in $(seq 25); do printf 'Closing paragraph %s stays as it is.\n\n' "$i"; done
  } >"$repo/login.md"
  "${git[@]}" add login.md && "${git[@]}" commit -qm base || return 1
  perl -0pi -e 's/- keep the old fallback\n//; s/\| 2 \| read the token \|/| 2 | read and check the token |/; s/(Middle paragraph 10 stays as it is\.\n\n)/$1## A new section\n\nAdded under the middle.\n\n/' "$repo/login.md"
  mdo() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"diffov\"${2:-}}}" | field "$1"; }
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  kanna open --window new md "$repo/login.md" --name diffov --diff main >/dev/null || return 1
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[1200,700]}}' >/dev/null
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == True ]]' || { echo "no overview on the diff: wanted $(mdo overview.wanted)"; return 1; }
  check wait_for 5 eval '(( $(mdo overview.marks_by_kind.added) >= 1 && $(mdo overview.marks_by_kind.removed) >= 1 && $(mdo overview.marks_by_kind.changed) >= 1 ))' || { echo "diff marks: $(mdo overview.marks_by_kind), changes $(mdo changes)"; return 1; }
  local added; added="$(mdo overview.marks_by_kind.added)"
  mdo overview.shown ',"overview_hover":true' >/dev/null
  mdo scroll ',"scroll":600' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"markdown_diff_overview-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/markdown_diff_overview-dark.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.5
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"markdown_diff_overview-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/markdown_diff_overview-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  mdo overview.shown ',"overview_hover":false' >/dev/null
  pictures="$(mdo pictures)"
  perl -0pi -e 's/(Closing paragraph 20 stays as it is\.\n\n)/$1A paragraph added near the end.\n\n/' "$repo/login.md"
  check wait_for 5 eval '(( $(mdo overview.marks_by_kind.added) > added && $(mdo pictures) > pictures ))' || { echo "the marks didn't follow the edit: $(mdo overview.marks_by_kind), pictures $pictures then $(mdo pictures)"; return 1; }
}

scenario cli_commands M4
cli_commands() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR" expected
  cd "$dir" || return 1
  expected="opened term-1 (cli_commands) in win-1, tab-1: running"; says kanna open --window new --name cli_commands --cmd 'printf "cli_commands READY\n"; exec cat' || return 1
  expected="opened term-2 (e2e) in win-1, tab-1: running"; says kanna open --window win-1 --name e2e --split right --size 1/3 || return 1
  check test "$(layout)" == '{"children": [{"pane": "cli_commands", "size": "2/3"}, {"pane": "e2e", "size": "1/3"}], "split": "row"}' || return 1
  check wait_for 5 app_text cli_commands "cli_commands READY" || return 1
  [[ "$(kanna workspace list --all)" == *$'Free windows\nwin-1\n  focus: e2e\n  *tab-1 "cli_commands"  cli_commands:2/3 | e2e:1/3'* ]] || { echo "workspace list --all should show the free window: $(kanna workspace list --all)"; return 1; }
  expected="focused cli_commands in win-1 (in front)"; says kanna focus cli_commands || return 1
  expected="e2e is zoomed (run again to unzoom)"; says kanna zoom e2e || return 1
  check test "$(kanna workspace list --all --json | field windows.0.zoomed)" == e2e || return 1
  expected="e2e is no longer zoomed"; says kanna zoom e2e || return 1
  expected="cli_commands:3/4 | e2e:1/4"; says kanna resize e2e 1/4 || return 1
  expected="cli_commands:1/2 / e2e:1/2"; says kanna move e2e --to cli_commands --side bottom || return 1
  expected="e2e:1/4 | cli_commands:3/4"; says kanna arrange --window win-1 '{"split":"row","children":[{"pane":"e2e","size":"1/4"},{"pane":"cli_commands"}]}' || return 1
  expected="opened term-3 (t) in win-1, tab-2: running"; says kanna open --window win-1 --tab --name t || return 1
  expected='tab-2 is now "shells"'; says kanna rename-tab tab-2 shells || return 1
  expected="win-1: tab-2, tab-1"; says kanna move-tab tab-2 --index 0 || return 1
  expected="switched to tab-1 in win-1 (in front)"; says kanna focus --tab tab-1 || return 1
  expected="t"; says kanna move t --window new || return 1
  check test "$(kanna workspace list --all --json | field windows.1.tabs.0.layout)" == '{"pane": "t"}' || return 1
  expected="brought win-1 forward"; says kanna focus --window win-1 || return 1
  expected="closed win-2 (panes term-3)"; says kanna close --window win-2 || return 1
  kanna send e2e 'echo cli_commands-$((40+2))' || return 1
  check wait_for 5 shows e2e cli_commands-42 || return 1
  check wait_for 5 app_text e2e cli_commands-42 || return 1
  kanna send e2e -- echo --json || return 1
  check wait_for 5 eval 'kanna read e2e --lines 2 | grep -qx -- --json' || return 1
  expected="term-4 (cli_commands) (printf \"cli_commands READY\\n\"; exec cat) is running"; says kanna restart cli_commands || return 1
  printf '<title>cli_commands</title><p>cli_commands PAGE</p>\n' >"$dir/a.html"
  printf '<title>cli_commands B</title><p>cli_commands OTHER</p>\n' >"$dir/b.html"
  expected="opened page-1 (site) in win-1, tab-1: running"; says kanna open --window win-1 web "file://$dir/a.html" --name site --history --split down || return 1
  check wait_for 5 page_text site "cli_commands PAGE" || return 1
  expected="page-1 (site) (file://$dir/b.html) is running"; says kanna navigate site "file://$dir/b.html" || return 1
  check wait_for 5 page_text site "cli_commands OTHER" || return 1
  expected="page-1 (site) (file://$dir/a.html) is running"; says kanna navigate site --back || return 1
  check wait_for 5 page_text site "cli_commands PAGE" || return 1
  expected="page-1 (site) (file://$dir/b.html) is running"; says kanna navigate site --forward || return 1
  kanna open --window win-1 web "file://$dir/a.html" --name plain --tab >/dev/null || return 1
  exits 2 kanna navigate plain --back || return 1
  printf '# cli_commands DOC\n\ncli_commands TEXT\n' >"$dir/cli_commands.md"
  printf '# cli_commands NEXT\n' >"$dir/next.md"
  kanna open --window win-1 md cli_commands.md --name doc --tab >/dev/null || return 1
  check is doc path "$(pwd -P)/cli_commands.md" || return 1
  check wait_for 5 doc_text doc "cli_commands TEXT" || return 1
  kanna navigate doc next.md >/dev/null || return 1
  check wait_for 5 doc_text doc "cli_commands NEXT" || return 1
  kanna navigate doc --back >/dev/null || return 1
  check wait_for 5 doc_text doc "cli_commands TEXT" || return 1
  local missing
  missing="$(kanna open --window win-1 md "$dir/missing.md" --tab 2>&1)"; check test $? -eq 1 || return 1
  check grep -q "No such file" <<<"$missing" || return 1
  check test "$(kanna capabilities | tail -1)" == "Kanna.app: attached" || return 1
  check grep -q "The running Kanna supports every command." <(kanna help) || return 1
  check test "$(kanna open --window win-1 --json --name j --tab | field pane.name)" == j || return 1
  exits 4 kanna close nope || return 1
  exits 1 kanna open --window win-1 --name e2e || return 1
  exits 2 kanna zoom || return 1
  exits 5 kanna raw '{"cmd":"nope"}' || return 1
  exits 0 kanna raw '{"cmd":"debug.app"}' || return 1
  kanna close --window win-1 >/dev/null || return 1
  check test "$(kanna workspace list --all --json | field windows)" == "[]" || return 1
  expected="no windows (open one with \`kanna open\`)"; says kanna workspace list --all
}

scenario cli_without_app M4
cli_without_app() {
  start_daemon || return 1
  local expected
  expected="opened term-1 (cli_without_app) in win-1, tab-1: running"; says kanna open --window new --name cli_without_app --cmd 'echo cli_without_app READY; exec cat' || return 1
  check wait_for 5 shows cli_without_app "cli_without_app READY" || return 1
  kanna send cli_without_app cli_without_app-SENT || return 1
  check wait_for 5 shows cli_without_app cli_without_app-SENT || return 1
  expected="term-2 (cli_without_app) (echo cli_without_app READY; exec cat) is running"; says kanna restart cli_without_app || return 1
  check grep -q "Kanna.app: not attached" <(kanna capabilities) || return 1
  printf '# cli_without_app DOC\n' >"$KANNA_STATE_DIR/cli_without_app.md"
  local args
  while IFS= read -r args; do
    eval "kanna $args" >/dev/null || { echo "failed without the app: kanna $args"; return 1; }
  done <<NEEDS
open --window win-1 --name gone --split right
close gone
open --window win-1 --name doc --split right md $KANNA_STATE_DIR/cli_without_app.md
resize cli_without_app 2/3
open --window win-1 --tab --name other
rename-tab tab-1 kept
focus --tab tab-1
NEEDS
  exits 6 kanna raw '{"cmd":"debug.app"}' || return 1
  check grep -q "KANNA_SOCKET is set, so kanna doesn't start Kanna.app" <(kanna raw '{"cmd":"debug.app"}' 2>&1) || return 1
  start_app || return 1
  check test "$(layout)" == '{"children": [{"pane": "cli_without_app", "size": "2/3"}, {"pane": "doc", "size": "1/3"}], "split": "row"}' || return 1
  check test "$(win win-1 tabs.0.title)" == kept || return 1
  check wait_for 5 doc_text doc "cli_without_app DOC" || return 1
  expected="focused cli_without_app in win-1 (in front)"; says kanna focus cli_without_app || return 1
  stop_app
  check test "$(kanna workspace list --all --json | field windows.0.id)" == win-1
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

key() { raw "{\"id\":1,\"cmd\":\"debug.key\",\"args\":{\"key\":\"$1\"}}" >/dev/null; sleep 0.3; }
state() { kanna workspace list --all --json; }
win() {
  state | python3 -c 'import json, sys
w = next(w for w in json.load(sys.stdin)["windows"] if w["id"] == sys.argv[1])
v = w
for k in sys.argv[2].split("."): v = v[int(k)] if k.isdigit() else v.get(k)
print(json.dumps(v, sort_keys=True) if isinstance(v, (dict, list)) else v)' "$1" "$2"
}
ws() {
  state | python3 -c 'import json, sys
w = next(w for w in json.load(sys.stdin)["workspaces"] if sys.argv[1] in (w["id"], w.get("task"), w["dock"]["pane"]))
v = w
for k in sys.argv[2].split("."): v = v[int(k)] if k.isdigit() else v.get(k)
print(json.dumps(v, sort_keys=True) if isinstance(v, (dict, list)) else v)' "$1" "$2"
}
app_view() { raw '{"id":1,"cmd":"debug.view"}'; }
close_desk() { raw "{\"id\":1,\"cmd\":\"desk\",\"args\":{\"action\":\"close\",\"desk\":\"$1\"}}" | grep -q '"ok":true'; }
dwin() {
  app_view | python3 -c 'import json, sys
view = json.load(sys.stdin)
w = next(w for w in view["windows"] if w["id"] == sys.argv[1])
w["key"] = view.get("key") == w["id"]
v = w
for k in sys.argv[2].split("."): v = v[int(k)] if k.isdigit() else v.get(k)
print(json.dumps(v, sort_keys=True) if isinstance(v, (dict, list)) else v)' "$1" "$2"
}
placed() { awk -F'  +' 'BEGIN { OFS = "  " } /^(Desk|Shelf)$/ { where = $1; next } NF > 1 { $2 = $2 OFS where; print }'; }
tid() { sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id from tasks order by rowid limit 1 offset $(($1 - 1))"; }
sid() { sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id from tasks where place != 'done'" | python3 -c 'import sys
id, others = sys.argv[1], [o[5:] for o in sys.stdin.read().split() if o != sys.argv[1]]
n = next((n for n in range(4, 8) if not any(o[:n] == id[5:5 + n] for o in others)), 8)
print(id[:5 + n])' "$(tid "$1")"; }
e2e_repo() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/${1:-tasks}"
  [[ -d "$repo/.git" ]] || { mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first; } || return 1
  (cd "$repo" && pwd -P)
}
new_task() {
  local repo; repo="$(e2e_repo "${3:-tasks}")" || return 1
  (cd "$repo" && kanna --json task new "$1" --agent "${2:-exec \$SHELL} #") | field task.id
}
desk_task() {
  local id; id="$(new_task "$@")" || return 1
  kanna task pick "$id" >/dev/null
}
shown_windows() { state | python3 -c 'import json, sys; print(" ".join(w["id"] for w in json.load(sys.stdin)["windows"]))'; }
xdg() { mkdir -p "$KANNA_STATE_DIR/xdg/ghostty"; printf '%s\n' "$@" >"$KANNA_STATE_DIR/xdg/ghostty/config"; }
term_app() { XDG_CONFIG_HOME="$KANNA_STATE_DIR/xdg" start_app; }
terminal() { raw "{\"id\":1,\"cmd\":\"debug.terminal\",\"args\":$2}" | python3 -c 'import json, sys; m = json.load(sys.stdin)
assert m.get("ok"), m
v = m
for k in sys.argv[1].split("."): v = v[int(k)] if k.isdigit() else v[k]
print(json.dumps(v) if isinstance(v, (dict, list)) else v)' "$1"; }
probe() {
  cat >"$KANNA_STATE_DIR/probe.py" <<'PY'
import os, sys, tty
tty.setraw(0)
arg = lambda a: a.encode().decode("unicode_escape").encode("latin1")
os.write(1, arg(sys.argv[1]) + b"READY\r\n")
until, count, got = arg(sys.argv[2]), int(sys.argv[3]), b""
while count == 0 or got.count(until) < count:
    chunk = os.read(0, 256)
    got += chunk
    if count == 0:
        os.write(1, b"IN " + chunk.replace(b"\x1b", b"^").replace(b"\x07", b"~").replace(b"\r", b"\\r") + b"\r\n")
os.write(1, b"GOT " + got.replace(b"\x1b", b"^").replace(b"\x07", b"~") + b"\r\n")
PY
  printf "python3 %s '%s' '%s' %s; exec cat" "$KANNA_STATE_DIR/probe.py" "$1" "${2:-\\007}" "${3:-1}"
}
count() { python3 -c 'import json, sys; print(len(json.load(sys.stdin)))'; }
long_comment="A long comment that runs on for a while, so that it wraps over many lines in the comment box and then some more past its cap. "
long_comment="$long_comment$long_comment$long_comment$long_comment$long_comment$long_comment"
box_fits() { python3 -c 'import json, sys
r = json.load(sys.stdin)
p = {k: (x, y, x + w, y + h) for k, (x, y, w, h) in r["box_parts"].items()}
box, f = p["box"], p["field"]
inside = lambda a: a[0] >= box[0] and a[1] >= box[1] and a[2] <= box[2] and a[3] <= box[3]
assert f[3] - f[1] == 160, ("the field should grow to its cap", r["box_parts"])
assert p["text"][3] - p["text"][1] > 160, ("past its cap the text should scroll inside the field", r["box_parts"])
for k in ("box", "field", "hint", "post"):
    if k in p: assert inside(p[k]), (k, "is outside the box", r["box_parts"])
for k in ("hint", "post"):
    if k in p: assert p[k][1] >= f[3], (k, "overlaps the field", r["box_parts"])
assert r["box_shown"], ("the box should stay in view", r["box_parts"])'; }
snap() {
  local shot
  shot="$(raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"window\":\"$1\",\"path\":\"$2.png\",\"marker\":\"*\"}}")"
  cp "$KANNA_STATE_DIR/$2.png" "$out/$2.png" 2>/dev/null
  printf '%s\n' "$shot"
}
drawn() {
  python3 -c 'import json, sys
shot = json.loads(sys.argv[1]); want = int(sys.argv[2])
panes = shot.get("panes", [])
assert len(panes) == want, f"{len(panes)} panes drawn, wanted {want}: {shot}"
for p in panes:
    m = p.get("marker", {})
    assert p["ink"] > 0 and m.get("found") and m["inked"] == m["glyphs"] > 0, p' "$1" "$2"
}
redrawn() { rm -f "$KANNA_STATE_DIR/$2.png"; shot="$(snap "$1" "$2")"; drawn "$shot" "$3"; }
click_tab() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"tab\":\"$1\",\"clicks\":${2:-1}}}" >/dev/null; sleep 0.3; }
drag() { last_drag="$1 → $(raw "{\"id\":1,\"cmd\":\"debug.drag\",\"args\":$1}")"; sleep 0.3; }

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
  shot="$(snap win-1 windows_tabs_ui-zoom-term-3)"
  check drawn "$shot" 1 || return 1
  check test "$(field tabBar.shown <<<"$shot")" == True || { echo "the tab bar should show while a pane is zoomed"; return 1; }
  key "cmd+]"; check test "$(win win-1 zoomed)" == k1 || return 1
  check drawn "$(snap win-1 windows_tabs_ui-zoom-k1)" 1 || return 1
  key cmd+shift+return; check test "$(win win-1 zoomed)" == None || return 1
  key "cmd+["; check test "$(win win-1 focused)" == term-3 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"k1"}}' >/dev/null; sleep 0.3
  check test "$(win win-1 focused)" == k1 || return 1
  sleep 1
  check drawn "$(snap win-1 windows_tabs_ui-splits)" 3 || return 1
  printf '# windows_tabs_ui DOC\n\nclick me\n' >"$KANNA_STATE_DIR/windows_tabs_ui.md"
  kanna open --window win-1 md --name doc --split right "$KANNA_STATE_DIR/windows_tabs_ui.md" >/dev/null || return 1
  key "cmd+["; check test "$(win win-1 focused)" != doc || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"doc","at":[4,200]}}' >/dev/null; sleep 0.3
  check test "$(win win-1 focused)" == doc || return 1
  mdzoom() { raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field zoom; }
  local width_before; width_before="$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field layout_width)"
  key cmd+shift+=; check python3 -c 'import sys; assert float(sys.argv[1]) > 1' "$(mdzoom)" || return 1
  check test "$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field layout_width)" == "$width_before" || { echo "width $width_before -> $(raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field layout_width)"; return 1; }
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
  check drawn "$(snap win-2 windows_tabs_ui-second-window)" 1 || return 1
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
  [[ "$menus" == *'"item":"New Desk","menu":"File","shortcut":"⌘n"'* ]] || { echo "New Desk isn't File ⌘N: $menus"; return 1; }
  [[ "$menus" == *'"item":"New Window","menu":"Window","shortcut":""'* ]] || { echo "New Window has a shortcut: $menus"; return 1; }
  [[ "$menus" != *'"New Instance"'* && "$menus" != *'"Start a Task"'* ]] || { echo "a removed menu item is back: $menus"; return 1; }
}

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
  check drawn "$(snap win-1 resize_arrange-resized)" 4 || return 1
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
  for item in "Split Right" "Close Pane" "Move Pane to New Window" "Reload"; do
    [[ "$menu" == *"\"$item\""* ]] || { echo "the pane menu has no \"$item\": $menu"; return 1; }
  done
  sleep 1
  local w
  for w in $(state | python3 -c 'import json, sys; print(" ".join(w["id"] for w in json.load(sys.stdin)["windows"]))'); do
    check drawn "$(snap "$w" "drag_panes_tabs-$w")" 1 || return 1
  done
}

near() { python3 -c 'import sys
a, b = (bytes.fromhex(x.lstrip("#")) for x in sys.argv[1:3])
sys.exit(0 if max(abs(x - y) for x, y in zip(a, b)) <= 16 else 1)' "$1" "$2"; }

scenario terminal_colours M4
terminal_colours() {
  xdg 'palette = 1=#0102fe' 'cursor-style = bar' \
    'theme = light:'"$KANNA_STATE_DIR"'/light,dark:'"$KANNA_STATE_DIR"'/dark'
  printf 'background = #f0f0e0\nforeground = #202020\n' >"$KANNA_STATE_DIR/light"
  printf 'background = #123456\nforeground = #e5d4b0\n' >"$KANNA_STATE_DIR/dark"
  start_daemon && term_app || return 1
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null || return 1
  kanna open --window new --name terminal_colours --cmd "$(probe '\033]11;?\007\033]10;?\007\033]4;1;?\007\033[?996n' n 1)" >/dev/null || return 1
  check wait_for 5 shows terminal_colours 'GOT ^]11;rgb:1212/3434/5656~^]10;rgb:e5e5/d4d4/b0b0~^]4;1;rgb:0101/0202/fefe~^\[?997;1n' || { kanna read terminal_colours; return 1; }
  check test "$(terminal settings.cursorStyle '{"pane":"terminal_colours"}')" == bar || return 1
  local drawn
  drawn="$(raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"terminal_colours.png"}}' | field panes.0.background)"
  cp "$KANNA_STATE_DIR/terminal_colours.png" "$out/terminal_colours-dark.png" 2>/dev/null
  check near "$drawn" '#123456' || return 1
  kanna open --window win-1 --name terminal_coloursb --cmd "$(probe '\033[?2031h' n 1)" >/dev/null || return 1
  check wait_for 5 shows terminal_coloursb READY || return 1
  terminal background '{"pane":"terminal_colours","appearance":"light"}' >/dev/null
  check wait_for 5 shows terminal_coloursb 'GOT ^\[?997;2n' || return 1
  check wait_for 5 eval '[[ "$(terminal settings.background "{\"pane\":\"terminal_colours\"}")" == "#f0f0e0" ]]' || return 1
  kanna open --window win-1 --name terminal_coloursc --cmd "$(probe '\033]11;?\007')" >/dev/null || return 1
  check wait_for 5 shows terminal_coloursc 'GOT ^]11;rgb:f0f0/f0f0/e0e0~' || return 1
  sleep 0.5
  drawn="$(raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"terminal_colours-light.png"}}' | field panes.0.background)"
  cp "$KANNA_STATE_DIR/terminal_colours-light.png" "$out/terminal_colours-light.png" 2>/dev/null
  check near "$drawn" '#f0f0e0'
}

scenario cwd_title M4
cwd_title() {
  start_daemon && start_app || return 1
  kanna open --window new --name cwd_title --cmd "printf '\033]7;file://localhost/private/tmp\007\033]2;cwd_title TITLE\007cwd_title-READY'; exec cat" >/dev/null || return 1
  check wait_for 5 shows cwd_title cwd_title-READY || return 1
  check wait_for 5 is cwd_title cwd /private/tmp || return 1
  check wait_for 5 eval '[[ "$(terminal title "{\"pane\":\"cwd_title\"}")" == "cwd_title TITLE" ]]' || return 1
  stop_app
  kanna open --window win-1 --name cwd_titleb --cmd "printf '\033]7;file://localhost/usr\007'; exec cat" >/dev/null || return 1
  check wait_for 5 is cwd_titleb cwd /usr || return 1
  stop_daemon; start_daemon || return 1
  check is cwd_titleb cwd /usr || return 1
  kanna restart cwd_titleb --cmd 'pwd; exec cat' >/dev/null 2>&1 || kanna restart cwd_titleb >/dev/null || return 1
  check wait_for 5 is cwd_titleb cwd /usr
}

scenario clipboard M4
clipboard() {
  xdg 'clipboard-read = allow'
  start_daemon && term_app || return 1
  kanna open --window new --name clipboard --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows clipboard READY || return 1
  terminal clipboard '{"pane":"clipboard","clipboard":"clipboard PASTE"}' >/dev/null || return 1
  raw '{"id":1,"cmd":"debug.key","args":{"pane":"clipboard","key":"cmd+v"}}' >/dev/null
  check wait_for 5 eval 'kanna read clipboard | tr -d "\\n" | sed "s/IN //g" | grep -q "\\^\\[200~clipboard PASTE\\^\\[201~"' || { kanna read clipboard; return 1; }
  kanna open --window win-1 --name clipboardw --cmd "printf '\033]52;c;Y2xpcGJvYXJkIENPUElFRA==\007clipboardW'; exec cat" >/dev/null || return 1
  check wait_for 5 shows clipboardw clipboardW || return 1
  check wait_for 5 eval '[[ "$(terminal clipboard "{\"pane\":\"clipboard\"}")" == "clipboard COPIED" ]]' || return 1
  kanna open --window win-1 --name clipboardr --cmd "$(probe '\033]52;c;?\007' '\\' 1)" >/dev/null || return 1
  check wait_for 5 shows clipboardr 'GOT ^]52;c;Y2xpcGJvYXJkIENPUElFRA==^' || return 1
  kanna open --window win-1 --name clipboards --cmd "printf 'clipboard SELECTED'; exec cat" >/dev/null || return 1
  check wait_for 5 shows clipboards 'clipboard SELECTED' || return 1
  terminal selection '{"pane":"clipboards","action":"select_all"}' >/dev/null
  terminal clipboard '{"pane":"clipboards","action":"copy_to_clipboard"}' >/dev/null
  check wait_for 5 eval 'terminal clipboard "{\"pane\":\"clipboards\"}" | grep -q "clipboard SELECTED"' || return 1
  stop_app; term_app || return 1
  terminal clipboard '{"pane":"clipboard","clipboard":"AFTER"}' >/dev/null || return 1
  raw '{"id":1,"cmd":"debug.key","args":{"pane":"clipboard","key":"cmd+v"}}' >/dev/null
  check wait_for 5 eval 'kanna read clipboard | tr -d "\\n" | sed "s/IN //g" | grep -q "\\^\\[200~AFTER\\^\\[201~"' || return 1
  desk_task clipboardd "$(probe '\033[?2004h' x 0)" && kanna task activate clipboardd >/dev/null || return 1
  check wait_for 5 shows clipboardd READY || return 1
  terminal clipboard '{"pane":"clipboardd","clipboard":"clipboard DOCKED"}' >/dev/null || return 1
  raw '{"id":1,"cmd":"debug.key","args":{"pane":"clipboardd","key":"cmd+v"}}' >/dev/null
  check wait_for 5 eval 'kanna read clipboardd | tr -d "\\n" | sed "s/IN //g" | grep -q "\\^\\[200~clipboard DOCKED\\^\\[201~"' || { kanna read clipboardd; return 1; }
}

scenario system_clipboard M4
system_clipboard() {
  start_daemon && need "$app" || return 1
  env -u KANNA_TEST_EFFECTS "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  check wait_for 10 app_attached || return 1
  kanna open --window new --name sysclip --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows sysclip READY || return 1
  local text; text="$(pbpaste)"
  check test "$(terminal clipboard '{"pane":"sysclip"}')" == "${text:-None}" || return 1
  [[ -n "$text" ]] || return 0
  raw '{"id":1,"cmd":"debug.key","args":{"pane":"sysclip","key":"cmd+v"}}' >/dev/null
  check wait_for 5 eval 'kanna read sysclip | tr -d "\\n" | grep -q "\\^\\[201~"'
}

scenario mouse_keys_focus M4
mouse_keys_focus() {
  start_daemon && start_app || return 1
  kanna open --window new --name mouse_keys_focus --cmd "$(probe '\033[>1u\033[?1000h\033[?1006h\033[?1004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows mouse_keys_focus READY || return 1
  stop_app; start_app || return 1
  raw '{"id":1,"cmd":"debug.key","args":{"pane":"mouse_keys_focus","key":"shift+return"}}' >/dev/null
  check wait_for 5 shows mouse_keys_focus 'IN ^\[13;2u' || return 1
  terminal cols '{"pane":"mouse_keys_focus","click":{"at":[5,5]}}' >/dev/null || return 1
  check wait_for 5 shows mouse_keys_focus 'IN ^\[<0;1;1M' || return 1
  check wait_for 5 shows mouse_keys_focus '\[<0;1;1m' || return 1
  kanna open --window win-1 --name mouse_keys_focusb --split right >/dev/null || return 1
  check wait_for 5 eval '[[ "$(kanna read mouse_keys_focus | grep -oE "IN \^\[[IO]" | tail -1)" == "IN ^[O" ]]' || return 1
  kanna focus mouse_keys_focus >/dev/null || return 1
  check wait_for 5 eval '[[ "$(kanna read mouse_keys_focus | grep -oE "IN \^\[[IO]" | tail -1)" == "IN ^[I" ]]'
}

scenario font_size_scrollback M4
font_size_scrollback() {
  start_daemon && start_app || return 1
  kanna open --window new --name font_size_scrollback >/dev/null || return 1
  kanna send font_size_scrollback 'seq 1 3000' || return 1
  check wait_for 5 shows font_size_scrollback '^3000$' || return 1
  local cols wide cell
  cols="$(terminal cols '{"pane":"font_size_scrollback"}')" cell="$(terminal cellWidth '{"pane":"font_size_scrollback"}')"
  terminal cols '{"pane":"font_size_scrollback","zoom":1}' >/dev/null
  check wait_for 5 eval '[[ "$(terminal cellWidth "{\"pane\":\"font_size_scrollback\"}")" -gt "$cell" ]]' || return 1
  check wait_for 5 eval '[[ "$(terminal cols "{\"pane\":\"font_size_scrollback\"}")" -lt "$cols" ]]' || return 1
  terminal cols '{"pane":"font_size_scrollback","zoom":0}' >/dev/null
  terminal cols '{"pane":"font_size_scrollback","zoom":-1}' >/dev/null
  check wait_for 5 eval '[[ "$(terminal cols "{\"pane\":\"font_size_scrollback\"}")" -gt "$cols" ]]' || return 1
  wide="$(terminal cols '{"pane":"font_size_scrollback"}')"
  kanna send font_size_scrollback "stty size" || return 1
  check wait_for 5 eval 'kanna read font_size_scrollback | grep -qE " $wide\$"' || return 1
  check test "$(kanna read font_size_scrollback --lines 10000 | grep -c '^[0-9]*$')" -ge 3000 || return 1
  stop_app; start_app || return 1
  check test "$(kanna read font_size_scrollback --lines 10000 | grep -c '^[0-9]*$')" -ge 3000 || return 1
  local before narrow
  before="$(terminal cols '{"pane":"font_size_scrollback"}')"
  key cmd+=; key cmd+=
  check wait_for 5 eval '[[ "$(terminal cols "{\"pane\":\"font_size_scrollback\"}")" -lt "$before" ]]' || return 1
  narrow="$(terminal cols '{"pane":"font_size_scrollback"}')"
  kanna send font_size_scrollback "clear; stty size" || return 1
  check wait_for 5 eval 'kanna read font_size_scrollback | grep -qE " $narrow\$"'
}

scenario links_bell_notify M4
links_bell_notify() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR/links"
  mkdir -p "$dir"; printf 'plain\n' >"$dir/notes.txt"; printf '#!/bin/sh\n' >"$dir/run.command"
  cat >"$KANNA_STATE_DIR/links.sh" <<SH
printf 'https://example.com/links_bell_notify\r\n'
printf '\033]8;;https://example.com/osc8\033\\\\osc8-web-link-osc8-web-link\033]8;;\033\\\\\r\n'
printf '\033]8;;x-apple.systempreferences:com.apple.Security\033\\\\osc8-custom-scheme-osc8-custom\033]8;;\033\\\\\r\n'
printf '\033]8;;file:///System/Applications/Calculator.app\033\\\\osc8-application-osc8-application\033]8;;\033\\\\\r\n'
printf '\033]8;;file://$dir/run.command\033\\\\osc8-command-file-osc8-command-file\033]8;;\033\\\\\r\n'
printf '\033]8;;file://$dir/notes.txt\033\\\\osc8-text-file-osc8-text-file-osc8\033]8;;\033\\\\\r\n'
printf '$dir/run.command\r\n'
SH
  kanna open --window new --name links_bell_notify --cmd "sh '$KANNA_STATE_DIR/links.sh'; printf '\007\033]9;links_bell_notify NOTE\007\033]9;4;1;40\007'; sleep 1.5; printf '\033]777;notify;links_bell_notify T;links_bell_notify B\007links_bell_notify-READY'; exec cat" >/dev/null || return 1
  check wait_for 5 shows links_bell_notify links_bell_notify-READY || return 1
  local row
  for row in 0 1 2 3 4 5 6; do
    terminal cols "{\"pane\":\"links_bell_notify\",\"click\":{\"cell\":[3,$row],\"mods\":[\"cmd\"]}}" >/dev/null || return 1
  done
  local links
  check wait_for 5 eval 'test "$(terminal links "{\"pane\":\"links_bell_notify\"}" | python3 -c "import json, sys; print(len(json.load(sys.stdin)))")" == 7' || { terminal links '{"pane":"links_bell_notify"}'; return 1; }
  links="$(terminal links '{"pane":"links_bell_notify"}' | python3 -c 'import json, sys; print("\n".join(l["decision"] + " " + l["url"] for l in json.load(sys.stdin)))')"
  check test "$links" == "allow https://example.com/links_bell_notify
allow https://example.com/osc8
deny:unsupportedScheme x-apple.systempreferences:com.apple.Security
deny:unsafeFile file:///System/Applications/Calculator.app
deny:unsafeFile file://$dir/run.command
allow file://$dir/notes.txt
deny:unsafeFile $dir/run.command" || { echo "$links"; return 1; }
  check test "$(terminal opened '{"pane":"links_bell_notify"}' | python3 -c 'import json, sys; print(len(json.load(sys.stdin)))')" == 3 || return 1
  check test "$(terminal bells '{"pane":"links_bell_notify"}')" -ge 1 || return 1
  check terminal notifications '{"pane":"links_bell_notify"}' | grep -q "links_bell_notify NOTE" || return 1
  check terminal notifications '{"pane":"links_bell_notify"}' | grep -q "links_bell_notify B" || return 1
  check test "$(terminal progress.value '{"pane":"links_bell_notify"}')" == 40
}

lum() { python3 -c 'import sys
r, g, b = bytes.fromhex(sys.argv[1].lstrip("#"))
print(int(0.2126 * r + 0.7152 * g + 0.0722 * b))' "$1"; }
tabbar() { raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$1.png\"}}" | field tabBar.background; cp "$KANNA_STATE_DIR/$1.png" "$out/$1.png" 2>/dev/null; }

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

scenario md_comments M4
md_comments() {
  start_daemon && start_app || return 1
  kanna open --window new --name agent --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows agent READY || return 1
  local dir="$KANNA_STATE_DIR/doc" fixture="$repo_root/references/prototypes/doc-markup/fixture" agent_id at xy kept
  mkdir -p "$dir/.git" "$dir/.kanna/docs"
  cp "$fixture/review.md" "$dir/review.md"
  for at in $(seq 1 40); do printf '\nMore of the doc, paragraph %s, so it scrolls.\n' "$at"; done >>"$dir/review.md"
  sed "s|FILE|$dir/review.md|" "$fixture/review.threads.json" >"$dir/.kanna/docs/review.md.json"
  agent_id="$(pane agent id)"
  KANNA_TERMINAL="$agent_id" kanna open md --name doc --tab "$dir/review.md" >/dev/null || return 1
  md_comments_md() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"doc\"${1:-}}}"; }
  check wait_for 5 eval '[[ "$(md_comments_md | field markup)" == False && "$(md_comments_md | field agent)" == None && "$(md_comments_md | field loads)" -ge 1 ]]' || { echo "open md should show the file without comments"; md_comments_md; return 1; }
  md_comments_kept() { md_comments_md | python3 -c 'import json, sys; d = json.load(sys.stdin); print(d["view"], d["loads"], round(d["scroll"]))'; }
  md_comments_md ',"scroll":120' >/dev/null
  check wait_for 5 eval '[[ "$(md_comments_kept | cut -d " " -f 3)" == 120 ]]' || { echo "the doc should scroll to 120"; md_comments_md; return 1; }
  kept="$(md_comments_kept)"
  KANNA_TERMINAL="$agent_id" kanna markup doc | grep -qF "comments on in" || return 1
  check wait_for 5 eval '[[ "$(md_comments_md | field markup)" == True ]]' || return 1
  check test "$(md_comments_kept)" == "$kept" || { echo "turning comments on should keep the view, its load and its scroll ($kept)"; return 1; }
  md_comments_md ',"scroll":0' >/dev/null
  md_comments_count() { md_comments_md | field threads | python3 -c 'import json, sys; print(len(json.load(sys.stdin)))'; }
  md_comments_thread() { md_comments_md | field threads | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin) if t["id"] == sys.argv[1]][0]
v = t[sys.argv[2]]
print(len(v) if isinstance(v, list) else v)' "$1" "$2"; }
  md_comments_shot() { raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$1.png\"}}" >/dev/null; cp "$KANNA_STATE_DIR/$1.png" "$out/$1.png" 2>/dev/null; }
  check wait_for 5 eval '[[ "$(md_comments_md | field agent)" == "$agent_id" ]]' || { echo "agent=$agent_id"; kanna workspace list --all --json; md_comments_md; return 1; }
  check test "$(md_comments_count)" == 3 || return 1
  sleep 0.5; md_comments_shot md_comments-threads
  at="$(md_comments_md | field text | python3 -c 'import sys; print(sys.stdin.read().index("SENDME"))')"
  md_comments_md ",\"select\":[$at,6]" >/dev/null
  key cmd+return; check test "$(md_comments_md | field box)" == new || return 1
  sleep 0.3; md_comments_shot md_comments-composer
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Please fix"}}' >/dev/null
  check test "$(md_comments_md | field box_text)" == "Please fix" || return 1
  key cmd+return
  check wait_for 5 eval '[[ "$(md_comments_thread c4 status)" == sent ]]' || { md_comments_md; return 1; }
  check test "$(md_comments_md | field cards.c4.typing)" == True || { md_comments_md; return 1; }
  check wait_for 8 eval '[[ "$(md_comments_md | field cards.c4.waiting)" == "Not answered · the agent stopped" ]]' || { md_comments_md; return 1; }
  check test "$(md_comments_md | field cards.c4.typing)" == False || return 1
  check test "$(md_comments_thread c4 line)" == 22 || return 1
  check wait_for 5 eval 'kanna read agent | sed "s/^IN //" | tr -d "\\n " | grep -qF "^[200~[kannacommentc4·review.md:22]>SENDMEPleasefix^[201~"' || { kanna read agent; return 1; }
  xy="$(md_comments_md ',"mark":"c1"')"
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"doc\",\"at\":[$(echo "$xy" | field x),$(echo "$xy" | field y)]}}" >/dev/null
  check test "$(md_comments_md | field box)" == c1 || return 1
  check test "$(md_comments_md | field active)" == c1 || return 1
  sleep 0.3; md_comments_shot md_comments-thread
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Thanks"}}' >/dev/null
  key return
  check wait_for 5 eval '[[ "$(md_comments_thread c1 messages)" == 3 ]]' || return 1
  check wait_for 5 eval 'kanna read agent | sed "s/^IN //" | tr -d "\\n " | grep -qF "Thanks^[201~"' || return 1
  key escape; check test "$(md_comments_md | field box)" == None || return 1
  xy="$(md_comments_md ',"mark":"c3"')"
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"doc\",\"command\":true,\"at\":[$(echo "$xy" | field x),$(echo "$xy" | field y)]}}" >/dev/null
  check test "$(md_comments_md | field box)" == new || return 1
  raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$long_comment\"}}" >/dev/null
  check eval 'md_comments_md | box_fits' || return 1
  sleep 0.3; md_comments_shot md_comments-long
  key escape; check test "$(md_comments_md | field box)" == None || return 1
  for _ in 1 2 3 4 5; do md_comments_md ',"zoom":1' >/dev/null; done
  check test "$(md_comments_md | field zoom)" == 2 || return 1
  xy="$(md_comments_md ',"mark":"c3"')"
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"doc\",\"command\":true,\"at\":[$(echo "$xy" | field x),$(echo "$xy" | field y)]}}" >/dev/null
  check test "$(md_comments_md | field box)" == new || return 1
  raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$long_comment\"}}" >/dev/null
  check eval 'md_comments_md | box_fits' || return 1
  sleep 0.3; md_comments_shot md_comments-long-zoomed
  key escape; check test "$(md_comments_md | field box)" == None || return 1
  for _ in 1 2 3 4 5; do md_comments_md ',"zoom":-1' >/dev/null; done
  check test "$(md_comments_count)" == 4 || return 1
  kanna markup reply c4 --doc "$dir/review.md" "Fixed the wording." >/dev/null || return 1
  check wait_for 5 eval '[[ "$(md_comments_thread c4 messages)" == 2 ]]' || return 1
  check wait_for 5 eval '[[ "$(md_comments_md | field cards.c4.waiting)" == None && "$(md_comments_md | field cards.c4.typing)" == False ]]' || { md_comments_md; return 1; }
  kanna markup resolve c4 --doc "$dir/review.md" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(md_comments_thread c4 state)" == resolved ]]' || return 1
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  sleep 0.5; md_comments_shot md_comments-dark
  kept="$(md_comments_md | field view)"
  kanna markup doc --off | grep -qF "comments off in" || return 1
  check wait_for 5 eval '[[ "$(md_comments_md | field markup)" == False ]]' || return 1
  check test "$(md_comments_md | field view)" == "$kept" || { echo "turning comments off should keep the view"; return 1; }
  KANNA_TERMINAL="$agent_id" kanna markup "$dir/review.md" --name doc2 --tab >/dev/null || return 1
  check wait_for 5 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"doc2\"}}" | field markup)" == True ]]'
}

scenario md_image_reload M4
md_image_reload() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR/imgdoc" top
  mkdir -p "$dir"
  png() { python3 -c 'import struct, sys, zlib
w, h = int(sys.argv[2]), int(sys.argv[3])
chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
raw = b"".join(b"\0" + b"\x40\x80\xc0" * w for _ in range(h))
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))' "$dir/shot.png" "$1" "$2"; }
  { echo "# md_image_reload"; for i in $(seq 120); do echo; echo "Line $i of the doc."; done; echo; echo "![shot](shot.png)"; } >"$dir/doc.md"
  png 40 30
  kanna open --window new md "$dir/doc.md" --name doc --tab >/dev/null || return 1
  md_image_reload_md() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"doc\"${1:-}}}"; }
  check wait_for 5 eval '[[ "$(md_image_reload_md | field images)" == "[[40, 30]]" ]]' || { md_image_reload_md; return 1; }
  md_image_reload_md ',"scroll":600' >/dev/null
  top="$(md_image_reload_md | field scroll)"
  check test "$top" != 0 || return 1
  png 80 60
  check wait_for 1 eval '[[ "$(md_image_reload_md | field images)" == "[[80, 60]]" ]]' || { md_image_reload_md | field images; return 1; }
  check test "$(md_image_reload_md | field scroll)" == "$top" || return 1
  rm "$dir/shot.png"
  check wait_for 1 eval '[[ "$(md_image_reload_md | field images)" == "[]" ]]' || return 1
  png 50 20
  check wait_for 1 eval '[[ "$(md_image_reload_md | field images)" == "[[50, 20]]" ]]' || return 1
  check test "$(md_image_reload_md | field scroll)" == "$top"
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
  check drawn "$(snap d1 docked_pane-docked)" 2 || return 1
  key "cmd+shift+["
  check test "$(dwin d1 tab)" == "$(dwin d1 tabs.0.id)" || return 1
  check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  sleep 0.5
  check drawn "$(snap d1 docked_pane-first-tab)" 2 || return 1
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
  shot="$(snap d1 docked_pane-tab-zoomed)"
  check drawn "$shot" 1 || return 1
  check python3 -c 'import json, sys; s = json.loads(sys.argv[1]); assert s["panes"][0]["width"] == s["width"] == s["tabBar"]["width"] and s["tabBar"]["shown"], s' "$shot" || return 1
  click_tab "$(dwin d1 tabs.0.id)"
  check test "$(dwin d1 tab):$(dwin d1 zoomed)" == "$(dwin d1 tabs.0.id):$dt1" || { echo "clicking a tab while zoomed should show its pane zoomed"; return 1; }
  check test "$(field tabBar.shown <<<"$(snap d1 docked_pane-tab-zoomed-clicked)")" == True || return 1
  click_tab "$(dwin d1 tabs.1.id)"
  check test "$(dwin d1 tab):$(dwin d1 zoomed)" == "$(dwin d1 tabs.1.id):$dt3" || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == None || return 1
  sleep 0.5
  check drawn "$(snap d1 docked_pane-tab-unzoomed)" 3 || return 1
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
  check drawn "$(snap d1 docked_pane-resized)" 2 || return 1
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
  shot="$(snap d1 docked_pane-only-dock-zoomed)"
  check drawn "$shot" 1 || return 1
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
  check drawn "$(snap d1 docked_pane-restored)" 2
}

scenario markup_reuse M4
markup_reuse() {
  start_daemon && start_app || { echo "markup_reuse line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent" dir="$KANNA_STATE_DIR/reuse" mine other said kept count
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "markup_reuse line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd "$repo" && kanna task new "Mine" "say hi" --agent "$agent" && kanna task new "Other" "say hi" --agent "$agent") >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null && kanna task pick "$(tid 2)" >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(pane Mine id)" == term-* && "$(pane Other id)" == term-* ]]' || { kanna workspace list --all; return 1; }
  mine="$(pane Mine id)" other="$(pane Other id)"
  mkdir -p "$dir/.git"
  { printf '# Spec\n'; for i in $(seq 1 60); do printf '\nParagraph %s of the spec.\n' "$i"; done; } >"$dir/spec.md"
  printf '<!doctype html><html><body><p>A page.</p></body></html>\n' >"$dir/page.html"
  KANNA_TERMINAL="$mine" kanna open md --name first --tab "$dir/spec.md" >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  KANNA_TERMINAL="$mine" kanna open md --name second --tab "$dir/spec.md" >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  KANNA_TERMINAL="$other" kanna open md --name theirs --tab "$dir/spec.md" >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  markup_reuse_md() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$1\"${2:-}}}"; }
  markup_reuse_kept() { markup_reuse_md "$1" | python3 -c 'import json, sys; d = json.load(sys.stdin); print(d["view"], d["loads"], round(d["scroll"]))'; }
  markup_reuse_count() { kanna workspace list --all --json | python3 -c 'import json, sys; print(len(json.load(sys.stdin)["panes"]))'; }
  check wait_for 5 eval '[[ "$(markup_reuse_md first | field loads)" -ge 1 && "$(markup_reuse_md second | field loads)" -ge 1 ]]' || { markup_reuse_md first; return 1; }
  markup_reuse_md first ',"scroll":120' >/dev/null
  check wait_for 5 eval '[[ "$(markup_reuse_kept first | cut -d " " -f 3)" == 120 ]]' || { markup_reuse_md first; return 1; }
  kanna focus first >/dev/null && kanna focus "$mine" >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  kept="$(markup_reuse_kept first)" count="$(markup_reuse_count)"
  said="$(cd "$dir" && KANNA_TERMINAL="$mine" kanna markup spec.md)" || { echo "markup_reuse line $LINENO: $said"; return 1; }
  [[ "$said" == "comments on in $(pane first id)"*"which already shows "*"spec.md"*"; focused" ]] || { echo "markup_reuse: kanna markup FILE should reuse first, focused last: $said"; return 1; }
  check wait_for 5 eval '[[ "$(markup_reuse_md first | field markup)" == True && "$(markup_reuse_md first | field agent)" == "$mine" ]]' || { markup_reuse_md first; return 1; }
  check test "$(markup_reuse_kept first)" == "$kept" || { echo "markup_reuse: reusing the pane should keep its view, its load and its scroll ($kept)"; markup_reuse_kept first; return 1; }
  check test "$(markup_reuse_count)" == "$count" || { echo "markup_reuse: kanna markup FILE opened a pane though one showed FILE"; kanna workspace list --all; return 1; }
  check test "$(ws "$(tid 1)" focused)" == first || { echo "markup_reuse: the reused pane isn't focused"; kanna workspace list --all; return 1; }
  check test "$(markup_reuse_md second | field markup)" == False || return 1
  check test "$(markup_reuse_md theirs | field markup)" == False || { echo "markup_reuse: a pane in another task's workspace was used"; return 1; }
  kanna markup first --off >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  kanna focus second >/dev/null && kanna focus "$mine" >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  said="$(KANNA_TERMINAL="$mine" kanna markup "$dir/spec.md")" || { echo "markup_reuse line $LINENO: $said"; return 1; }
  [[ "$said" == "comments on in $(pane second id)"* ]] || { echo "markup_reuse: second was focused last: $said"; return 1; }
  kanna focus second >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  kanna markup second --off >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  said="$(KANNA_TERMINAL="$mine" kanna markup "$dir/spec.md")" || { echo "markup_reuse line $LINENO: $said"; return 1; }
  [[ "$said" == "comments on in $(pane second id)"* ]] || { echo "markup_reuse: second is focused: $said"; return 1; }
  said="$(KANNA_TERMINAL="$mine" kanna markup "$dir/spec.md" --tab)" || { echo "markup_reuse line $LINENO: $said"; return 1; }
  [[ "$said" == opened* ]] || { echo "markup_reuse: --tab should open a new pane: $said"; return 1; }
  KANNA_TERMINAL="$mine" kanna open web --name pg --tab "$dir/page.html" >/dev/null || { echo "markup_reuse line $LINENO"; return 1; }
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"pg\"}}" | grep -q "A page"' || return 1
  kanna focus "$mine" >/dev/null
  count="$(markup_reuse_count)"
  said="$(KANNA_TERMINAL="$mine" kanna markup "$dir/page.html")" || { echo "markup_reuse line $LINENO: $said"; return 1; }
  [[ "$said" == "comments on in $(pane pg id)"*"already shows"*"page.html"* ]] || { echo "markup_reuse: kanna markup PAGE should reuse pg: $said"; return 1; }
  check wait_for 5 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"pg\"}}" | field markup)" == True ]]' || return 1
  check test "$(markup_reuse_count)" == "$count" || return 1
  check test "$(ws "$(tid 1)" focused)" == pg
}

scenario markup_button M4
markup_button() {
  start_daemon && start_app || { echo "markup_button line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent" dir="$KANNA_STATE_DIR/button" mine planner opened w
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "markup_button line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd "$repo" && kanna task new "Mine" "say hi" --agent "$agent") >/dev/null || { echo "markup_button line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null || { echo "markup_button line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(pane Mine id)" == term-* ]]' || { kanna workspace list --all; return 1; }
  mine="$(pane Mine id)"
  check wait_for 10 is Mine status idle || { echo "markup_button: the task's agent has no status"; return 1; }
  mkdir -p "$dir/.git"
  { printf '# Spec\n\nThe redirect keeps the query string.\n'; for i in $(seq 1 30); do printf '\nParagraph %s.\n' "$i"; done; } >"$dir/spec.md"
  mkdir -p "$dir/.kanna/docs"
  printf '{"file":"%s","agent":null,"socket":null,"next":2,"threads":[{"id":"c1","quote":"keeps the query string","prefix":"The redirect ","suffix":".","line":3,"state":"open","status":"sent","messages":[{"who":"you","text":"And the fragment?","at":"2026-01-01T00:00:00Z"}]}]}\n' "$dir/spec.md" >"$dir/.kanna/docs/spec.md.json"
  printf '<!doctype html><html><body><p>A page.</p></body></html>\n' >"$dir/page.html"
  markup_button_c() { raw "{\"id\":1,\"cmd\":\"debug.comments\",\"args\":{\"pane\":\"$1\"${2:-}}}"; }
  KANNA_TERMINAL="$mine" kanna open md --name spec --tab "$dir/spec.md" >/dev/null || { echo "markup_button line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(markup_button_c spec | field comments_button)" == True ]]' || { echo "markup_button: a doc pane with comments off should offer Comments"; markup_button_c spec; return 1; }
  markup_button_c spec ',"press":"comments"' >/dev/null
  check wait_for 5 eval '[[ "$(markup_button_c spec | field markup)" == True && "$(pane spec agent)" == "$mine" ]]' || { echo "markup_button: Comments in a task's doc should send comments to its agent"; markup_button_c spec; pane spec agent; return 1; }
  check wait_for 5 eval '[[ "$(markup_button_c spec | field comments_button)" == False && "$(markup_button_c spec | field bar)" == *"Threads (1)"* && "$(markup_button_c spec | field bar)" == *"Agent: Mine"* ]]' || { markup_button_c spec; return 1; }
  check test "$(markup_button_c spec | field marks)" == True || return 1
  markup_button_c spec ',"press":"threads"' >/dev/null
  sleep 0.3
  [[ "$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"spec"}}' | field threads_shown)" == True ]] && markup_button_c spec ',"press":"threads"' >/dev/null
  check wait_for 5 eval '[[ "$(markup_button_c spec | field marks)" == False ]]' || { echo "markup_button: Threads should hide the marks"; markup_button_c spec; return 1; }
  check test "$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"spec"}}' | field threads_shown)" == False || return 1
  markup_button_c spec ',"press":"threads"' >/dev/null
  check wait_for 5 eval '[[ "$(markup_button_c spec | field marks)" == True ]]' || return 1
  markup_button_c spec ',"press":"menu"' >/dev/null
  check wait_for 5 eval '[[ "$(markup_button_c spec | field markup)" == False && "$(markup_button_c spec | field comments_button)" == True ]]' || { echo "markup_button: Pane ▸ Comments should turn comments off"; markup_button_c spec; return 1; }
  check grep -q '"c1"' "$dir/.kanna/docs/spec.md.json" || { echo "markup_button: turning comments off lost the threads"; return 1; }
  markup_button_c spec ',"press":"menu"' >/dev/null
  check wait_for 5 eval '[[ "$(markup_button_c spec | field markup)" == True && "$(pane spec agent)" == "$mine" ]]' || { echo "markup_button: Pane ▸ Comments should route as the button does"; return 1; }
  opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^(opened|brought\ forward)\ (d[0-9]+) ]] || { echo "markup_button line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[2]}"
  check wait_for 10 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field lead)" == term-* ]]' || { echo "markup_button: no planner"; return 1; }
  planner="$(raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field lead)"
  KANNA_TERMINAL="$planner" kanna open md --name plan --tab "$dir/spec.md" >/dev/null || { echo "markup_button line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(markup_button_c plan | field comments_button)" == True ]]' || { markup_button_c plan; return 1; }
  markup_button_c plan ',"press":"comments"' >/dev/null
  check wait_for 5 eval '[[ "$(markup_button_c plan | field markup)" == True && "$(pane plan agent)" == "$planner" ]]' || { echo "markup_button: Comments in the Desk view should send comments to the planner"; pane plan agent; return 1; }
  kanna open web --window new --name loose "$dir/page.html" >/dev/null || { echo "markup_button line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(markup_button_c loose | field comments_button)" == True ]]' || { echo "markup_button: a local page should have the bar"; markup_button_c loose; return 1; }
  markup_button_c loose ',"press":"comments"' >/dev/null
  check wait_for 5 eval '[[ "$(markup_button_c loose | field menu)" == *"\"$mine\""* ]]' || { echo "markup_button: a free window's Comments should offer the running agents"; markup_button_c loose; return 1; }
  check test "$(markup_button_c loose | field markup)" == False || return 1
  markup_button_c loose ",\"pick\":\"$mine\"" >/dev/null
  check wait_for 5 eval '[[ "$(markup_button_c loose | field markup)" == True && "$(pane loose agent)" == "$mine" ]]' || { echo "markup_button: picking an agent should turn comments on, sent there"; markup_button_c loose; return 1; }
  check wait_for 5 eval '[[ "$(markup_button_c loose | field bar)" == *"Threads (0)"* ]]' || { markup_button_c loose; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"markup_button.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/markup_button.png" "$out/markup_button.png" 2>/dev/null
  return 0
}

scenario page_comments M4
page_comments() {
  start_daemon && start_app || return 1
  kanna open --window new --name agent --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows agent READY || return 1
  local dir="$KANNA_STATE_DIR/page" agent_id xy mark kept page_comments_web_kept
  mkdir -p "$dir/.git"
  cat >"$dir/page.html" <<'HTML'
<!doctype html>
<html><head><meta charset="utf-8"><style>body { font: 16px sans-serif; margin: 40px; height: 2400px; width: 2400px; } button { font-size: 22px; padding: 10px 24px; } #red { position: absolute; left: 560px; top: 420px; width: 120px; height: 60px; background: #ff0000; }</style></head>
<body>
<h1>Mockup</h1>
<p>A page to mark up.</p>
<button id="go">Ship it</button>
<div id="red"></div>
</body></html>
HTML
  agent_id="$(pane agent id)"
  KANNA_TERMINAL="$agent_id" kanna open web --name plain --tab "$dir/page.html" >/dev/null || return 1
  check test "$(pane plain url)" == "file://$dir/page.html" || return 1
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"plain\"}}" | grep -q "Ship it"' || return 1
  [[ "$(raw '{"id":1,"cmd":"debug.web","args":{"pane":"plain"}}')" != *threads* ]] || { echo "open web should show the page without comments"; return 1; }
  page_comments_plain() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"plain\"${1:-}}}"; }
  page_comments_kept() { page_comments_web_kept="$(page_comments_plain ',"js":"[window.scrollY, window.kept || 0]"' | field value)"; page_comments_plain | python3 -c 'import json, sys; d = json.load(sys.stdin); print(d["view"], d["web_view"], d["loads"], sys.argv[1])' "$page_comments_web_kept"; }
  page_comments_plain ',"js":"window.kept = 42; window.scrollTo(0, 300); window.scrollY"' >/dev/null
  check wait_for 5 eval '[[ "$(page_comments_plain ",\"js\":\"window.scrollY\"" | field value)" == 300 ]]' || { page_comments_plain ',"js":"window.scrollY"'; return 1; }
  kept="$(page_comments_kept)"
  KANNA_TERMINAL="$agent_id" kanna markup plain | grep -qF "comments on in" || return 1
  check wait_for 5 eval '[[ "$(page_comments_plain | field markup)" == True ]]' && check wait_for 5 eval 'page_comments_plain | grep -q threads' || { page_comments_plain; return 1; }
  check test "$(page_comments_kept)" == "$kept" || { echo "turning comments on should keep the web view, its page, its scroll and its JS state ($kept)"; return 1; }
  kanna markup plain --off | grep -qF "comments off in" || return 1
  check wait_for 5 eval '[[ "$(page_comments_plain | field markup)" == False ]]' || return 1
  [[ "$(page_comments_plain)" != *threads* ]] || { echo "turning comments off should take the comment UI away"; return 1; }
  check test "$(page_comments_kept)" == "$kept" || { echo "turning comments off should keep the page ($kept)"; return 1; }
  kanna close plain >/dev/null || return 1
  KANNA_TERMINAL="$agent_id" kanna markup --name page --tab "$dir/page.html" >/dev/null || return 1
  page_comments_web() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"page\"${1:-}}}"; }
  page_comments_thread() { page_comments_web | field threads | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin) if t["id"] == sys.argv[1]][0]
for k in sys.argv[2].split("."): t = t[k]
print(len(t) if isinstance(t, list) else t)' "$1" "$2"; }
  page_comments_click() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"page\",$1}}" >/dev/null; }
  page_comments_shot() { raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$1.png\"}}" >/dev/null; cp "$KANNA_STATE_DIR/$1.png" "$out/$1.png" 2>/dev/null; }
  check wait_for 5 eval '[[ "$(page_comments_web | field agent)" == "$agent_id" ]]' || { kanna workspace list --all --json; page_comments_web; return 1; }
  check test "$(pane page markup)" == True || return 1
  check wait_for 5 eval 'page_comments_web ",\"find\":\"#go\"" | grep -q at' || return 1
  xy="$(page_comments_web ',"find":"#go"' | field at)"
  page_comments_click "\"command\":true,\"at\":$xy"
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == new ]]' || { page_comments_web; return 1; }
  sleep 0.3; page_comments_shot page_comments-composer
  raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$long_comment\"}}" >/dev/null
  check eval 'page_comments_web | box_fits' || return 1
  sleep 0.3; page_comments_shot page_comments-long
  key escape; check wait_for 5 eval '[[ "$(page_comments_web | field box)" == None ]]' || return 1
  page_comments_click "\"command\":true,\"at\":$xy"
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == new ]]' || { page_comments_web; return 1; }
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Bigger"}}' >/dev/null
  key cmd+return
  check wait_for 5 eval '[[ "$(page_comments_thread c1 status)" == sent ]]' || { page_comments_web; return 1; }
  check wait_for 3 eval 'page_comments_web | field dot_typing | grep -q c1' || { page_comments_web; return 1; }
  check test "$(page_comments_thread c1 page.selector)" == "#go" || return 1
  check test "$(page_comments_thread c1 page.text)" == "Ship it" || return 1
  check test "$(page_comments_thread c1 line)" == 6 || return 1
  mark="$(page_comments_thread c1 mark)"
  check test "$mark" == ".kanna/docs/marks/page.html-c1.png" || return 1
  [[ "$(head -c 4 "$dir/$mark" | tail -c 3)" == PNG ]] || { echo "the mark should be a PNG: $dir/$mark"; return 1; }
  cp "$dir/$mark" "$out/page_comments-mark-c1.png"
  check wait_for 5 eval 'kanna read agent | sed "s/^IN //" | tr -d "\\n " | grep -qF "^[200~[kannacommentc1·page.html:6]>thepage,element#go\"Shipit\">box"' || { kanna read agent; return 1; }
  check wait_for 5 eval 'kanna read agent | sed "s/^IN //" | tr -d "\\n " | grep -qF ">markedup:.kanna/docs/marks/page.html-c1.pngBigger^[201~"' || { kanna read agent; return 1; }
  page_comments_click '"command":true,"at":[300,330],"to":[500,420]'
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == new ]]' || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Too empty"}}' >/dev/null
  key cmd+return
  check wait_for 5 eval '[[ "$(page_comments_thread c2 status)" == sent ]]' || { page_comments_web; return 1; }
  check test "$(page_comments_thread c2 page.selector)" == "" || return 1
  check wait_for 5 eval 'kanna read agent | sed "s/^IN //" | tr -d "\\n " | grep -qF "[kannacommentc2·page.html:1]>thepage,abox>box"' || { kanna read agent; return 1; }
  check wait_for 5 eval '[[ "$(page_comments_web | field marks | python3 -c "import json, sys; print(sorted(json.load(sys.stdin)))")" == "['"'"'c1'"'"', '"'"'c2'"'"']" ]]' || { page_comments_web; return 1; }
  sleep 0.3; page_comments_shot page_comments-marks
  xy="$(page_comments_web | field marks.c1)"
  page_comments_click "\"at\":$xy"
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == c1 ]]' || { page_comments_web; return 1; }
  check test "$(page_comments_web | field active)" == c1 || return 1
  sleep 0.3; page_comments_shot page_comments-thread
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Thanks"}}' >/dev/null
  key return
  check wait_for 5 eval '[[ "$(page_comments_thread c1 messages)" == 2 ]]' || return 1
  key escape; check wait_for 5 eval '[[ "$(page_comments_web | field box)" == None ]]' || return 1
  page_comments_click "\"at\":$xy"
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == c1 ]]' || return 1
  page_comments_click '"at":[150,450]'
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == None ]]' || { page_comments_web; return 1; }
  check test "$(page_comments_web | field cards.c2.reply)" == False || { page_comments_web; return 1; }
  page_comments_click "\"at\":$(page_comments_web | field cards.c2.at)"
  check wait_for 5 eval '[[ "$(page_comments_web | field active)" == c2 ]]' || { page_comments_web; return 1; }
  check test "$(page_comments_web | field cards.c2.reply)" == False || { page_comments_web; return 1; }
  page_comments_click "\"at\":$(page_comments_web | field cards.c2.field)"
  check wait_for 5 eval '[[ "$(page_comments_web | field cards.c2.reply)" == True ]]' || { page_comments_web; return 1; }
  page_comments_click '"at":[150,450]'
  kanna markup reply c2 --doc "$dir/page.html" "Filled it." >/dev/null || return 1
  check wait_for 5 eval '[[ "$(page_comments_thread c2 messages)" == 2 ]]' || return 1
  sed -i '' 's|<p>A page to mark up.</p>|<p>A page to mark up.</p><p>More above the button.</p>|' "$dir/page.html"
  check wait_for 5 eval 'page_comments_web | field text | grep -q "More above"' || return 1
  check wait_for 5 eval '[[ "$(page_comments_web | field marks | python3 -c "import json, sys; print(len(json.load(sys.stdin)))")" == 2 ]]' || return 1
  kanna focus page >/dev/null || return 1
  key cmd+=; key cmd+=
  check wait_for 5 eval '[[ "$(page_comments_web | field zoom)" == 1.25 ]]' || { page_comments_web; return 1; }
  sleep 0.3
  check test "$(python3 -c 'import json, sys; a, b = json.loads(sys.argv[1]), json.loads(sys.argv[2]); print(abs(a[0] - b[0]) < 2 and abs(a[1] - b[1]) < 2)' "$(page_comments_web | field areas.c1)" "$(page_comments_web ',"find":"#go"' | field at)")" == True || { page_comments_web; page_comments_web ',"find":"#go"'; return 1; }
  page_comments_shot page_comments-zoomed
  key cmd+0
  kanna navigate page "file://$dir/page.html#later" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(page_comments_web | field hash)" == "#later" ]]' || { page_comments_web; return 1; }
  check wait_for 5 eval '[[ "$(page_comments_web | field marks)" == "{}" ]]' || { page_comments_web; return 1; }
  kanna navigate page "file://$dir/page.html" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(page_comments_web | field hash)" == "" ]]' || return 1
  xy="$(page_comments_web ',"find":"#red"' | field at)"
  page_comments_click "\"command\":true,\"at\":$xy"
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == new ]]' || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Red"}}' >/dev/null
  key cmd+return
  check wait_for 5 eval '[[ "$(page_comments_thread c3 status)" == sent ]]' || { page_comments_web; return 1; }
  page_comments_red() {
    raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$1.png\"}}" >/dev/null; cp "$KANNA_STATE_DIR/$1.png" "$out/$1.png"
    python3 - "$KANNA_STATE_DIR/$1.png" <<'PY'
import struct, sys, zlib
data = open(sys.argv[1], "rb").read()
at, idat = 8, b""
while at < len(data):
    n = struct.unpack(">I", data[at:at + 4])[0]
    kind, chunk = data[at + 4:at + 8], data[at + 8:at + 8 + n]
    at += 12 + n
    if kind == b"IHDR": w, h, _, ct = struct.unpack(">IIBB", chunk[:10])
    if kind == b"IDAT": idat += chunk
raw, bpp = zlib.decompress(idat), 4 if ct == 6 else 3
stride, prev, o, hits = w * bpp, bytearray(w * bpp), 0, []
for y in range(h):
    f, line = raw[o], bytearray(raw[o + 1:o + 1 + stride])
    o += 1 + stride
    for x in range(stride):
        a = line[x - bpp] if x >= bpp else 0
        b, c = prev[x], prev[x - bpp] if x >= bpp else 0
        if f == 1: line[x] = (line[x] + a) & 255
        elif f == 2: line[x] = (line[x] + b) & 255
        elif f == 3: line[x] = (line[x] + (a + b) // 2) & 255
        elif f == 4:
            p = a + b - c
            line[x] = (line[x] + (a if abs(p - a) <= abs(p - b) and abs(p - a) <= abs(p - c) else b if abs(p - b) <= abs(p - c) else c)) & 255
    prev = line
    for x in range(0, w, 2):
        if line[x * bpp] > 200 and line[x * bpp + 1] < 90 and line[x * bpp + 2] < 90: hits.append((x, y))
xs, ys = [p[0] for p in hits], [p[1] for p in hits]
print(f"[{(min(xs) + max(xs)) / 4}, {(min(ys) + max(ys)) / 4}]")
PY
  }
  page_comments_moved() { python3 -c 'import json, sys; a, b, c, d = (json.loads(v) for v in sys.argv[1:]); dx, dy = (b[0] - a[0]) - (d[0] - c[0]), (b[1] - a[1]) - (d[1] - c[1]); print(abs(dx) < 3 and abs(dy) < 3, dx, dy)' "$@"; }
  local red0 mark0 red1 mark1
  sleep 0.3
  red0="$(page_comments_red page_comments-red-start)"; mark0="$(page_comments_web | field areas.c3)"
  key cmd+=; key cmd+=
  raw '{"id":1,"cmd":"debug.web","args":{"pane":"page","js":"scrollTo(150, 200)"}}' >/dev/null
  sleep 0.6
  red1="$(page_comments_red page_comments-red-zoomed)"; mark1="$(page_comments_web | field areas.c3)"
  [[ "$(page_comments_moved "$red0" "$red1" "$mark0" "$mark1")" == True* ]] || { echo "the mark should move with the page: red $red0 → $red1, mark $mark0 → $mark1: $(page_comments_moved "$red0" "$red1" "$mark0" "$mark1")"; return 1; }
  raw '{"id":1,"cmd":"debug.pinch","args":{"pane":"page","steps":[0.15,0.1]}}' >/dev/null
  sleep 0.6
  red1="$(page_comments_red page_comments-red-pinched)"; mark1="$(page_comments_web | field areas.c3)"
  [[ "$(page_comments_moved "$red0" "$red1" "$mark0" "$mark1")" == True* ]] || { echo "the mark should move with a pinch: red $red0 → $red1, mark $mark0 → $mark1"; return 1; }
  key cmd+0
  raw '{"id":1,"cmd":"debug.web","args":{"pane":"page","js":"(() => { scrollTo(0, 0); const r = document.createRange(); r.selectNodeContents(document.querySelector(\"p\")); getSelection().removeAllRanges(); getSelection().addRange(r); return 1; })()"}}' >/dev/null
  kanna focus page >/dev/null; sleep 0.3
  key cmd+return
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == new ]]' || { page_comments_web; return 1; }
  sleep 0.3; page_comments_shot page_comments-selection
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Reword"}}' >/dev/null
  key cmd+return
  check wait_for 5 eval '[[ "$(page_comments_thread c4 status)" == sent ]]' || { page_comments_web; return 1; }
  check test "$(page_comments_thread c4 page.text)" == "A page to mark up." || return 1
  check wait_for 5 eval 'kanna read agent | sed "s/^IN //" | tr -d "\n " | grep -qF "[kannacommentc4·page.html:5]>thepage,thetext\"Apagetomarkup.\"inbody>p:nth-of-type(1)>box"' || { kanna read agent; return 1; }
  check wait_for 5 eval 'page_comments_web | field marks | grep -q c4' || { page_comments_web; return 1; }
  local size
  size="$(raw '{"id":1,"cmd":"debug.web","args":{"pane":"page","js":"[innerWidth, innerHeight]"}}' | field value)"
  page_comments_click "\"command\":true,\"at\":[4,38],\"to\":$(python3 -c 'import json, sys; w, h = json.loads(sys.argv[1]); print(json.dumps([w - 4, h + 30]))' "$size")"
  check wait_for 5 eval '[[ "$(page_comments_web | field box)" == new ]]' || { page_comments_web; return 1; }
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Too busy overall"}}' >/dev/null
  key cmd+return
  check wait_for 5 eval '[[ "$(page_comments_thread c5 status)" == sent ]]' || { page_comments_web; return 1; }
  key escape
  check wait_for 5 eval 'page_comments_web | field areas | grep -q c5' || { page_comments_web; return 1; }
  check test "$(python3 -c 'import json, sys; x, y = json.loads(sys.argv[1]); print(x < 20 and y < 60)' "$(page_comments_web | field marks.c5)")" == True || { echo "the whole view's dot should sit at its top-left"; page_comments_web; return 1; }
  sleep 0.3; page_comments_shot page_comments-dots
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  sleep 0.5; page_comments_shot page_comments-dark
}

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

scenario zsh_completion M4
zsh_completion() {
  PATH="$bin:$PATH" start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR/zsh_completion"
  mkdir -p "$dir/sub" && touch "$dir/alpha-notes.md" "$dir/alpha.html" "$dir/alpha.txt"
  kanna open --window new --name zsh_completion --cwd "$dir" --cmd "zsh -f -i" >/dev/null || return 1
  kanna open --window win-1 --name zsh_completion-web --tab web "$dir/alpha.html" >/dev/null || return 1
  kanna open --window win-1 --name notes --tab md "$dir/alpha-notes.md" >/dev/null || return 1
  kanna focus zsh_completion >/dev/null || return 1
  sleep 1
  kanna send zsh_completion "PATH='$bin':\$PATH; stty cols 120; source <(kanna completions zsh); clear" || return 1
  sleep 1
  zsh_completion_tab() {
    raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$1\"}}" >/dev/null
    sleep 0.3; key tab; [[ -n "$2" ]] && key tab; sleep 0.7
    zsh_completion_screen=$(kanna read zsh_completion)
    [[ -n "${4:-}" ]] && { raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$4.png\"}}" >/dev/null; cp "$KANNA_STATE_DIR/$4.png" "$out/$4.png" 2>/dev/null; }
    key ctrl+c; sleep 0.3
    printf "%s\n" "$zsh_completion_screen" | grep -qE -- "$3" || { echo "kanna $1⇥ didn't show $3:"; printf "%s\n" "$zsh_completion_screen" | tail -8; return 1; }
  }
  check zsh_completion_tab "kanna open md al" "" 'open md alpha-notes\.md *$' || return 1
  check zsh_completion_tab "kanna markup alpha." "" 'markup alpha\.html *$' || return 1
  check zsh_completion_tab "kanna markup al" 2 'alpha-notes\.md.*alpha\.html|alpha\.html.*alpha-notes\.md' || return 1
  check zsh_completion_tab "kanna nav" "" 'kanna navigate *$' || return 1
  check zsh_completion_tab "kanna open " 2 '^md +-- A markdown file' || return 1
  check test "$(printf "%s\n" "$zsh_completion_screen" | grep -oE '^(term|web|md|ios) ' | tr -d ' \n')" == termwebmdios || return 1
  check zsh_completion_tab "kanna close " 2 'zsh_completion-web +-- page-[0-9]+ · web · alpha\.html' zsh_completion-panes || return 1
  check zsh_completion_tab "kanna focus --tab " 2 'tab-[0-9]+ +-- "' || return 1
  check zsh_completion_tab "kanna open --split " 2 'right' || return 1
  check zsh_completion_tab "kanna open md alpha-notes.md --sp" "" '--split *$' || return 1
  kanna open --window win-1 --name zsh_completion-login --tab --cwd "$dir" >/dev/null || return 1
  check wait_for 10 eval '[[ -n "$(kanna read zsh_completion-login | tr -d "[:space:]")" ]]' || return 1
  sleep 2
  kanna send zsh_completion-login 'print -r -- "zsh_completion:$_comps[kanna]:${ZDOTDIR-home}:${KANNA_ZSH_ZDOTDIR-gone}"' || return 1
  check wait_for 5 shows zsh_completion-login 'zsh_completion:_kanna:home:gone'
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
  sleep 0.5; check drawn "$(snap d1 desk_tasks-desk)" 2 || return 1
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

scenario task_shelf M4
task_shelf() {
  start_daemon && start_app || { echo "task_shelf line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_shelf line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  local added; added="$(in_repo task new "Fix it" "say hi" --agent "$agent" 2>&1)"
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
  check wait_for 5 eval '[[ "$(desk note)" == *"3 tasks on the Desk"* ]]' || { desk note; return 1; }
  stop_app; start_app || { echo "desk_shelf line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk lead)" == "$planner" ]]' || { echo "desk_shelf: the planning terminal didn't survive the app's restart"; kanna workspace list --all; return 1; }
  local refused; refused="$(raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"blank\",\"repo\":\"$repo\"}}")"
  [[ "$refused" == *'"ok":false'* && "$refused" == *"tasks needs an action"* ]] || { echo "desk_shelf: the Shelf still starts blank tasks: $refused"; return 1; }
  key cmd+shift+w
  check wait_for 5 eval '! kanna workspace list --all --json | grep -q "\"$planner\""' || { echo "desk_shelf: closing the window left its planning terminal running"; kanna workspace list --all; return 1; }
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

scenario move_to_shelf M4
move_to_shelf() {
  start_daemon && start_app || { echo "move_to_shelf line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"${2:-d1}\"}}" | field "$1"; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "move_to_shelf line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd "$repo" && kanna task new "Later" "say hi" --agent "$agent") >/dev/null || { echo "move_to_shelf line $LINENO"; return 1; }
  local started wt; started="$(kanna task pick $(tid 1))" || { echo "move_to_shelf line $LINENO"; return 1; }
  [[ "$started" =~ " in "([^ ]+)" (" ]] && wt="${BASH_REMATCH[1]/#\~/$HOME}" || { echo "move_to_shelf line $LINENO: $started"; return 1; }
  check wait_for 5 eval '[[ "$(pane Later id)" == term-1 ]]' || { echo "move_to_shelf line $LINENO"; return 1; }
  touch "$wt/work-in-progress"
  (cd "$repo" && kanna task new "Newer" "say hi" --agent "$agent") >/dev/null || { echo "move_to_shelf line $LINENO"; return 1; }
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

scenario shelf_and_back M4
shelf_and_back() {
  start_daemon && start_app || { echo "shelf_and_back line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_and_back line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  local w
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  (cd "$repo" && kanna task new "Later" "say hi" --agent "$agent") >/dev/null || { echo "shelf_and_back line $LINENO"; return 1; }
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

scenario code_pane M4
code_pane() {
  start_daemon && start_app || return 1
  local file="$KANNA_STATE_DIR/code_pane.rs"
  for i in $(seq 120); do echo "let line_$i = $i;"; done >"$file"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | field "$2"; }
  kanna open --window new code --name src "$file:80" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 80 ]]' || return 1
  check eval '(( $(code src top) > 50 && $(code src top) < 80 ))' || { echo "top $(code src top)"; return 1; }
  check is src line 80 || return 1
  check test "$(code src language)" == rust || return 1
  check wait_for 5 eval '(( $(code src spans) > 300 ))' || { echo "spans $(code src spans)"; return 1; }
  ov() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"src\"${2:-}}}" | field "overview.$1"; }
  codewin() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"win-1\",\"size\":[$1,700]}}" >/dev/null; }
  codewin 1200
  check wait_for 5 eval '[[ "$(ov shown)" == True ]]' || { echo "no overview: wanted $(ov wanted), clip $(code src clip_width) pt"; return 1; }
  check eval '(( $(ov box_height | cut -d. -f1) > 10 && $(ov top | cut -d. -f1) > 0 ))' || { echo "box $(ov box_y) $(ov box_height), top $(ov top)"; return 1; }
  check test "$(code src clip_width | cut -d. -f1)" -le "$((1200 - 98))" || { echo "the text runs under the overview: $(code src clip_width)"; return 1; }
  local tiles; tiles="$(ov rendered_tiles)"
  ov top ',"overview_click":3' >/dev/null
  check wait_for 5 eval '[[ "$(ov top)" == 0* ]]' || { echo "a click at the top left top at $(ov top)"; return 1; }
  ov top ',"overview_drag":[10,60]' >/dev/null
  check wait_for 5 eval '(( $(ov top | cut -d. -f1) > 200 ))' || { echo "dragging the box left top at $(ov top)"; return 1; }
  ov top ',"overview_marks_click":690' >/dev/null
  check wait_for 5 eval '(( $(code src top) > 90 ))' || { echo "a click at the foot of the marks left line $(code src top) at the top"; return 1; }
  check test "$(ov rendered_tiles)" == "$tiles" || { echo "tiles drawn again on scroll: $tiles, then $(ov rendered_tiles)"; return 1; }
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","pick":40}}' >/dev/null
  ov shown ',"overview_hover":true' >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"code_pane-overview-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/code_pane-overview-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"code_pane-overview-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/code_pane-overview-dark.png" "$out/" 2>/dev/null
  ov shown ',"overview_hover":false' >/dev/null
  check test "$(ov shown ',"overview_enabled":false')" == False || { echo "View › Overview off left it shown"; return 1; }
  check test "$(ov enabled)" == False || return 1
  check test "$(ov shown ',"overview_enabled":true')" == True || { echo "View › Overview on didn't bring it back"; return 1; }
  codewin 700
  check wait_for 5 eval '[[ "$(ov shown)" == False && "$(ov wanted)" == True ]]' || { echo "at 700 pt: shown $(ov shown)"; return 1; }
  codewin 1200
  check wait_for 5 eval '[[ "$(ov shown)" == True ]]' || { echo "not back at 1200 pt"; return 1; }
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","reveal":80}}' >/dev/null
  echo 'let code_pane_edit = 1;' >>"$file"
  check wait_for 5 eval 'code src text | grep -q code_pane_edit' || return 1
  check wait_for 5 eval '[[ "$(code src changed)" == 121 ]]' || { echo "changed $(code src changed)"; return 1; }
  check wait_for 5 eval '[[ "$(ov marks_by_kind.changed)" == 1* ]]' || { echo "no changed mark: $(ov marks_by_kind)"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"src","at":[120,200]}}' >/dev/null; sleep 0.3
  key() { raw "{\"id\":1,\"cmd\":\"debug.key\",\"args\":{\"key\":\"$1\"}}" >/dev/null; }
  key cmd+shift+=; key cmd+shift+=
  key cmd+f
  check wait_for 5 eval '[[ "$(code src find_bar)" == 1 ]]' || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"line_77 "}}' >/dev/null
  check wait_for 5 eval '[[ "$(ov marks_by_kind.hit)" == 1* ]]' || { echo "no find hit mark: $(ov marks_by_kind)"; return 1; }
  key return
  check wait_for 5 eval '[[ "$(code src selection_line)" == 77 && "$(code src selection_length)" == 8 ]]' || { echo "found at $(code src selection_line)"; return 1; }
  key escape
  check wait_for 5 eval '[[ "$(code src font_size)" == 14.5 ]]' || { echo "font $(code src font_size)"; return 1; }
  kanna navigate src "$file:10" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 10 ]]' || return 1
  kanna navigate src --back >/dev/null || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 80 ]]' || return 1
  press() { raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$1\"}}" >/dev/null; sleep 0.2; }
  press gg; check wait_for 5 eval '[[ "$(code src top)" == 1 ]]' || { echo "gg top $(code src top)"; return 1; }
  press 3j; press 5j; press k; check wait_for 5 eval '[[ "$(code src top)" == 8 ]]' || { echo "j k top $(code src top)"; return 1; }
  press G; check wait_for 5 eval '(( $(code src top) > 80 && $(code src top) < 100 ))' || { echo "G top $(code src top)"; return 1; }
  press b; check wait_for 5 eval '(( $(code src top) < 70 ))' || { echo "b top $(code src top)"; return 1; }
  check wait_for 5 eval '(( $(code src top) < 80 ))' || { echo "b top $(code src top)"; return 1; }
  [[ "$(kanna open --window win-1 code "$KANNA_STATE_DIR/gone.rs" 2>&1)" == *"No such file: $KANNA_STATE_DIR/gone.rs"* ]] || { echo "code_pane: a missing file"; return 1; }
  [[ "$(kanna open --window win-1 code "$KANNA_STATE_DIR" 2>&1)" == *"is a folder, not a file"* ]] || { echo "code_pane: a folder"; return 1; }
  local nested="$KANNA_STATE_DIR/code_pane-nested.rs"
  { echo 'fn outer() {'; echo '    loop {'; echo '        if ready() {'; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; } >"$nested"
  kanna open --window win-1 code --name nested "$nested" --tab >/dev/null || return 1
  check wait_for 5 eval '(( $(code nested scopes) == 3 ))' || { echo "scopes $(code nested scopes)"; return 1; }
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"nested","pick":40}}' >/dev/null
  check wait_for 5 eval '[[ "$(code nested top)" == 37 && "$(code nested pinned)" == 3 ]]' || { echo "top $(code nested top), pinned $(code nested pinned)"; return 1; }
  check test "$(code nested indent_unit)" == 4 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"nested","at":[300,300]}}' >/dev/null; sleep 0.3
  press gg; press ]]; press ]]; check wait_for 5 eval '[[ "$(code nested current)" == 3 ]]' || { echo "]] ]] current $(code nested current)"; return 1; }
  press [[; check wait_for 5 eval '[[ "$(code nested current)" == 2 ]]' || { echo "[[ current $(code nested current)"; return 1; }
  press G; press }; check wait_for 5 eval '[[ "$(code nested current)" == 63 ]]' || { echo "} current $(code nested current)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 80 ]]' || return 1
  check eval 'code src text | grep -q code_pane_edit' || return 1
  check test "$(code src font_size)" == 14.5 || { echo "font after relaunch $(code src font_size)"; return 1; }
}

scenario file_panes M4
file_panes() {
  start_daemon && start_app || return 1
  local doc="$KANNA_STATE_DIR/file_panes.md" src="$KANNA_STATE_DIR/file_panes.rs"
  printf '# file_panes DOC\n\nfile_panes ONE\n' >"$doc"
  printf 'fn one() {}\n' >"$src"
  doc_shows() { raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | field text | grep -q "$1"; }
  src_shows() { raw '{"id":1,"cmd":"debug.code","args":{"pane":"src"}}' | field text | grep -q "$1"; }
  watched() { kanna raw '{"cmd":"files"}' | python3 -c 'import json, sys; print(" ".join(sorted(f["kind"] + ":" + f["path"].rsplit("/", 1)[-1] for f in json.load(sys.stdin)["files"])))'; }
  kanna open --window new md --name doc "$doc" >/dev/null || return 1
  kanna open --window win-1 code --name src --tab "$src" >/dev/null || return 1
  check wait_for 5 doc_shows "file_panes ONE" || return 1
  check wait_for 5 src_shows "fn one" || return 1
  check test "$(watched)" == "code:file_panes.rs md:file_panes.md" || { echo "watched: $(watched)"; return 1; }
  printf '# file_panes DOC\n\nfile_panes TWO\n' >"$doc"
  check wait_for 5 doc_shows "file_panes TWO" || return 1
  printf '# file_panes DOC\n\nfile_panes THREE\n' >"$doc.tmp" && mv "$doc.tmp" "$doc"
  check wait_for 5 doc_shows "file_panes THREE" || return 1
  printf 'fn one() {}\nfn two() {}\n' >"$src"
  check wait_for 5 src_shows "fn two" || return 1
  check wait_for 5 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"src\"}}" | field changed)" == 2 ]]' || return 1
  rm "$src"; sleep 0.5
  check src_shows "fn two" || return 1
  printf 'fn three() {}\n' >"$src"
  check wait_for 5 src_shows "fn three" || return 1
  stop_daemon; start_daemon || return 1
  check wait_for 10 app_attached || return 1
  check wait_for 5 eval '[[ "$(watched)" == "code:file_panes.rs md:file_panes.md" ]]' || { echo "after a daemon restart: $(watched)"; return 1; }
  printf '# file_panes DOC\n\nfile_panes FOUR\n' >"$doc"
  check wait_for 5 doc_shows "file_panes FOUR" || return 1
  stop_app; start_app || return 1
  check wait_for 5 src_shows "fn three" || return 1
  printf 'fn four() {}\n' >"$src"
  check wait_for 5 src_shows "fn four" || return 1
  kanna close src >/dev/null || return 1
  check wait_for 5 eval '[[ "$(watched)" == "md:file_panes.md" ]]' || { echo "after close: $(watched)"; return 1; }
  [[ "$(kanna open --window win-1 md "$KANNA_STATE_DIR/file_panes-gone.md" 2>&1)" == *"No such file: $KANNA_STATE_DIR/file_panes-gone.md"* ]] || { echo "file_panes: a missing doc"; return 1; }
}

scenario diff_pane M4
diff_pane() {
  start_daemon && start_app || return 1
  local repo="$KANNA_STATE_DIR/diff_repo"
  mkdir -p "$repo" && cd "$repo" || return 1
  dgit() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null; }
  dgit init -q -b main
  for i in $(seq 40); do echo "fn f$i() -> u32 { $i }"; done >a.rs
  echo 'x' >Cargo.lock
  dgit add . && dgit commit -qm first && dgit checkout -qb task
  sed -i '' 's/fn f5() -> u32 { 5 }/fn f5() -> u32 { 500 }/' a.rs && dgit commit -qam second
  sed -i '' 's/fn f30() -> u32 { 30 }/fn f30() -> u64 { 30 }/' a.rs
  echo 'y' >>Cargo.lock
  printf 'fn new() {}\n' >b.rs
  diffpane() { raw "{\"id\":1,\"cmd\":\"debug.diff\",\"args\":{\"pane\":\"changes\"${2:-}}}" | field "$1"; }
  kanna open --window new diff --merge-base main --name changes >/dev/null || { cd - >/dev/null; return 1; }
  cd - >/dev/null
  check wait_for 5 eval '[[ "$(diffpane files)" == *b.rs* ]]' || { echo "files $(diffpane files)"; return 1; }
  check test "$(diffpane command)" == "git diff --merge-base main" || return 1
  check python3 -c 'import json, sys
files = json.loads(sys.argv[1])
assert [f["path"] for f in files] == ["Cargo.lock", "a.rs", "b.rs"], files
assert files[0]["fold"] == "lock" and files[2]["untracked"] and files[1]["added"] == 2, files' "$(diffpane files)" || return 1
  check python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
kinds = [r["kind"] for r in rows]
assert kinds.count("header") == 3 and "note" in kinds and "fold" in kinds, kinds
assert {"kind": "added", "text": "fn f5() -> u32 { 500 }", "new": 5} in rows, rows' "$(diffpane rows)" || return 1
  check wait_for 5 eval '(( $(diffpane highlighted) >= 2 ))' || { echo "highlighted $(diffpane highlighted)"; return 1; }
  local added
  added="$(diffpane rows | python3 -c 'import json, sys; print([i for i, r in enumerate(json.load(sys.stdin)) if r["kind"] == "added"][0])')"
  check eval '[[ "$(diffpane colors ",\"colors\":$added")" != "[]" ]]' || { echo "no colours on row $added"; return 1; }
  refreshed() { check wait_for 5 eval '[[ "$(diffpane stale)" == True ]]' || { echo "no newer diff: stale $(diffpane stale)"; return 1; }; diffpane stale ',"refresh":true' >/dev/null; }
  local before; before="$(diffpane rows)"
  diffpane current ',"keys":"gn"' >/dev/null
  local reading; reading="$(diffpane current)"
  local row; row="$(diffpane rows | python3 -c 'import json, sys; print(json.dumps(json.load(sys.stdin)[int(sys.argv[1])]))' "$reading")"
  sed -i '' 's/fn f20() -> u32 { 20 }/fn f20() -> u32 { 2000 }/' "$repo/a.rs"
  check wait_for 5 eval '[[ "$(diffpane stale)" == True ]]' || { echo "the edit didn't set changed: stale $(diffpane stale)"; return 1; }
  check test "$(diffpane rows)" == "$before" || { echo "the rows moved while reading"; return 1; }
  check test "$(diffpane current)" == "$reading" || { echo "the current line moved while reading"; return 1; }
  check test "$(diffpane fresh)" == "[]" || { echo "marked fresh before a refresh: $(diffpane fresh)"; return 1; }
  check test "$(diffpane updating)" == False || { echo "updating shown while reading"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-changed.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-changed.png" "$out/" 2>/dev/null
  kanna focus changes >/dev/null
  raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+r"}}' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane rows)" == *"{ 2000 }"* && "$(diffpane stale)" == False ]]' || { echo "⌘R didn't bring the edit in"; return 1; }
  check eval '[[ "$(diffpane fresh)" != "[]" ]]' || { echo "nothing marked fresh"; return 1; }
  check python3 -c 'import json, sys
rows, current = json.loads(sys.argv[1]), int(sys.argv[2])
assert rows[current] == json.loads(sys.argv[3]), (rows[current], sys.argv[3])' "$(diffpane rows)" "$(diffpane current)" "$row" || { echo "the refresh lost the place"; return 1; }
  touch "$repo/a.rs"
  sleep 0.6
  check test "$(diffpane stale)" == False || { echo "changed after a re-diff with nothing new"; return 1; }
  diffpane view ',"keys":"g"' >/dev/null
  raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+f"}}' >/dev/null
  local typed; typed=$(raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"2000"}}')
  check eval '[[ "$typed" == *NSFindPatternFieldEditor* ]]' || { echo "⌘F showed no find field: $typed"; return 1; }
  raw '{"id":1,"cmd":"debug.key","args":{"key":"return"}}' >/dev/null
  raw '{"id":1,"cmd":"debug.key","args":{"key":"escape"}}' >/dev/null
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-unified.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-unified.png" "$out/diff_pane-unified.png" 2>/dev/null
  diffpane current ',"keys":"gn"' >/dev/null
  check python3 -c 'import json, sys
rows, current = json.loads(sys.argv[1]), int(sys.argv[2])
assert rows[current]["kind"] in ("added", "removed") and rows[current]["text"].startswith("fn f5"), rows[current]' "$(diffpane rows)" "$(diffpane current)" || return 1
  diffpane current ',"keys":"]"' >/dev/null
  check test "$(diffpane selected)" == b.rs || { echo "selected $(diffpane selected)"; return 1; }
  diffwin() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"win-1\",\"size\":[$1,700]}}" >/dev/null; }
  diffwin 1400
  check wait_for 5 eval '[[ "$(diffpane list)" == True ]]' || { echo "no files list at $(diffpane width) pt"; return 1; }
  dov() { diffpane "overview.$1" "${2:-}"; }
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[1400,320]}}' >/dev/null
  check wait_for 5 eval '[[ "$(dov shown)" == True && "$(diffpane list)" == True ]]' || { echo "short window: overview $(dov shown) wanted $(dov wanted), list $(diffpane list)"; return 1; }
  check eval '(( $(dov box_height | cut -d. -f1) > 5 ))' || { echo "box $(dov box_height)"; return 1; }
  check python3 -c 'import json, sys
marks = json.loads(sys.argv[1])
assert marks.get("changed", 0) >= 2 and marks.get("added", 0) >= 1, marks' "$(dov marks_by_kind)" || return 1
  dov top ',"overview_click":200' >/dev/null
  check wait_for 5 eval '(( $(dov top | cut -d. -f1) > 0 ))' || { echo "a click in the diff overview left top at $(dov top)"; return 1; }
  dov top ',"overview_drag":[1,-300]' >/dev/null
  check wait_for 5 eval '[[ "$(dov top)" == 0* ]]' || { echo "dragging up left top at $(dov top)"; return 1; }
  dov shown ',"overview_hover":true' >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-overview-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-overview-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-overview-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-overview-dark.png" "$out/" 2>/dev/null
  dov shown ',"overview_hover":false' >/dev/null
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[1000,320]}}' >/dev/null
  check wait_for 5 eval '[[ "$(dov shown)" == True && "$(diffpane list)" == False ]]' || { echo "at 1000 pt the list should go first: overview $(dov shown), list $(diffpane list)"; return 1; }
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[640,320]}}' >/dev/null
  check wait_for 5 eval '[[ "$(dov shown)" == False && "$(diffpane list)" == False ]]' || { echo "at 640 pt: overview $(dov shown), list $(diffpane list)"; return 1; }
  diffwin 1400
  check wait_for 5 eval '[[ "$(diffpane list)" == True ]]' || { echo "no files list back at $(diffpane width) pt"; return 1; }
  diffpane view ',"zoom":6' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane list)" == False ]]' || { echo "files list zoomed in at $(diffpane width) pt"; return 1; }
  diffpane view ',"zoom":0' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane list)" == True ]]' || { echo "no files list back from zoom"; return 1; }
  diffwin 960
  check wait_for 5 eval '[[ "$(diffpane list)" == False ]]' || { echo "files list at $(diffpane width) pt"; return 1; }
  diffpane current ',"keys":"["' >/dev/null
  check test "$(diffpane selected)" == a.rs || { echo "without the list, [ selected $(diffpane selected)"; return 1; }
  diffpane current ',"keys":"]"' >/dev/null
  check test "$(diffpane selected)" == b.rs || { echo "without the list, ] selected $(diffpane selected)"; return 1; }
  diffwin 1400
  check python3 -c 'import json, sys
rows, current = json.loads(sys.argv[1]), int(sys.argv[2])
assert rows[current]["text"] == "fn new() {}", rows[current]' "$(diffpane rows)" "$(diffpane current)" || return 1
  diffpane current ',"keys":"~"' >/dev/null
  check wait_for 5 eval 'kanna workspace list --all --json | grep -q "diff_repo/b.rs"' || { echo "no code pane for b.rs"; kanna workspace list --all; return 1; }
  kanna focus changes >/dev/null
  diffpane view ',"keys":"s"' >/dev/null
  check wait_for 5 is changes view split || { echo "view $(pane changes view)"; return 1; }
  check eval '[[ "$(diffpane rows)" == *"\"right\": \"blank\""* || "$(diffpane rows)" == *"\"left\": \"blank\""* ]]' || { echo "no blank side in split"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane.png" "$out/diff_pane.png" 2>/dev/null
  diffpane view ',"fold":"a.rs"' >/dev/null
  check python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
assert not any(r["text"].startswith("fn f5") for r in rows), "a.rs still open"' "$(diffpane rows)" || return 1
  diffpane view ',"fold":"a.rs"' >/dev/null
  diffpane view ',"zoom":12' >/dev/null
  diffpane view ',"keys":"gG"' >/dev/null
  check wait_for 5 eval '[[ "$(diffpane sticky)" != None ]]' || { echo "no sticky header at the bottom: current $(diffpane current) of $(diffpane rows | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')"; raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-sticky.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-sticky.png" "$out/" 2>/dev/null; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane-sticky.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane-sticky.png" "$out/" 2>/dev/null
  check python3 -c 'import json, sys
for c in json.loads(sys.argv[1]): assert c["x"] == -c["inset_left"] and c["inset_left"] == c["gutter"], c' "$(diffpane columns)" || return 1
  diffpane view ',"zoom":0' >/dev/null
  printf 'one\r\ntwo\xe2\x80\xa8three\r\n' >"$repo/crlf.txt"
  refreshed || return 1
  check wait_for 5 eval '[[ "$(diffpane files)" == *crlf.txt* ]]' || { echo "crlf.txt didn't arrive"; return 1; }
  check python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
assert {"kind": "added", "text": "one\r", "new": 1, "left": "blank", "right": "added"} in rows, [r for r in rows if r.get("new") == 1]' "$(diffpane rows)" || return 1
  printf 'fn later() {}\n' >"$repo/c.rs"
  refreshed || return 1
  check wait_for 5 eval '[[ "$(diffpane files)" == *c.rs* ]]' || { echo "the untracked c.rs didn't arrive"; return 1; }
  watchers() { raw '{"id":1,"cmd":"diffs"}' | python3 -c 'import json, sys; print(" ".join(json.dumps(d["args"]) + ":" + str(len(d["views"])) for d in json.load(sys.stdin)["diffs"]))'; }
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "watchers: $(watchers)"; return 1; }
  (cd "$repo" && kanna open --window win-1 diff --merge-base main --name changes2 >/dev/null) || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:2" ]]' || { echo "two views, watchers: $(watchers)"; return 1; }
  kanna close changes2 >/dev/null || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "after closing one view, watchers: $(watchers)"; return 1; }
  stop_daemon; start_daemon || return 1
  check wait_for 10 app_attached || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "after a daemon restart, watchers: $(watchers)"; return 1; }
  printf 'fn restarted() {}\n' >"$repo/d.rs"
  refreshed || return 1
  check wait_for 5 eval '[[ "$(diffpane files)" == *d.rs* ]]' || { echo "d.rs didn't arrive after a daemon restart: $(diffpane files)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(diffpane view)" == split && "$(diffpane files)" == *c.rs* ]]' || { echo "after relaunch: $(diffpane view) $(diffpane files) $(diffpane error) $(watchers)"; return 1; }
  before="$(diffpane files)"
  mv "$repo/.git" "$repo/.git-away"
  check wait_for 5 eval '[[ "$(diffpane notice)" == *"not a git repository"* ]]' || { echo "git's error while reading: notice $(diffpane notice), error $(diffpane error)"; mv "$repo/.git-away" "$repo/.git"; return 1; }
  check test "$(diffpane files)" == "$before" || { echo "git's error replaced the diff being read"; mv "$repo/.git-away" "$repo/.git"; return 1; }
  check test "$(diffpane error)" == None || { echo "error $(diffpane error)"; mv "$repo/.git-away" "$repo/.git"; return 1; }
  mv "$repo/.git-away" "$repo/.git"
  check wait_for 5 eval '[[ "$(diffpane notice)" == None && "$(diffpane stale)" == False ]]' || { echo "git back: notice $(diffpane notice), stale $(diffpane stale)"; return 1; }
  kanna navigate changes diff --staged >/dev/null || return 1
  check wait_for 5 eval '[[ "$(diffpane command)" == "git diff --staged" && "$(diffpane files)" == "[]" ]]' || { echo "staged: $(diffpane command) $(diffpane files)"; return 1; }
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--staged\"]:1" ]]' || { echo "after navigating, watchers: $(watchers)"; return 1; }
  kanna navigate changes diff --no-such-flag >/dev/null || return 1
  check wait_for 5 eval '[[ "$(diffpane error)" == *no-such-flag* ]]' || { echo "a failed diff shows no error: $(diffpane error)"; return 1; }
  desk_task diffagent 'exec sleep 600' || return 1
  (cd "$repo" && KANNA_TERMINAL=diffagent kanna open diff --merge-base main --name deskdiff >/dev/null) || return 1
  deskdiff() { raw '{"id":1,"cmd":"debug.diff","args":{"pane":"deskdiff"}}' | field "$1"; }
  kanna task activate diffagent >/dev/null || return 1
  local agent; agent="$(pane diffagent id)"
  deskwin() { app_view | python3 -c 'import json, sys; print(next((w["id"] for w in json.load(sys.stdin)["windows"] if (w.get("dock") or {}).get("pane") == sys.argv[1]), ""))' "$agent"; }
  check wait_for 5 eval '[[ -n "$(deskwin)" ]]' || { echo "the agent's Desk isn't shown: $(app_view)"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$(deskwin)\",\"size\":[1728,1000]}}" >/dev/null
  check wait_for 5 eval '[[ "$(deskdiff width)" == 1036* && "$(deskdiff files)" == *b.rs* ]]' || { echo "beside the agent at 2/5 in 1728 pt: $(deskdiff width) pt"; return 1; }
  check test "$(deskdiff list)" == False || { echo "files list beside the agent at $(deskdiff width) pt"; return 1; }
  kanna resize diffagent 1/5 >/dev/null || return 1
  check wait_for 5 eval '[[ "$(deskdiff list)" == True ]]' || { echo "no files list beside the agent at 1/5: $(deskdiff width) pt"; return 1; }
}

scenario diff_pane_worktree_gone M4
diff_pane_worktree_gone() {
  start_daemon && start_app || return 1
  local repo="$KANNA_STATE_DIR/gone_repo" tree="$KANNA_STATE_DIR/gone_tree"
  mkdir -p "$repo" && cd "$repo" || return 1
  dgit() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null; }
  dgit init -q -b main
  for i in $(seq 200); do echo "fn f$i() -> u32 { $i }"; done >a.rs
  dgit add . && dgit commit -qm first && dgit worktree add -q -b task "$tree"
  cd - >/dev/null
  for i in $(seq 200); do echo "fn f$i() -> u64 { $i }"; done >"$tree/a.rs"
  gone() { raw "{\"id\":1,\"cmd\":\"debug.diff\",\"args\":{\"pane\":\"gone\"${2:-}}}" | field "$1"; }
  (cd "$tree" && kanna open --window new diff --merge-base main --name gone >/dev/null) || return 1
  check wait_for 5 eval '[[ "$(gone files)" == *a.rs* ]]' || { echo "files $(gone files) error $(gone error)"; return 1; }
  gone view ',"zoom":-40' >/dev/null
  check wait_for 5 eval '[[ "$(gone zoom)" == 0.72 ]]' || { echo "zoomed out to $(gone zoom)"; return 1; }
  gone view ',"keys":"G"' >/dev/null
  check wait_for 5 eval '[[ "$(gone sticky)" == a.rs ]]' || { echo "no sticky header zoomed out: $(gone sticky)"; return 1; }
  git -C "$repo" worktree remove --force "$tree" || return 1
  check wait_for 5 eval '[[ "$(gone notice)" != None || "$(gone error)" != None ]]' || { echo "the removed worktree went unnoticed: notice $(gone notice), error $(gone error)"; return 1; }
  for keys in g G g G; do gone view ",\"keys\":\"$keys\"" >/dev/null; done
  gone view ',"zoom":-1' >/dev/null
  check app_attached || { echo "the app died after its worktree went"; return 1; }
  check test "$(gone sticky)" == a.rs || { echo "sticky $(gone sticky) after the worktree went"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"diff_pane_worktree_gone.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/diff_pane_worktree_gone.png" "$out/" 2>/dev/null
}

scenario log_pane M4
log_pane() {
  start_daemon && start_app || return 1
  local repo; repo="$(e2e_repo logs)" || return 1
  lgit() { git -C "$repo" -c user.name=Ann -c user.email=ann@example.invalid -c commit.gpgsign=false "$@" >/dev/null; }
  lc() { echo "$2" >>"$repo/$1" && lgit add "$1" && lgit commit -qm "$2"; }
  lc a.txt "main 0"
  lgit checkout -qb long; for i in 1 2 3 4 5; do lc long.txt "long $i"; done
  lgit checkout -q main; lc a.txt "main 1"; lgit merge -q --no-ff long -m "Merge long"
  for b in x y z; do lgit checkout -qb "$b" main~1; lc "$b.txt" "$b 1"; done
  lgit checkout -q main; lgit merge -q --no-ff x y z -m Octopus
  lgit checkout -qb lived; lc l.txt "lived 1"
  lgit checkout -q main; lc a.txt "main 2"
  lgit checkout -q lived; lgit merge -q --no-ff main -m "Merge branch 'main' into lived"; lc l.txt "lived 2"
  lgit checkout -q main
  for i in 3 4 5 6 7 8 9; do lc a.txt "main $i"; done
  lgit checkout -q lived; lgit merge -q --no-ff main -m "Merge main into lived again"; lgit checkout -q main
  desk_task "Labelled task" 'exec sleep 600' logs || return 1
  local tree; tree="$(ls -d "$repo"/.kanna-worktrees/task-* | head -1)"
  check wait_for 10 test -d "$tree" || { echo "no worktree for the task"; return 1; }
  git -C "$tree" branch -m labelled >/dev/null 2>&1
  git -C "$tree" -c user.name=Bo -c user.email=bo@example.invalid -c commit.gpgsign=false commit -q --allow-empty -m "task work" || return 1
  logpane() { raw "{\"id\":1,\"cmd\":\"debug.log\",\"args\":{\"pane\":\"history\"${2:-}}}" | field "$1"; }
  rowsof() { logpane rows | python3 -c "import json, sys; rows = json.load(sys.stdin); $1"; }
  hash_of() { git -C "$repo" log --all --format='%H %s' | awk -v s="$1" '{h=$1; $1=""; if (substr($0, 2) == s) { print h; exit }}'; }
  (cd "$repo" && kanna open --window new log --name history >/dev/null) || return 1
  check wait_for 10 eval '[[ "$(logpane total)" == 22 ]]' || { echo "total $(logpane total), error $(logpane error), range $(logpane range)"; return 1; }
  check test "$(logpane range)" == '["labelled", "main"]' || { echo "range $(logpane range)"; return 1; }
  check rowsof '
by = {r["subject"]: r for r in rows}
assert "long 1" not in by, "the long branch should fold"
m = by["Merge long"]; assert m["folded"] and m["fold"] == 5 and m["lane"] == 0, m
assert all(r["lane"] == 0 for r in rows if r["spine"]), [r for r in rows if r["spine"] and r["lane"]]
o = by["Octopus"]; assert len(o["parents"]) == 4 and len(set(o["parents"])) == 4, o
t = by["task work"]; assert t["lane"] > 0 and t["labels"][0]["name"] == "Labelled task" and t["labels"][0]["place"] == "desk" and t["labels"][0]["ahead"] == 1, t
assert "lived 1" not in by, "lived is not in the default range"' || return 1
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"log_pane.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/log_pane.png" "$out/" 2>/dev/null
  kanna navigate history log main lived >/dev/null || return 1
  check wait_for 10 eval '[[ "$(logpane command)" == "git log main lived" && "$(logpane total)" == 25 ]]' || { echo "navigate: $(logpane command) $(logpane total)"; return 1; }
  check rowsof '
by = {r["subject"]: r for r in rows}
assert by["lived 1"]["lane"] > 0 and len(by["Merge branch '"'"'main'"'"' into lived"]["parents"]) == 2, by["lived 1"]' || return 1
  local merge; merge="$(hash_of "Merge long")"
  logpane open ",\"fold\":\"$merge\"" >/dev/null
  check rowsof '
by = {r["subject"]: r for r in rows}
assert len(rows) == 25 and by["Merge long"]["open"] and by["long 3"]["lane"] > 0, len(rows)' || return 1
  logpane open ",\"select\":\"$(hash_of "long 3")\",\"keys\":\"h\"" >/dev/null
  check test "$(logpane selected)" == "$merge" || { echo "← from inside the side selected $(logpane selected)"; return 1; }
  check rowsof 'assert len(rows) == 20, len(rows)' || return 1
  logpane selected ',"keys":"g"' >/dev/null
  local top; top="$(logpane selected)"
  check wait_for 5 eval '[[ "$(logpane detail)" == "$top" ]]' || { echo "detail $(logpane detail) for $top"; return 1; }
  logpane selected ',"keys":"jj"' >/dev/null
  local reading; reading="$(logpane selected)"
  local before; before="$(logpane rows)"
  lgit checkout -q lived; lc l.txt "lived 3"; lgit checkout -q main
  check wait_for 5 eval '[[ "$(logpane stale)" == True ]]' || { echo "the commit didn't hold: stale $(logpane stale)"; return 1; }
  check test "$(logpane held)" == "1 new commit" || { echo "held $(logpane held)"; return 1; }
  check test "$(logpane rows)" == "$before" || { echo "the rows moved while reading"; return 1; }
  check test "$(logpane selected)" == "$reading" || { echo "the selection moved while reading"; return 1; }
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"log_pane-held.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/log_pane-held.png" "$out/" 2>/dev/null
  kanna focus history >/dev/null
  raw '{"id":1,"cmd":"debug.key","args":{"key":"cmd+r"}}' >/dev/null
  check wait_for 5 eval '[[ "$(logpane stale)" == False && "$(logpane rows)" == *"lived 3"* ]]' || { echo "⌘R didn't bring the commit in"; return 1; }
  check test "$(logpane selected)" == "$reading" || { echo "the refresh lost the selection"; return 1; }
  check eval '[[ "$(logpane fresh)" == *"$(hash_of "lived 3")"* ]]' || { echo "fresh $(logpane fresh)"; return 1; }
  logpane hits ',"find":"lived"' >/dev/null
  check eval '[[ "$(logpane hits | python3 -c "import json,sys; print(len(json.load(sys.stdin)))")" == 5 ]]' || { echo "hits $(logpane hits)"; return 1; }
  logpane hits ',"find":"long 2"' >/dev/null
  logpane hits ',"next":true' >/dev/null
  check test "$(logpane selected)" == "$(hash_of "long 2")" || { echo "find didn't go to long 2: $(logpane selected)"; return 1; }
  check eval '[[ "$(logpane open)" == *"$merge"* ]]' || { echo "find didn't unfold its merge: $(logpane open)"; return 1; }
  logpane hits ',"find":""' >/dev/null
  logpane selected ",\"select\":\"$(hash_of "lived 2")\",\"keys\":\"~\"" >/dev/null
  check wait_for 5 eval 'kanna workspace list --all --json | grep -q "$(hash_of "lived 2" | cut -c1-12)^!"' || { echo "↩ opened no diff of lived 2"; kanna workspace list --all; return 1; }
  kanna focus history >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"log_pane-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/log_pane-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"log_pane-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/log_pane-dark.png" "$out/" 2>/dev/null
  logs() { raw '{"id":1,"cmd":"logs"}' | python3 -c 'import json, sys; print(" ".join(json.dumps(d["args"]) + ":" + str(len(d["views"])) for d in json.load(sys.stdin)["logs"]))'; }
  check wait_for 5 eval '[[ "$(logs)" == "[\"main\", \"lived\"]:1" ]]' || { echo "logs: $(logs)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 10 eval '[[ "$(logpane total)" == 26 && "$(logpane command)" == "git log main lived" ]]' || { echo "after relaunch: $(logpane total) $(logpane command) $(logpane error)"; return 1; }
  kanna navigate history log no-such-branch >/dev/null || return 1
  check wait_for 5 eval '[[ "$(logpane error)" == *no-such-branch* ]]' || { echo "a bad range shows no error: $(logpane error)"; return 1; }
}

scenario code_tour M4
code_tour() {
  start_daemon && start_app || return 1
  local tour="$KANNA_STATE_DIR/code_tour.rs" other="$KANNA_STATE_DIR/code_tour-other.rs"
  { echo 'fn outer() {'; echo '    loop {'; echo '        if ready() {'; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; } >"$tour"
  for i in $(seq 30); do echo "let line_$i = $i;"; done >"$other"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | field "$2"; }
  local said id
  said="$(kanna comment --window new "$tour:3" "Waits until it's ready.")" || return 1
  [[ "$said" == "opened file-"*" · code_tour.rs:3–61" ]] || { echo "said: $said"; return 1; }
  id="$(awk '{print $2}' <<<"$said")"
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 3 && "$(code $id comment_last)" == 61 ]]' || return 1
  check wait_for 5 eval '[[ "$(code $id comment_text)" == "Waits until it'"'"'s ready." ]]' || return 1
  check eval '(( $(code $id comment_gap) > 20 ))' || { echo "gap $(code $id comment_gap)"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_box_visible)" == 1 ]]' || { echo "box at $(code $id comment_box_visible)"; return 1; }
  said="$(kanna comment --window win-1 "$tour:40-42" "Each step.")" || return 1
  [[ "$said" == "$id · code_tour.rs:40–42" ]] || { echo "again: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 40 && "$(code $id comment_last)" == 42 ]]' || return 1
  check wait_for 5 eval '[[ "$(code $id comment_box_visible)" == 1 ]]' || return 1
  said="$(kanna comment --window win-1 "$other:7" "Line seven.")" || return 1
  [[ "$said" == "opened file-"*" · code_tour-other.rs:7" && "$said" != *" $id "* ]] || { echo "other: $said"; return 1; }
  [[ "$(kanna comment --window win-1 "$KANNA_STATE_DIR/gone.rs:3" "Nothing." 2>&1)" == *"No such file: $KANNA_STATE_DIR/gone.rs"* ]] || { echo "code_tour: a missing file"; return 1; }
  [[ "$(kanna comment --window win-1 "$other:99" "Past the end." 2>&1)" == *"has 30 lines"* ]] || { echo "code_tour: past the end"; return 1; }
  local doc="$KANNA_STATE_DIR/code_tour.md" agent md
  printf '# Tabs\n\nA tab container holds **several** panes\nand shows one at a time.\n\n- Panes only: it holds panes.\n- One active: the rest keep running.\n' >"$doc"
  agent="$(kanna open --window win-1 --name code_touragent --cmd cat | awk '{print $2}')"
  said="$(KANNA_TERMINAL=$agent kanna comment "$doc:4" "The key idea: one pane shows at a time.")" || return 1
  [[ "$said" == "c1 · code_tour.md:3 in file-"* ]] || { echo "doc: $said"; return 1; }
  md="${said##* in }"
  mdf() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$md\"}}" | field "$1"; }
  check wait_for 5 eval '[[ "$(mdf threads.0.messages.0)" == "agent: The key idea: one pane shows at a time." ]]' || return 1
  check wait_for 5 eval '[[ "$(mdf markup)" == True && "$(mdf agent)" == "$agent" ]]' || { echo "markup $(mdf markup) agent $(mdf agent)"; return 1; }
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$md\",\"mark\":\"c1\"}}" | grep -q "\"y\""' || { echo "c1 has no mark"; return 1; }
  said="$(KANNA_TERMINAL=$agent kanna comment "$doc:7" "The others keep running.")" || return 1
  [[ "$said" == "c2 · code_tour.md:7 in $md" ]] || { echo "doc again: $said"; return 1; }
  check wait_for 5 eval '[[ "$(mdf active)" == c2 ]]' || { echo "active $(mdf active)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 40 && "$(code $id comment_text)" == "Each step." ]]' || return 1
}

scenario code_tour_steps M4
code_tour_steps() {
  start_daemon && start_app || return 1
  local tour="$KANNA_STATE_DIR/code_tour_steps.rs" other="$KANNA_STATE_DIR/code_tour_steps-other.rs" said id more agent
  { echo 'fn outer() {'; echo '    loop {'; echo '        if ready() {'; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; } >"$tour"
  for i in $(seq 30); do echo "let line_$i = $i;"; done >"$other"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"${3:-}}}" | field "$2"; }
  press() { code "$1" pressed ",\"tour\":\"$2\""; }
  agent="$(kanna open --window new --name agent --cmd cat | awk '{print $2}')"
  [[ "$(kanna tour "$tour:3" "One." 2>&1)" == *"run it from the agent's terminal"* ]] || { echo "code_tour_steps: a tour without an agent"; return 1; }
  [[ "$(KANNA_TERMINAL=$agent kanna tour "$tour:3" "One." "$other:99" "Past." 2>&1)" == *"has 30 lines"* ]] || { echo "code_tour_steps: a bad stop"; return 1; }
  [[ "$(KANNA_TERMINAL=$agent kanna tour)" == "no tour" ]] || { echo "code_tour_steps: a refused tour was kept"; return 1; }
  said="$(KANNA_TERMINAL=$agent kanna tour "$tour:3" "One." "$tour:40-42" "Two." "$other:7" "Three.")" || return 1
  [[ "$said" == "tour of 3 · 1 of 3 · opened file-"*" · code_tour_steps.rs:3–61" ]] || { echo "said: $said"; return 1; }
  id="$(grep -o 'file-[0-9]*' <<<"$said")"
  check wait_for 5 eval '[[ "$(code $id tour.at)" == 1 && "$(code $id tour.of)" == 3 && "$(code $id tour.back)" == False && "$(code $id tour.next)" == "Next ›" ]]' || { echo "footer $(code $id tour)"; return 1; }
  check wait_for 5 eval '[[ "$(code $id tour.visible)" == True ]]' || return 1
  [[ "$(press $id back)" == False ]] || { echo "code_tour_steps: Back on the first stop"; return 1; }
  [[ "$(press $id next)" == True ]] || return 1
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 40 && "$(code $id tour.at)" == 2 && "$(code $id tour.back)" == True ]]' || { echo "stop 2: $(code $id tour)"; return 1; }
  sleep 0.5
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"code_tour_steps.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/code_tour_steps.png" "$out/code_tour_steps.png" 2>/dev/null
  press $id next >/dev/null
  check wait_for 5 eval '[[ "$(KANNA_TERMINAL=$agent kanna tour | head -1)" == "at 3 of 3 · "*"code_tour_steps-other.rs:7" ]]' || { KANNA_TERMINAL=$agent kanna tour; return 1; }
  more="$(kanna workspace list --all --json | python3 -c 'import json, sys
print(next(p["id"] for p in json.load(sys.stdin)["panes"] if (p.get("path") or "").endswith("-other.rs")))')"
  check wait_for 5 eval '[[ "$(code $more tour.at)" == 3 && "$(code $more tour.next)" == Done ]]' || { echo "stop 3: $(code $more tour)"; return 1; }
  check wait_for 5 eval '[[ "$(code $id tour)" == None && "$(code $id comment_first)" == 40 ]]' || { echo "the last stop kept its footer: $(code $id tour)"; return 1; }
  said="$(KANNA_TERMINAL=$agent kanna tour add --next "$tour:20-22" "An answer.")" || return 1
  [[ "$said" == "added 1 · 4 of 4 · $id · code_tour_steps.rs:20–22" ]] || { echo "added: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 20 && "$(code $id tour.at)" == 4 && "$(code $id tour.next)" == Done ]]' || return 1
  check wait_for 5 eval '[[ "$(code $more tour)" == None ]]' || return 1
  said="$(KANNA_TERMINAL=$agent kanna tour add "$other:12" "Later.")" || return 1
  [[ "$said" == "added 1 · at 4 of 5" ]] || { echo "added later: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id tour.of)" == 5 && "$(code $id tour.next)" == "Next ›" ]]' || return 1
  press $id back >/dev/null
  check wait_for 5 eval '[[ "$(code $more tour.at)" == 3 && "$(code $more tour.of)" == 5 ]]' || return 1
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(code $more tour.at)" == 3 && "$(code $more comment_first)" == 7 ]]' || { echo "after a relaunch: $(code $more tour)"; return 1; }
  press $more next >/dev/null
  check wait_for 5 eval '[[ "$(code $id tour.at)" == 4 ]]' || return 1
  press $id next >/dev/null
  check wait_for 5 eval '[[ "$(code $more tour.at)" == 5 && "$(code $more comment_first)" == 12 ]]' || return 1
  press $more next >/dev/null
  check wait_for 5 eval '[[ "$(KANNA_TERMINAL=$agent kanna tour)" == "no tour" && "$(code $more tour)" == None && "$(code $more comment_first)" == 12 ]]' || { echo "done: $(code $more tour)"; return 1; }
  local doc="$KANNA_STATE_DIR/code_tour_steps.md" md
  printf '# Trip\n\nThe keeper queues\nthe message.\n\n- It sends.\n' >"$doc"
  mdf() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$md\"${2:-}}}" | field "$1"; }
  said="$(KANNA_TERMINAL=$agent kanna tour "$doc:4" "The whole trip." "$tour:3" "Then the code.")" || return 1
  [[ "$said" == "tour of 2 · 1 of 2 · opened file-"*" · code_tour_steps.md:3 · c1" ]] || { echo "doc: $said"; return 1; }
  md="$(grep -o 'file-[0-9]*' <<<"$said")"
  check wait_for 5 eval '[[ "$(mdf tour.thread)" == c1 && "$(mdf tour.at)" == 1 && "$(mdf tour.next)" == "Next ›" && "$(mdf tour.back)" == False ]]' || { echo "doc footer: $(mdf tour)"; return 1; }
  check wait_for 5 eval '[[ "$(mdf active)" == c1 && "$(mdf threads.0.messages.0)" == "agent: The whole trip." && "$(mdf markup)" == True ]]' || { echo "doc thread: $(mdf active) $(mdf threads)"; return 1; }
  sleep 0.5
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"code_tour_steps-md.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/code_tour_steps-md.png" "$out/code_tour_steps-md.png" 2>/dev/null
  [[ "$(mdf pressed ',"tour":"next"')" == True ]] || return 1
  check wait_for 5 eval '[[ "$(code $id tour.at)" == 2 && "$(code $id tour.next)" == Done && "$(code $id comment_first)" == 3 ]]' || { echo "after the doc: $(code $id tour)"; return 1; }
  check wait_for 5 eval '[[ "$(mdf tour)" == None ]]' || { echo "the doc kept its footer: $(mdf tour)"; return 1; }
  press $id back >/dev/null
  check wait_for 5 eval '[[ "$(mdf tour.thread)" == c1 && "$(mdf tour.at)" == 1 && "$(mdf active)" == c1 ]]' || { echo "back to the doc: $(mdf tour)"; return 1; }
  [[ "$(mdf threads | python3 -c 'import json, sys; print(len(json.load(sys.stdin)))')" == 1 ]] || { echo "code_tour_steps: going back wrote another thread"; return 1; }
  mdf pressed ',"tour":"next"' >/dev/null
  check wait_for 5 eval '[[ "$(code $id tour.at)" == 2 ]]' || return 1
  press $id next >/dev/null
  check wait_for 5 eval '[[ "$(KANNA_TERMINAL=$agent kanna tour)" == "no tour" && "$(code $id tour)" == None ]]' || return 1
  [[ -z "$(kanna read "$agent" | tr -d '[:space:]')" ]] || { echo "code_tour_steps: stepping sent the agent: $(kanna read "$agent")"; return 1; }
}

scenario code_comment_box M4
code_comment_box() {
  start_daemon && start_app || return 1
  local link="$KANNA_STATE_DIR/app_link.rs" tasks="$KANNA_STATE_DIR/tasks.rs" said id other
  cp "$repo_root/crates/kanna-daemon/src/app_link.rs" "$link" && cp "$repo_root/crates/kanna-daemon/src/tasks.rs" "$tasks" || return 1
  local long="The handshake: the app says hello with its version and instance, the daemon checks both against its own, and only then does it hand the link to the router; anything else closes the socket so a stale app from another build can never drive this daemon."
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"${3:-}}}" | field "$2"; }
  fits() {
    raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | python3 -c 'import json, sys
d = json.load(sys.stdin)
ok = d["comment_gap"] > 0 and d["comment_gap_top"] <= d["comment_box_top"] and d["comment_box_bottom"] <= d["comment_line_top"] and d["comment_line_top"] - d["comment_gap_top"] == d["comment_gap"]
if not ok: print("box %s–%s, gap %s–%s" % (d["comment_box_top"], d["comment_box_bottom"], d["comment_gap_top"], d["comment_line_top"]), file=sys.stderr)
sys.exit(0 if ok else 1)'
  }
  code_comment_box_shot() { raw "{\"id\":1,\"cmd\":\"debug.snapshot\",\"args\":{\"path\":\"$1.png\"}}" >/dev/null; cp "$KANNA_STATE_DIR/$1.png" "$out/$1.png" 2>/dev/null; }
  id="$(kanna open --window new code "$link" | awk '{print $2}')"
  check wait_for 5 eval '(( $(code $id scopes) > 0 ))' || return 1
  said="$(kanna comment --window win-1 "$link:196-205" "$long")" || return 1
  [[ "$said" == "$id · app_link.rs:196–205" ]] || { echo "said: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 196 && "$(code $id scopes)" -gt 0 ]]' || return 1
  sleep 0.5
  check wait_for 5 eval 'fits $id' || { fits $id; return 1; }
  check wait_for 5 eval '(( $(code $id pinned) > 0 ))' || { echo "pinned $(code $id pinned)"; return 1; }
  code_comment_box_shot code_comment_box-open
  raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$id\",\"reveal\":400}}" >/dev/null
  raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$id\",\"pick\":198}}" >/dev/null
  check wait_for 5 eval 'fits $id' || { fits $id; return 1; }
  local blank=$(( $(grep -n '^fn gone() -> KmuxError' "$tasks" | cut -d: -f1) - 1 ))
  said="$(kanna comment --window win-1 "$tasks:$blank" "$long")" || return 1
  [[ "$said" == "opened file-"*" · tasks.rs:$blank" ]] || { echo "said: $said"; return 1; }
  other="$(awk '{print $2}' <<<"$said")"
  check wait_for 5 eval '[[ "$(code $other comment_first)" == $blank && "$(code $other scopes)" -gt 0 ]]' || return 1
  sleep 0.5
  check wait_for 5 eval 'fits $other' || { fits $other; return 1; }
  check wait_for 5 eval '[[ "$(code $other comment_box_visible)" == 1 ]]' || return 1
  code_comment_box_shot code_comment_box-new
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  sleep 0.5
  check wait_for 5 eval 'fits $other' || { fits $other; return 1; }
  code_comment_box_shot code_comment_box-dark
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

png_size() { python3 -c 'import struct, sys
d = open(sys.argv[1], "rb").read(24)
assert d[:8] == b"\x89PNG\r\n\x1a\n"
print("%dx%d" % struct.unpack(">II", d[16:24]))' "$1"; }
png_ink() { python3 -c 'import struct, sys, zlib, collections
d = open(sys.argv[1], "rb").read(); i = 8; idat = b""
while i < len(d):
    n, kind = struct.unpack(">I4s", d[i:i + 8]); body = d[i + 8:i + 8 + n]; i += 12 + n
    if kind == b"IHDR": w, h, depth, color = struct.unpack(">IIBB", body[:10])
    if kind == b"IDAT": idat += body
assert depth == 8 and color in (2, 6)
bpp = 3 if color == 2 else 4; raw = zlib.decompress(idat); stride = w * bpp; prev = bytearray(stride); rows = []
for y in range(h):
    f = raw[y * (stride + 1)]; line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
    for x in range(stride):
        a = line[x - bpp] if x >= bpp else 0; b = prev[x]; c = prev[x - bpp] if x >= bpp else 0
        if f == 1: line[x] = (line[x] + a) & 255
        elif f == 2: line[x] = (line[x] + b) & 255
        elif f == 3: line[x] = (line[x] + (a + b) // 2) & 255
        elif f == 4:
            p = a + b - c; pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
    rows.append(line); prev = line
px = [tuple(r[x:x + 3]) for r in rows[::2] for x in range(0, stride, bpp * 2)]
bg = collections.Counter(px).most_common(1)[0][0]
print(round(sum(1 for p in px if sum(abs(p[k] - bg[k]) for k in range(3)) > 60) / len(px), 4))' "$1"; }

scenario screenshot M4
screenshot() {
  start_daemon || return 1
  env -u KANNA_DEBUG -u KANNA_IGNORE_OCCLUSION "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  check wait_for 10 eval '[[ "$(kanna capabilities)" == *"Kanna.app: attached"* ]]' || return 1
  local shots="$KANNA_STATE_DIR/shots"; mkdir -p "$shots" && shots="$(cd "$shots" && pwd -P)"
  kanna open --window new --name screenshot --cmd 'for i in $(seq 80); do echo "screenshot $i the quick brown fox jumps over the lazy dog"; done; exec cat' >/dev/null || return 1
  check wait_for 5 shows screenshot "screenshot 80" || return 1
  printf '# screenshot\n\nA markdown pane.\n' >"$KANNA_STATE_DIR/screenshot.md"
  kanna open --window win-1 md "$KANNA_STATE_DIR/screenshot.md" --name doc --split right >/dev/null || return 1
  check wait_for 5 is doc state running || return 1
  raw '{"id":1,"cmd":"debug.app"}' | grep -q '"unknown command' || { echo "screenshot: the app answers debug commands"; return 1; }
  local window pane doc
  window="$(cd "$shots" && kanna screenshot --window win-1 -o win-1.png)" || { echo "screenshot line $LINENO"; return 1; }
  check test "$window" == "$shots/win-1.png" || return 1
  pane="$(cd "$shots" && kanna screenshot screenshot --out screenshot.png)" || { echo "screenshot line $LINENO"; return 1; }
  doc="$(kanna screenshot doc --json)" || { echo "screenshot line $LINENO"; return 1; }
  cp "$shots"/*.png "$out/" 2>/dev/null
  local ww wh pw ph; IFS=x read -r ww wh <<<"$(png_size "$shots/win-1.png")"; IFS=x read -r pw ph <<<"$(png_size "$shots/screenshot.png")"
  check test "$ww" -ge 800 -a "$wh" -ge 400 || return 1
  check test $((pw * 2 - ww)) -ge -4 -a $((pw * 2 - ww)) -le 4 || { echo "screenshot: pane $pw wide in a window $ww wide"; return 1; }
  check test "$ph" -lt "$wh" -a "$ph" -gt $((wh / 2)) || return 1
  check python3 -c 'import json, sys
r = json.loads(sys.argv[1]); assert r["pane"] == "file-1" and r["window"] == "win-1" and r["path"].endswith(".png") and r["width"] > 0, r' "$doc" || return 1
  check test "$(png_size "$(echo "$doc" | field path)")" == "$(echo "$doc" | field width)x$(echo "$doc" | field height)" || return 1
  local ink; ink="$(png_ink "$shots/screenshot.png")"
  check python3 -c 'import sys; assert 0.01 < float(sys.argv[1]) < 0.5, sys.argv[1]' "$ink" || { echo "screenshot: the terminal pane isn't legible (ink $ink)"; return 1; }
  (cd "$shots" && kanna screenshot screenshot -o screenshot.png) 2>/dev/null && { echo "screenshot: it overwrote screenshot.png"; return 1; }
  (cd "$shots" && kanna screenshot screenshot -o screenshot.png --force) >/dev/null || { echo "screenshot line $LINENO"; return 1; }
  kanna open --window win-1 --tab --name other >/dev/null || return 1
  local hidden; hidden="$(kanna screenshot screenshot 2>&1)" && { echo "screenshot: shot a pane that isn't showing"; return 1; }
  [[ "$hidden" == *"is in tab tab-1, which isn't showing"* ]] || { echo "screenshot: $hidden"; return 1; }
  kanna open --window new --name elsewhere >/dev/null || return 1
  local caller; caller="$(KANNA_TERMINAL="$(pane other id)" kanna screenshot --json | field window)" || return 1
  check test "$caller" == win-1 || { echo "screenshot: from other's terminal, with win-2 in front, it shot $caller"; return 1; }
  local refused; refused="$(kanna screenshot 2>&1)" && { echo "screenshot: it shot with no target from outside Kanna"; return 1; }
  [[ "$refused" == *"screenshot: outside a Kanna terminal there's no window of yours: say PANE or --window WINDOW"* ]] || { echo "screenshot: $refused"; return 1; }
  refused="$(KANNA_TERMINAL=term-99 kanna screenshot 2>&1)" && { echo "screenshot: it shot for an unknown terminal"; return 1; }
  [[ "$refused" == *"term-99 isn't in a window that's showing: say PANE or --window WINDOW"* ]] || { echo "screenshot: $refused"; return 1; }
}

scenario diagram_check M4
diagram_check() {
  start_daemon && start_app || return 1
  local doc="$KANNA_STATE_DIR/diagram_check.md" said err
  printf '# diagram_check\n\n```mermaid\nflowchart LR\n    A --> B\n```\n\n```mermaid\nerDiagram\n    A ||--o{ B : has\n```\n' >"$doc"
  said="$(kanna open --window new md "$doc" --name doc --tab 2>"$KANNA_STATE_DIR/open.err")" || { echo "diagram_check: open md failed"; cat "$KANNA_STATE_DIR/open.err"; return 1; }
  err="$(cat "$KANNA_STATE_DIR/open.err")"
  [[ "$said" == "opened file-1 (doc) in win-1"* ]] || { echo "diagram_check: open said $said"; return 1; }
  [[ "$err" == "$doc:8: unsupported diagram type: er (Kanna draws "* ]] || { echo "diagram_check: open md reported \"$err\""; return 1; }
  check test "$(printf '%s\n' "$err" | wc -l | tr -d ' ')" == 1 || return 1
  check test "$(kanna open --window win-1 md "$doc" --json 2>/dev/null | field diagrams.0.line)" == 8 || return 1
  check wait_for 5 doc_text doc "Unsupported diagram type: er" || return 1
  check test "$(raw '{"id":1,"cmd":"debug.md","args":{"pane":"doc"}}' | python3 -c 'import json, sys; print([d["type"] for d in json.load(sys.stdin)["diagrams"]])')" == "['flowchart-v2']" || return 1
  kanna diagrams check "$doc" >/dev/null 2>&1; check test $? -eq 1 || return 1
}

scenario bash_completion M4
bash_completion() {
  SHELL=/bin/bash PATH="$bin:$PATH" start_daemon && start_app || return 1
  kanna open --window new --name bash_completion >/dev/null || return 1
  check wait_for 10 eval '[[ -n "$(kanna read bash_completion | tr -d "[:space:]")" ]]' || return 1
  sleep 2
  kanna send bash_completion 'echo "bash_completion:$(complete -p kanna):${PROMPT_COMMAND-none}:${KANNA_BASH_INIT-gone}"' || return 1
  check wait_for 5 shows bash_completion 'bash_completion:complete -o filenames -F _kanna kanna:'
  kanna read bash_completion | grep -E '^bash_completion:' | grep -qF ':gone' || { kanna read bash_completion | tail -4; return 1; }
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
sys.exit(not (len(items) == 2 and all(i["meta"].startswith("· ") for i in items) and all(i["right"] <= g["width"] for i in items)))'; }
  check eval 'fits "\"size\":[1400,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "19 on the Shelf" || { desk waiting; return 1; }
  check eval 'fits "\"size\":[520,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "19 on the Shelf" || { echo "desk_scrolls: the Shelf count gave way before the titles"; desk stripLayout; return 1; }
  check eval 'fits "\"size\":[400,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "" || { echo "desk_scrolls: the Shelf count didn't give way on a narrow strip"; desk stripLayout; return 1; }
  sleep 0.3; snap d1 desk_scrolls-strip >/dev/null
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
  for n in 1 2 3 4 5; do (cd "$repo" && kanna task new "Task $n" "say $n" --agent "$agent") >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }; done
  for n in 1 2 3 4 5; do
    [[ "$(kanna task start "$(tid $n)")" == "started $(sid $n) on the Shelf: term-"* ]] || { echo "task_start_stop: $(sid $n) didn't start on the Shelf"; return 1; }
  done
  check wait_for 10 eval '[[ "$(cd "$repo" && kanna task list | placed | grep -cE "^task-[0-9a-f]{4,8} +Task [1-5] +Shelf +(idle|working|running)")" == 5 ]]' || { (cd "$repo" && kanna task list); return 1; }
  for n in 1 2 3 4 5; do check wait_for 5 shows "Task $n" "say $n" || { echo "task_start_stop: Task $n's agent didn't get its first message"; return 1; }; done
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
  unwrapped "Task 1" "claude --resume $session" || { echo "task_start_stop: Task 1's agent wasn't quit with its keys"; kanna read "Task 1"; return 1; }
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
  (cd "$repo" && kanna task new "Line tables" "say hi" --agent "$agent") >/dev/null && kanna task start $(tid 1) >/dev/null || { echo "pick_at_once line $LINENO"; return 1; }
  check wait_for 5 shows "Line tables" "say hi" || { echo "pick_at_once: the task didn't start"; return 1; }
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
  (( took <= 200 )) || { echo "pick_at_once: the task took ${took}ms to reach the Shelf"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"task":"'"$card"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$(pane "Line tables" id)" ]]' || { echo "pick_at_once: pulling it at once didn't put it on the Desk"; raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  check wait_for 5 shows "Line tables" "--resume" || { echo "pick_at_once: its agent didn't come back"; kanna read "Line tables"; return 1; }
  check wait_for 5 is lines state running && check is tables state running || { echo "pick_at_once: its tabs didn't run again"; kanna workspace list --all; return 1; }
  check test "$(desk note)" == "" || { echo "pick_at_once: $(desk note)"; return 1; }
  grep -qE '^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9:.]{12} kanna-daemon: ' "$out/daemon.log" || { echo "pick_at_once: daemon.log's lines have no time"; tail -3 "$out/daemon.log"; return 1; }
}

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
  (cd "$repo" && kanna task new "Timed" "say hi" --agent "$agent") >/dev/null && kanna task start $(tid 1) >/dev/null || { echo "perf_intervals line $LINENO"; return 1; }
  check wait_for 5 shows "Timed" "say hi" || { echo "perf_intervals: the task didn't start"; return 1; }
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
  python3 -c 'import sys; sys.exit(float(sys.argv[1]) > 100)' "$shown" || { echo "perf_intervals: with 50 tasks on the Shelf the card showed ${shown}ms after the click"; raw '{"id":1,"cmd":"debug.perf"}'; return 1; }
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
  (cd "$repo" && kanna task new "Line tables" "say hi" --agent "$agent") >/dev/null && kanna task start $(tid 1) >/dev/null || { echo "pick_keeper_paused line $LINENO"; return 1; }
  check wait_for 5 shows "Line tables" "say hi" || { echo "pick_keeper_paused: the task didn't start"; return 1; }
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
  (( took <= 200 )) || { echo "pick_keeper_paused: the task took ${took}ms to reach the Desk"; return 1; }
  [[ "$listed" =~ ^([0-9]+)\ Desk\ .*Line\ tables ]] && (( BASH_REMATCH[1] <= 200 )) || { echo "pick_keeper_paused: task list, asked while the keeper was paused, answered (ms, where) $listed"; return 1; }
  check wait_for 5 shows "Line tables" "--resume" || { echo "pick_keeper_paused: its agent didn't come back once the keeper went on"; kanna read "Line tables"; return 1; }
  check wait_for 5 is lines state running || { echo "pick_keeper_paused: its tab didn't run again"; kanna workspace list --all; return 1; }
  check test "$(desk note)" == "" || { echo "pick_keeper_paused: $(desk note)"; return 1; }
  kill -0 "$(cat "$KANNA_STATE_DIR/daemon.pid")" || { echo "pick_keeper_paused: the daemon stopped"; tail -5 "$out/daemon.log"; return 1; }
}

scenario task_list_speed M4
task_list_speed() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude" n=40 i started ms worst=0
  mkdir -p "$repo" "${agent%/*}" && ln -s "$mock_agent" "$agent" || { echo "task_list_speed line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_list_speed line $LINENO"; return 1; }
  start_daemon || { echo "task_list_speed line $LINENO"; return 1; }
  for ((i = 1; i <= n; i++)); do (cd "$repo" && kanna task new "Task $i" "say hi" --agent "$agent") >/dev/null || { echo "task_list_speed line $LINENO"; return 1; }; done
  for ((i = 1; i <= n; i++)); do kanna task start "$(tid $i)" >/dev/null || { echo "task_list_speed: task $i didn't start"; return 1; }; done
  for ((i = 1; i <= n; i++)); do kanna task stop "$(tid $i)" >/dev/null || { echo "task_list_speed: task $i didn't stop"; return 1; }; done
  started="$(find "$repo/.kanna-worktrees" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
  (( started == n )) || { echo "task_list_speed: $started of $n tasks have a worktree"; return 1; }
  raw '{"id":1,"cmd":"debug.daemon.perf","args":{"reset":true}}' >/dev/null
  for i in 1 2 3 4 5; do
    ms="$( { /usr/bin/time -p kanna task list --json >/dev/null; } 2>&1 | awk '/^real/ {printf "%d", $2 * 1000}')"
    (( ms > worst )) && worst=$ms
  done
  echo "task_list_speed: $n stopped tasks with worktrees; kanna task list took at most ${worst}ms; daemon $(raw '{"id":1,"cmd":"debug.daemon.perf"}' | python3 -c 'import json, sys
r = json.load(sys.stdin); r = r.get("result", r)
i = {i["name"]: i for i in r["intervals"]}.get("loop.request", {})
print("loop.request max %sms" % i.get("max"))')"
  (( worst < 300 )) || { echo "task_list_speed: a list of $n tasks took ${worst}ms"; return 1; }
}

scenario typing_latency M4
typing_latency() {
  local keys="${KANNA_LATENCY_KEYS:-60}" floods="${KANNA_LATENCY_FLOODS:-1}" hidden="${KANNA_LATENCY_HIDDEN:-4}" agents="${KANNA_LATENCY_AGENTS:-0}" burners="${KANNA_LATENCY_CPU:-0}"
  local bound="${KANNA_LATENCY_BOUND:-100}" flood="exec yes 'typing_latency flood ........................................................................'" i burner_pids=()
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
        "loop.chores", "loop.output", "loop.app.message", "loop.status", "loop.request", "message.output", "message.status", "render"]
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

scenario task_restart_typing M4
task_restart_typing() {
  start_daemon && start_app || { echo "task_restart_typing line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$KANNA_STATE_DIR/bin/claude"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" || { echo "task_restart_typing line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_restart_typing line $LINENO"; return 1; }
  (cd "$repo" && kanna task new Typed "say hi" --agent "$agent") >/dev/null || { echo "task_restart_typing line $LINENO"; return 1; }
  kanna task pick "$(tid 1)" >/dev/null && kanna task activate "$(tid 1)" >/dev/null || { echo "task_restart_typing line $LINENO"; return 1; }
  check wait_for 10 shows Typed "say hi" || { kanna read Typed; return 1; }
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

scenario code_pinned_line M4
code_pinned_line() {
  start_daemon && start_app || return 1
  local file="$KANNA_STATE_DIR/code_pinned_line.rs"
  nested() { echo 'fn outer() {'; echo '    loop {'; echo "        if $1() {"; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; }
  nested ready >"$file"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | field "$2"; }
  key() { raw "{\"id\":1,\"cmd\":\"debug.key\",\"args\":{\"key\":\"$1\"}}" >/dev/null; }
  kanna open --window new code --name src "$file" >/dev/null || return 1
  check wait_for 5 eval '(( $(code src scopes) == 3 ))' || { echo "scopes $(code src scopes)"; return 1; }
  nested ready_now >"$file"
  check wait_for 5 eval '[[ "$(code src changed)" == 3 ]]' || { echo "changed $(code src changed)"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"src","at":[300,300]}}' >/dev/null; sleep 0.3
  key cmd+f
  check wait_for 5 eval '[[ "$(code src find_bar)" == 1 ]]' || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"ready"}}' >/dev/null
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","select":[3,11,5]}}' >/dev/null
  check wait_for 5 eval '[[ "$(code src selection_line)" == 3 && "$(code src selection_length)" == 5 ]]' || return 1
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","pick":40}}' >/dev/null
  check wait_for 5 eval '[[ "$(code src pinned)" == 3 && "$(code src pinned_rows.2.line)" == 3 ]]' || { echo "pinned $(code src pinned_rows)"; return 1; }
  check wait_for 5 eval '[[ "$(code src pinned_rows.2.matches)" == "[[11, 5]]" ]]' || { echo "matches $(code src pinned_rows)"; return 1; }
  check test "$(code src pinned_rows.2.selection)" == "[[11, 5]]" || { echo "selection $(code src pinned_rows)"; return 1; }
  check test "$(code src pinned_rows.2.changed)" == True || { echo "changed $(code src pinned_rows)"; return 1; }
  for row in 0 1; do
    check test "$(code src pinned_rows.$row.selection)$(code src pinned_rows.$row.matches)$(code src pinned_rows.$row.changed)" == "[][]False" || { echo "row $row $(code src pinned_rows)"; return 1; }
  done
  raw '{"id":1,"cmd":"debug.code","args":{"pane":"src","select":[2,4,4]}}' >/dev/null
  check wait_for 5 eval '[[ "$(code src pinned_rows.1.selection)" == "[[4, 4]]" && "$(code src pinned_rows.2.selection)" == "[]" ]]' || { echo "reselect $(code src pinned_rows)"; return 1; }
  key escape
  check wait_for 5 eval '[[ "$(code src pinned_rows.2.matches)" == "[]" ]]' || { echo "after find $(code src pinned_rows)"; return 1; }
}

scenario send_key M4
send_key() {
  start_daemon || return 1
  kanna open --window new --name bytes --cmd 'stty raw -echo; printf "READY\r\n"; while :; do printf "%s " "$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d " \n")"; done' >/dev/null || return 1
  check wait_for 5 shows bytes READY || return 1
  kanna send-key bytes up ctrl+c return esc shift+tab || return 1
  check wait_for 5 shows bytes "1b 5b 41 03 0d 1b 1b 5b 5a" || { kanna read bytes; return 1; }
  kanna open --window win-1 --name agent --cmd "$mock_agent" >/dev/null || return 1
  check wait_for 5 shows agent "type turn" || return 1
  kanna send agent ask || return 1
  check wait_for 5 shows agent "proceed?" || return 1
  kanna send-key agent down return || return 1
  check wait_for 5 shows agent "Declined." || { kanna read agent; return 1; }
  kanna send agent turn || return 1
  check wait_for 10 shows agent "proceed?" || return 1
  kanna send-key agent down up return || return 1
  check wait_for 5 shows agent TURN_DONE || { kanna read agent; return 1; }
  kanna send agent ask && check wait_for 5 shows agent "proceed?" || return 1
  kanna send agent 2 || return 1
  check wait_for 5 shows agent "Declined." || { kanna read agent; return 1; }
  ! shows agent "not in application cursor mode" || { echo "an arrow was sent in normal cursor mode"; kanna read agent; return 1; }
  [[ "$(kanna send-key agent cmd+c 2>&1)" == *"cmd+c only means something to Kanna.app"* ]] || { echo "cmd+c wasn't refused"; return 1; }
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
    said="$(kanna read "$agent task" | tr -d '\r\n' | grep -oE "$agent (--remote [^ ]+ )?(--)?resume [0-9a-f-]{36}" | tail -1 | grep -oE '[0-9a-f-]{36}$')"
    [[ -n "$said" ]] || { echo "real_agent_tasks: $agent wasn't quit with its keys, so it didn't say its session"; kanna read "$agent task" --lines 60; return 1; }
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
  printf '{"version": 1, "shelf": {"runners": 0}, "tasks": {"agent": "%s"}}\n' "$fake/codex" >"$KANNA_AGENTS_HOME/.kanna-v4/config.json"
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
  sleep 0.5; check drawn "$(snap d1 desk_zoom-shelf)" 1 || return 1
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 zoomed)" == "$log" ]]' || { echo "desk_zoom: the task lost its zoom to the Shelf's"; kanna workspace list --all; return 1; }
  stop_agents
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
  events() { raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"events\",\"task\":\"$1\"}}" | python3 -c 'import json, sys; print(" ".join(e["what"] + ("@" + e["by"] if e["by"] else "") for e in json.load(sys.stdin)["events"]))'; }
  [[ "$(events "$(tid 1)")" == "made renumbered@daemon" ]] || { echo "task_history: the imported task's events are $(events "$(tid 1)")"; return 1; }
  (cd "$repo" && kanna task new "Fresh" "say fresh" --agent "$agent") | grep -qE '^added task-[0-9a-f]{4,8} ' || { echo "task_history: a new task didn't get an ID"; return 1; }
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

scenario quiz_results M4
quiz_results() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" state="$KANNA_STATE_DIR"
  mkdir -p "$repo/src" && printf 'one\ntwo\nthree\n' >"$repo/src/a.rs" || { echo "quiz_results line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" add . && git -C "$repo" -c user.email=e@e -c user.name=e commit -q -m first || { echo "quiz_results line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  local head; head="$(git -C "$repo" rev-parse main)"
  start_daemon || { echo "quiz_results line $LINENO"; return 1; }
  quiz() { (cd "$repo/src" && kanna quiz "$@"); }
  [[ "$(quiz history)" == "no quiz rounds in this repo yet" ]] || { echo "quiz_results: a new repo has history: $(quiz history)"; return 1; }
  [[ "$(quiz start --json '{"commits": ["first"]}')" == r1 ]] || { echo "quiz_results: the first round isn't r1"; return 1; }
  [[ "$(quiz answer r1 --kind why --file a.rs:2 --wrong --json '{"question": "Why two?", "pick": 3, "answer": 1}')" == "a1 in r1" ]] || { echo "quiz_results: the miss wasn't saved"; return 1; }
  quiz answer r1 --kind architecture --file "$repo/src/a.rs:1-3" --right >/dev/null || { echo "quiz_results line $LINENO"; return 1; }
  quiz answer r1 --kind trivia --file a.rs:1 --right >/dev/null 2>&1 && { echo "quiz_results: an unknown kind was saved"; return 1; }
  stop_daemon; start_daemon || { echo "quiz_results line $LINENO"; return 1; }
  local misses; misses="$(quiz history --misses --json | python3 -c '
import json, sys
rounds = json.load(sys.stdin)["rounds"]
answers = [a for r in rounds for a in r["answers"]]
print(" ".join(r["round"] + "@" + r["head"] for r in rounds), " ".join(":".join([a["answer"], a["kind"], a["file"], str(a["doc"]["line"]), a["doc"]["question"]]) for a in answers))')"
  [[ "$misses" == "r1@$head a1:why:src/a.rs:2:Why two?" ]] || { echo "quiz_results: the misses read back as \"$misses\""; return 1; }
  [[ "$(quiz start)" == r2 ]] || { echo "quiz_results line $LINENO"; return 1; }
  quiz answer r2 --kind why --file a.rs:2 --right --again a1 >/dev/null || { echo "quiz_results line $LINENO"; return 1; }
  [[ "$(quiz history --misses)" == "no misses to bring back" ]] || { echo "quiz_results: a miss asked again is still a miss: $(quiz history --misses)"; return 1; }
  quiz history | grep -qE '^  a2 +✓ architecture +src/a.rs:1$' || { echo "quiz_results: the history lacks the right answer"; quiz history; return 1; }
  [[ "$(sqlite3 -readonly "$state/kanna.db" "select count(*) from quiz_rounds; select group_concat(right) from quiz_answers")" == $'2\n0,1,1' ]] || { echo "quiz_results: kanna.db doesn't hold the rounds and answers"; return 1; }
}

scenario stats_tab M4
stats_tab() {
  local repo; repo="$(e2e_repo stats)" || { echo "stats_tab line $LINENO"; return 1; }
  local day=86400 now; now="$(date +%s)"
  dated() { GIT_AUTHOR_DATE="@$1 +0000" GIT_COMMITTER_DATE="@$1 +0000" git -C "$repo" -c user.email=e@e -c user.name=e "${@:2}"; }
  dated $((now - 3 * day)) commit -q --allow-empty -m plain || { echo "stats_tab line $LINENO"; return 1; }
  git -C "$repo" checkout -q -b feature && dated $((now - 2 * day)) commit -q --allow-empty -m one && dated $((now - 2 * day + 60)) commit -q --allow-empty -m two || { echo "stats_tab line $LINENO"; return 1; }
  git -C "$repo" checkout -q main && dated "$now" merge -q --no-ff -m "Merge feature" feature || { echo "stats_tab line $LINENO"; return 1; }
  start_daemon || { echo "stats_tab line $LINENO"; return 1; }
  quiz() { (cd "$repo" && kanna quiz "$@"); }
  quiz start >/dev/null || { echo "stats_tab line $LINENO"; return 1; }
  quiz answer r1 --kind why --file docs/specs/a.md:1 --wrong >/dev/null && quiz answer r1 --kind why --file docs/specs/a.md:2 --right >/dev/null \
    && quiz answer r1 --kind bug --file crates/one/src/a.rs:3 --right >/dev/null && quiz answer r1 --kind where --file README.md:1 --right >/dev/null || { echo "stats_tab line $LINENO"; return 1; }
  local one two three
  one="$(new_task "Stats one" "" stats)" && two="$(new_task "Stats two" "" stats)" && three="$(new_task "Stats three" "" stats)" || { echo "stats_tab line $LINENO"; return 1; }
  kanna task start "$one" >/dev/null && kanna task start "$three" >/dev/null || { echo "stats_tab line $LINENO"; return 1; }
  kanna task done "$one" >/dev/null || { echo "stats_tab line $LINENO"; return 1; }
  local json; json="$(cd "$repo" && kanna stats --json)" || { echo "stats_tab: kanna stats --json failed"; return 1; }
  check python3 -c 'import json, sys
r = json.loads(sys.argv[1])
assert r["project"] == sys.argv[2], r["project"]
assert len(r["days"]) == 30, r["days"]
s = {x["id"]: x for x in r["sections"]}
assert list(s) == ["quiz", "main", "tasks", "code"], list(s)
t = {(sid, x["id"]): x for sid, sec in s.items() for x in sec["tiles"]}
assert t["quiz", "right"]["value"] == 75 and t["quiz", "right"]["note"] == "of 4 answers", t["quiz", "right"]
assert t["quiz", "misses"]["value"] == 1, t["quiz", "misses"]
assert [x["label"] for x in t["quiz", "areas"]["chart"]["rows"]] == ["README.md", "crates/one", "docs/specs"], t["quiz", "areas"]
assert t["main", "commits"]["value"] == 3 and t["main", "commits"]["note"] == "today · 4 in 30 days", t["main", "commits"]
assert t["main", "landed"]["value"] == 1, t["main", "landed"]
assert t["tasks", "made"]["note"] == "today · 3 in 30 days", t["tasks", "made"]
assert t["tasks", "started"]["note"] == "today · 2 in 30 days", t["tasks", "started"]
assert t["tasks", "ended"]["note"] == "today · 1 in 30 days", t["tasks", "ended"]
assert t["tasks", "lead"]["note"] == "median of 1" and t["tasks", "lead"]["unit"] == "s", t["tasks", "lead"]
assert s["code"]["placeholder"] is True and s["code"]["tiles"] == [], s["code"]' "$json" "$repo" || return 1
  [[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*), sum(value) from samples where metric = 'commits'")" == "3|4.0" ]] || { echo "stats_tab: the samples are $(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select metric, value from samples")"; return 1; }
  (cd "$repo" && kanna stats) | grep -qE '^Main +Commits +3  today · 4 in 30 days ' || { echo "stats_tab: kanna stats prints"; (cd "$repo" && kanna stats); return 1; }
  start_app || { echo "stats_tab line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  local opened d; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "stats_tab line $LINENO: $opened"; return 1; }
  d="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk pages "$d")" == '"'"'["Desk", "Machines", "Stats"]'"'"' ]]' || { echo "stats_tab: the Desk view's fixed tabs"; desk pages "$d"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$d\",\"size\":[1300,900]}}" >/dev/null
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"Machines\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(desk machinesPage "$d")" == True ]]' || { echo "stats_tab: the Machines tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Next Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk statsPage "$d")" == True && "$(desk machinesPage "$d")" == False ]]' || { echo "stats_tab: Next Tab from Machines didn't show Stats"; return 1; }
  check wait_for 10 eval '[[ "$(desk stats "$d")" != "[]" ]]' || { echo "stats_tab: the Stats page drew no tiles"; desk statsSections "$d"; return 1; }
  check python3 -c 'import json, sys
shown = {(x["section"], x["id"]): x for x in json.loads(sys.argv[1])}
r = json.loads(sys.argv[2])
for sec in r["sections"]:
    for x in sec["tiles"]:
        got = shown[sec["id"], x["id"]]
        assert got["label"] == x["label"] and got["note"] == x["note"], (got, x)
        assert got["frame"] and got["frame"]["width"] > 100 and got["frame"]["height"] > 40, got
assert shown["quiz", "right"]["figure"] == "75%" and shown["main", "commits"]["figure"] == "3", shown
assert shown["tasks", "lead"]["figure"] not in ("", "—"), shown["tasks", "lead"]
code = [s for s in json.loads(sys.argv[3]) if s["id"] == "code"][0]
assert code["placeholder"] is True and code["empty"].startswith("Not measured yet"), code' "$(desk stats "$d")" "$json" "$(desk statsSections "$d")" || return 1
  sleep 0.5; snap "$d" stats_tab-dark >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.5
  snap "$d" stats_tab-light >/dev/null
  [[ -s "$out/stats_tab-dark.png" && -s "$out/stats_tab-light.png" ]] || { echo "stats_tab: no snapshots"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"Desk\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(desk statsPage "$d")" == False && "$(desk page "$d")" == True ]]' || { echo "stats_tab: the Desk tab didn't leave the Stats page"; return 1; }
}

scenario quiz_window M4
quiz_window() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo/src" && printf 'one\ntwo\nthree\n' >"$repo/src/a.rs" || { echo "quiz_window line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" add . && git -C "$repo" -c user.email=e@e -c user.name=e commit -q -m first || { echo "quiz_window line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon && start_app || { echo "quiz_window line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  local opened d; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "quiz_window line $LINENO: $opened"; return 1; }
  d="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk lead "$d")" == term-* ]]' || { echo "quiz_window: the Desk has no planning terminal"; kanna workspace list --all; return 1; }
  local planner; planner="$(desk lead "$d")"
  opened="$(cd "$repo/src" && KANNA_TERMINAL="$planner" kanna quiz open --agent "$agent")" && [[ "$opened" =~ ^opened\ (term-[0-9]+)\ in\ (win-[0-9]+)$ ]] || { echo "quiz_window: kanna quiz open said \"$opened\""; return 1; }
  local quiz="${BASH_REMATCH[1]}" w="${BASH_REMATCH[2]}"
  [[ "$w" != "$d" ]] || { echo "quiz_window: the quiz opened on the Desk"; return 1; }
  check wait_for 5 eval '[[ "$(desk window.lead.pane "$w")" == "$quiz" ]]' || { echo "quiz_window: the quiz agent isn't its window's lead"; kanna workspace list --all --json; return 1; }
  check test "$(pane "$quiz" name)" == Quiz || return 1
  check test "$(cd "$(pane "$quiz" cwd)" && pwd -P)" == "$repo" || { echo "quiz_window: the quiz agent isn't in the repo's main checkout"; return 1; }
  check wait_for 5 shows "$quiz" "args: You are the Kanna quiz agent" || { kanna read "$quiz"; return 1; }
  check wait_for 5 eval '[[ "$(desk lead "$w")" == "$quiz" && "$(desk desk "$w")" == False && "$(desk strip "$w")" == False && "$(desk page "$w")" == False ]]' || { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}"; return 1; }
  check wait_for 5 eval '[[ "$(desk header "$w")" == "Quiz · "*"${repo#/private} · main" && "$(desk title "$w")" == Quiz* ]]' || { desk header "$w"; desk title "$w"; return 1; }
  check test "$(kanna task list --all | grep -c "$quiz")" == 0 || { echo "quiz_window: the quiz agent is listed as a task"; kanna task list; return 1; }
  check test "$(desk running "$d")" == 0 || { echo "quiz_window: the quiz agent counts as a running task"; desk running "$d"; return 1; }
  check test "$(desk tasks "$d")" == "[]" || { echo "quiz_window: the quiz agent is on the Shelf"; desk tasks "$d"; return 1; }
  local said code; said="$(KANNA_TERMINAL="$quiz" kanna comment "$repo/src/a.rs:2" "Two comes second.")" && [[ "$said" == "opened file-"*" · a.rs:2"* ]] || { echo "quiz_window: kanna comment said \"$said\""; return 1; }
  code="$(awk '{print $2}' <<<"$said")"
  check wait_for 5 eval '[[ "$(desk window.tabs.0.panes "$w")" == "[\"$code\"]" && "$(desk window.lead.pane "$w")" == "$quiz" && "$(desk window.tab "$w")" == "$(desk window.tabs.0.id "$w")" ]]' || { echo "quiz_window: the comment isn't a tab of the Quiz window"; kanna workspace list --all; return 1; }
  sleep 0.5; snap "$w" quiz_window >/dev/null
  kanna move "$quiz" --tab >/dev/null 2>&1 && { echo "quiz_window: the quiz agent moved into a tab"; return 1; }
  KANNA_TERMINAL="$quiz" kanna task park >/dev/null 2>&1 && { echo "quiz_window: the quiz agent parked"; return 1; }
  kanna task activate "$quiz" >/dev/null 2>&1 && { echo "quiz_window: the quiz agent was pulled onto a Desk"; return 1; }
  kanna move "$quiz" --dock --window "$d" >/dev/null 2>&1 && { echo "quiz_window: the quiz agent docked on the Desk"; return 1; }
  local t; for t in One Two Three; do (cd "$repo" && kanna task new "$t" "hi" --agent "$agent") >/dev/null || { echo "quiz_window line $LINENO"; return 1; }; done
  for t in $(tid 1) $(tid 2) $(tid 3); do kanna task pick "$t" >/dev/null || { echo "quiz_window: the quiz agent took one of the project's three places"; kanna task list; return 1; }; done
  check wait_for 10 eval '[[ "$(desk running "$d")" == 3 ]]' || { echo "quiz_window: three tasks didn't fit on the Shelf beside the quiz"; kanna task list; return 1; }
  opened="$(cd "$repo" && kanna quiz open --agent "$agent")"
  [[ "$opened" == "brought forward $quiz in $w" ]] || { echo "quiz_window: a second kanna quiz open said \"$opened\""; kanna workspace list --all; return 1; }
  [[ "$(kanna workspace list --all --json | python3 -c 'import json, sys; print(sum(1 for w in json.load(sys.stdin)["windows"] if w.get("lead")))')" == 1 ]] || { echo "quiz_window: a second Quiz window opened"; kanna workspace list --all; return 1; }
  kanna close --window "$w" >/dev/null || { echo "quiz_window line $LINENO"; return 1; }
  check wait_for 5 eval '! kanna workspace list --all --json | grep -q "\"$quiz\""' || { echo "quiz_window: closing the window left its quiz agent running"; kanna workspace list --all; return 1; }
}

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
  check wait_for 5 shows "$planner" "Let Kanna's agents work without permission prompts?" || { echo "planner_choice: the first Desk didn't ask about permission prompts"; kanna read "$planner"; return 1; }
  ! shows "$planner" "args:" || { echo "planner_choice: the agent started before the question was answered"; return 1; }
  kanna send "$planner" y >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 5 shows "$planner" "Kanna's agents work without permission prompts from now on" || { kanna read "$planner"; return 1; }
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
  ! shows "$planner" "permission prompts?" || { echo "planner_choice: the next Desk asked about permission prompts again"; return 1; }
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

scenario cli_caller M4
cli_caller() {
  start_daemon && start_app || { echo "cli_caller line $LINENO"; return 1; }
  local file="$KANNA_STATE_DIR/cli_caller.rs" doc="$KANNA_STATE_DIR/cli_caller.md" refused
  for i in $(seq 20); do echo "let line_$i = $i;"; done >"$file"
  printf '# cli_caller\n\nA doc.\n' >"$doc"
  for line in "open --name x" "open md $doc" "comment $file:3 Three." "markup $doc" "arrange '{\"pane\":\"x\"}'"; do
    refused="$(eval "kanna $line" 2>&1)"; check test $? -eq 2 || { echo "cli_caller: kanna $line outside Kanna: $refused"; return 1; }
    [[ "$refused" == *"${line%% *}: outside a Kanna terminal there's no window of yours: add --window WINDOW"* ]] || { echo "cli_caller: kanna $line said: $refused"; return 1; }
  done
  refused="$(kanna screenshot 2>&1)"; check test $? -eq 2 || return 1
  [[ "$refused" == *"screenshot: outside a Kanna terminal there's no window of yours: say PANE or --window WINDOW"* ]] || { echo "cli_caller: screenshot said: $refused"; return 1; }
  refused="$(kanna task park 2>&1)"; check test $? -eq 2 || return 1
  [[ "$refused" == *"task: park needs a TASK, or run it from the task's terminal"* ]] || { echo "cli_caller: task park said: $refused"; return 1; }
  check test "$(state | field windows | count)" == 0 || { echo "cli_caller: a refused command opened a window"; return 1; }
  refused="$(KANNA_TERMINAL=term-99 kanna open --name x 2>&1)" && { echo "cli_caller: an unknown caller opened a pane"; return 1; }
  [[ "$refused" == *"term-99 isn't one of this Kanna's panes"*"--window"* ]] || { echo "cli_caller: an unknown caller: $refused"; return 1; }
  expected="opened term-1 (a) in win-1, tab-1: running"; says kanna open --window new --name a || return 1
  expected="opened term-2 (b) in win-2, tab-2: running"; says kanna open --window new --name b || return 1
  check wait_for 5 eval '[[ "$(dwin win-2 key)" == True ]]' || { echo "cli_caller: the app doesn't have win-2 in front"; return 1; }
  check test "$(state | python3 -c 'import json, sys; print([w["id"] for w in json.load(sys.stdin)["windows"] if "key" in w])')" == "[]" || { echo "cli_caller: the daemon names a key window"; return 1; }
  refused="$(kanna raw '{"cmd":"open","args":{"type":"term"}}' 2>&1)" && { echo "cli_caller: an open naming no window, with two windows, opened: $refused"; return 1; }
  [[ "$refused" == *"several windows"*"--window"* ]] || { echo "cli_caller: an open naming no window: $refused"; return 1; }
  expected="focused a in win-1 (in front)"; says kanna focus a || return 1
  check wait_for 5 eval '[[ "$(dwin win-1 key)" == True ]]' || { echo "cli_caller: focus a didn't bring win-1 forward"; return 1; }
  kanna focus b >/dev/null || return 1
  local a; a="$(pane a id)"
  expected="opened term-3 (c) in win-1, tab-1: running"; KANNA_TERMINAL="$a" says kanna open --name c || { echo "cli_caller: open from a's terminal"; return 1; }
  local said code
  said="$(KANNA_TERMINAL="$a" kanna comment "$file:3" "Three.")" || return 1
  [[ "$said" == "opened file-"*" · cli_caller.rs:3" ]] || { echo "cli_caller: comment said $said"; return 1; }
  code="$(awk '{print $2}' <<<"$said")"
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys
s = json.load(sys.stdin); print(next(w["id"] for w in s["windows"] if any(sys.argv[1] in json.dumps(t) for t in w["tabs"])))' "$code")" == win-1 || { echo "cli_caller: the comment's pane isn't in a's window"; return 1; }
  expected="$code · cli_caller.rs:5"; KANNA_TERMINAL="$a" says kanna comment "$file:5" "Five." || { echo "cli_caller: a second comment from a's terminal didn't reuse its pane"; return 1; }
  kanna focus b >/dev/null || return 1
  expected="$code · cli_caller.rs:7"; says kanna comment --window win-1 "$file:7" "Seven." || { echo "cli_caller: a comment --window win-1 didn't reuse its pane"; return 1; }
  KANNA_TERMINAL="$a" kanna markup "$doc" --name spec --tab >/dev/null || return 1
  check test "$(pane spec type)" == md || return 1
  kanna focus b >/dev/null || return 1
  expected="a:1/2 | c:1/2"; KANNA_TERMINAL="$a" says kanna arrange '{"split":"row","children":[{"pane":"a"},{"pane":"c"}]}' || { echo "cli_caller: arrange from a's terminal"; kanna workspace list --all; return 1; }
  check test "$(win win-2 tabs.0.layout.pane)" == b || { echo "cli_caller: arrange touched win-2"; return 1; }
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

scenario blocked_tasks M4
blocked_tasks() {
  start_daemon && start_app || { echo "blocked_tasks line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "blocked_tasks line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  in_repo task new "Screenshot command" "say hi" --agent "$agent" >/dev/null && in_repo task new "Status fix" --agent "$agent" >/dev/null || { echo "blocked_tasks line $LINENO"; return 1; }
  local first added; first="$(tid 1)"; added="$(in_repo task new "Diff viewer" "say hi" --agent "$agent" --after "${first:5:4}" 2>&1)"
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
  check wait_for 5 eval '[[ "$(desk note)" == *"waits for $(sid 1)"* ]]' || { echo "blocked_tasks: clicking a blocked row didn't say why"; desk note; return 1; }
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
  in_repo task new A "say hi" --agent "$agent" >/dev/null || { echo "stacked_tasks line $LINENO"; return 1; }
  in_repo task new B "say hi" --agent "$agent" --after "$(tid 1)" >/dev/null && in_repo task new C "say hi" --agent "$agent" --after "$(tid 2)" >/dev/null || { echo "stacked_tasks line $LINENO"; return 1; }
  local a b c; a="$(tid 1)" b="$(tid 2)" c="$(tid 3)"
  folder() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
print(next(t["folder"] for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]))' "$1"; }
  of() { in_repo task list --json | python3 -c 'import json, sys
t = next(t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1])
names = lambda k: ",".join(x.get("name") or x["id"] for x in t.get(k) or [])
print("ready" if t.get("ready") else "-", "stale:" + names("stale"), "bases:" + names("bases"))' "$1"; }
  head() { git -C "$1" rev-parse HEAD; }
  in_repo task start "$a" >/dev/null || { echo "stacked_tasks: A didn't start"; return 1; }
  [[ "$(in_repo task start "$b" 2>&1)" == *"waits for $(sid 1) \"A\": it can start once it's ready or done"* ]] || { echo "stacked_tasks: B started before A was ready"; return 1; }
  local fa fb fc said; fa="$(folder "$a")"
  git -C "$fa" commit -q --allow-empty -m a1 || return 1
  said="$(in_repo task ready set "$a" 2>&1)"
  [[ "$said" == "$(sid 1) is ready at "*$'\n'"$(sid 2) \"B\" can start now: nothing it waits for is left" ]] || { echo "stacked_tasks: ready said $said"; return 1; }
  in_repo task start "$b" >/dev/null || { echo "stacked_tasks: B didn't start on A's ready work"; return 1; }
  fb="$(folder "$b")"
  check test "$(head "$fb")" == "$(head "$fa")" || { echo "stacked_tasks: B didn't start from A's branch"; return 1; }
  check test "$(of "$b")" == "- stale: bases:A" || { of "$b"; return 1; }
  git -C "$fb" commit -q --allow-empty -m b1 && in_repo task ready set "$b" >/dev/null || { echo "stacked_tasks line $LINENO"; return 1; }
  in_repo task start "$c" >/dev/null || { echo "stacked_tasks: C didn't start on B's ready work"; return 1; }
  fc="$(folder "$c")"
  check test "$(head "$fc")" == "$(head "$fb")" || { echo "stacked_tasks: C didn't start from B's branch"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 2)"' +B +Shelf +ready +merges after "A"$' || { echo "stacked_tasks: B doesn't say it merges after A"; in_repo task list; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "stacked_tasks line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  stack() { desk scroll.stacked | python3 -c 'import json, sys
print(next((r["stack"] for r in json.load(sys.stdin) if r["task"] == sys.argv[1]), "-"))' "$1"; }
  check wait_for 10 eval '[[ "$(stack "$b")" == "ready · merges after “A”" ]]' || { echo "stacked_tasks: B's Shelf row doesn't show its stack"; desk scroll.stacked; return 1; }

  git -C "$fa" commit -q --allow-empty -m a2 || return 1
  check wait_for 10 eval '[[ "$(of "$c")" == "- stale:B bases:B" ]]' || { echo "stacked_tasks: a new commit on A didn't make C stale"; of "$c"; return 1; }
  check test "$(of "$a")" == "- stale: bases:" || { echo "stacked_tasks: A is still ready after its branch moved"; of "$a"; return 1; }
  check test "$(of "$b")" == "- stale:A bases:A" || { echo "stacked_tasks: B isn't stale"; of "$b"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 3)"' +C +Shelf +stale +behind "B"$' || { in_repo task list; return 1; }
  check wait_for 10 eval '[[ "$(stack "$c")" == "stale · behind “B”" ]]' || { echo "stacked_tasks: C's row doesn't show it's stale"; desk scroll.stacked; return 1; }
  sleep 0.5; snap "$w" stacked_tasks-stale >/dev/null
  in_repo task messages "$b" | grep -q "kanna .*\"A\" moved since this task was built on it" || { echo "stacked_tasks: B wasn't told to catch up"; in_repo task messages "$b"; return 1; }
  in_repo task messages "$c" | grep -q "kanna .*\"B\" is stale, so this task is too" || { echo "stacked_tasks: C wasn't told to wait"; in_repo task messages "$c"; return 1; }
  [[ "$(in_repo task ready set "$b" 2>&1)" == *"\"A\" isn't in this branch yet: merge it first (git merge $(head "$fa"))"* ]] || { echo "stacked_tasks: B was let be ready without A's new work"; return 1; }
  [[ "$(in_repo task ready set "$c" 2>&1)" == *"\"B\" is stale itself"* ]] || { echo "stacked_tasks: C caught up before B"; return 1; }

  git -C "$fb" merge -q --no-edit "$(head "$fa")" && in_repo task ready set "$b" >/dev/null || { echo "stacked_tasks: B couldn't catch up"; return 1; }
  check test "$(of "$b")" == "ready stale: bases:A" || { of "$b"; return 1; }
  sleep 3
  check test "$(of "$c")" == "- stale:B bases:B" || { echo "stacked_tasks: C stopped being stale before it caught up"; of "$c"; return 1; }
  [[ "$(in_repo task ready set "$c" 2>&1)" == *"\"B\" isn't in this branch yet"* ]] || { echo "stacked_tasks: C was let be ready without B's new work"; return 1; }
  in_repo task list | placed | grep -qE '^'"$(sid 2)"' +B +Shelf +ready +merges after "A"$' || { echo "stacked_tasks: B doesn't say it waits for A to merge"; in_repo task list; return 1; }
  git -C "$fc" merge -q --no-edit "$(head "$fb")" && in_repo task ready set "$c" >/dev/null || { echo "stacked_tasks: C couldn't catch up"; return 1; }
  check test "$(of "$c")" == "ready stale: bases:B" || { of "$c"; return 1; }
  check wait_for 10 eval '[[ "$(desk scroll.stacked)" != *"waits for"* ]]' || { echo "stacked_tasks: a started task's row says it waits"; desk scroll.stacked; return 1; }

  git -C "$repo" merge -q --no-edit "$(head "$fa")" || return 1
  check wait_for 10 eval '[[ "$(of "$b")" == "ready stale: bases:" ]]' || { echo "stacked_tasks: A in main didn't make B an ordinary task"; of "$b"; return 1; }
  in_repo task messages "$b" | grep -q "\"A\"'s work is in main now: merge main" || { echo "stacked_tasks: B wasn't told to merge main"; in_repo task messages "$b"; return 1; }
  raw '{"id":1,"cmd":"tasks","args":{"action":"events","task":"'"$c"'"}}' | python3 -c 'import json, sys
seen = [e["what"] for e in json.load(sys.stdin)["events"]]
sys.exit(0 if all(w in seen for w in ("ready", "stale")) else print(seen) or 1)' || { echo "stacked_tasks: C's events don't record ready and stale"; return 1; }
  sleep 0.5; snap "$w" stacked_tasks >/dev/null
  stop_agents
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


scenario shelf_runners M4
shelf_runners() {
  start_daemon && start_app || { echo "shelf_runners line $LINENO"; return 1; }
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_runners line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  runners() { printf '{"version": 1, "shelf": {"runners": %s}}\n' "$1" >"$repo/.kanna/config.json"; }
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
  in_repo task done $(tid 1) >/dev/null || { echo "shelf_runners line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(of $(tid 3) 2)" != waiting ]]' || { echo "shelf_runners: the blocked task didn't run once it was free"; shelf; return 1; }
  [[ "$(kanna task pick $(tid 2))" == "moved $(sid 2) to the Desk: "* ]] || { echo "shelf_runners: the stopped task didn't move to the Shelf"; return 1; }
  check wait_for 5 eval '[[ "$(pane Asks id)" != "$asks" ]]' && asks="$(pane Asks id)" || { echo "shelf_runners: the started task kept its old terminal"; return 1; }
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
  [[ "$(kanna task pick $(tid 8))" == "moved $(sid 8) to the Desk: "* ]] || { echo "shelf_runners: the presenting task didn't move to the Shelf"; return 1; }
  check wait_for 10 eval '[[ "$(pane Presents id)" != "$presenter" ]]' || { echo "shelf_runners: the task's agent didn't start again"; return 1; }
  presenter="$(pane Presents id)"
  rerun() { ws $(tid 8) panes | python3 -c 'import json, sys; print(" ".join(p for p in json.load(sys.stdin) if p not in sys.argv[1:]))' "$md" "$short" "$presenter"; }
  check wait_for 10 eval '[[ "$(rerun)" == term-* && "$(rerun)" != "$long" ]]' && long="$(rerun)" || { echo "shelf_runners: the hung-up terminal didn't run again as a new terminal"; kanna workspace list --all; return 1; }
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
  local docs; docs="$(pane "Docs pass" id)"
  local said_docs="$(sid 3) \"Docs pass\"" said_inked="$(sid 1) \"Inked\"" said_unmerged="$(sid 4) \"Unmerged\""
  check wait_for 10 eval 'raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"task\":\"$docs\",\"window\":\"$d\"}}" | grep -q "\"ok\":true"' || { echo "desk_news: couldn't click Docs pass on the Desk"; return 1; }
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$docs" ]]' || { echo "desk_news: the click didn't make Docs pass active"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Move to Shelf"}}' | grep -q '"ok":true' || { echo "desk_news: Move to Shelf didn't run"; return 1; }
  check wait_for 10 eval '[[ "$(news)" == *"$said_docs moved to the Shelf"* ]]' || { echo "desk_news: the user's Desk changes weren't told: $(news)"; return 1; }
  [[ "$(news)" == *"$said_docs is the active task"$'\n'"$said_docs moved to the Shelf"* ]] || { echo "desk_news: activating and shelving at once weren't one message: $(news)"; return 1; }
  [[ "$(news)" == "$said_docs moved to the Desk"* ]] || { echo "desk_news: the Shelf click's pick wasn't told first: $(news)"; return 1; }
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

scenario toasts M4
toasts() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "toasts line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "toasts line $LINENO"; return 1; }
  check test "$(kanna toast "Early" "raised before the app")" == "raised toast-1; no app is attached, so it shows when one attaches within a minute" || return 1
  (cd "$repo" && kanna task new "Lint fixes" "say 1" --agent "$agent" && kanna task new "Docs pass" "say 2" --agent "$agent") >/dev/null || { echo "toasts line $LINENO"; return 1; }
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
  check wait_for 5 eval '[[ "$(desk title)" == "Lint fixes"* ]]' || { echo "toasts: clicking the toast didn't bring Lint fixes onto the Desk: $(desk title)"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 focused)" == "$(pane "Lint fixes" id)" ]]' || { echo "toasts: the task's agent isn't focused"; return 1; }
  check wait_for 5 eval '[[ "$(toasted)" == "d1 toast-2 toast-1" ]]' || { echo "toasts: the clicked toast stayed: $(toasted)"; return 1; }
  check test "$(kanna toast "Docs pass waits" --task "$(tid 2)")" == "raised toast-4" || return 1
  check wait_for 5 eval '[[ "$(toasted)" == "d1 toast-4"* ]]' || return 1
  raw '{"id":1,"cmd":"debug.toasts","args":{"click":"toast-4"}}' >/dev/null
  check wait_for 15 eval '[[ "$(desk title)" == "Docs pass"* ]]' || { echo "toasts: clicking a Shelf task's toast didn't shelve and show it: $(desk title)"; desk note; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 focused)" == "$(pane "Docs pass" id)" ]]' || { echo "toasts: the Shelf task's agent isn't focused"; return 1; }
  kanna toast "Kept" "the pointer is on it" >/dev/null && kanna toast "Fades" >/dev/null || { echo "toasts line $LINENO"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"hover":"toast-5"}}' >/dev/null
  check wait_for 12 eval '[[ "$(toasted)" == "d1 toast-5" ]]' || { echo "toasts: after 6s only the hovered toast should be left: $(toasted)"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"hover":"toast-5","on":false}}' >/dev/null
  check wait_for 5 eval '[[ "$(toasted)" == "d1 " ]]' || { echo "toasts: the toast left by the pointer didn't fade: $(toasted)"; return 1; }
  kanna toast "Closed" >/dev/null && check wait_for 5 eval '[[ "$(toasted)" == "d1 toast-7" ]]' || return 1
  raw '{"id":1,"cmd":"debug.toasts","args":{"click":"toast-7","close":true}}' >/dev/null
  check wait_for 2 eval '[[ "$(toasted)" == "d1 " ]]' || { echo "toasts: × didn't close it"; return 1; }
}

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
  check test "$(desk pages)" == '["Desk", "Machines", "Stats"]' || { echo "machine_remote_task: the empty Desk's fixed tabs"; desk pages; return 1; }
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
  check wait_for 5 eval '[[ "$(desk page)" == True && "$(desk statsPage)" == False ]]' || { echo "machine_remote_task: Next Tab from the Stats tab didn't wrap to the Shelf tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Previous Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk statsPage)" == True && "$(desk page)" == False ]]' || { echo "machine_remote_task: Previous Tab from the Shelf tab didn't show the Stats tab"; return 1; }
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

scenario serial_pane M4
serial_pane() {
  local devices="$KANNA_STATE_DIR/serial-devices"
  serial_device=""
  mkdir -p "$devices"
  export KANNA_SERIAL_PORTS="$devices/ports.json"
  plug() {
    : >"$devices/out"
    python3 "$repo_root/tools/fake-serial/fake-serial.py" "$devices" >"$devices/out" 2>&1 &
    serial_device=$!
    wait_for 5 test -s "$devices/out"
  }
  trap 'stop_pid "$serial_device"' EXIT
  plug || { echo "serial_pane: the fake device didn't start"; return 1; }
  start_daemon && start_app || return 1
  ser() { raw "{\"id\":1,\"cmd\":\"debug.serial\",\"args\":{\"pane\":\"esp\"${2:-}}}" | field "$1"; }
  kanna open --window new serial 303a:1001 --name esp >/dev/null || return 1
  check is esp type serial || return 1
  check wait_for 5 eval '[[ "$(ser state)" == connected && "$(ser send)" == True ]]' || { echo "serial_pane: $(ser state)"; return 1; }
  check test "$(pane esp title)" == "USB JTAG/serial debug unit · $(basename "$(cat "$devices/out")")" || { echo "serial_pane: title $(pane esp title)"; return 1; }
  kanna serial send esp reboot >/dev/null || return 1
  check wait_for 5 eval 'ser text | grep -q "app: ready, free heap"' || { echo "serial_pane: no boot in the view: $(ser text | tail -3)"; return 1; }
  ser state ',"type":"wifi status"' >/dev/null
  check wait_for 5 eval 'kanna serial read esp --last 3 | grep -q "rssi -58"' || { echo "serial_pane: the send line didn't reach the device"; return 1; }
  check test "$(ser history)" == '["wifi status"]' || { echo "serial_pane: history $(ser history)"; return 1; }
  check wait_for 5 eval 'ser text | grep -q "→ wifi status"' || return 1
  check wait_for 5 eval 'ser status | grep -q "ESP-IDF log levels"' || { echo "serial_pane: status $(ser status)"; return 1; }
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"serial_pane-dark.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/serial_pane-dark.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.4
  raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"serial_pane-light.png"}}' >/dev/null; cp "$KANNA_STATE_DIR/serial_pane-light.png" "$out/" 2>/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  kanna serial send esp binary >/dev/null || return 1
  check wait_for 5 eval 'ser text | grep -qF "\\x00\\x01\\x02\\xfe\\xffraw"' || { echo "serial_pane: bytes that aren't text: $(ser text | tail -2)"; return 1; }
  check test "$(ser hex ',"hex":true')" == True || return 1
  check wait_for 5 eval 'ser text | grep -q "fe ff 72 61 77"' || { echo "serial_pane: hex view: $(ser text | tail -3)"; return 1; }
  ser hex ',"hex":false' >/dev/null
  check test "$(ser paused ',"pause":true')" == True || return 1
  kanna serial send esp crash >/dev/null || return 1
  check wait_for 5 eval '[[ "$(ser unseen)" -gt 0 ]]' || { echo "serial_pane: paused, but no new lines counted"; return 1; }
  check test "$(ser following ',"follow":true')" == True || return 1
  check wait_for 5 eval 'ser text | grep -q "Guru Meditation"' || return 1
  stop_pid "$serial_device"; serial_device=""
  check wait_for 5 eval '[[ "$(ser state)" == unplugged && "$(ser send)" == False ]]' || { echo "serial_pane: unplugging left $(ser state)"; return 1; }
  check wait_for 5 eval 'ser status | grep -q "unplugged · waiting for 303a:1001"' || { echo "serial_pane: status $(ser status)"; return 1; }
  plug || return 1
  check wait_for 10 eval '[[ "$(ser state)" == connected ]]' || { echo "serial_pane: plugging back in left $(ser state)"; return 1; }
  check test "$(ser path)" == "$(cat "$devices/out")" || { echo "serial_pane: back on $(ser path)"; return 1; }
  check wait_for 5 eval 'ser text | grep -q "── back "' || return 1
  local rows; rows="$(ser rows)"
  stop_app && start_app || return 1
  check wait_for 10 eval '[[ "$(ser rows)" -ge "$rows" ]]' || { echo "serial_pane: after a relaunch the view has $(ser rows) rows, not $rows"; return 1; }
  kanna close esp >/dev/null || return 1
  check wait_for 5 eval 'kanna serial list | grep -q "serial-" && false || true' || return 1
  unset KANNA_SERIAL_PORTS
}

for s in ${only[@]+"${only[@]}"}; do due "$s" >/dev/null || { echo "e2e-kanna: no scenario named $s (see --help)" >&2; exit 2; }; done
if [[ ${#only[@]} -gt 0 ]]; then run=("${only[@]}"); else run=("${scenarios[@]%%:*}"); fi
passed=0 failed=0 limit="${KANNA_E2E_TIMEOUT:-600}"
for s in "${run[@]}"; do
  m="$(due "$s")"
  if [[ "$m" > "$through" ]]; then printf '%-20s --   not due (%s)\n' "$s" "$m"; continue; fi
  cleanup
  new_state_dir
  run_scenario "$s" >"$out/$s.log" 2>&1 &
  scenario_pid=$! ends=$((SECONDS + limit))
  echo "$scenario_pid" >"$KANNA_STATE_DIR/e2e-scenario.pid"
  while kill -0 "$scenario_pid" 2>/dev/null && [[ $SECONDS -lt $ends ]]; do sleep 0.2; done
  if kill -0 "$scenario_pid" 2>/dev/null; then
    echo "$s didn't finish in ${limit}s (KANNA_E2E_TIMEOUT)" >>"$out/$s.log"; result=1
  else
    wait "$scenario_pid"; result=$?; scenario_pid=""
  fi
  crashed="$(find "$KANNA_STATE_DIR" -path '*/crashes/*.json' 2>/dev/null)"
  if [[ -n "$crashed" ]]; then
    result=1
    while IFS= read -r record; do
      echo "a crash record appeared in $s's instance: $record" | tee -a "$out/$s.log"
      python3 -c 'import json, sys
r = json.load(open(sys.argv[1])); print("  " + r.get("process", "?") + ": " + r.get("reason", "?"))
for f in r.get("stack", [])[:12]: print("    " + f.get("image", "?") + "  " + f.get("symbol", "?"))' "$record" 2>/dev/null | tee -a "$out/$s.log"
    done <<<"$crashed"
  fi
  if [[ $result -eq 0 ]]; then
    printf '%-20s PASS (%s)\n' "$s" "$m"; passed=$((passed + 1))
  else
    printf '%-20s FAIL (%s)  %s\n' "$s" "$m" "$(tail -1 "$out/$s.log")"; failed=$((failed + 1))
  fi
done
echo "$passed passed, $failed failed"
[[ $failed -eq 0 ]]
