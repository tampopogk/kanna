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
  NAME…        only these scenarios (e.g. cli_terminal shelf_zoom)

A scenario that runs longer than $KANNA_E2E_TIMEOUT seconds (600) fails.
Every run stops what it started however it ends, and first stops what runs
it can't see anymore left behind (docs/specs/e2e-cleanup.md). Run it
unsandboxed: it needs ps, signals, Unix sockets and /tmp.

Uses target/release/{kanna,kanna-daemon,kanna-keeper,kanna-build} and target/Kanna.app (or $KANNA_E2E_APP)
(scripts/build.sh). The private instance gets:
  KANNA_SOCKET     the daemon's socket (the CLI and the app connect to it)
  KANNA_STATE_DIR  where the daemon, keeper and app keep their files
                   (the keeper writes keeper.pid there)
  KANNA_KEEPER     the keeper binary the daemon starts (once it uses one)
real_agent_tasks, real_agent_planners, real_agent_program_status and keeper_upgrade run the real claude and codex (on PATH,
or $KANNA_E2E_CLAUDE / $KANNA_E2E_CODEX) against tools/agent-harness's mock
model, each in a HOME of its own. A planning terminal
offers no agent (KANNA_PLANNING_AGENTS=none) unless a scenario gives it some. Backlog tasks
don't run by themselves (backlog.runners is 0) unless a scenario's repo sets it.
USAGE
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$repo_root/target/release"
app="${KANNA_E2E_APP:-$repo_root/target/Kanna.app}/Contents/MacOS/Kanna"
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
  export KANNA_AGENT_RULES_DIR="$KANNA_STATE_DIR/agent-rules-override"
  export KANNA_AGENTS_HOME="$KANNA_STATE_DIR/home"
  [[ -n "$KANNA_STATE_DIR" && -d "$KANNA_STATE_DIR" ]] || { echo "e2e-kanna: mktemp gave no folder" >&2; exit 1; }
  export KANNA_STATE_DIR KANNA_SOCKET="$KANNA_STATE_DIR/kanna.sock"
  mkdir -p "$KANNA_AGENTS_HOME/.kanna-v4" && echo '{"version": 2, "setup": false}' >"$KANNA_AGENTS_HOME/.kanna-v4/agents.json"
  printf '{"version": 1, "backlog": {"runners": 0}, "tasks": {"agent": "%s"}}\n' "$mock_agent" >"$KANNA_AGENTS_HOME/.kanna-v4/config.json"
}
KANNA_STATE_DIR=""
export KANNA_IGNORE_OCCLUSION=1
export KANNA_DEBUG=1
export KANNA_TEST_EFFECTS=1
export KANNA_NO_INITIAL_WINDOW=1
export KANNA_AGENT_RULES_URL=none
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
  for socket in "$KANNA_STATE_DIR/kanna.sock" "$KANNA_STATE_DIR/home/Library/Application Support"/kanna-dev*/kanna.sock; do
    [[ -S "$socket" ]] && echo "$socket"
  done
}
stop_instances() {
  [[ -n "$KANNA_STATE_DIR" && -d "$KANNA_STATE_DIR" ]] || return 0
  local socket pids=()
  while IFS= read -r socket; do
    env -u KANNA_INSTANCE KANNA_SOCKET="$socket" "$bin/kanna" instances stop >/dev/null 2>>"$out/stop.log" &
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
  trap 'stop_instances; stop_jobs' EXIT
  "$1"
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
  kanna open --name cli_terminal >/dev/null || return 1
  kanna send cli_terminal 'echo cli_terminal-$((40+2))' || return 1
  check wait_for 5 shows cli_terminal cli_terminal-42
}

scenario app_typing M0
app_typing() {
  start_daemon && start_app || return 1
  kanna open --name app_typing >/dev/null || return 1
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
  kanna open --name without_app --cmd 'echo without_app-READY; exec sleep 600' >/dev/null || return 1
  stop_app
  check wait_for 5 shows without_app without_app-READY || return 1
  check is without_app state running
}

scenario survives_relaunch M1
survives_relaunch() {
  start_daemon && start_app || return 1
  kanna open --name survives_relaunch --cmd 'printf "\033[2J\033[3;5Hsurvives_relaunch TOP\033[10;20Hsurvives_relaunch MIDDLE"; exec sleep 600' >/dev/null || return 1
  wait_for 5 shows survives_relaunch 'survives_relaunch MIDDLE' || { echo "survives_relaunch never drew"; return 1; }
  local pid before after
  pid="$(pane survives_relaunch pid)" before="$(kanna read survives_relaunch)"
  stop_app; start_app || return 1
  check is survives_relaunch pid "$pid" || return 1
  after="$(raw '{"id":1,"cmd":"debug.text","args":{"pane":"survives_relaunch"}}' | field text)"
  check test "$after" == "$before"
}

scenario layout_restored M1
layout_restored() {
  start_daemon && start_app || return 1
  kanna open --name left >/dev/null && kanna open --name right --split right --size 1/3 >/dev/null || return 1
  kanna open --name agent --dock >/dev/null && KANNA_TERMINAL=agent kanna open --name own --window new >/dev/null || return 1
  kanna task pull agent >/dev/null || return 1
  local before agent
  before="$(layout)" agent="$(pane agent id)"
  check test "$(ws agent windows)" == '["win-2"]' || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$agent" ]]' || return 1
  stop_app; start_app || return 1
  check test "$(layout)" == "$before" || return 1
  check wait_for 5 eval '[[ "$(ws agent windows)" == "[\"win-2\"]" && "$(ws agent where)" == "{\"shelf\": \"mac\"}" && "$(dwin d1 dock.pane)" == "$agent" ]]'
}

scenario daemon_restart M2
daemon_restart() {
  start_daemon && start_app || return 1
  kanna open --name daemon_restart >/dev/null || return 1
  kanna open --name daemon_restartb --split right >/dev/null || return 1
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
  kanna open --name exit_code --cmd 'exit 3' >/dev/null || return 1
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
  kanna open web --name web --history "file://$dir/web_md_ios_panes.html" >/dev/null || return 1
  kanna open md --name doc --split right "$dir/web_md_ios_panes.md" >/dev/null || return 1
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
  kanna open ios --name sim --app com.apple.Preferences --device "$udid" --tab >/dev/null || return 1
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
  folder="$("$bin/kanna-build" | sed -n 's/^folder=//p')"
  instance="$("$bin/kanna-build" | sed -n 's/^instance=//p')"
  [[ "$instance" == default ]] || folder="$folder-$instance"
  socket="$home/Library/Application Support/$folder/kanna.sock"
  mkdir -p "$(dirname "$socket")"
  KANNA_SOCKET="$socket" start_daemon || return 1
  dev() { env -u KANNA_SOCKET HOME="$home" KANNA_MUX=kanna-dev KANNA_APP="$repo_root/target/Kanna.app" KANNA_BG=1 "$bin/kanna" "$@"; }
  dev open --name left >/dev/null || return 1
  KANNA_SOCKET="$socket" "$bin/kanna" open --name right --split right >/dev/null || return 1
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
  kanna open --name live --split right --cmd 'echo LIVE-READY; exec cat' >/dev/null || return 1
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

scenario cli_commands M4
cli_commands() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR" expected
  cd "$dir" || return 1
  expected="opened term-1 (cli_commands) in win-1, tab-1: running"; says kanna open --name cli_commands --cmd 'printf "cli_commands READY\n"; exec cat' || return 1
  expected="opened term-2 (e2e) in win-1, tab-1: running"; says kanna open --name e2e --split right --size 1/3 || return 1
  check test "$(layout)" == '{"children": [{"pane": "cli_commands", "size": "2/3"}, {"pane": "e2e", "size": "1/3"}], "split": "row"}' || return 1
  check wait_for 5 app_text cli_commands "cli_commands READY" || return 1
  [[ "$(kanna workspace list --all)" == *$'Free windows\nwin-1\n  focus: e2e\n  *tab-1 "cli_commands"  cli_commands:2/3 | e2e:1/3'* ]] || { echo "workspace list --all should show the free window: $(kanna workspace list --all)"; return 1; }
  expected="focused cli_commands (key window win-1)"; says kanna focus cli_commands || return 1
  expected="e2e is zoomed (run again to unzoom)"; says kanna zoom e2e || return 1
  check test "$(kanna workspace list --all --json | field windows.0.zoomed)" == e2e || return 1
  expected="e2e is no longer zoomed"; says kanna zoom e2e || return 1
  expected="cli_commands:3/4 | e2e:1/4"; says kanna resize e2e 1/4 || return 1
  expected="cli_commands:1/2 / e2e:1/2"; says kanna move e2e --to cli_commands --side bottom || return 1
  expected="e2e:1/4 | cli_commands:3/4"; says kanna arrange '{"split":"row","children":[{"pane":"e2e","size":"1/4"},{"pane":"cli_commands"}]}' || return 1
  expected="opened term-3 (t) in win-1, tab-2: running"; says kanna open --tab --name t || return 1
  expected='tab-2 is now "shells"'; says kanna rename-tab tab-2 shells || return 1
  expected="win-1: tab-2, tab-1"; says kanna move-tab tab-2 --index 0 || return 1
  expected="switched to tab-1 (key window win-1)"; says kanna focus --tab tab-1 || return 1
  expected="t"; says kanna move t --window new || return 1
  check test "$(kanna workspace list --all --json | field windows.1.tabs.0.layout)" == '{"pane": "t"}' || return 1
  expected="key window is now win-1"; says kanna focus --window win-1 || return 1
  expected="closed win-2 (panes term-3)"; says kanna close --window win-2 || return 1
  kanna send e2e 'echo cli_commands-$((40+2))' || return 1
  check wait_for 5 shows e2e cli_commands-42 || return 1
  check wait_for 5 app_text e2e cli_commands-42 || return 1
  kanna send e2e -- echo --json || return 1
  check wait_for 5 eval 'kanna read e2e --lines 2 | grep -qx -- --json' || return 1
  expected="term-4 (cli_commands) (printf \"cli_commands READY\\n\"; exec cat) is running"; says kanna restart cli_commands || return 1
  printf '<title>cli_commands</title><p>cli_commands PAGE</p>\n' >"$dir/a.html"
  printf '<title>cli_commands B</title><p>cli_commands OTHER</p>\n' >"$dir/b.html"
  expected="opened page-1 (site) in win-1, tab-1: running"; says kanna open web "file://$dir/a.html" --name site --history --split down || return 1
  check wait_for 5 page_text site "cli_commands PAGE" || return 1
  expected="page-1 (site) (file://$dir/b.html) is running"; says kanna navigate site "file://$dir/b.html" || return 1
  check wait_for 5 page_text site "cli_commands OTHER" || return 1
  expected="page-1 (site) (file://$dir/a.html) is running"; says kanna navigate site --back || return 1
  check wait_for 5 page_text site "cli_commands PAGE" || return 1
  expected="page-1 (site) (file://$dir/b.html) is running"; says kanna navigate site --forward || return 1
  kanna open web "file://$dir/a.html" --name plain --tab >/dev/null || return 1
  exits 2 kanna navigate plain --back || return 1
  printf '# cli_commands DOC\n\ncli_commands TEXT\n' >"$dir/cli_commands.md"
  printf '# cli_commands NEXT\n' >"$dir/next.md"
  kanna open md cli_commands.md --name doc --tab >/dev/null || return 1
  check is doc path "$(pwd -P)/cli_commands.md" || return 1
  check wait_for 5 doc_text doc "cli_commands TEXT" || return 1
  kanna navigate doc next.md >/dev/null || return 1
  check wait_for 5 doc_text doc "cli_commands NEXT" || return 1
  kanna navigate doc --back >/dev/null || return 1
  check wait_for 5 doc_text doc "cli_commands TEXT" || return 1
  local missing
  missing="$(kanna open md "$dir/missing.md" --tab 2>&1)"; check test $? -eq 1 || return 1
  check grep -q "No such file" <<<"$missing" || return 1
  check test "$(kanna capabilities | tail -1)" == "Kanna.app: attached" || return 1
  check grep -q "The running Kanna supports every command." <(kanna help) || return 1
  check test "$(kanna open --json --name j --tab | field pane.name)" == j || return 1
  exits 4 kanna close nope || return 1
  exits 1 kanna open --name e2e || return 1
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
  expected="opened term-1 (cli_without_app) in win-1, tab-1: running"; says kanna open --name cli_without_app --cmd 'echo cli_without_app READY; exec cat' || return 1
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
open --name gone --split right
close gone
open --name doc --split right md $KANNA_STATE_DIR/cli_without_app.md
resize cli_without_app 2/3
open --tab --name other
rename-tab tab-1 kept
focus --tab tab-1
NEEDS
  exits 6 kanna raw '{"cmd":"debug.app"}' || return 1
  check grep -q "KANNA_SOCKET is set, so kanna doesn't start Kanna.app" <(kanna raw '{"cmd":"debug.app"}' 2>&1) || return 1
  start_app || return 1
  check test "$(layout)" == '{"children": [{"pane": "cli_without_app", "size": "2/3"}, {"pane": "doc", "size": "1/3"}], "split": "row"}' || return 1
  check test "$(win win-1 tabs.0.title)" == kept || return 1
  check wait_for 5 doc_text doc "cli_without_app DOC" || return 1
  expected="focused cli_without_app (key window win-1)"; says kanna focus cli_without_app || return 1
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
  check test "$(inst instances)" == "No Kanna instances are running. Any command starts one, e.g. kanna open or kanna --instance work open." || return 1
  inst --instance e2e --bg open --name instances --cmd 'echo "instances $KANNA_INSTANCE"; exec cat' >/dev/null || return 1
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
  inst --instance e2e open --name instancesb --split right >/dev/null || return 1
  check test "$(inst --instance e2e workspace list --all --json | field windows.0.tabs.0.layout)" == '{"children": [{"pane": "instances", "size": "1/2"}, {"pane": "instancesb", "size": "1/2"}], "split": "row"}' || return 1
  local daemon keeper
  daemon="$(cat "$folder/daemon.pid")" keeper="$(cat "$folder/keeper.pid")"
  check test "$(inst --instance e2e instances stop)" == "stopped e2e ($folder/kanna.sock)" || return 1
  check test ! -e "$folder/daemon.pid" || return 1
  check wait_for 5 eval '! kill -0 $daemon && ! kill -0 $keeper' || { echo "instances: the stopped instance's daemon or keeper is still running"; return 1; }
  check test "$(inst instances)" == "No Kanna instances are running. Any command starts one, e.g. kanna open or kanna --instance work open."
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
tid() { sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id from tasks order by rowid limit 1 offset $(($1 - 1))"; }
sid() { sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select id from tasks where place != 'done'" | python3 -c 'import sys
id, others = sys.argv[1], [o[5:] for o in sys.stdin.read().split() if o != sys.argv[1]]
n = next((n for n in range(4, 8) if not any(o[:n] == id[5:5 + n] for o in others)), 8)
print(id[:5 + n])' "$(tid "$1")"; }
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
click_tab() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"tab\":\"$1\",\"clicks\":${2:-1}}}" >/dev/null; sleep 0.3; }
drag() { last_drag="$1 → $(raw "{\"id\":1,\"cmd\":\"debug.drag\",\"args\":$1}")"; sleep 0.3; }

scenario windows_tabs_ui M4
windows_tabs_ui() {
  start_daemon && start_app || return 1
  kanna open --name k1 >/dev/null || return 1
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
  check drawn "$(snap win-1 windows_tabs_ui-zoom-term-3)" 1 || return 1
  key "cmd+]"; check test "$(win win-1 zoomed)" == k1 || return 1
  check drawn "$(snap win-1 windows_tabs_ui-zoom-k1)" 1 || return 1
  key cmd+shift+return; check test "$(win win-1 zoomed)" == None || return 1
  key "cmd+["; check test "$(win win-1 focused)" == term-3 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"k1"}}' >/dev/null; sleep 0.3
  check test "$(win win-1 focused)" == k1 || return 1
  sleep 1
  check drawn "$(snap win-1 windows_tabs_ui-splits)" 3 || return 1
  printf '# windows_tabs_ui DOC\n\nclick me\n' >"$KANNA_STATE_DIR/windows_tabs_ui.md"
  kanna open md --name doc --split right "$KANNA_STATE_DIR/windows_tabs_ui.md" >/dev/null || return 1
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
  kanna open web --name page --split right "file://$KANNA_STATE_DIR/windows_tabs_ui.html" >/dev/null || return 1
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
  check test "$(win win-2 key)" == True || return 1
  sleep 1
  check drawn "$(snap win-2 windows_tabs_ui-second-window)" 1 || return 1
  key 'cmd+`'; check test "$(win win-1 key)" == True || return 1
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

scenario tab_names M4
tab_names() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR"
  printf 'fn main() {}\n' >"$dir/tab_names.rs"
  printf '<title>tab_names PAGE</title><p>tab_names</p>\n' >"$dir/tab_names.html"
  printf '<title>tab_names NEXT</title><p>next</p>\n' >"$dir/tab_names_next.html"
  kanna open >/dev/null || return 1
  check test "$(win win-1 tabs.0.title)" == "Tab 1" || return 1
  kanna open code "$dir/tab_names.rs" --tab >/dev/null || return 1
  check test "$(win win-1 tabs.1.title)" == tab_names.rs || return 1
  [[ "$(kanna workspace list --all)" == *'"tab_names.rs"'* ]] || { echo "workspace list doesn't show the tab as tab_names.rs"; kanna workspace list --all; return 1; }
  kanna open web --name page "file://$dir/tab_names.html" --tab >/dev/null || return 1
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
  kanna open --name da >/dev/null && kanna open --name db --split right >/dev/null || return 1
  drag '{"divider":"da","at":0.26}'
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "da", "size": "1/4"}, {"pane": "db", "size": "3/4"}], "split": "row"}' || { echo "$last_drag"; return 1; }
  drag '{"divider":"da","at":0.6}'
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "da", "size": "3/5"}, {"pane": "db", "size": "2/5"}], "split": "row"}' || return 1
  drag '{"divider":"da","at":0.01}'
  local share
  share="$(win win-1 tabs.0.layout.children.0.size)"
  check python3 -c 'import sys; s = float(sys.argv[1].rstrip("%")); assert 5 < s < 12, s' "$share" || return 1
  drag '{"divider":"da","at":0.25}'
  kanna open --name wide >/dev/null || return 1
  check test "$(win win-1 tabs.0.layout)" == '{"children": [{"pane": "da", "size": "1/4"}, {"pane": "db", "size": "3/8"}, {"pane": "wide", "size": "3/8"}], "split": "row"}' || return 1
  kanna focus da >/dev/null && kanna open --name tall >/dev/null || return 1
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
  kanna open --name ended --cmd 'exit 3' --tab >/dev/null || return 1
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
  kanna open --name doc >/dev/null && kanna open --name mine --split right >/dev/null || return 1
  kanna open --name user --window new >/dev/null && kanna open --name open --split right >/dev/null || return 1
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
  kanna open --name da >/dev/null && kanna open --name db --split right >/dev/null || return 1
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
  kanna open --name dc --tab >/dev/null || return 1
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
  kanna open --name terminal_colours --cmd "$(probe '\033]11;?\007\033]10;?\007\033]4;1;?\007\033[?996n' n 1)" >/dev/null || return 1
  check wait_for 5 shows terminal_colours 'GOT ^]11;rgb:1212/3434/5656~^]10;rgb:e5e5/d4d4/b0b0~^]4;1;rgb:0101/0202/fefe~^\[?997;1n' || { kanna read terminal_colours; return 1; }
  check test "$(terminal settings.cursorStyle '{"pane":"terminal_colours"}')" == bar || return 1
  local drawn
  drawn="$(raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"terminal_colours.png"}}' | field panes.0.background)"
  cp "$KANNA_STATE_DIR/terminal_colours.png" "$out/terminal_colours-dark.png" 2>/dev/null
  check near "$drawn" '#123456' || return 1
  kanna open --name terminal_coloursb --cmd "$(probe '\033[?2031h' n 1)" >/dev/null || return 1
  check wait_for 5 shows terminal_coloursb READY || return 1
  terminal background '{"pane":"terminal_colours","appearance":"light"}' >/dev/null
  check wait_for 5 shows terminal_coloursb 'GOT ^\[?997;2n' || return 1
  check wait_for 5 eval '[[ "$(terminal settings.background "{\"pane\":\"terminal_colours\"}")" == "#f0f0e0" ]]' || return 1
  kanna open --name terminal_coloursc --cmd "$(probe '\033]11;?\007')" >/dev/null || return 1
  check wait_for 5 shows terminal_coloursc 'GOT ^]11;rgb:f0f0/f0f0/e0e0~' || return 1
  sleep 0.5
  drawn="$(raw '{"id":1,"cmd":"debug.snapshot","args":{"path":"terminal_colours-light.png"}}' | field panes.0.background)"
  cp "$KANNA_STATE_DIR/terminal_colours-light.png" "$out/terminal_colours-light.png" 2>/dev/null
  check near "$drawn" '#f0f0e0'
}

scenario cwd_title M4
cwd_title() {
  start_daemon && start_app || return 1
  kanna open --name cwd_title --cmd "printf '\033]7;file://localhost/private/tmp\007\033]2;cwd_title TITLE\007cwd_title-READY'; exec cat" >/dev/null || return 1
  check wait_for 5 shows cwd_title cwd_title-READY || return 1
  check wait_for 5 is cwd_title cwd /private/tmp || return 1
  check wait_for 5 eval '[[ "$(terminal title "{\"pane\":\"cwd_title\"}")" == "cwd_title TITLE" ]]' || return 1
  stop_app
  kanna open --name cwd_titleb --cmd "printf '\033]7;file://localhost/usr\007'; exec cat" >/dev/null || return 1
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
  kanna open --name clipboard --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows clipboard READY || return 1
  terminal clipboard '{"pane":"clipboard","clipboard":"clipboard PASTE"}' >/dev/null || return 1
  raw '{"id":1,"cmd":"debug.key","args":{"pane":"clipboard","key":"cmd+v"}}' >/dev/null
  check wait_for 5 eval 'kanna read clipboard | tr -d "\\n" | sed "s/IN //g" | grep -q "\\^\\[200~clipboard PASTE\\^\\[201~"' || { kanna read clipboard; return 1; }
  kanna open --name clipboardw --cmd "printf '\033]52;c;Y2xpcGJvYXJkIENPUElFRA==\007clipboardW'; exec cat" >/dev/null || return 1
  check wait_for 5 shows clipboardw clipboardW || return 1
  check wait_for 5 eval '[[ "$(terminal clipboard "{\"pane\":\"clipboard\"}")" == "clipboard COPIED" ]]' || return 1
  kanna open --name clipboardr --cmd "$(probe '\033]52;c;?\007' '\\' 1)" >/dev/null || return 1
  check wait_for 5 shows clipboardr 'GOT ^]52;c;Y2xpcGJvYXJkIENPUElFRA==^' || return 1
  kanna open --name clipboards --cmd "printf 'clipboard SELECTED'; exec cat" >/dev/null || return 1
  check wait_for 5 shows clipboards 'clipboard SELECTED' || return 1
  terminal selection '{"pane":"clipboards","action":"select_all"}' >/dev/null
  terminal clipboard '{"pane":"clipboards","action":"copy_to_clipboard"}' >/dev/null
  check wait_for 5 eval 'terminal clipboard "{\"pane\":\"clipboards\"}" | grep -q "clipboard SELECTED"' || return 1
  stop_app; term_app || return 1
  terminal clipboard '{"pane":"clipboard","clipboard":"AFTER"}' >/dev/null || return 1
  raw '{"id":1,"cmd":"debug.key","args":{"pane":"clipboard","key":"cmd+v"}}' >/dev/null
  check wait_for 5 eval 'kanna read clipboard | tr -d "\\n" | sed "s/IN //g" | grep -q "\\^\\[200~AFTER\\^\\[201~"' || return 1
  kanna open --name clipboardd --dock --cmd "$(probe '\033[?2004h' x 0)" >/dev/null && kanna task pull clipboardd >/dev/null || return 1
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
  kanna open --name sysclip --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
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
  kanna open --name mouse_keys_focus --cmd "$(probe '\033[>1u\033[?1000h\033[?1006h\033[?1004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows mouse_keys_focus READY || return 1
  stop_app; start_app || return 1
  raw '{"id":1,"cmd":"debug.key","args":{"pane":"mouse_keys_focus","key":"shift+return"}}' >/dev/null
  check wait_for 5 shows mouse_keys_focus 'IN ^\[13;2u' || return 1
  terminal cols '{"pane":"mouse_keys_focus","click":{"at":[5,5]}}' >/dev/null || return 1
  check wait_for 5 shows mouse_keys_focus 'IN ^\[<0;1;1M' || return 1
  check wait_for 5 shows mouse_keys_focus '\[<0;1;1m' || return 1
  kanna open --name mouse_keys_focusb --split right >/dev/null || return 1
  check wait_for 5 eval '[[ "$(kanna read mouse_keys_focus | grep -oE "IN \^\[[IO]" | tail -1)" == "IN ^[O" ]]' || return 1
  kanna focus mouse_keys_focus >/dev/null || return 1
  check wait_for 5 eval '[[ "$(kanna read mouse_keys_focus | grep -oE "IN \^\[[IO]" | tail -1)" == "IN ^[I" ]]'
}

scenario font_size_scrollback M4
font_size_scrollback() {
  start_daemon && start_app || return 1
  kanna open --name font_size_scrollback >/dev/null || return 1
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
  kanna open --name links_bell_notify --cmd "sh '$KANNA_STATE_DIR/links.sh'; printf '\007\033]9;links_bell_notify NOTE\007\033]9;4;1;40\007'; sleep 1.5; printf '\033]777;notify;links_bell_notify T;links_bell_notify B\007links_bell_notify-READY'; exec cat" >/dev/null || return 1
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
  kanna open --name chrome_and_drops --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
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
  kanna open --name agent --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows agent READY || return 1
  local dir="$KANNA_STATE_DIR/doc" fixture="$repo_root/references/prototypes/doc-markup/fixture" agent_id at xy
  mkdir -p "$dir/.git" "$dir/.kanna/docs"
  cp "$fixture/review.md" "$dir/review.md"
  sed "s|FILE|$dir/review.md|" "$fixture/review.threads.json" >"$dir/.kanna/docs/review.md.json"
  agent_id="$(pane agent id)"
  KANNA_TERMINAL="$agent_id" kanna open md --name doc --tab "$dir/review.md" >/dev/null || return 1
  md_comments_md() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"doc\"${1:-}}}"; }
  check wait_for 5 eval '[[ "$(md_comments_md | field markup)" == False && "$(md_comments_md | field agent)" == None ]]' || { echo "open md should show the file without comments"; md_comments_md; return 1; }
  KANNA_TERMINAL="$agent_id" kanna markup doc | grep -qF "comments on in" || return 1
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
  kanna markup doc --off | grep -qF "comments off in" || return 1
  check wait_for 5 eval '[[ "$(md_comments_md | field markup)" == False ]]' || return 1
  KANNA_TERMINAL="$agent_id" kanna markup "$dir/review.md" --name doc2 --tab >/dev/null || return 1
  check wait_for 5 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"doc2\"}}" | field markup)" == True ]]'
}

scenario docked_pane M4
docked_pane() {
  start_daemon && start_app || return 1
  local opened refused shot first agent dt1 dt2 dt3
  opened="$(kanna open --name agent --dock --cmd "printf 'agent: zoom as a mode\n'; exec sleep 600")"
  [[ "$opened" == *"(agent) docked left in ws-"*" (on the Shelf): running" ]] || { echo "open --dock should say where it docked: $opened"; return 1; }
  agent="$(pane agent id)"
  KANNA_TERMINAL=agent kanna open --name dt1 --cmd "printf 'test result: ok. 41 passed; 0 failed\n'; exec sleep 600" >/dev/null || return 1
  KANNA_TERMINAL=agent kanna open --name dt2 --cmd "printf 'dev server on http://localhost:3000\n'; exec sleep 600" >/dev/null || return 1
  dt1="$(pane dt1 id)" dt2="$(pane dt2 id)"
  kanna rename-tab "$(ws agent tabs.0.id)" tests >/dev/null && kanna rename-tab "$(ws agent tabs.1.id)" server >/dev/null || return 1
  check test "$(ws agent dock.edge):$(ws agent dock.width)" == "left:2/5" || return 1
  [[ "$(kanna workspace list --all)" == *"dock: $agent (agent)  left 2/5"* ]] || { echo "workspace list should show the dock on its own line: $(kanna workspace list --all)"; return 1; }
  kanna task pull agent >/dev/null || return 1
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
  key "cmd+]"; check test "$(dwin d1 zoomed)" == "$dt2" || return 1
  check test "$(field panes <<<"$(snap d1 docked_pane-zoomed-server)" | count)" == 1 || return 1
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
  check python3 -c 'import json, sys; s = json.loads(sys.argv[1]); assert s["panes"][0]["width"] == s["width"], s' "$shot" || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == None || return 1
  sleep 0.5
  check drawn "$(snap d1 docked_pane-tab-unzoomed)" 3 || return 1
  kanna close dt3 >/dev/null || return 1
  kanna move agent --dock right >/dev/null || return 1
  check test "$(ws agent dock.edge)" == right || return 1
  drag '{"divider":"agent","at":0.34}'
  check python3 -c 'import sys; assert abs(float(sys.argv[1]) - 1/3) < 0.001, sys.argv[1]' "$(dwin d1 dock.width)" || { echo "$last_drag"; return 1; }
  check test "$(ws agent dock.width)" == 2/5 || return 1
  kanna resize agent 1/2 >/dev/null || return 1
  check test "$(dwin d1 dock.width)" == 0.5 || return 1
  drag '{"divider":"agent","at":0.34}'
  sleep 0.5
  check drawn "$(snap d1 docked_pane-right)" 2 || return 1
  first="$(dwin d1 tabs.0.id)"
  kanna move-tab "$first" --window new >/dev/null || return 1
  check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  check test "$(dwin d1 tabs | count)" == 1 || return 1
  check test "$(ws agent windows | count)" == 1 || { echo "a tab torn off a task's workspace should make one of its windows"; return 1; }
  refused="$(kanna move dt2 --dock 2>&1)" && { echo "docking a pane that isn't a task should fail"; return 1; }
  [[ "$refused" == *"dt2 isn't a task, and only a task's agent docks"* ]] || { echo "docking a pane that isn't a task should say why: $refused"; return 1; }
  kanna task done agent >/dev/null || return 1
  check test "$(kanna workspace list --all --json | field workspaces)" == "[]" || return 1
  opened="$(kanna open --name agent --dock --frozen --cmd "printf 'agent: zoom as a mode\n'; exec sleep 600")" || return 1
  [[ "$opened" == *"(agent) docked left, frozen, in ws-"*" (on the Shelf): running" ]] || { echo "open --dock --frozen should say it's frozen: $opened"; return 1; }
  agent="$(pane agent id)"
  [[ "$(kanna workspace list --all)" == *"(agent)  left 2/5  frozen"* ]] || { echo "workspace list should show the dock frozen: $(kanna workspace list --all)"; return 1; }
  KANNA_TERMINAL=agent kanna open --name dt4 --cmd "exec sleep 600" >/dev/null || return 1
  kanna task pull agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$agent" ]]' || return 1
  refused="$(kanna move agent --dock right 2>&1)" && { echo "moving a frozen docked pane should fail"; return 1; }
  [[ "$refused" == *"frozen: agent ("*") is frozen in its dock and doesn't move"* ]] || { echo "moving a frozen docked pane should say why: $refused"; return 1; }
  refused="$(kanna move dt4 --to agent 2>&1)" && { echo "swapping onto a frozen docked pane should fail"; return 1; }
  [[ "$refused" == *"is frozen in ws-"*"'s dock; nothing drops on it"* ]] || { echo "swapping onto a frozen docked pane should say why: $refused"; return 1; }
  check test "$(raw '{"id":1,"cmd":"debug.drag","args":{"pane":"dt4","to":{"pane":"agent","x":0.5,"y":0.5}}}' | field dropped)" == False || return 1
  check test "$(raw '{"id":1,"cmd":"debug.drag","args":{"pane":"agent","to":{"pane":"dt4","x":0.5,"y":0.5}}}' | field dropped)" == False || return 1
  check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  kanna close dt4 >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs)" == "[]" ]]' || return 1
  check test "$(dwin d1 dock.pane)" == "$agent" || return 1
  sleep 0.5
  check drawn "$(snap d1 docked_pane-only-dock)" 1 || return 1
  kanna focus agent >/dev/null || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == "$agent" || return 1
  sleep 0.5
  shot="$(snap d1 docked_pane-only-dock-zoomed)"
  check drawn "$shot" 1 || return 1
  check python3 -c 'import json, sys; s = json.loads(sys.argv[1]); assert s["panes"][0]["width"] == s["width"], s' "$shot" || return 1
  key cmd+shift+return; check test "$(dwin d1 zoomed)" == None || return 1
  kanna task done agent >/dev/null || return 1
  [[ "$(kanna workspace list --all)" != *agent* ]] || { echo "done should close the docked pane"; return 1; }

  local before after
  kanna open --name back --dock right --cmd "printf 'DOCK BACK\n'; exec sleep 600" >/dev/null || return 1
  KANNA_TERMINAL=back kanna open --name tab1 >/dev/null || return 1
  kanna resize back 1/4 >/dev/null || return 1
  kanna open --name lone --dock >/dev/null || return 1
  kanna task pull back >/dev/null || return 1
  sleep 1.5
  check python3 -c 'import json, sqlite3, sys
saved = json.loads(sqlite3.connect(sys.argv[1]).execute("select doc from layout").fetchone()[0])
docks = {s["dock"]["pane"]: s for s in saved["workspaces"]}
back, lone = docks[sys.argv[2]], docks[sys.argv[3]]
assert back["dock"]["edge"] == "right" and back["dock"]["width"] == 0.25, back
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

scenario page_comments M4
page_comments() {
  start_daemon && start_app || return 1
  kanna open --name agent --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows agent READY || return 1
  local dir="$KANNA_STATE_DIR/page" agent_id xy mark
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
  kanna open --name agent_statusshell >/dev/null || return 1
  kanna open --name agent_status --tab --cmd "$repo_root/tools/mock-agent/mock-agent" >/dev/null || return 1
  agent_status_status() { raw '{"id":1,"cmd":"debug.status"}' | field "panes.$(pane agent_status id).$1"; }
  check wait_for 5 is agent_status status idle || return 1
  check test "$(pane agent_statusshell status)" == None || return 1
  kanna focus agent_statusshell >/dev/null || return 1
  kanna send agent_status turn || return 1
  check wait_for 5 is agent_status status working || return 1
  check wait_for 10 is agent_status status needs-you || return 1
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys; print(next(p.get("status") for p in json.load(sys.stdin)["panes"] if p["name"] == "agent_status"))')" == needs-you || return 1
  kanna status explain agent_status | grep -qF 'rule: mock/needs-you/proceed' || { kanna status explain agent_status; return 1; }
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
  kanna open --name zsh_completion --cwd "$dir" --cmd "zsh -f -i" >/dev/null || return 1
  kanna open --name zsh_completion-web --tab web "$dir/alpha.html" >/dev/null || return 1
  kanna open --name notes --tab md "$dir/alpha-notes.md" >/dev/null || return 1
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
  kanna open --name zsh_completion-login --tab --cwd "$dir" >/dev/null || return 1
  check wait_for 10 eval '[[ -n "$(kanna read zsh_completion-login | tr -d "[:space:]")" ]]' || return 1
  sleep 2
  kanna send zsh_completion-login 'print -r -- "zsh_completion:$_comps[kanna]:${ZDOTDIR-home}:${KANNA_ZSH_ZDOTDIR-gone}"' || return 1
  check wait_for 5 shows zsh_completion-login 'zsh_completion:_kanna:home:gone'
}

scenario dock_keeps_focus_through_tabs M4
dock_keeps_focus_through_tabs() {
  start_daemon && start_app || return 1
  local agent
  kanna open --name agent --dock --cmd 'exec cat' >/dev/null || return 1
  KANNA_TERMINAL=agent kanna open --name spec --cmd "printf 'spec\n'; exec sleep 600" >/dev/null || return 1
  KANNA_TERMINAL=agent kanna open --name plan --cmd "printf 'plan\n'; exec sleep 600" >/dev/null || return 1
  agent="$(pane agent id)"
  kanna task pull agent >/dev/null || return 1
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

scenario desk_shelf M4
desk_shelf() {
  start_daemon && start_app || return 1
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"${2:-d1}\"}}" | field "$1"; }
  app_windows() { app_view | python3 -c 'import json, sys; print(" ".join(w["id"] for w in json.load(sys.stdin)["windows"]))'; }
  kanna open --name agent --dock --cwd "$repo_root" >/dev/null || return 1
  KANNA_TERMINAL=term-1 kanna open --name notes >/dev/null || return 1
  check test "$(app_windows)" == "" || { echo "desk_shelf: a task on the Shelf showed a window"; app_view; return 1; }
  expected="pulled term-1 onto d1"; says kanna task pull agent || return 1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || return 1
  check test "$(desk desk)" == True || return 1
  check test "$(desk strip)" == True || return 1
  check test "$(desk title)" == agent || return 1
  check wait_for 5 eval '[[ "$(desk header)" == "${repo_root/#$HOME/~} · $(git -C "$repo_root" branch --show-current)" ]]' || { desk header; return 1; }
  expected="parked term-1; d1 is an empty Desk"; says kanna task park agent || return 1
  check test "$(desk title)" == Shelf || return 1
  check test "$(desk empty)" == True || return 1
  check test "$(desk page)" == True || return 1
  check test "$(desk shelf.0.task)" == term-1 || return 1
  check test "$(dwin d1 dock)" == None || return 1
  sleep 0.5; snap d1 desk_shelf-empty >/dev/null
  kanna send notes 'echo PARKED-$((6*7))' >/dev/null || return 1
  check wait_for 5 shows notes PARKED-42 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"term-1","window":"d1"}}' >/dev/null; sleep 0.3
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || return 1
  check test "$(dwin d1 tabs.0.panes.0)" == "$(pane notes id)" || return 1
  check test "$(desk empty)" == False || return 1
  local cheer; cheer="$(desk cheer)"; [[ -n "$cheer" ]] || return 1
  kanna task park >/dev/null || return 1
  check wait_for 5 eval '[[ "$(desk empty)" == True && "$(desk cheer)" != "$cheer" ]]' || { desk cheer; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(desk shelf.0.task)" == term-1 ]]' || { echo "desk_shelf: the Desk and its Shelf didn't survive the app's relaunch"; app_view; return 1; }
  check test "$(pane notes state)" == running || return 1
  expected="pulled term-1 onto d1"; says kanna task pull term-1 || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs.0.panes.0)" == "$(pane notes id)" ]]' || return 1
  sleep 0.5; check drawn "$(snap d1 desk_shelf-desk)" 2 || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"park":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk empty)" == True ]]' || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"term-1","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || return 1
  kanna open --name second --dock --cwd "$repo_root" >/dev/null || return 1
  key cmd+n
  check wait_for 5 eval '[[ "$(dwin d2 key)" == True ]]' || { app_view; return 1; }
  kanna task pull second >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d2 dock.pane)" == "$(pane second id)" ]]' || return 1
  expected="term-1 is on d1; brought it forward"; says kanna task pull agent || return 1
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || return 1
  kanna task park agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(desk empty)" == True ]]' || return 1
  key cmd+1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || return 1
  check test "$(dwin d2 dock.pane)" == "$(pane second id)" || { echo "desk_shelf: one Desk's pull moved another's task"; return 1; }
  key cmd+.
  check wait_for 5 eval '[[ "$(desk empty)" == True ]]' || return 1
  key cmd+.
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || return 1
  kanna open --name asker --dock --cwd "$repo_root" --cmd "$repo_root/tools/mock-agent/mock-agent" >/dev/null || return 1
  check wait_for 5 is asker status idle || return 1
  kanna send asker ask || return 1
  check wait_for 5 is asker status needs-you || return 1
  check wait_for 5 eval '[[ "$(desk shelf.0.task)" == "$(pane asker id)" ]]' || { desk shelf; return 1; }
  key cmd+shift+a
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$(pane asker id)" ]]' || return 1
  key cmd+1
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == term-1 ]]' || return 1
  kanna task done asker >/dev/null || return 1
  KANNA_TERMINAL=term-1 kanna open --name side >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs.1.panes.0)" == "$(pane side id)" ]]' || return 1
  kanna task park agent >/dev/null || return 1
  KANNA_TERMINAL=term-1 kanna open --name later >/dev/null || return 1
  check test "$(dwin d1 tabs)" == "[]" || return 1
  kanna task pull agent >/dev/null || return 1
  check wait_for 5 eval '[[ "$(dwin d1 tabs.2.panes.0)" == "$(pane later id)" ]]' || return 1
  local own loose task; task="$(ws agent id)"
  own="$(KANNA_TERMINAL=term-1 kanna open --name own --window new)" || return 1
  [[ "$own" =~ ^"opened "term-[0-9]+" (own) in $task's "(win-[0-9]+)" (on the Shelf), tab-" ]] || { echo "desk_shelf: a task's open --window new: $own"; return 1; }
  own="${BASH_REMATCH[1]}"
  check test "$(win "$own" workspace)" == "$task" || return 1
  loose="$(kanna open --name loose --window new)" || return 1
  [[ "$loose" =~ " in "(win-[0-9]+)", tab-" ]] || { echo "desk_shelf: $loose"; return 1; }
  loose="${BASH_REMATCH[1]}"
  expected="assigned $loose to $task (on the Shelf)"; says kanna window assign "$loose" agent || return 1
  check wait_for 5 eval '[[ " $(app_windows) " == *" $own "* && " $(app_windows) " == *" $loose "* ]]' || { echo "desk_shelf: a task's windows didn't show with it on a Desk"; app_windows; return 1; }
  kanna task park agent >/dev/null || return 1
  check wait_for 5 eval '[[ " $(app_windows) " != *" $own "* && " $(app_windows) " != *" $loose "* ]]' || { echo "desk_shelf: a parked task's windows didn't close"; app_windows; return 1; }
  check test "$(ws "$task" windows)" == "[\"$own\", \"$loose\"]" || return 1
  check is own state running || return 1
  [[ "$(kanna open --name hidden --window "$own" --tab)" == "opened "*" (hidden) in $task's $own (on the Shelf), tab-"* ]] || { echo "desk_shelf: an open into a hidden window"; return 1; }
  kanna task pull agent >/dev/null || return 1
  check wait_for 5 eval '[[ " $(app_windows) " == *" $own "* && " $(app_windows) " == *" $loose "* ]]' || { echo "desk_shelf: a pulled task's windows didn't open"; app_windows; return 1; }
  check test "$(win "$own" tabs.1.layout.pane)" == hidden || return 1
  expected="$loose is free (out of $task)"; says kanna window free "$loose" || return 1
  kanna task park agent >/dev/null || return 1
  check wait_for 5 eval '[[ " $(app_windows) " != *" $own "* && " $(app_windows) " == *" $loose "* ]]' || { echo "desk_shelf: a freed window closed with the task"; app_windows; return 1; }
  kanna task pull agent >/dev/null || return 1
  kanna close --window "$own" >/dev/null && kanna close --window "$loose" >/dev/null || return 1
  check close_desk d1 || return 1
  check wait_for 5 eval 'kanna task list | grep -q "^term-1 .*Shelf"' || { kanna task list; return 1; }
  check test "$(pane agent state)" == running || return 1
  check test "$(pane notes state)" == running || return 1
  kanna open --name ta --dock --cwd "$repo_root/docs" >/dev/null || return 1
  kanna open --name tc --dock --cwd "$repo_root" 2>"$KANNA_STATE_DIR/desk_shelf-limit" && return 1
  grep -q "has 3 tasks on the Shelf already" "$KANNA_STATE_DIR/desk_shelf-limit" || { cat "$KANNA_STATE_DIR/desk_shelf-limit"; return 1; }
  kanna open --name elsewhere --dock --cwd "$KANNA_STATE_DIR" >/dev/null || return 1
  KANNA_TERMINAL=ta kanna open --name ta-notes >/dev/null || return 1
  expected="ta is done: closed $(pane ta id), $(pane ta-notes id)"; says kanna task done ta || return 1
  expected="agent is done: closed term-1, $(pane notes id), $(pane side id), $(pane later id)"; says kanna task done agent || return 1
  kanna task list | grep -q "^term-1 " && return 1
  kanna open --name tc --dock --cwd "$repo_root" >/dev/null || return 1
}

scenario task_backlog M4
task_backlog() {
  start_daemon && start_app || { echo "task_backlog line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_backlog line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  local added; added="$(in_repo task new "Fix it" "say hi" --agent "$agent" 2>&1)"
  [[ "$added" =~ ^"added "(task-[0-9a-f]{4,8})" \"Fix it\" to the backlog of $repo"$ && "${BASH_REMATCH[1]}" == "$(sid 1)" ]] || { echo "task_backlog line $LINENO: $added"; return 1; }
  in_repo task new "Plain" >/dev/null || { echo "task_backlog line $LINENO"; return 1; }
  [[ "$(cd "$KANNA_STATE_DIR" && kanna task new "Nowhere" 2>&1)" == *"belongs to a git repo"* ]] || { echo "task_backlog line $LINENO"; return 1; }
  in_repo task list | grep -qE '^'"$(sid 1)"' +Fix it +Backlog +stopped +mock-agent +made' || { in_repo task list; return 1; }
  expected="took $(sid 2) off the backlog"; says kanna task done $(tid 2) || { echo "task_backlog line $LINENO"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "task_backlog line $LINENO"; return 1; }
  in_repo task list | grep -qE '^'"$(sid 1)"' +Fix it +Backlog' || { echo "the backlog didn't survive a restart"; in_repo task list; return 1; }
  local started wt; started="$(kanna task shelve $(tid 1))" || { echo "task_backlog line $LINENO"; return 1; }
  [[ "$started" =~ ^"moved $(sid 1) to the Shelf: term-1 in $repo/.kanna-worktrees/"(task-[0-9a-f]{8})" ("(task-[0-9a-f]{8})")"$ && "${BASH_REMATCH[1]}" == "${BASH_REMATCH[2]}" ]] || { echo "task_backlog line $LINENO: $started"; return 1; }
  wt="$repo/.kanna-worktrees/${BASH_REMATCH[1]}"
  check wait_for 5 eval '[[ "$(pane "Fix it" id)" == term-1 ]]' || { echo "task_backlog line $LINENO"; return 1; }
  check test "$(git -C "$wt" branch --show-current)" == "${wt##*/}" || { echo "task_backlog line $LINENO"; return 1; }
  check test -z "$(git -C "$repo" status --porcelain)" || { echo "task_backlog line $LINENO"; return 1; }
  in_repo task list | grep -qE '^'"$(sid 1)"' +Fix it +Shelf' || { in_repo task list; return 1; }
  check wait_for 5 is "Fix it" status idle || { echo "task_backlog line $LINENO"; return 1; }
  expected="named $(sid 1) \"Fixed\""; says kanna task name term-1 Fixed || { echo "task_backlog line $LINENO"; return 1; }
  for n in 3 4 5; do in_repo task new "More $n" --agent "$agent" >/dev/null || return 1; done
  kanna task shelve $(tid 3) >/dev/null && kanna task shelve $(tid 4) >/dev/null || { echo "task_backlog line $LINENO"; return 1; }
  kanna task shelve $(tid 5) 2>"$KANNA_STATE_DIR/task_backlog-limit" && { echo "task_backlog line $LINENO"; return 1; }
  grep -q "has 3 tasks on the Shelf" "$KANNA_STATE_DIR/task_backlog-limit" || { cat "$KANNA_STATE_DIR/task_backlog-limit"; return 1; }
  in_repo task list | grep -q '^'"$(sid 5)"' .*Backlog' || { echo "task_backlog line $LINENO"; return 1; }
  check test "$(ls "$repo/.kanna-worktrees" | wc -l | tr -d ' ')" == 3 || { echo "task_backlog line $LINENO"; return 1; }
  in_repo open --name plain >/dev/null || { echo "task_backlog line $LINENO"; return 1; }
  in_repo move plain --dock 2>"$KANNA_STATE_DIR/task_backlog-dock" && { echo "task_backlog line $LINENO"; return 1; }
  grep -q "plain isn't a task, and only a task's agent docks" "$KANNA_STATE_DIR/task_backlog-dock" || { cat "$KANNA_STATE_DIR/task_backlog-dock"; return 1; }
  in_repo task list | grep -q ' plain$' && { echo "task_backlog line $LINENO"; return 1; }
  kanna task pull Fixed >/dev/null || { echo "task_backlog line $LINENO"; return 1; }
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys; print([w["dock"].get("frozen") for w in json.load(sys.stdin)["workspaces"] if w["dock"]["id"] == "term-1"])')" == '[True]' || { kanna workspace list --all; return 1; }
  kanna move Fixed --dock right 2>"$KANNA_STATE_DIR/task_backlog-frozen" && { echo "task_backlog line $LINENO"; return 1; }
  grep -q "frozen: Fixed (term-1) is frozen in its dock and doesn't move" "$KANNA_STATE_DIR/task_backlog-frozen" || { cat "$KANNA_STATE_DIR/task_backlog-frozen"; return 1; }
  kanna move plain --to Fixed 2>"$KANNA_STATE_DIR/task_backlog-swap" && { echo "task_backlog line $LINENO"; return 1; }
  grep -q "Fixed (term-1) is frozen in .*'s dock; nothing drops on it" "$KANNA_STATE_DIR/task_backlog-swap" || { cat "$KANNA_STATE_DIR/task_backlog-swap"; return 1; }
  in_repo task list | grep -qE '^'"$(sid 1)"' +Fixed ' || { in_repo task list; return 1; }
  kanna close plain >/dev/null || { echo "task_backlog line $LINENO"; return 1; }
  kanna task done Fixed >/dev/null || { echo "task_backlog line $LINENO"; return 1; }
  kanna task shelve $(tid 5) >/dev/null || { echo "task_backlog line $LINENO"; return 1; }
  check wait_for 10 eval '[[ ! -e "$wt" ]]' || { echo "task_backlog: the done task's worktree, merged and clean, was kept"; return 1; }
}

scenario desk_backlog M4
desk_backlog() {
  local repo="$KANNA_STATE_DIR/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_backlog line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  mkdir -p "$repo/.kanna" && printf '{"planning": {"context": 400000}}\n' >"$repo/.kanna/config.json"
  rm "$KANNA_AGENTS_HOME/.kanna-v4/agents.json"
  start_daemon || { echo "desk_backlog line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"${2:-d1}\"}}" | field "$1"; }
  local planner; check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { desk lead; kanna workspace list --all; return 1; }
  planner="$(desk lead)"
  check test "$(desk page)" == True || { echo "desk_backlog line $LINENO"; return 1; }
  check test "$(desk strip)" == False || { echo "desk_backlog line $LINENO"; return 1; }
  check wait_for 10 shows "$planner" "Install Kanna's skills? \[Y/n\]" || { echo "desk_backlog: the planning terminal didn't ask to install Kanna's skills"; kanna read "$planner"; return 1; }
  check shows "$planner" "Kanna needs its skills installed" || { kanna read "$planner"; return 1; }
  kanna send "$planner" "y" >/dev/null || { echo "desk_backlog line $LINENO"; return 1; }
  check wait_for 10 shows "$planner" "Folder, repo or name:" || { echo "desk_backlog: the planning terminal in ~ didn't ask where"; kanna read "$planner"; return 1; }
  check shows "$planner" "Kanna's skills are installed" || { kanna read "$planner"; return 1; }
  check test -f "$KANNA_AGENTS_HOME/.agents/skills/kanna-tasks/SKILL.md" || { echo "desk_backlog: answering yes didn't install the skills"; return 1; }
  check grep -q "$KANNA_SOCKET" "$KANNA_AGENTS_HOME/.claude/settings.json" || return 1
  ! grep -q SessionStart "$KANNA_AGENTS_HOME/.claude/settings.json" || { echo "desk_backlog: setup installed a SessionStart hook"; return 1; }
  check test "$(cd "$(pane "$planner" cwd)" && pwd -P)" == "$(cd ~ && pwd -P)" || { echo "desk_backlog: Kanna started from no folder should plan in ~"; return 1; }
  check test "$(kanna task list | grep -c "$planner")" == 0 || { echo "desk_backlog: the planning terminal is listed as a task"; kanna task list; return 1; }
  kanna send "$planner" "$repo" >/dev/null || { echo "desk_backlog line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk header)" == "Planning · "*"${repo#/private} · main" ]]' || { desk header; return 1; }
  check wait_for 5 shows "$planner" "No agent Kanna knows is installed" || { kanna read "$planner"; return 1; }
  kanna send "$planner" 'echo "planner=${KANNA_PLANNER-none} context=$CLAUDE_CODE_AUTO_COMPACT_WINDOW"' >/dev/null || { echo "desk_backlog line $LINENO"; return 1; }
  check wait_for 5 shows "$planner" "planner=none context=300000" || { kanna read "$planner"; return 1; }
  (cd "$repo" && KANNA_TERMINAL="$planner" kanna task new "First" "hello" && KANNA_TERMINAL="$planner" kanna task new "Second") >/dev/null || { echo "desk_backlog line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk backlog)" == "[\"$(tid 1)\", \"$(tid 2)\"]" && "$(desk page)" == True ]]' || { desk backlog; return 1; }
  sleep 0.5; snap d1 desk_backlog-planner >/dev/null
  raw '{"id":1,"cmd":"debug.click","args":{"backlog":"'"$(tid 1)"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk backlog)" == "[\"$(tid 2)\"]" && "$(desk shelf.0.title)" == First ]]' || { desk shelf; return 1; }
  check test "$(desk running)" == 1 || { echo "desk_backlog: the planning terminal counts as a running task"; desk running; return 1; }
  check wait_for 5 shows First "mock-agent" || { echo "desk_backlog line $LINENO"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"shelf":"'"$(pane First id)"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$(pane First id)" && "$(desk lead)" == None && "$(desk page)" == False && "$(desk strip)" == True ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  check test "$(desk waiting)" == "1 in backlog" || { desk waiting; return 1; }
  is "$planner" state running || { echo "desk_backlog: the hidden planning terminal stopped"; return 1; }
  kanna task park First >/dev/null || { echo "desk_backlog line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk lead)" == "$planner" && "$(desk page)" == True && "$(desk strip)" == False ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}'; return 1; }
  check shows "$planner" "planner=none context=300000" || { echo "desk_backlog: the planning terminal lost its scrollback"; return 1; }
  [[ "$(desk header)" == "Planning · "*"${repo#/private} · main" ]] || { desk header; return 1; }
  kanna task shelve $(tid 2) >/dev/null || { echo "desk_backlog line $LINENO"; return 1; }
  (cd "$repo" && kanna task new "Third" && kanna task new "Fourth") >/dev/null || { echo "desk_backlog line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk running)" == 2 && "$(desk backlog)" == "[\"$(tid 3)\", \"$(tid 4)\"]" ]]' || { desk running; desk backlog; return 1; }
  key cmd+t; key cmd+d
  check test "$(desk window.tabs)" == "[]" || { echo "desk_backlog: the Shelf got a tab"; desk window.tabs; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"backlog":"'"$(tid 3)"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk running)" == 3 && "$(desk backlog)" == "[\"$(tid 4)\"]" ]]' || { desk running; desk backlog; return 1; }
  check test "$(desk window.dock)" == None || { echo "desk_backlog: starting a backlog task docked it"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"backlog":"'"$(tid 4)"'","window":"d1"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk note)" == *"3 tasks on the Shelf"* ]]' || { desk note; return 1; }
  stop_app; start_app || { echo "desk_backlog line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk lead)" == "$planner" ]]' || { echo "desk_backlog: the planning terminal didn't survive the app's restart"; kanna workspace list --all; return 1; }
  local refused; refused="$(raw "{\"id\":1,\"cmd\":\"backlog\",\"args\":{\"action\":\"blank\",\"repo\":\"$repo\"}}")"
  [[ "$refused" == *'"ok":false'* && "$refused" == *"backlog needs an action"* ]] || { echo "desk_backlog: the backlog still starts blank tasks: $refused"; return 1; }
  key cmd+shift+w
  check wait_for 5 eval '! kanna workspace list --all --json | grep -q "\"$planner\""' || { echo "desk_backlog: closing the window left its planning terminal running"; kanna workspace list --all; return 1; }
}

scenario desk_by_path M4
desk_by_path() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_STATE_DIR/repo" fresh="$KANNA_STATE_DIR/fresh"
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
  check wait_for 5 eval '[[ "$(desk empty "$w")" == True && "$(desk backlog "$w")" == "[\"$(tid 1)\"]" && "$(desk running "$w")" == 0 ]]' || { desk backlog "$w"; desk running "$w"; return 1; }
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

scenario move_to_backlog M4
move_to_backlog() {
  start_daemon && start_app || { echo "move_to_backlog line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "move_to_backlog line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd "$repo" && kanna task new "Later" "say hi" --agent "$agent") >/dev/null || { echo "move_to_backlog line $LINENO"; return 1; }
  local started wt; started="$(kanna task shelve $(tid 1))" || { echo "move_to_backlog line $LINENO"; return 1; }
  [[ "$started" =~ " in "([^ ]+)" (" ]] && wt="${BASH_REMATCH[1]/#\~/$HOME}" || { echo "move_to_backlog line $LINENO: $started"; return 1; }
  check wait_for 5 eval '[[ "$(pane Later id)" == term-1 ]]' || { echo "move_to_backlog line $LINENO"; return 1; }
  touch "$wt/work-in-progress"
  local pid; pid="$(pane Later pid)"
  expected="moved term-1 to the backlog as $(sid 1), still running: shelve it to bring it back"; says kanna task defer Later || { echo "move_to_backlog line $LINENO"; return 1; }
  check test "$(ws $(tid 1) where)" == backlog || { echo "move_to_backlog: $(sid 1)'s workspace isn't in the backlog"; state; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "move_to_backlog: the deferred agent stopped"; kanna workspace list --all; return 1; }
  (cd "$repo" && kanna task list) | grep -qE '^'"$(sid 1)"' +Later +Backlog' || { (cd "$repo" && kanna task list); return 1; }
  (cd "$repo" && kanna task list) | grep -qE '^'"$(sid 1)"' .* Shelf ' && { echo "move_to_backlog: the deferred task is still on the Shelf"; (cd "$repo" && kanna task list); return 1; }
  check test -f "$wt/work-in-progress" || { echo "move_to_backlog line $LINENO"; return 1; }
  started="$(kanna task shelve $(tid 1))" || { echo "move_to_backlog line $LINENO"; return 1; }
  [[ "$started" == "moved $(sid 1) to the Shelf: term-1 in ${wt/#$HOME/\~} ("* ]] || { echo "move_to_backlog: it didn't come back as it was: $started"; return 1; }
  check test "$(ws $(tid 1) where)" == '{"shelf": "mac"}' || { echo "move_to_backlog line $LINENO"; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "move_to_backlog: the agent was started again"; kanna workspace list --all; return 1; }
  check test "$(ls "$repo/.kanna-worktrees" | wc -l | tr -d ' ')" == 1 || { echo "move_to_backlog line $LINENO"; return 1; }
  kanna open --name mine --dock --cwd "$repo" --cmd "exec $agent" >/dev/null || { echo "move_to_backlog line $LINENO"; return 1; }
  local mine deferred; mine="$(pane mine id)"; deferred="$(kanna task defer mine 2>&1)"
  [[ "$deferred" == "moved $mine to the backlog as $(sid 2), still running: shelve it to bring it back" ]] || { echo "move_to_backlog line $LINENO: $deferred"; return 1; }
  (cd "$repo" && kanna task done $(tid 2)) >/dev/null || { echo "move_to_backlog line $LINENO"; return 1; }
  check wait_for 5 eval '! kanna workspace list --all --json | grep -q "\"mine\""' || { echo "move_to_backlog: Done left the backlog task's agent running"; kanna workspace list --all; return 1; }
  [[ "$(kanna task defer nothing 2>&1)" == *'no running task "nothing"'* ]] || { echo "move_to_backlog line $LINENO"; return 1; }
}

scenario backlog_and_back M4
backlog_and_back() {
  start_daemon && start_app || { echo "backlog_and_back line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "backlog_and_back line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  local w
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  (cd "$repo" && kanna task new "Later" "say hi" --agent "$agent") >/dev/null || { echo "backlog_and_back line $LINENO"; return 1; }
  kanna task shelve $(tid 1) >/dev/null || { echo "backlog_and_back line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(pane Later id)" == term-* ]]' || { echo "backlog_and_back line $LINENO"; return 1; }
  local pulled; pulled="$(kanna task pull Later)" && [[ "$pulled" =~ " onto "(d[0-9]+)$ ]] || { echo "backlog_and_back line $LINENO: $pulled"; return 1; }
  w="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk window.dock.pane)" == "$(pane Later id)" ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"'"$w"'"}}'; return 1; }
  check test "$(ws $(tid 1) where)" == '{"shelf": "mac"}' || { echo "backlog_and_back: $(sid 1)'s workspace isn't on the app's Shelf"; state; return 1; }
  check wait_for 5 shows Later "mock-agent" || { echo "backlog_and_back line $LINENO"; return 1; }
  local task pid; task="$(pane Later id)" pid="$(pane Later pid)"
  KANNA_TERMINAL="$task" kanna open --name side --cmd 'echo SIDE-READY; exec cat' >/dev/null || { echo "backlog_and_back line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin "$w" tabs)" == *"$(pane side id)"* ]]' || { echo "backlog_and_back: the side terminal isn't on the task's Desk"; kanna workspace list --all --json; return 1; }
  local tabs; tabs="$(dwin "$w" tabs)"
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Move to Backlog"}}' | grep -q '"ok":true' || { echo "backlog_and_back: Move to Backlog didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(desk empty)" == True && "$(desk backlog)" == "[\"$(tid 1)\"]" && "$(desk shelf)" == "[]" && "$(desk running)" == 0 ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"'"$w"'"}}'; return 1; }
  check test "$(desk window.dock)" == None || { echo "backlog_and_back: the task is still on the Desk"; return 1; }
  check test "$(ws $(tid 1) where)" == backlog || { echo "backlog_and_back line $LINENO"; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "backlog_and_back: Move to Backlog stopped the agent"; return 1; }
  kanna send Later "work 1" >/dev/null || { echo "backlog_and_back line $LINENO"; return 1; }
  check wait_for 10 shows Later "WORK_DONE" || { echo "backlog_and_back: the agent didn't keep working in the backlog"; kanna read Later; return 1; }
  is side state running || { echo "backlog_and_back: the task's tab stopped in the backlog"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "backlog_and_back line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk backlog)" == "[\"$(tid 1)\"]" && "$(desk shelf)" == "[]" ]]' || { echo "backlog_and_back: the backlog task didn't survive a restart"; raw '{"id":1,"cmd":"debug.desk","args":{"window":"'"$w"'"}}'; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "backlog_and_back: the restart stopped the agent"; return 1; }
  check test "$(ws $(tid 1) where)" == backlog || { echo "backlog_and_back: $(sid 1)'s workspace didn't come back in the backlog"; state; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"backlog":"'"$(tid 1)"'","window":"'"$w"'"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk backlog)" == "[]" && "$(desk shelf.0.task)" == "$task" ]]' || { raw '{"id":1,"cmd":"debug.desk","args":{"window":"'"$w"'"}}'; return 1; }
  check test "$(pane Later pid)" == "$pid" || { echo "backlog_and_back: Move to Shelf started the agent again"; return 1; }
  kanna task pull Later >/dev/null || { echo "backlog_and_back line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk window.dock.pane)" == "$task" ]]' || { echo "backlog_and_back line $LINENO"; return 1; }
  check test "$(dwin "$w" tabs)" == "$tabs" || { echo "backlog_and_back: the task's tabs didn't come back as they were"; dwin "$w" tabs; return 1; }
  check shows side "SIDE-READY" || { echo "backlog_and_back: the side terminal lost its screen"; return 1; }
}

scenario code_pane M4
code_pane() {
  start_daemon && start_app || return 1
  local file="$KANNA_STATE_DIR/code_pane.rs"
  for i in $(seq 120); do echo "let line_$i = $i;"; done >"$file"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | field "$2"; }
  kanna open code --name src "$file:80" >/dev/null || return 1
  check wait_for 5 eval '[[ "$(code src current)" == 80 ]]' || return 1
  check eval '(( $(code src top) > 50 && $(code src top) < 80 ))' || { echo "top $(code src top)"; return 1; }
  check is src line 80 || return 1
  check test "$(code src language)" == rust || return 1
  check wait_for 5 eval '(( $(code src spans) > 300 ))' || { echo "spans $(code src spans)"; return 1; }
  echo 'let code_pane_edit = 1;' >>"$file"
  check wait_for 5 eval 'code src text | grep -q code_pane_edit' || return 1
  check wait_for 5 eval '[[ "$(code src changed)" == 121 ]]' || { echo "changed $(code src changed)"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"src","at":[120,200]}}' >/dev/null; sleep 0.3
  key() { raw "{\"id\":1,\"cmd\":\"debug.key\",\"args\":{\"key\":\"$1\"}}" >/dev/null; }
  key cmd+shift+=; key cmd+shift+=
  key cmd+f
  check wait_for 5 eval '[[ "$(code src find_bar)" == 1 ]]' || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"line_77 "}}' >/dev/null; key return
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
  [[ "$(kanna open code "$KANNA_STATE_DIR/gone.rs" 2>&1)" == *"No such file: $KANNA_STATE_DIR/gone.rs"* ]] || { echo "code_pane: a missing file"; return 1; }
  [[ "$(kanna open code "$KANNA_STATE_DIR" 2>&1)" == *"is a folder, not a file"* ]] || { echo "code_pane: a folder"; return 1; }
  local nested="$KANNA_STATE_DIR/code_pane-nested.rs"
  { echo 'fn outer() {'; echo '    loop {'; echo '        if ready() {'; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; } >"$nested"
  kanna open code --name nested "$nested" --tab >/dev/null || return 1
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
  kanna open diff --merge-base main --name changes >/dev/null || { cd - >/dev/null; return 1; }
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
  sed -i '' 's/fn f20() -> u32 { 20 }/fn f20() -> u32 { 2000 }/' "$repo/a.rs"
  check wait_for 5 eval '[[ "$(diffpane rows)" == *"{ 2000 }"* ]]' || { echo "the edit didn't arrive"; return 1; }
  check eval '[[ "$(diffpane fresh)" != "[]" ]]' || { echo "nothing marked fresh"; return 1; }
  touch "$repo/a.rs"
  check wait_for 5 eval '[[ "$(diffpane updating)" == False ]]' || { echo "still updating after a re-diff with nothing new"; return 1; }
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
  check test "$(diffpane list)" == True || { echo "no files list"; return 1; }
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
  check wait_for 5 eval '[[ "$(diffpane files)" == *crlf.txt* ]]' || { echo "crlf.txt didn't arrive"; return 1; }
  check python3 -c 'import json, sys
rows = json.loads(sys.argv[1])
assert {"kind": "added", "text": "one\r", "new": 1, "left": "blank", "right": "added"} in rows, [r for r in rows if r.get("new") == 1]' "$(diffpane rows)" || return 1
  printf 'fn later() {}\n' >"$repo/c.rs"
  check wait_for 5 eval '[[ "$(diffpane files)" == *c.rs* ]]' || { echo "the untracked c.rs didn't arrive"; return 1; }
  watchers() { raw '{"id":1,"cmd":"diffs"}' | python3 -c 'import json, sys; print(" ".join(json.dumps(d["args"]) + ":" + str(len(d["views"])) for d in json.load(sys.stdin)["diffs"]))'; }
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "watchers: $(watchers)"; return 1; }
  (cd "$repo" && kanna open diff --merge-base main --name changes2 >/dev/null) || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:2" ]]' || { echo "two views, watchers: $(watchers)"; return 1; }
  kanna close changes2 >/dev/null || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "after closing one view, watchers: $(watchers)"; return 1; }
  stop_daemon; start_daemon || return 1
  check wait_for 10 app_attached || return 1
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--merge-base\", \"main\"]:1" ]]' || { echo "after a daemon restart, watchers: $(watchers)"; return 1; }
  printf 'fn restarted() {}\n' >"$repo/d.rs"
  check wait_for 5 eval '[[ "$(diffpane files)" == *d.rs* ]]' || { echo "d.rs didn't arrive after a daemon restart: $(diffpane files)"; return 1; }
  stop_app; start_app || return 1
  check wait_for 5 eval '[[ "$(diffpane view)" == split && "$(diffpane files)" == *c.rs* ]]' || { echo "after relaunch: $(diffpane view) $(diffpane files) $(diffpane error) $(watchers)"; return 1; }
  kanna navigate changes diff --staged >/dev/null || return 1
  check wait_for 5 eval '[[ "$(diffpane command)" == "git diff --staged" && "$(diffpane files)" == "[]" ]]' || { echo "staged: $(diffpane command) $(diffpane files)"; return 1; }
  check wait_for 5 eval '[[ "$(watchers)" == "[\"--staged\"]:1" ]]' || { echo "after navigating, watchers: $(watchers)"; return 1; }
  kanna navigate changes diff --no-such-flag >/dev/null || return 1
  check wait_for 5 eval '[[ "$(diffpane error)" == *no-such-flag* ]]' || { echo "a failed diff shows no error: $(diffpane error)"; return 1; }
}

scenario code_tour M4
code_tour() {
  start_daemon && start_app || return 1
  local tour="$KANNA_STATE_DIR/code_tour.rs" other="$KANNA_STATE_DIR/code_tour-other.rs"
  { echo 'fn outer() {'; echo '    loop {'; echo '        if ready() {'; for i in $(seq 4 60); do echo "            step($i);"; done; echo '        }'; echo '    }'; echo '}'; } >"$tour"
  for i in $(seq 30); do echo "let line_$i = $i;"; done >"$other"
  code() { raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$1\"}}" | field "$2"; }
  local said id
  said="$(kanna comment "$tour:3" "Waits until it's ready.")" || return 1
  [[ "$said" == "opened file-"*" · code_tour.rs:3–61" ]] || { echo "said: $said"; return 1; }
  id="$(awk '{print $2}' <<<"$said")"
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 3 && "$(code $id comment_last)" == 61 ]]' || return 1
  check wait_for 5 eval '[[ "$(code $id comment_text)" == "Waits until it'"'"'s ready." ]]' || return 1
  check eval '(( $(code $id comment_gap) > 20 ))' || { echo "gap $(code $id comment_gap)"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_box_visible)" == 1 ]]' || { echo "box at $(code $id comment_box_visible)"; return 1; }
  said="$(kanna comment "$tour:40-42" "Each step.")" || return 1
  [[ "$said" == "$id · code_tour.rs:40–42" ]] || { echo "again: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 40 && "$(code $id comment_last)" == 42 ]]' || return 1
  check wait_for 5 eval '[[ "$(code $id comment_box_visible)" == 1 ]]' || return 1
  said="$(kanna comment "$other:7" "Line seven.")" || return 1
  [[ "$said" == "opened file-"*" · code_tour-other.rs:7" && "$said" != *" $id "* ]] || { echo "other: $said"; return 1; }
  [[ "$(kanna comment "$KANNA_STATE_DIR/gone.rs:3" "Nothing." 2>&1)" == *"No such file: $KANNA_STATE_DIR/gone.rs"* ]] || { echo "code_tour: a missing file"; return 1; }
  [[ "$(kanna comment "$other:99" "Past the end." 2>&1)" == *"has 30 lines"* ]] || { echo "code_tour: past the end"; return 1; }
  local doc="$KANNA_STATE_DIR/code_tour.md" agent md
  printf '# Tabs\n\nA tab container holds **several** panes\nand shows one at a time.\n\n- Panes only: it holds panes.\n- One active: the rest keep running.\n' >"$doc"
  agent="$(kanna open --name code_touragent --cmd cat | awk '{print $2}')"
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
  agent="$(kanna open --name agent --cmd cat | awk '{print $2}')"
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
  local link="$KANNA_STATE_DIR/app_link.rs" backlog="$KANNA_STATE_DIR/backlog.rs" said id other
  cp "$repo_root/crates/kanna-daemon/src/app_link.rs" "$link" && cp "$repo_root/crates/kanna-daemon/src/backlog.rs" "$backlog" || return 1
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
  id="$(kanna open code "$link" | awk '{print $2}')"
  check wait_for 5 eval '(( $(code $id scopes) > 0 ))' || return 1
  said="$(kanna comment "$link:196-205" "$long")" || return 1
  [[ "$said" == "$id · app_link.rs:196–205" ]] || { echo "said: $said"; return 1; }
  check wait_for 5 eval '[[ "$(code $id comment_first)" == 196 && "$(code $id scopes)" -gt 0 ]]' || return 1
  sleep 0.5
  check wait_for 5 eval 'fits $id' || { fits $id; return 1; }
  check wait_for 5 eval '(( $(code $id pinned) > 0 ))' || { echo "pinned $(code $id pinned)"; return 1; }
  code_comment_box_shot code_comment_box-open
  raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$id\",\"reveal\":400}}" >/dev/null
  raw "{\"id\":1,\"cmd\":\"debug.code\",\"args\":{\"pane\":\"$id\",\"pick\":198}}" >/dev/null
  check wait_for 5 eval 'fits $id' || { fits $id; return 1; }
  local blank=$(( $(grep -n '^fn gone() -> KmuxError' "$backlog" | cut -d: -f1) - 1 ))
  said="$(kanna comment "$backlog:$blank" "$long")" || return 1
  [[ "$said" == "opened file-"*" · backlog.rs:$blank" ]] || { echo "said: $said"; return 1; }
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
  local repo="$KANNA_STATE_DIR/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "new_desk line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon && start_app || return 1
  kanna open --name plain >/dev/null || { echo "new_desk line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(win win-1 key)" == True ]]' || { echo "new_desk line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
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
  check test "$(kanna task list | grep -c "$planner")" == 0 || { echo "new_desk: the planning terminal is listed as a task"; return 1; }
  check test "$(raw '{"id":1,"cmd":"debug.stats"}' | field pid)" == "$app_pid" || { echo "new_desk: the new Desk is in another instance"; return 1; }
  kanna send "$planner" "$repo" >/dev/null || { echo "new_desk line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk header d1)" == "Planning · "*"${repo#/private} · main" ]]' || { desk header d1; return 1; }
  (cd "$repo" && KANNA_TERMINAL="$planner" kanna task new "Here") >/dev/null || { echo "new_desk line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(desk backlog d1)" == "[\"$(tid 1)\"]" ]]' || { echo "new_desk: the Shelf didn't follow the planning terminal"; desk backlog d1; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"New Desk"}}' | grep -q '"ok":true' || { echo "new_desk: File > New Desk didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(app_view | field windows | count)" == 3 && "$(desk desk d2)" == True ]]' || { echo "new_desk: New Desk opened no Desk"; kanna workspace list --all; return 1; }
  check wait_for 10 eval '[[ "$(desk lead d2)" == term-* ]]' || { echo "new_desk line $LINENO"; return 1; }
  check test "$(cd "$(pane "$(desk lead d2)" cwd)" && pwd -P)" == "$(cd ~ && pwd -P)" || { echo "new_desk: the second new Desk isn't in ~"; return 1; }
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
  kanna open --name screenshot --cmd 'for i in $(seq 80); do echo "screenshot $i the quick brown fox jumps over the lazy dog"; done; exec cat' >/dev/null || return 1
  check wait_for 5 shows screenshot "screenshot 80" || return 1
  printf '# screenshot\n\nA markdown pane.\n' >"$KANNA_STATE_DIR/screenshot.md"
  kanna open md "$KANNA_STATE_DIR/screenshot.md" --name doc --split right >/dev/null || return 1
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
  kanna open --tab --name other >/dev/null || return 1
  local hidden; hidden="$(kanna screenshot screenshot 2>&1)" && { echo "screenshot: shot a pane that isn't showing"; return 1; }
  [[ "$hidden" == *"is in tab tab-1, which isn't showing"* ]] || { echo "screenshot: $hidden"; return 1; }
  local caller; caller="$(KANNA_TERMINAL="$(pane other id)" kanna screenshot --json | field window)" || return 1
  check test "$caller" == win-1
}

scenario bash_completion M4
bash_completion() {
  SHELL=/bin/bash PATH="$bin:$PATH" start_daemon && start_app || return 1
  kanna open --name bash_completion >/dev/null || return 1
  check wait_for 10 eval '[[ -n "$(kanna read bash_completion | tr -d "[:space:]")" ]]' || return 1
  sleep 2
  kanna send bash_completion 'echo "bash_completion:$(complete -p kanna):${PROMPT_COMMAND-none}:${KANNA_BASH_INIT-gone}"' || return 1
  check wait_for 5 shows bash_completion 'bash_completion:complete -o filenames -F _kanna kanna:'
  kanna read bash_completion | grep -E '^bash_completion:' | grep -qF ':gone' || { kanna read bash_completion | tail -4; return 1; }
}

scenario desk_scrolls M4
desk_scrolls() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_STATE_DIR/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "desk_scrolls line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "desk_scrolls line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"${2:+,$2}}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "desk_scrolls: no planning terminal"; kanna workspace list --all; return 1; }
  for i in $(seq 20); do (cd "$repo" && kanna task new "Task $i" "hello" --agent "$agent") >/dev/null || { echo "desk_scrolls line $LINENO"; return 1; }; done
  check wait_for 5 eval '[[ "$(desk backlog)" == *"$(tid 20)"* && "$(desk page)" == True ]]' || { desk backlog; return 1; }
  desk page '"size":[760,420]' >/dev/null; sleep 0.3
  check eval '(( $(desk scroll.height) > $(desk scroll.visible) + 600 ))' || { desk scroll; return 1; }
  check test "$(desk scroll.top)" == 0 || { desk scroll; return 1; }
  [[ "$(desk scroll.backlog)" != *'"'"$(tid 20)"'"'* ]] || { echo "desk_scrolls: $(tid 20) fits a short window"; desk scroll; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"backlog":"'"$(tid 20)"'","window":"d1"}}' | grep -q 'scrolled out' || { echo "desk_scrolls: clicked a row that isn't on screen"; return 1; }
  desk page '"scroll":100000' >/dev/null; sleep 0.3
  check eval '(( $(desk scroll.top) > 0 ))' || { desk scroll; return 1; }
  check eval '[[ "$(desk scroll.backlog)" == *"\"$(tid 20)\""* ]]' || { echo "desk_scrolls: $(tid 20) can't be scrolled to"; desk scroll; return 1; }
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.3; snap d1 desk_scrolls-light >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.3; snap d1 desk_scrolls-dark >/dev/null
  local top; top="$(desk scroll.top)"
  raw '{"id":1,"cmd":"debug.click","args":{"backlog":"'"$(tid 20)"'","window":"d1"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk backlog)" != *"$(tid 20)"* && "$(desk shelf.0.title)" == "Task 20" ]]' || { desk backlog; desk shelf; return 1; }
  check eval '(( $(desk scroll.top) > 0 && $(desk scroll.top) <= top ))' || { echo "desk_scrolls: the page lost its scroll position"; desk scroll; return 1; }
  check eval '(( $(desk scroll.top) + $(desk scroll.visible) <= $(desk scroll.height) ))' || { desk scroll; return 1; }
  (cd "$repo" && kanna task new "The Shelf strip shows every parked task's time beside its name" "hello" --agent "$agent" && kanna task new "Long titles truncate before the times and the strip never overflows" "hello" --agent "$agent") >/dev/null || { echo "desk_scrolls line $LINENO"; return 1; }
  kanna task shelve $(tid 21) >/dev/null && kanna task shelve $(tid 22) >/dev/null || { echo "desk_scrolls line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk running)" == 3 ]]' || { desk running; return 1; }
  kanna task pull "Task 20" >/dev/null || { echo "desk_scrolls line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk strip)" == True && "$(desk title)" == "Task 20" ]]' || { desk stripLayout; return 1; }
  fits() { desk stripLayout "$1" | python3 -c 'import json, sys; g = json.load(sys.stdin); items = g["items"]
sys.exit(not (len(items) == 2 and all(i["meta"].startswith("· parked ") for i in items) and all(i["right"] <= g["width"] for i in items)))'; }
  check eval 'fits "\"size\":[1400,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "19 in backlog" || { desk waiting; return 1; }
  check eval 'fits "\"size\":[520,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "19 in backlog" || { echo "desk_scrolls: the backlog count gave way before the titles"; desk stripLayout; return 1; }
  check eval 'fits "\"size\":[400,420]"' || { desk stripLayout; return 1; }
  check test "$(desk waiting)" == "" || { echo "desk_scrolls: the backlog count didn't give way on a narrow strip"; desk stripLayout; return 1; }
  sleep 0.3; snap d1 desk_scrolls-strip >/dev/null
}

scenario backlog_search M4
backlog_search() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_STATE_DIR/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "backlog_search line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "backlog_search line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  search() { desk search."$1"; }
  type() { raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$1\"}}" >/dev/null; sleep 0.3; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "backlog_search: no planning terminal"; kanna workspace list --all; return 1; }
  check test "$(search shown)" == False || { echo "backlog_search: a search field over an empty backlog"; return 1; }
  (cd "$repo" && kanna task new "Fix flaky status test" "agent_status in the e2e waits for working; follow the status" --agent "$agent" \
    && kanna task new "Resume every agent's session" "Give codex its own session ID and resume it" --agent "$agent" \
    && kanna task new "Shelf strip shows parked time" "Show how long ago each item was parked" --agent "$agent" \
    && kanna task new "Start tasks past the Shelf" "Let a fourth task wait for a slot" --agent "$agent") >/dev/null || { echo "backlog_search line $LINENO"; return 1; }
  for i in $(seq 5 12); do (cd "$repo" && kanna task new "Task $i" "hello" --agent "$agent") >/dev/null || { echo "backlog_search line $LINENO"; return 1; }; done
  check wait_for 5 eval '[[ "$(desk backlog)" == *"$(tid 12)"* && "$(search shown)" == True ]]' || { desk backlog; desk search; return 1; }
  key cmd+f
  check test "$(search focused)" == True || { echo "backlog_search: ⌘F didn't reach the backlog search"; desk responder; return 1; }
  local at; at="$(search left),$(search right)"
  type shelf
  check test "$(search left),$(search right)" == "$at" || { echo "backlog_search: the search field moved as the count changed"; desk search; return 1; }
  check test "$(search found)" == '["'"$(tid 3)"'", "'"$(tid 4)"'"]' || { desk search; return 1; }
  check test "$(search selected)" == 0 || { desk search; return 1; }
  type " parked"
  check test "$(search found)" == '["'"$(tid 3)"'"]' || { desk search; return 1; }
  key escape
  check test "$(search query)" == "" || { echo "backlog_search: esc didn't clear the search"; desk search; return 1; }
  check test "$(search found | grep -o '"t' | wc -l | tr -d ' ')" == 12 || { desk search; return 1; }
  check test "$(search focused)" == True || { desk search; return 1; }
  key escape
  check test "$(search focused)" == False || { echo "backlog_search: a second esc stayed in the search"; desk search; return 1; }
  check test "$(desk responder)" == TerminalSurfaceView || { echo "backlog_search: a second esc didn't go back to the planning terminal"; desk responder; return 1; }
  key cmd+f; type "RESUME codex"
  check test "$(search found)" == '["'"$(tid 2)"'"]' || { echo "backlog_search: no match on a first message, ignoring case"; desk search; return 1; }
  type " nothing"
  check test "$(search found)" == '[]' || { desk search; return 1; }
  key escape; type shelf
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.3; snap d1 backlog_search-light >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.3; snap d1 backlog_search-dark >/dev/null
  key down
  check test "$(search selected)" == 1 || { desk search; return 1; }
  key down
  check test "$(search selected)" == 1 || { echo "backlog_search: the selection ran past the last match"; desk search; return 1; }
  key up
  check test "$(search selected)" == 0 || { desk search; return 1; }
  key down; key return
  check wait_for 10 eval '[[ "$(desk backlog)" != *"$(tid 4)"* && "$(desk shelf.0.title)" == "Start tasks past the Shelf" ]]' || { desk backlog; desk shelf; desk note; return 1; }
  check wait_for 5 eval '[[ "$(search query)" == "" ]]' || { echo "backlog_search: the search stayed after starting its task"; desk search; return 1; }
  local listed; listed="$(cd "$repo" && kanna task list shelf)"
  check test "$(wc -l <<<"$listed" | tr -d ' ')" == 3 || { echo "$listed"; return 1; }
  [[ "$listed" == *"Start tasks past the Shelf"*"Shelf"* && "$listed" == *"$(sid 3)"*"Backlog"* ]] || { echo "backlog_search: kanna task list QUERY"; echo "$listed"; return 1; }
}

scenario task_start_stop M4
task_start_stop() {
  local logged; logged="$(cat "$out/daemon.log" 2>/dev/null | wc -l)"
  start_daemon && start_app || { echo "task_start_stop line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" agent="$KANNA_STATE_DIR/bin/claude"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" || { echo "task_start_stop line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_start_stop line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  local n listed
  for n in 1 2 3 4 5; do (cd "$repo" && kanna task new "Task $n" "say $n" --agent "$agent") >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }; done
  for n in 1 2 3 4 5; do
    [[ "$(kanna task start "$(tid $n)")" == "started $(sid $n) in the backlog: term-"* ]] || { echo "task_start_stop: $(sid $n) didn't start in the backlog"; return 1; }
  done
  check wait_for 10 eval '[[ "$(cd "$repo" && kanna task list | grep -cE "^task-[0-9a-f]{4,8} +Task [1-5] +Backlog +(idle|working|running)")" == 5 ]]' || { (cd "$repo" && kanna task list); return 1; }
  for n in 1 2 3 4 5; do check wait_for 5 shows "Task $n" "say $n" || { echo "task_start_stop: Task $n's agent didn't get its first message"; return 1; }; done
  [[ "$(kanna task start $(tid 1) 2>&1)" == *"$(sid 1) is running"* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  for n in 1 2 3; do kanna task shelve "$(tid $n)" >/dev/null || { echo "task_start_stop: $(sid $n) didn't move to the Shelf"; return 1; }; done
  [[ "$(kanna task shelve $(tid 4) 2>&1)" == *"has 3 tasks on the Shelf"* ]] || { echo "task_start_stop: a fourth task went to the Shelf"; return 1; }
  listed="$(cd "$repo" && kanna task list)"
  [[ "$(grep -cE ' Shelf ' <<<"$listed")" == 3 && "$(grep -cE ' Backlog ' <<<"$listed")" == 2 ]] || { echo "$listed"; return 1; }
  local task session; task="$(pane "Task 1" id)"
  session="$(kanna read "Task 1" | tr -d '\r\n' | sed -n 's/.*--session-id \([0-9a-f-]\{36\}\).*/\1/p')"
  [[ "$session" =~ ^[0-9a-f-]{36}$ ]] || { echo "task_start_stop: Task 1 started without a session"; kanna read "Task 1"; return 1; }
  KANNA_TERMINAL="$task" kanna open --name side --cmd 'exec cat' >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }
  expected="stopped $(sid 1); \`kanna task start $(sid 1)\` starts it again"; says kanna task stop "Task 1" || { echo "task_start_stop line $LINENO"; return 1; }
  check is "Task 1" state exited || { echo "task_start_stop: Task 1's agent is still running"; return 1; }
  unwrapped "Task 1" "claude --resume $session" || { echo "task_start_stop: Task 1's agent wasn't quit with its keys"; kanna read "Task 1"; return 1; }
  ! tail -n +"$((logged + 1))" "$out/daemon.log" | grep -E "in $task (has no quit keys|didn't quit)|quit keys in $task " || { echo "task_start_stop: Task 1's agent was hung up"; return 1; }
  check is side state exited || { echo "task_start_stop: Task 1's terminal tab didn't stay, hung up"; kanna workspace list --all; return 1; }
  (cd "$repo" && kanna task list) | grep -qE "^$(sid 1) +Task 1 +Shelf +stopped" || { echo "task_start_stop: Task 1 left the Shelf"; (cd "$repo" && kanna task list); return 1; }
  [[ "$(kanna task stop "Task 1" 2>&1)" == *"is stopped"* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  [[ "$(kanna task shelve $(tid 4) 2>&1)" == *"has 3 tasks on the Shelf"* ]] || { echo "task_start_stop: a stopped task stopped counting on the Shelf"; return 1; }
  [[ "$(kanna task start "Task 1")" == "started $(sid 1) again: term-"* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(kanna read "Task 1" | tr -d "\r\n")" == *"--resume $session"* ]]' || { echo "task_start_stop: Task 1 didn't resume its conversation"; kanna read "Task 1"; return 1; }
  check wait_for 5 is side state running || { echo "task_start_stop: Task 1's terminal tab didn't run again"; kanna workspace list --all; return 1; }
  (cd "$repo" && kanna task list) | grep -qE "^$(sid 1) +Task 1 +Shelf +(idle|working|running)" || { (cd "$repo" && kanna task list); return 1; }
  [[ "$(kanna task stop $(tid 5))" == "stopped $(sid 5) in the backlog; "* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  (cd "$repo" && kanna task list) | grep -qE '^'"$(sid 5)"' +Task 5 +Backlog +stopped' || { (cd "$repo" && kanna task list); return 1; }
  [[ "$(kanna task start $(tid 5))" == "started $(sid 5) again in the backlog: term-"* ]] || { echo "task_start_stop line $LINENO"; return 1; }
  check wait_for 5 shows "Task 5" "--resume" || { echo "task_start_stop: $(sid 5) didn't resume"; kanna read "Task 5"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "task_start_stop line $LINENO"; return 1; }
  KANNA_TERMINAL="$(pane "Task 4" id)" kanna open --name side4 --cmd 'exec cat' >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }
  kanna task stop $(tid 4) >/dev/null || { echo "task_start_stop line $LINENO"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "task_start_stop line $LINENO"; return 1; }
  (cd "$repo" && kanna task list) | grep -qE '^'"$(sid 4)"' +Task 4 +Backlog +stopped' || { echo "task_start_stop: a stopped task didn't survive a restart"; (cd "$repo" && kanna task list); return 1; }
  kanna task start $(tid 4) >/dev/null && check wait_for 5 shows "Task 4" "--resume" || { echo "task_start_stop: $(sid 4) didn't resume after a restart"; return 1; }
  check wait_for 5 is side4 state running || { echo "task_start_stop: a hung-up terminal didn't run again after a restart"; kanna workspace list --all; return 1; }
}

scenario task_restart_typing M4
task_restart_typing() {
  start_daemon && start_app || { echo "task_restart_typing line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" agent="$KANNA_STATE_DIR/bin/claude"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" || { echo "task_restart_typing line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_restart_typing line $LINENO"; return 1; }
  (cd "$repo" && kanna task new Typed "say hi" --agent "$agent") >/dev/null || { echo "task_restart_typing line $LINENO"; return 1; }
  kanna task shelve "$(tid 1)" >/dev/null && kanna task pull "$(tid 1)" >/dev/null || { echo "task_restart_typing line $LINENO"; return 1; }
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
  kanna open code --name src "$file" >/dev/null || return 1
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
  kanna open --name bytes --cmd 'stty raw -echo; printf "READY\r\n"; while :; do printf "%s " "$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d " \n")"; done' >/dev/null || return 1
  check wait_for 5 shows bytes READY || return 1
  kanna send-key bytes up ctrl+c return esc shift+tab || return 1
  check wait_for 5 shows bytes "1b 5b 41 03 0d 1b 1b 5b 5a" || { kanna read bytes; return 1; }
  kanna open --name agent --cmd "$mock_agent" >/dev/null || return 1
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
  start_daemon && start_app || { echo "real_agent_tasks line $LINENO"; return 1; }
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_STATE_DIR/repo" harness="$repo_root/tools/agent-harness"
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
    kanna restart "$agent task" >/dev/null || { echo "real_agent_tasks: $agent's pane didn't restart"; return 1; }
    check wait_for 60 eval 'shows "$agent task" "$first" && shows "$agent task" "Agent status probe"' || { echo "real_agent_tasks: $agent didn't restart into its conversation"; kanna read "$agent task"; return 1; }
    sleep 3
    [[ "$(asked "$first")" == 1 ]] || { echo "real_agent_tasks: $agent restarted with its first message instead of resuming"; return 1; }
    kanna task shelve "$id" >/dev/null || { echo "real_agent_tasks: $agent's task didn't move to the Shelf"; return 1; }
    task="$(pane "$agent task" id)"
    [[ "$(kanna task defer "$task")" == *"to the backlog as $id"* ]] || { echo "real_agent_tasks: $agent's task didn't move back to the backlog as $id"; return 1; }
    [[ "$(kanna task stop "$id")" == "stopped $id in the backlog; "* ]] || { echo "real_agent_tasks: $agent's task didn't stop"; return 1; }
    check is "$agent task" state exited || { echo "real_agent_tasks: $agent is still running"; return 1; }
    said="$(kanna read "$agent task" | tr -d '\r\n' | grep -oE "$agent (--remote [^ ]+ )?(--)?resume [0-9a-f-]{36}" | tail -1 | grep -oE '[0-9a-f-]{36}$')"
    [[ -n "$said" ]] || { echo "real_agent_tasks: $agent wasn't quit with its keys, so it didn't say its session"; kanna read "$agent task" --lines 60; return 1; }
    ! tail -n +"$((logged + 1))" "$out/daemon.log" | grep -E "in $task (has no quit keys|didn't quit)|quit keys in $task " || { echo "real_agent_tasks: $agent was hung up"; return 1; }
    [[ "$(raw '{"id":1,"cmd":"backlog","args":{"action":"list"}}' | python3 -c 'import json, sys; print(next(t for t in json.load(sys.stdin)["tasks"] if t["id"].startswith(sys.argv[1])).get("session"))' "$id")" == "$said" ]] || { echo "real_agent_tasks: the session $agent said ($said) wasn't kept"; return 1; }
    [[ "$(kanna task start "$id")" == "started $id again in the backlog: "* ]] || { echo "real_agent_tasks: $agent's task didn't start again"; return 1; }
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
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_STATE_DIR/repo" harness="$repo_root/tools/agent-harness"
  mkdir -p "$agents" "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "real_agent_program_status line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  python3 -I "$harness/mock_server.py" --log "$agents/mock.jsonl" >"$agents/port" 2>"$agents/mock.log" &
  check wait_for 10 eval '[[ -s "$agents/port" ]]' || { echo "real_agent_program_status: the mock model didn't start"; return 1; }
  reported() { raw "{\"id\":1,\"cmd\":\"status\",\"args\":{\"action\":\"explain\",\"pane\":\"$1\"}}" | python3 -c 'import json, sys; r = json.load(sys.stdin); print(r.get("source"), (r.get("reported") or {}).get("state"), r.get("status"))'; }
  local agent real pane
  for agent in claude codex; do
    [[ $agent == claude ]] && real="$claude" || real="$codex"
    pane="$agent reporting"
    python3 -I "$harness/isolate.py" --agent $agent --real "$real" --home "$agents/$agent-home" --port "$(cat "$agents/port")" --trust "$repo" --out "$agents/bin/$agent" || { echo "real_agent_program_status line $LINENO"; return 1; }
    kanna open --name "$pane" --cwd "$repo" --cmd "$agents/bin/$agent" >/dev/null || { echo "real_agent_program_status line $LINENO"; return 1; }
    check wait_for 60 shows "$pane" "for shortcuts\\|shift+tab" || { echo "real_agent_program_status: $agent didn't start"; kanna read "$pane"; return 1; }
    kanna send "$pane" "PLAIN_TURN" || { echo "real_agent_program_status line $LINENO"; return 1; }
    check wait_for 30 eval '[[ "$(reported "$pane")" == "osc7501 working working" ]]' || { echo "real_agent_program_status: $agent's turn wasn't working from OSC 7501 but $(reported "$pane")"; kanna status explain "$pane"; kanna read "$pane"; tail -2 "$agents/mock.jsonl" | cut -c1-300; return 1; }
    kanna status explain "$pane" | grep -qF "source: OSC 7501" || { kanna status explain "$pane"; return 1; }
    check wait_for 60 eval '[[ "$(reported "$pane")" =~ ^"osc7501 "(idle|done)" idle"$ ]]' || { echo "real_agent_program_status: $agent's turn didn't end idle from OSC 7501 but $(reported "$pane")"; kanna status explain "$pane"; kanna read "$pane"; return 1; }
    shows "$pane" "slow streamed answer" || { echo "real_agent_program_status: $agent didn't show the mock's answer"; kanna read "$pane"; return 1; }
  done
  stop_agents
}

scenario send_submits M4
send_submits() {
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}" codex="${KANNA_E2E_CODEX:-$(command -v codex)}"
  [[ -x "$claude" ]] || { echo "send_submits: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  start_daemon || return 1
  local shell name
  for shell in "zsh -f" "bash --norc --noprofile"; do
    name="send_${shell%% *}"
    kanna open --name "$name" --cmd "exec $shell" >/dev/null || return 1
    kanna send "$name" "echo $name-\$((40+2))" || return 1
    check wait_for 5 shows "$name" "^$name-42" || { kanna read "$name"; return 1; }
  done
  kanna open --name send_cat --cmd "exec cat" >/dev/null || return 1
  kanna send send_cat send_cat-LINE || return 1
  check wait_for 5 eval '[[ "$(kanna read send_cat | grep -c send_cat-LINE)" == 2 ]]' || { kanna read send_cat; return 1; }
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_STATE_DIR/repo" harness="$repo_root/tools/agent-harness"
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
    kanna open --name "send_$agent" --cwd "$repo" --cmd "$agents/bin/$agent" >/dev/null || return 1
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
  local repo="$KANNA_STATE_DIR/repo" fake="$KANNA_STATE_DIR/agents" w folder
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "codex_task line $LINENO"; return 1; }
  ln -s "$mock_agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  printf '{"version": 1, "backlog": {"runners": 0}, "tasks": {"agent": "%s"}}\n' "$fake/codex" >"$KANNA_AGENTS_HOME/.kanna-v4/config.json"
  printf '{"tasks": {"agent": "touch %s/owned; claude"}}\n' "$KANNA_STATE_DIR" >"$repo/.kanna/config.json"
  in_repo task new "Codex task" "run git branch -m codex-mock-work" >/dev/null || { echo "codex_task line $LINENO"; return 1; }
  check test "$(in_repo --json task list | field backlog.0.agent)" == "$fake/codex" || { echo "codex_task: tasks.agent in your config wasn't the task's agent, or the repo's command was taken"; in_repo --json task list; return 1; }
  in_repo task list | grep -qE '^'"$(sid 1)"' +Codex task +Backlog +stopped +codex +made ' || { echo "codex_task: task list doesn't say the agent"; in_repo task list; return 1; }
  w="$(cd / && kanna "$repo")" && [[ "$w" =~ ^opened\ (d[0-9]+) ]] && w="${BASH_REMATCH[1]}" || { echo "codex_task line $LINENO: $w"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk agents)" == "[\"codex\"]" ]]' || { echo "codex_task: the backlog row doesn't say the agent"; desk agents; return 1; }
  kanna task start $(tid 1) >/dev/null || { echo "codex_task: the task didn't start"; return 1; }
  check wait_for 10 shows "Codex task" "RUN_DONE 0" || { echo "codex_task: the first message didn't run"; kanna read "Codex task"; return 1; }
  spaceless "Codex task" "args: You're a task's agent in Kanna (terminal $(pane "Codex task" id)), in your task's worktree. Your task: run git branch -m codex-mock-work --remote unix://" || { echo "codex_task: codex wasn't started as a task's agent"; kanna read "Codex task"; return 1; }
  folder="$(pane "Codex task" cwd)"
  check test "$(git -C "$folder" branch --show-current)" == codex-mock-work || { echo "codex_task: the branch wasn't renamed"; git -C "$folder" branch; return 1; }
  check wait_for 10 is "Codex task" status idle || { echo "codex_task: not idle after its turn"; pane "Codex task" status; return 1; }
  kanna send "Codex task" "work 3" && check wait_for 5 is "Codex task" status working || { echo "codex_task: not working on kanna send"; return 1; }
  check wait_for 10 is "Codex task" status idle && shows "Codex task" WORK_DONE || { echo "codex_task: not idle after kanna send's turn"; return 1; }
  kanna send "Codex task" ask && check wait_for 5 is "Codex task" status needs-you || { echo "codex_task: not needs you at codex's question"; kanna read "Codex task"; return 1; }
  kanna send-key "Codex task" return && check wait_for 10 shows "Codex task" TURN_DONE && check wait_for 10 is "Codex task" status idle || { echo "codex_task: the answer didn't go through"; return 1; }
  [[ "$(kanna task shelve $(tid 1))" == "moved $(sid 1) to the Shelf: "* ]] || { echo "codex_task: the task didn't move to the Shelf"; return 1; }
  check wait_for 10 eval '[[ "$(desk shelf.0.agent)" == codex ]]' || { echo "codex_task: the Shelf row doesn't say the agent"; desk shelf; return 1; }
  in_repo task list | grep -qE "^[^ ]+ +Codex task +Shelf +idle +codex +parked " || { echo "codex_task: task list doesn't say the Shelf task's agent"; in_repo task list; return 1; }
  kanna task done $(tid 1) >/dev/null || { echo "codex_task: done failed"; return 1; }
  check wait_for 10 eval '! kanna workspace list --all --json | grep -q "\"Codex task\""' || { echo "codex_task: the agent outlived done"; return 1; }
  check test ! -e "$KANNA_STATE_DIR/owned" || { echo "codex_task: a repo's tasks.agent ran a command"; return 1; }
}

scenario shelf_zoom M4
shelf_zoom() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_STATE_DIR/repo" planner task shot log
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "shelf_zoom line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "shelf_zoom line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"d1\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk lead)" == term-* ]]' || { echo "shelf_zoom: no planning terminal"; kanna workspace list --all; return 1; }
  planner="$(desk lead)"
  (cd "$repo" && kanna task new "Zoomed task" "hello" --agent "$agent") >/dev/null && kanna task shelve $(tid 1) >/dev/null || { echo "shelf_zoom line $LINENO"; return 1; }
  kanna task pull "Zoomed task" >/dev/null || { echo "shelf_zoom line $LINENO"; return 1; }
  task="$(dwin d1 dock.pane)"
  KANNA_TERMINAL="$task" kanna open --name log --cmd "printf 'GET / 200\n'; exec sleep 600" >/dev/null || { echo "shelf_zoom line $LINENO"; return 1; }
  log="$(pane log id)"
  KANNA_TERMINAL="$log" kanna open --name tests --split right --cmd "printf 'test result: ok\n'; exec sleep 600" >/dev/null || { echo "shelf_zoom line $LINENO"; return 1; }
  kanna focus log >/dev/null && key cmd+shift+return || { echo "shelf_zoom line $LINENO"; return 1; }
  check test "$(dwin d1 zoomed)" == "$log" || { echo "shelf_zoom: ⇧⌘↩ didn't zoom the task's pane"; kanna workspace list --all; return 1; }
  shown_alone() { shot="$(snap d1 "$1")"; python3 -c 'import json, sys; s = json.loads(sys.argv[1]); p = s["panes"]; assert len(p) == 1 and p[0]["width"] == s["width"], s' "$shot"; }
  sleep 0.5; check shown_alone shelf_zoom-task-zoomed || { echo "shelf_zoom: the zoomed pane doesn't fill the window"; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "shelf_zoom: ⌘. didn't park the task"; return 1; }
  check test "$(ws $(tid 1) where):$(ws $(tid 1) tabs.0.title)" == "{\"shelf\": \"mac\"}:log" || { echo "shelf_zoom: $(sid 1)'s workspace isn't on the Shelf with its tabs"; state; return 1; }
  check test "$(dwin d1 focused):$(dwin d1 zoomed)" == "$planner:None" || { echo "shelf_zoom: the Shelf view came up zoomed"; kanna workspace list --all; return 1; }
  check test "$(desk page)" == True || { echo "shelf_zoom: no full Shelf beside the planner"; return 1; }
  key cmd+shift+return
  check test "$(dwin d1 zoomed)" == "$planner" || { echo "shelf_zoom: ⇧⌘↩ didn't zoom the planner"; kanna workspace list --all; return 1; }
  sleep 0.5; check shown_alone shelf_zoom-planner-zoomed || { echo "shelf_zoom: the zoomed planner doesn't fill the window"; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$task" ]]' || { echo "shelf_zoom: ⌘. didn't pull the task back"; return 1; }
  check test "$(dwin d1 focused):$(dwin d1 zoomed)" == "$log:$log" || { echo "shelf_zoom: the task didn't come back zoomed where it was"; kanna workspace list --all; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "shelf_zoom line $LINENO"; return 1; }
  check test "$(dwin d1 zoomed)" == "$planner" || { echo "shelf_zoom: the Shelf view didn't come back zoomed"; kanna workspace list --all; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$task" ]]' || { echo "shelf_zoom line $LINENO"; return 1; }
  stop_app; start_app || { echo "shelf_zoom line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(dwin d1 zoomed)" == "$log" ]]' || { echo "shelf_zoom: the task's zoom didn't survive a relaunch"; kanna workspace list --all; return 1; }
  sleep 0.5; check shown_alone shelf_zoom-task-relaunched || { echo "shelf_zoom line $LINENO"; return 1; }
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "shelf_zoom line $LINENO"; return 1; }
  check test "$(dwin d1 zoomed)" == "$planner" || { echo "shelf_zoom: the Shelf's zoom didn't survive a relaunch"; kanna workspace list --all; return 1; }
  kanna zoom "$planner" >/dev/null || { echo "shelf_zoom line $LINENO"; return 1; }
  check test "$(dwin d1 zoomed)" == None || { echo "shelf_zoom: kanna zoom didn't turn the Shelf's zoom off"; return 1; }
  sleep 0.5; check drawn "$(snap d1 shelf_zoom-shelf)" 1 || return 1
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 zoomed)" == "$log" ]]' || { echo "shelf_zoom: the task lost its zoom to the Shelf's"; kanna workspace list --all; return 1; }
  stop_agents
}

scenario task_history M4
task_history() {
  local repo="$KANNA_STATE_DIR/repo" agent="$KANNA_STATE_DIR/bin/claude" state="$KANNA_STATE_DIR"
  mkdir -p "$repo" "${agent%/*}" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$agent" || { echo "task_history line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "task_history line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  printf '{"counter": 3, "tasks": [{"id": "t3", "name": "From JSON", "message": "hi", "repo": "%s", "madeAt": 1700000000, "agent": "%s"}], "running": []}' "$repo" "$agent" >"$state/backlog.json"
  start_daemon && start_app || { echo "task_history line $LINENO"; return 1; }
  [[ -f "$state/kanna.db" && -f "$state/backlog.json.imported" && ! -e "$state/backlog.json" ]] || { echo "task_history: backlog.json wasn't imported"; ls "$state"; return 1; }
  (cd "$repo" && kanna task list) | grep -qE '^'"$(sid 1)"' +From JSON +Backlog' || { echo "task_history: the imported task isn't in the backlog"; (cd "$repo" && kanna task list); return 1; }
  events() { raw "{\"id\":1,\"cmd\":\"backlog\",\"args\":{\"action\":\"events\",\"task\":\"$1\"}}" | python3 -c 'import json, sys; print(" ".join(e["what"] + ("@" + e["by"] if e["by"] else "") for e in json.load(sys.stdin)["events"]))'; }
  [[ "$(events "$(tid 1)")" == "made renumbered@daemon" ]] || { echo "task_history: the imported task's events are $(events "$(tid 1)")"; return 1; }
  (cd "$repo" && kanna task new "Fresh" "say fresh" --agent "$agent") | grep -qE '^added task-[0-9a-f]{4,8} ' || { echo "task_history: a new task didn't get an ID"; return 1; }
  kanna task shelve $(tid 2) >/dev/null || { echo "task_history line $LINENO"; return 1; }
  local task window; task="$(pane Fresh id)"
  window="$(KANNA_TERMINAL="$task" kanna task pull "$task")" && window="${window##* onto }" && [[ "$window" =~ ^d[0-9]+$ ]] || { echo "task_history: pull said $window"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"park\":\"$window\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(dwin "$window" dock)" == None ]]' || { echo "task_history: the app's park didn't park"; return 1; }
  kanna task stop "$task" >/dev/null && kanna task start "$task" >/dev/null || { echo "task_history line $LINENO"; return 1; }
  kanna task name $(tid 2) "Fresher" >/dev/null || { echo "task_history line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(events $(tid 2))" == *renamed ]]' || { echo "task_history: the rename wasn't recorded: $(events $(tid 2))"; return 1; }
  kanna task defer $(tid 2) >/dev/null && kanna task done $(tid 2) >/dev/null || { echo "task_history line $LINENO"; return 1; }
  local want="made@user shelved@user stopped@user started@user renamed deferred@user done@user"
  [[ "$(events $(tid 2))" == "$want" ]] || { echo "task_history: the task's events are \"$(events $(tid 2))\", not \"$want\""; return 1; }
  [[ "$(sqlite3 -readonly "$state/kanna.db" "select place from tasks where id = '$(tid 2)'")" == done ]] || { echo "task_history: a done task's row is gone or not done"; return 1; }
  stop_app; stop_daemon
  local lines; lines="$(wc -l <"$out/daemon.log")"
  rm -f "$state/kanna.db-wal" "$state/kanna.db-shm"
  printf 'this is not a database %.0s' {1..200} >"$state/kanna.db"
  start_daemon && start_app || { echo "task_history line $LINENO"; return 1; }
  tail -n +"$((lines + 1))" "$out/daemon.log" | grep -q "kanna.db can't be used" || { echo "task_history: the daemon didn't say the db was bad"; return 1; }
  tail -n +"$((lines + 1))" "$out/daemon.log" | grep -q "restored .*kanna.db from" || { echo "task_history: the daemon didn't say it restored the db"; return 1; }
  (cd "$repo" && kanna task list) | grep -qE '^'"$(sid 1)"' +From JSON +Backlog' || { echo "task_history: a backlog task was lost with the db"; (cd "$repo" && kanna task list); return 1; }
  ls "$state" | grep -q '^kanna.db.bad-' || { echo "task_history: the bad db wasn't kept"; return 1; }
}

scenario quiz_results M4
quiz_results() {
  local repo="$KANNA_STATE_DIR/repo" state="$KANNA_STATE_DIR"
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

scenario quiz_window M4
quiz_window() {
  local repo="$KANNA_STATE_DIR/repo" agent="$repo_root/tools/mock-agent/mock-agent"
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
  check test "$(kanna task list | grep -c "$quiz")" == 0 || { echo "quiz_window: the quiz agent is listed as a task"; kanna task list; return 1; }
  check test "$(desk running "$d")" == 0 || { echo "quiz_window: the quiz agent counts as a running task"; desk running "$d"; return 1; }
  check test "$(desk shelf "$d")" == "[]" || { echo "quiz_window: the quiz agent is on the Shelf"; desk shelf "$d"; return 1; }
  local said code; said="$(KANNA_TERMINAL="$quiz" kanna comment "$repo/src/a.rs:2" "Two comes second.")" && [[ "$said" == "opened file-"*" · a.rs:2"* ]] || { echo "quiz_window: kanna comment said \"$said\""; return 1; }
  code="$(awk '{print $2}' <<<"$said")"
  check wait_for 5 eval '[[ "$(desk window.tabs.0.panes "$w")" == "[\"$code\"]" && "$(desk window.lead.pane "$w")" == "$quiz" && "$(desk window.tab "$w")" == "$(desk window.tabs.0.id "$w")" ]]' || { echo "quiz_window: the comment isn't a tab of the Quiz window"; kanna workspace list --all; return 1; }
  sleep 0.5; snap "$w" quiz_window >/dev/null
  kanna move "$quiz" --tab >/dev/null 2>&1 && { echo "quiz_window: the quiz agent moved into a tab"; return 1; }
  KANNA_TERMINAL="$quiz" kanna task park >/dev/null 2>&1 && { echo "quiz_window: the quiz agent parked"; return 1; }
  kanna task pull "$quiz" >/dev/null 2>&1 && { echo "quiz_window: the quiz agent was pulled onto a Desk"; return 1; }
  kanna move "$quiz" --dock --window "$d" >/dev/null 2>&1 && { echo "quiz_window: the quiz agent docked on the Desk"; return 1; }
  local t; for t in One Two Three; do (cd "$repo" && kanna task new "$t" "hi" --agent "$agent") >/dev/null || { echo "quiz_window line $LINENO"; return 1; }; done
  for t in $(tid 1) $(tid 2) $(tid 3); do kanna task shelve "$t" >/dev/null || { echo "quiz_window: the quiz agent took one of the project's three places"; kanna task list; return 1; }; done
  check wait_for 10 eval '[[ "$(desk running "$d")" == 3 ]]' || { echo "quiz_window: three tasks didn't fit on the Shelf beside the quiz"; kanna task list; return 1; }
  opened="$(cd "$repo" && kanna quiz open --agent "$agent")"
  [[ "$opened" == "brought forward $quiz in $w" ]] || { echo "quiz_window: a second kanna quiz open said \"$opened\""; kanna workspace list --all; return 1; }
  [[ "$(kanna workspace list --all --json | python3 -c 'import json, sys; print(sum(1 for w in json.load(sys.stdin)["windows"] if w.get("lead")))')" == 1 ]] || { echo "quiz_window: a second Quiz window opened"; kanna workspace list --all; return 1; }
  kanna close --window "$w" >/dev/null || { echo "quiz_window line $LINENO"; return 1; }
  check wait_for 5 eval '! kanna workspace list --all --json | grep -q "\"$quiz\""' || { echo "quiz_window: closing the window left its quiz agent running"; kanna workspace list --all; return 1; }
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
  kanna open --name daemon_upgrade --cmd 'echo daemon_upgrade-BEFORE; exec cat' >/dev/null || return 1
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
  kanna open --name daemon_upgradeb --split right >/dev/null || return 1
  check app_text daemon_upgrade daemon_upgrade-BEFORE || return 1
  [[ "$(kanna send nosuch x 2>&1)" == *"another build"* ]] && { echo "daemon_upgrade: the CLI notes a daemon of its own build"; return 1; }
  return 0
}

scenario login_shell M4
login_shell() {
  local zdot="$KANNA_STATE_DIR/zdot" nvm="$KANNA_STATE_DIR/nvm/bin" repo="$KANNA_STATE_DIR/repo"
  mkdir -p "$zdot" "$nvm" "$repo" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$nvm/codex" || { echo "login_shell line $LINENO"; return 1; }
  printf 'export login_shell_ZSHRC=from-zshrc\npath=(%q $path)\necho login_shell zshrc says hello\n' "$nvm" >"$zdot/.zshrc"
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "login_shell line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  SHELL=/bin/zsh ZDOTDIR="$zdot" start_daemon && start_app || { echo "login_shell line $LINENO"; return 1; }
  kanna open --name login_shell --cmd 'echo "login_shell:$login_shell_ZSHRC:$(command -v codex)"; exec cat' >/dev/null || return 1
  check wait_for 5 shows login_shell "login_shell:from-zshrc:$nvm/codex" || { echo "login_shell: a command pane didn't read .zshrc"; kanna read login_shell; return 1; }
  local id; id="$(cd "$repo" && kanna task new "Codex from nvm" "login_shell first message" --agent codex | sed -n 's/^added \(task-[0-9a-f]*\).*/\1/p')"
  [[ "$id" =~ ^task-[0-9a-f]{4,8}$ ]] || { echo "login_shell: the task wasn't added"; return 1; }
  kanna task start "$id" >/dev/null || { echo "login_shell: the task didn't start"; return 1; }
  check wait_for 10 shows "Codex from nvm" "Your task: login_shell first message" || { echo "login_shell: codex from .zshrc's PATH didn't start"; kanna read "Codex from nvm"; return 1; }
  kanna read "Codex from nvm" | grep -q "command not found" && { echo "login_shell: the task's shell didn't find codex"; return 1; }
  return 0
}

scenario planner_choice M4
planner_choice() {
  local fake="$KANNA_STATE_DIR/fake" repo="$KANNA_STATE_DIR/repo"
  mkdir -p "$fake" "$repo" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/claude" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex" || { echo "planner_choice line $LINENO"; return 1; }
  git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "planner_choice line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
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
  check wait_for 5 shows "$planner" "Make codex the planning agent for repo? \[y/N\]" || { kanna read "$planner"; return 1; }
  kanna send "$planner" y >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 5 shows "$planner" "codex plans repo from now on" || { kanna read "$planner"; return 1; }
  check wait_for 10 spaceless "$planner" "args: -c model_auto_compact_token_limit=300000 You are the Kanna planning agent. --remote unix://" || { echo "planner_choice: codex didn't start as the planner"; kanna read "$planner"; return 1; }
  check grep -Eq '"agent" ?: "codex"' "$repo/.kanna/config.json" || { cat "$repo/.kanna/config.json"; return 1; }
  kanna send "$planner" exit >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 5 shows "$planner" "The planning agent quit. Start it again:" || { kanna read "$planner"; return 1; }
  check is "$planner" state running || { echo "planner_choice: quitting the agent ended the planning terminal"; return 1; }
  close_desk d1 || { echo "planner_choice line $LINENO"; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "planner_choice line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk lead "$w")" == term-* ]]' || { echo "planner_choice: the next Desk has no planning terminal"; return 1; }
  planner="$(desk lead "$w")"
  check wait_for 10 spaceless "$planner" "args: -c model_auto_compact_token_limit=300000 You are the Kanna planning agent. --remote unix://" || { echo "planner_choice: the next Desk didn't start the project's planner"; kanna read "$planner"; return 1; }
  ! shows "$planner" "Choose an agent" || { echo "planner_choice: the next Desk asked again"; return 1; }
  key cmd+n
  check wait_for 10 eval '[[ "$(desk lead d3)" == term-* ]]' || { echo "planner_choice: ⌘N opened no Desk"; kanna workspace list --all; return 1; }
  planner="$(desk lead d3)"
  check wait_for 10 shows "$planner" "Folder, repo, name or number \[1\]:" || { echo "planner_choice: a Desk in ~ didn't ask where"; kanna read "$planner"; return 1; }
  kanna send "$planner" "$repo" >/dev/null || { echo "planner_choice line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk header d3)" == "Planning · "*"${repo#/private} · main" ]]' || { echo "planner_choice: the Desk didn't follow to the folder"; desk header d3; return 1; }
  check wait_for 10 spaceless "$planner" "args: -c model_auto_compact_token_limit=300000 You are the Kanna planning agent. --remote unix://" || { echo "planner_choice: the Desk didn't start the folder's planner"; kanna read "$planner"; return 1; }
}

scenario real_agent_planners M4
real_agent_planners() {
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}" codex="${KANNA_E2E_CODEX:-$(command -v codex)}"
  [[ -x "$claude" ]] || { echo "real_agent_planners: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  [[ -x "$codex" ]] || { echo "real_agent_planners: codex isn't installed (or set KANNA_E2E_CODEX)"; return 1; }
  local agents="$KANNA_STATE_DIR/agents" repo="$KANNA_STATE_DIR/repo" harness="$repo_root/tools/agent-harness"
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
    kanna send "$planner" "$choice" >/dev/null && check wait_for 5 shows "$planner" "\[y/N\]" && kanna send "$planner" n >/dev/null || { echo "real_agent_planners line $LINENO"; kanna read "$planner"; return 1; }
    check wait_for 60 eval '[[ "$(told "$path")" -ge 1 ]]' || { echo "real_agent_planners: $agent wasn't told it's the planning agent"; kanna read "$planner"; tail -3 "$agents/mock.jsonl" | cut -c1-300; return 1; }
    check wait_for 30 shows "$planner" "Agent status probe" || { echo "real_agent_planners: $agent didn't show the mock's answer"; kanna read "$planner"; return 1; }
    close_desk "$w" || { echo "real_agent_planners line $LINENO"; return 1; }
    opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "real_agent_planners line $LINENO: $opened"; return 1; }
    w="${BASH_REMATCH[1]}"
  done
  stop_agents
}

scenario path_reuses_desk M4
path_reuses_desk() {
  start_daemon && start_app || { echo "path_reuses_desk line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" wt="$KANNA_STATE_DIR/wt" other="$KANNA_STATE_DIR/other"
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
  expected="brought forward d1 on $repo"; says kanna "$repo" || return 1
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || { echo "path_reuses_desk: the project's Desk isn't the key window"; app_view; return 1; }
  check test "$(app_view | field windows | count)" == 2 || { kanna workspace list --all; return 1; }
}

scenario recent_projects M4
recent_projects() {
  start_daemon && start_app || { echo "recent_projects line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" other="$KANNA_STATE_DIR/other"
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
  check shows "$planner" "1  ${repo/#$HOME/~}" || { kanna read "$planner"; return 1; }
  kanna send-key "$planner" return || { echo "recent_projects line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk header d2)" == "Planning · "*"${repo#/private} · main" ]]' || { echo "recent_projects: Return didn't open the latest project"; desk header d2; kanna read "$planner"; return 1; }
  expected="opened d3 on $other"; says kanna "$other" || return 1
  check wait_for 5 eval '[[ "$(kanna projects | head -1)" == "$other" && "$(kanna projects | sed -n 2p)" == "$repo" ]]' || { echo "recent_projects: the latest isn't first"; kanna projects; return 1; }
  check wait_for 5 eval '[[ "$(dwin d3 key)" == True ]]' || { app_view; return 1; }
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.menu\"}" | grep -q "\"item\":\"${repo/#$HOME/~}\",\"menu\":\"Open Recent\""' || { echo "recent_projects: Open Recent doesn't list the project"; raw '{"id":1,"cmd":"debug.menu"}'; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.menu\",\"args\":{\"item\":\"${repo/#$HOME/~}\"}}" | grep -q '"ok":true' || { echo "recent_projects: Open Recent's item didn't run"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d2 key)" == True ]]' || { echo "recent_projects: Open Recent didn't bring the project's Desk forward"; app_view; return 1; }
  check test "$(app_view | field windows | count)" == 2 || { echo "recent_projects: Open Recent opened another Desk"; kanna workspace list --all; return 1; }
  check wait_for 5 eval '[[ "$(kanna projects | head -1)" == "$repo" ]]' || { echo "recent_projects: Open Recent didn't move it to the top"; kanna projects; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Clear Menu"}}' | grep -q '"ok":true' || { echo "recent_projects: Clear Menu didn't run"; return 1; }
  check wait_for 5 eval '[[ -z "$(kanna projects)" ]]' || { echo "recent_projects: Clear Menu left projects"; kanna projects; return 1; }
}

scenario blocked_tasks M4
blocked_tasks() {
  start_daemon && start_app || { echo "blocked_tasks line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "blocked_tasks line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  in_repo task new "Screenshot command" "say hi" --agent "$agent" >/dev/null && in_repo task new "Status fix" --agent "$agent" >/dev/null || { echo "blocked_tasks line $LINENO"; return 1; }
  local first added; first="$(tid 1)"; added="$(in_repo task new "Diff viewer" "say hi" --agent "$agent" --after "${first:5:4}" 2>&1)"
  [[ "$added" == "added $(sid 3) \"Diff viewer\" to the backlog of $repo; it waits for $(sid 1)" ]] || { echo "blocked_tasks line $LINENO: $added"; return 1; }
  added="$(in_repo task new "Release notes" --agent "$agent" --after "$(tid 1),$(tid 2)" 2>&1)"
  [[ "$added" == "added $(sid 4) \"Release notes\" to the backlog of $repo; it waits for $(sid 1), $(sid 2)" ]] || { echo "blocked_tasks line $LINENO: $added"; return 1; }
  [[ "$(in_repo task new "Nowhere" --after t9 2>&1)" == *'no task "t9"'* ]] || { echo "blocked_tasks: a blocker that isn't a task was taken"; return 1; }
  [[ "$(in_repo task edit $(tid 1) --after $(tid 3) 2>&1)" == *"$(sid 3) waits for $(sid 1): they'd wait for each other"* ]] || { echo "blocked_tasks: a task was let wait for itself"; return 1; }
  in_repo task list | grep -qE '^'"$(sid 3)"' +Diff viewer +Backlog +blocked +mock-agent +made .* · waits for '"$(sid 1)"'$' || { in_repo task list; return 1; }
  [[ "$(in_repo task start $(tid 3) 2>&1)" == *''"$(sid 3)"' waits for '"$(sid 1)"' "Screenshot command": it can start once it'"'"'s done'* ]] || { echo "blocked_tasks: a blocked task started"; in_repo task list; return 1; }
  in_repo task shelve $(tid 3) >/dev/null 2>&1 && { echo "blocked_tasks: a blocked task moved to the Shelf"; return 1; }
  check test ! -e "$repo/.kanna-worktrees" || { echo "blocked_tasks: a blocked task got a worktree"; ls "$repo/.kanna-worktrees"; return 1; }
  stop_app; stop_daemon; start_daemon && start_app || { echo "blocked_tasks line $LINENO"; return 1; }
  in_repo task list | grep -qE '^'"$(sid 4)"' +Release notes +Backlog +blocked +mock-agent +made .* · waits for '"$(sid 1)"', '"$(sid 2)"'$' || { echo "blocked_tasks: blockers didn't survive a restart"; in_repo task list; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "blocked_tasks line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk scroll.blocked.0.task)" == $(tid 3) ]]' || { desk scroll; return 1; }
  check test "$(desk scroll.blocked.0.go)" == Blocked || { desk scroll.blocked; return 1; }
  check test "$(desk scroll.blocked.0.waits)" == '["Screenshot command"]' || { desk scroll.blocked; return 1; }
  check test "$(desk scroll.blocked.1.waits)" == '["Screenshot command", "Status fix"]' || { desk scroll.blocked; return 1; }
  sleep 0.5; snap "$w" blocked_tasks-blocked >/dev/null
  raw '{"id":1,"cmd":"debug.click","args":{"backlog":"'"$(tid 3)"'","window":"'"$w"'"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk note)" == *"waits for $(sid 1)"* ]]' || { echo "blocked_tasks: clicking a blocked row didn't say why"; desk note; return 1; }
  check test "$(desk running)" == 0 || { echo "blocked_tasks: clicking a blocked row started it"; return 1; }
  local started pane; started="$(kanna task shelve $(tid 1))" && [[ "$started" =~ ^"moved $(sid 1) to the Shelf: "(term-[0-9]+) ]] || { echo "blocked_tasks line $LINENO: $started"; return 1; }
  pane="${BASH_REMATCH[1]}"
  check wait_for 5 is "$pane" status idle || { echo "blocked_tasks line $LINENO"; return 1; }
  local done; done="$(kanna task done "$pane")" || { echo "blocked_tasks line $LINENO"; return 1; }
  [[ "$done" == *$'\n'"$(sid 3)"' "Diff viewer" can start now: nothing it waits for is left' && "$done" != *$(sid 4)* ]] || { echo "blocked_tasks: done didn't name what it freed: $done"; return 1; }
  check wait_for 10 eval '[[ "$(desk scroll.blocked.0.task)" == $(tid 4) && "$(desk scroll.blocked.0.waits)" == "[\"Status fix\"]" ]]' || { echo "blocked_tasks: the freed row is still blocked"; desk scroll.blocked; return 1; }
  in_repo task list | grep -qE '^'"$(sid 3)"' +Diff viewer +Backlog +stopped +mock-agent +made [^·]*· [^·]*$' || { in_repo task list; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"backlog":"'"$(tid 3)"'","window":"'"$w"'"}}' >/dev/null
  check wait_for 10 eval '[[ "$(desk running)" == 1 && "$(desk backlog)" == "[\"$(tid 2)\", \"$(tid 4)\"]" ]]' || { echo "blocked_tasks: the freed task didn't start"; desk backlog; desk note; return 1; }
  expected="$(sid 4) waits for nothing: it can start"; says in_repo task edit $(tid 4) --after none || { echo "blocked_tasks line $LINENO"; return 1; }
  in_repo task start $(tid 4) >/dev/null || { echo "blocked_tasks: a task with its blockers taken off didn't start"; return 1; }
  [[ "$(in_repo task edit $(tid 4) --after $(tid 2) 2>&1)" == *"$(sid 4) has started"* ]] || { echo "blocked_tasks: a started task's blockers changed"; return 1; }
}

scenario keeper_upgrade M4
keeper_upgrade() {
  local claude="${KANNA_E2E_CLAUDE:-$(command -v claude)}"
  [[ -x "$claude" ]] || { echo "keeper_upgrade: claude isn't installed (or set KANNA_E2E_CLAUDE)"; return 1; }
  need "$bin/kanna-daemon" && need "$bin/kanna-keeper" || return 1
  local old="$KANNA_STATE_DIR/old" agents="$KANNA_STATE_DIR/agents" repo="$KANNA_STATE_DIR/repo" harness="$repo_root/tools/agent-harness"
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
  kanna task shelve "$id" >/dev/null && kanna task pull "claude task" >/dev/null || { echo "keeper_upgrade: the claude task didn't reach the Desk"; return 1; }
  kanna task shelve "$parked" >/dev/null || { echo "keeper_upgrade: the parked task didn't go to the Shelf"; return 1; }
  check wait_for 60 eval '[[ "$(asked "$first")" == 1 ]]' || { echo "keeper_upgrade: claude's first message didn't reach the mock model"; kanna read "claude task"; return 1; }
  check wait_for 30 shows "claude task" "Agent status probe" || { echo "keeper_upgrade: claude didn't show the mock's answer"; kanna read "claude task"; return 1; }
  in_repo open --name upgradeshell --tab >/dev/null || { echo "keeper_upgrade line $LINENO"; return 1; }
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


scenario backlog_runners M4
backlog_runners() {
  start_daemon && start_app || { echo "backlog_runners line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "backlog_runners line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  runners() { printf '{"version": 1, "backlog": {"runners": %s}}\n' "$1" >"$repo/.kanna/config.json"; }
  backlog() { raw '{"id":1,"cmd":"backlog","args":{"action":"list"}}' | python3 -c 'import json, sys
tasks = json.load(sys.stdin)["tasks"]
for t in tasks: print(t["id"], "running" if t.get("running") else "stopped:" + (t.get("stopped") or "-") if t.get("folder") else "waiting", t.get("held") or "-", t.get("session") or "-")'; }
  running() { backlog | awk '$2 == "running" { print $1 }' | xargs; }
  of() { backlog | awk -v id="$1" '$1 == id { print $'"$2"' }'; }
  runners 0
  in_repo task new "Work one" "work 3" --agent "$fake/codex" >/dev/null && in_repo task new "Asks" "turn" --agent "$fake/codex" >/dev/null || { echo "backlog_runners line $LINENO"; return 1; }
  in_repo task new "Blocked" "work 1" --agent "$fake/codex" --after $(tid 1) >/dev/null || { echo "backlog_runners line $LINENO"; return 1; }
  for name in Four Five Six; do in_repo task new "Work $name" "work 3" --agent "$fake/codex" >/dev/null || { echo "backlog_runners line $LINENO"; return 1; }; done
  sleep 4
  check test -z "$(running)" || { echo "backlog_runners: backlog.runners 0 ran a task"; backlog; return 1; }
  check test ! -e "$repo/.kanna-worktrees" || { echo "backlog_runners: backlog.runners 0 made a worktree"; return 1; }
  runners 3
  check wait_for 10 eval '[[ "$(running)" == "$(tid 1) $(tid 2) $(tid 4)" ]]' || { echo "backlog_runners: three didn't start in order, skipping the blocked one"; backlog; return 1; }
  local seen="" most=0 now t
  for _ in $(seq 400); do
    now="$(running)"
    (( $(wc -w <<<"$now") > most )) && most=$(wc -w <<<"$now")
    for t in $now; do [[ " $seen " == *" $t "* ]] || seen="$seen $t"; done
    [[ "$(of $(tid 6) 2)" == stopped:* && "$(of $(tid 5) 2)" == stopped:* ]] && break
    sleep 0.1
  done
  check test "$most" -le 3 || { echo "backlog_runners: $most ran at once"; return 1; }
  check test "${seen# }" == "$(tid 1) $(tid 2) $(tid 4) $(tid 5) $(tid 6)" || { echo "backlog_runners: they started as $seen"; backlog; return 1; }
  check test "$(of $(tid 1) 2)" == stopped:finished || { backlog; return 1; }
  check test "$(of $(tid 2) 2)" == stopped:needs-you || { echo "backlog_runners: the task that asked didn't stop as needs you"; backlog; return 1; }
  check test "$(of $(tid 6) 2)" == stopped:finished || { backlog; return 1; }
  check test "$(of $(tid 3) 2)" == waiting || { echo "backlog_runners: the blocked task ran"; backlog; return 1; }
  local asks sid; asks="$(of $(tid 2) 3)" sid="$(of $(tid 1) 4)"
  [[ "$sid" =~ ^[0-9a-f-]{36}$ && "$(kanna read "$(of $(tid 1) 3)")" == *$'\n'"session $sid"$'\n'* ]] || { echo "backlog_runners: $(sid 1)'s session isn't the one it started ($sid)"; kanna read "$(of $(tid 1) 3)"; return 1; }
  sid="$(of $(tid 2) 4)"
  [[ "$sid" =~ ^[0-9a-f-]{36}$ ]] && unwrapped "$asks" "To continue this session, run: codex resume $sid" || { echo "backlog_runners: the session codex said as it quit wasn't recorded ($sid)"; backlog; kanna read "$asks"; return 1; }
  check is "$asks" state exited || { echo "backlog_runners: the stopped agent still runs"; return 1; }
  in_repo task list | grep -qE '^'"$(sid 2)"' +Asks +Backlog +stopped +codex +made ' || { echo "backlog_runners: task list shows how far a backlog task got"; in_repo task list; return 1; }
  raw '{"id":1,"cmd":"backlog","args":{"action":"events","task":"'"$(tid 2)"'"}}' | grep -q '"why": *"needs-you"' || { echo "backlog_runners: why $(sid 2) stopped isn't in its events"; return 1; }
  local opened w; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "backlog_runners line $LINENO: $opened"; return 1; }
  w="${BASH_REMATCH[1]}"
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$w\"}}" | field "$1"; }
  check wait_for 10 eval '[[ "$(desk backlog)" == "[\"$(tid 1)\", \"$(tid 2)\", \"$(tid 3)\", \"$(tid 4)\", \"$(tid 5)\", \"$(tid 6)\"]" ]]' || { echo "backlog_runners: the rows aren't oldest first"; desk backlog; return 1; }
  in_repo task done $(tid 1) >/dev/null || { echo "backlog_runners line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(of $(tid 3) 2)" != waiting ]]' || { echo "backlog_runners: the blocked task didn't run once it was free"; backlog; return 1; }
  [[ "$(kanna task shelve $(tid 2))" == "moved $(sid 2) to the Shelf: "* ]] || { echo "backlog_runners: the stopped task didn't move to the Shelf"; return 1; }
  check wait_for 5 eval '[[ "$(pane Asks id)" != "$asks" ]]' && asks="$(pane Asks id)" || { echo "backlog_runners: the started task kept its old terminal"; return 1; }
  check wait_for 10 shows "$asks" "resumed $sid" || { echo "backlog_runners: the stopped task didn't resume its session on the Shelf"; kanna read "$asks"; return 1; }
  unwrapped "$asks" "args: resume $sid --remote unix://" || { kanna read "$asks"; return 1; }
  sleep 6
  check is "$asks" state running || { echo "backlog_runners: a task on the Shelf was stopped"; return 1; }
  [[ "$(kanna task defer "$asks")" == *"to the backlog as $(sid 2)"* ]] || { echo "backlog_runners: the resumed task didn't move back to the backlog"; return 1; }
  check wait_for 15 eval '[[ "$(of $(tid 2) 2)" == stopped:finished ]]' || { echo "backlog_runners: a task moved to the backlog idle didn't stop"; backlog; return 1; }
  runners 0
  in_repo task new "Seven" "work 1" --agent "$fake/codex" >/dev/null || { echo "backlog_runners line $LINENO"; return 1; }
  sleep 4
  check test "$(of $(tid 7) 2)" == waiting || { echo "backlog_runners: backlog.runners 0 ran a task"; backlog; return 1; }
  in_repo task done $(tid 7) >/dev/null || { echo "backlog_runners line $LINENO"; return 1; }
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
  in_repo task new "Presents" "run $fake/present.sh" --agent "$fake/codex" >/dev/null || { echo "backlog_runners line $LINENO"; return 1; }
  runners 1
  check wait_for 60 eval '[[ "$(of $(tid 8) 2)" == stopped:finished ]]' || { echo "backlog_runners: the presenting task didn't stop"; backlog; cat "$KANNA_STATE_DIR/present.log"; return 1; }
  local presenter md long short; presenter="$(of $(tid 8) 3)"
  read -r md long short <"$KANNA_STATE_DIR/presented" || { echo "backlog_runners: the agent didn't present"; kanna read "$presenter"; cat "$KANNA_STATE_DIR/present.log"; return 1; }
  check is "$md" type md || { echo "backlog_runners: stopping closed the markdown tab"; kanna workspace list --all; return 1; }
  [[ "$(cat "$KANNA_STATE_DIR/opened")" =~ ^"opened $md in ws-"[0-9]+" ($(sid 8), in the backlog), tab-"[0-9]+": "[a-z]+$ ]] || { echo "backlog_runners: a held task's open said: $(cat "$KANNA_STATE_DIR/opened")"; return 1; }
  [[ "$(cat "$KANNA_STATE_DIR/focused")" =~ ^"focused $md in ws-"[0-9]+" ($(sid 8), in the backlog)"$ ]] || { echo "backlog_runners: a held task's focus said: $(cat "$KANNA_STATE_DIR/focused")"; return 1; }
  check is "$long" state exited || { echo "backlog_runners: stopping didn't hang up the running terminal"; kanna workspace list --all; return 1; }
  check is "$short" state exited || { echo "backlog_runners line $LINENO"; return 1; }
  held() { raw '{"id":1,"cmd":"debug.saveState"}' | python3 -c 'import json, sys
w = next(w for w in json.load(sys.stdin)["state"]["parked"] if (w.get("dock") or {}).get("pane") == sys.argv[1])
print(len(w["tabs"]), w.get("focused"), w.get("zoomed"))' "$presenter"; }
  check test "$(held)" == "3 $md $md" || { echo "backlog_runners: the stopped task's layout changed: $(held)"; return 1; }
  runners 0
  [[ "$(kanna task shelve $(tid 8))" == "moved $(sid 8) to the Shelf: "* ]] || { echo "backlog_runners: the presenting task didn't move to the Shelf"; return 1; }
  check wait_for 10 eval '[[ "$(pane Presents id)" != "$presenter" ]]' || { echo "backlog_runners: the task's agent didn't start again"; return 1; }
  presenter="$(pane Presents id)"
  rerun() { ws $(tid 8) panes | python3 -c 'import json, sys; print(" ".join(p for p in json.load(sys.stdin) if p not in sys.argv[1:]))' "$md" "$short" "$presenter"; }
  check wait_for 10 eval '[[ "$(rerun)" == term-* && "$(rerun)" != "$long" ]]' && long="$(rerun)" || { echo "backlog_runners: the hung-up terminal didn't run again as a new terminal"; kanna workspace list --all; return 1; }
  check wait_for 10 is "$long" state running || { echo "backlog_runners: the hung-up terminal didn't run again"; kanna workspace list --all; return 1; }
  check wait_for 5 shows "$long" LONG_RUNNING || { kanna read "$long"; return 1; }
  check is "$short" state exited || { echo "backlog_runners: a terminal that had exited before the stop ran again"; return 1; }
  local desk8; desk8="$(kanna task pull "$presenter")" && desk8="${desk8##* onto }" && [[ "$desk8" =~ ^d[0-9]+$ ]] || { echo "backlog_runners line $LINENO: $desk8"; return 1; }
  shown() { echo "$(dwin "$desk8" tabs | count) $(dwin "$desk8" focused) $(dwin "$desk8" zoomed)"; }
  check wait_for 5 eval '[[ "$(shown)" == "3 $md $md" ]]' || { echo "backlog_runners: the pulled task doesn't show what it presented: $(shown)"; app_view; return 1; }
  [[ "$(kanna task shelve $(tid 2))" == "moved $(sid 2) to the Shelf: "* ]] || { echo "backlog_runners line $LINENO"; return 1; }
  kanna task pull $(tid 2) >/dev/null || { echo "backlog_runners line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin "$desk8" dock.pane)" == "$(pane Asks id)" ]]' || { echo "backlog_runners: $(sid 2) didn't take $(sid 8)'s place on the Desk"; app_view; return 1; }
  [[ "$(ws $(tid 8) panes)" == *"\"$md\""* && "$(ws $(tid 2) panes)" != *"\"$md\""* ]] || { echo "backlog_runners: $(sid 8)'s tabs aren't in its workspace"; state; return 1; }
  [[ "$(KANNA_TERMINAL="$presenter" kanna open --name after --tab --no-wait)" == "opened "*" (after) in ws-"*" ($(sid 8), on the Shelf), tab-"* ]] || { echo "backlog_runners: $(sid 8)'s open went elsewhere"; state; return 1; }
}

scenario agent_messaging M4
agent_messaging() {
  start_daemon && start_app || { echo "agent_messaging line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$repo/.kanna" "$fake" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "agent_messaging line $LINENO"; return 1; }
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/codex"
  repo="$(cd "$repo" && pwd -P)"
  in_repo() { (cd "$repo" && kanna "$@"); }
  printf '{"version": 1, "backlog": {"runners": 1}}\n' >"$repo/.kanna/config.json"
  task_state() { raw '{"id":1,"cmd":"backlog","args":{"action":"list"}}' | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin)["tasks"] if t["id"] == sys.argv[1]][0]
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
  check wait_for 15 eval '[[ "$(task_state $busy)" == running* ]]' || { echo "agent_messaging: a message didn't wake the stopped backlog task"; task_state "$busy"; return 1; }
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
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/copilot"
  in_repo task new "Copilot" "work 1" --agent "$fake/copilot" >/dev/null || { echo "agent_messaging line $LINENO"; return 1; }
  said="$(in_repo task send "$(tid 4)" "can you hear me")"
  [[ "$said" == *$'\n'"warning: it waits: copilot can't take messages: only claude and codex can" ]] || { echo "agent_messaging: send to a copilot task said: $said"; return 1; }
  waits() { raw '{"id":1,"cmd":"debug.toasts"}' | python3 -c 'import json, sys
print(" ".join(t["pane"] + ":" + t["about"] for t in json.load(sys.stdin)["toasts"] if t["title"] == "Messages wait for Copilot"))'; }
  local copilot; copilot="$(task_state "$(tid 4)" | cut -d' ' -f2)"
  check wait_for 15 eval '[[ "$(waits)" == "$copilot:Copilot" ]]' || { echo "agent_messaging: the waiting message raised no toast about $copilot: $(waits)"; return 1; }
  sleep 2
  check test "$(waits)" == "$copilot:Copilot" || { echo "agent_messaging: the same reason raised another toast: $(waits)"; return 1; }
  check wait_for 15 eval '[[ "$(in_repo task messages "$(tid 4)")" == *"held for $(sid 4): copilot can'"'"'t take messages"* ]]' || { echo "agent_messaging: task messages shows: $(in_repo task messages "$(tid 4)")"; return 1; }
}

scenario task_scripts M4
task_scripts() {
  start_daemon && start_app || { echo "task_scripts line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" agent="$KANNA_STATE_DIR/agent"
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
  local started wt p1; started="$(kanna task shelve $(tid 1))" || { echo "task_scripts line $LINENO"; return 1; }
  [[ "$started" =~ ^"moved $(sid 1) to the Shelf: term-"[0-9]+" in "([^ ]+)" (" ]] || { echo "task_scripts line $LINENO: $started"; return 1; }
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
  check wait_for 5 eval 'in_repo task list | grep -qE "^$(sid 1) +Sets up +Shelf +teardown failed \(exit 2\)"' || { echo "task_scripts: task list doesn't say teardown failed"; in_repo task list; return 1; }
  rm -f "$KANNA_STATE_DIR/teardown-fails" "$KANNA_STATE_DIR/torn"
  kanna task done "$p1" >/dev/null || { echo "task_scripts line $LINENO"; return 1; }
  check wait_for 10 eval '[[ ! -e "$wt" ]]' || { echo "task_scripts: the merged task's worktree wasn't removed"; kanna read "$p1"; raw '{"id":1,"cmd":"backlog","args":{"action":"events","task":"'"$(tid 1)"'"}}'; return 1; }
  check test "$(cat "$KANNA_STATE_DIR/torn")" == "KANNA_TASK=$(tid 1)" || { echo "task_scripts: teardown didn't run again on Done"; return 1; }
  kanna read "$p1" >/dev/null 2>&1 && { echo "task_scripts: the agent's pane stayed after teardown succeeded"; return 1; }
  check test "$(cat "$elsewhere/kanna")" == built || { echo "task_scripts: cleanup removed what target/ links to"; return 1; }
  git -C "$repo" rev-parse --verify --quiet "refs/heads/${wt##*/}" >/dev/null && { echo "task_scripts: the merged branch is left"; return 1; }
  raw '{"id":1,"cmd":"backlog","args":{"action":"events","task":"'"$(tid 1)"'"}}' | grep -q '"cleaned up"' || { echo "task_scripts: cleanup isn't in the first task's events"; return 1; }
  rm -f "$KANNA_STATE_DIR/torn"
  started="$(kanna task shelve $(tid 2))" || { echo "task_scripts line $LINENO"; return 1; }
  [[ "$started" =~ ^"moved $(sid 2) to the Shelf: term-"[0-9]+" in "([^ ]+)" (" ]] || { echo "task_scripts line $LINENO: $started"; return 1; }
  local p2="Fails" wt2="${BASH_REMATCH[1]}"
  check wait_for 15 is "$p2" state exited || { echo "task_scripts: a failed setup didn't stop the task"; kanna read "$p2"; return 1; }
  shows "$p2" "setup failed (exit 3)" || { echo "task_scripts: the terminal doesn't say setup failed"; kanna read "$p2"; return 1; }
  shows "$p2" "agent sees:" && { echo "task_scripts: the agent started after a failed setup"; return 1; }
  check wait_for 5 eval 'in_repo task list | grep -qE "^$(sid 2) +Fails +Shelf +setup failed \(exit 3\)"' || { echo "task_scripts: task list doesn't say setup failed"; in_repo task list; return 1; }
  raw '{"id":1,"cmd":"backlog","args":{"action":"events","task":"'"$(tid 2)"'"}}' | grep -q '"setup failed"' || { echo "task_scripts: the failure isn't in t2's events"; return 1; }
  kanna task start "$p2" >/dev/null || { echo "task_scripts line $LINENO"; return 1; }
  check wait_for 10 shows "$p2" "agent sees:" || { echo "task_scripts: start didn't start the agent anyway"; kanna read "$p2"; return 1; }
  in_repo task list | grep -qE "^$(sid 2) +Fails +Shelf +setup failed" && { echo "task_scripts: a task started anyway still says setup failed"; return 1; }
  [[ "$(kanna task setup "$p2")" == "$(sid 2)'s setup runs again in "*"(a Setup tab)" ]] || { echo "task_scripts: task setup didn't open a Setup tab"; return 1; }
  check wait_for 10 eval '[[ "$(cat "$KANNA_STATE_DIR/setups" | xargs)" == "$(tid 1) $(tid 1) $(tid 2) $(tid 2)" ]]' || { echo "task_scripts: task setup didn't run it again: $(cat "$KANNA_STATE_DIR/setups" | xargs)"; return 1; }
  echo work >"$wt2/work.txt" && git -C "$wt2" add work.txt && git -C "$wt2" -c user.email=e@e -c user.name=e commit -q -m work || { echo "task_scripts line $LINENO"; return 1; }
  kanna task done "$p2" >/dev/null || { echo "task_scripts line $LINENO"; return 1; }
  check wait_for 10 eval 'raw "{\"id\":1,\"cmd\":\"backlog\",\"args\":{\"action\":\"events\",\"task\":\"$(tid 2)\"}}" | grep -q "isn.t merged into main"' || { echo "task_scripts: an unmerged task wasn't kept, saying why"; raw '{"id":1,"cmd":"backlog","args":{"action":"events","task":"'"$(tid 2)"'"}}'; return 1; }
  check test "$(cat "$KANNA_STATE_DIR/torn")" == "KANNA_TASK=$(tid 2)" || { echo "task_scripts: teardown didn't run on t2's Done"; return 1; }
  check test -f "$wt2/work.txt" && git -C "$repo" rev-parse --verify --quiet "refs/heads/${wt2##*/}" >/dev/null || { echo "task_scripts: the unmerged worktree or branch went"; return 1; }
  rm -f "$KANNA_STATE_DIR/torn"
  started="$(kanna task shelve $(tid 3))" || { echo "task_scripts line $LINENO"; return 1; }
  [[ "$started" =~ ^"moved $(sid 3) to the Shelf: term-"[0-9]+" in "([^ ]+)" (" ]] || { echo "task_scripts line $LINENO: $started"; return 1; }
  local p3="Kept" wt3="${BASH_REMATCH[1]}"
  check wait_for 15 shows "$p3" "agent sees:" || { echo "task_scripts line $LINENO"; kanna read "$p3"; return 1; }
  [[ "$(kanna task done "$p3" --keep)" == "$(sid 3) is done: closed "* ]] || { echo "task_scripts: done --keep didn't end the task"; return 1; }
  check wait_for 10 eval 'raw "{\"id\":1,\"cmd\":\"backlog\",\"args\":{\"action\":\"events\",\"task\":\"$(tid 3)\"}}" | grep -q "kanna task done --keep"' || { echo "task_scripts: done --keep isn't in the task's events"; return 1; }
  check test ! -e "$KANNA_STATE_DIR/torn" && check test -d "$wt3" || { echo "task_scripts: done --keep ran teardown or removed the worktree"; return 1; }
}

scenario pane_sizes M4
pane_sizes() {
  local repo="$KANNA_STATE_DIR/repo" want
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
  sizer docked && sizer shelved && sizer backlogged
  saw() { sort -u "$KANNA_STATE_DIR/$1.sizes" 2>/dev/null | xargs; }
  view() { echo "$(terminal rows "{\"pane\":\"$1\"}")x$(terminal cols "{\"pane\":\"$1\"}")"; }
  only() { [[ "$(saw docked)" == "$1" && "$(saw shelved)" == "$1" ]]; }
  start_daemon && start_app || { echo "pane_sizes line $LINENO"; return 1; }
  kanna open --name docked --dock --cwd "$repo" --cmd "$KANNA_STATE_DIR/docked" >/dev/null && kanna task pull docked >/dev/null || { echo "pane_sizes line $LINENO"; return 1; }
  (cd "$repo" && kanna task new Shelved "go" --agent "$KANNA_STATE_DIR/shelved") >/dev/null && kanna task shelve $(tid 1) >/dev/null || { echo "pane_sizes line $LINENO"; return 1; }
  check wait_for 10 eval '[[ -s "$KANNA_STATE_DIR/docked.sizes" && -s "$KANNA_STATE_DIR/shelved.sizes" ]]' || { echo "pane_sizes: the programs didn't start"; return 1; }
  sleep 1; want="$(view docked)"
  [[ "$want" =~ ^[0-9]+x[0-9]+$ && "$want" != 24x80 ]] || { echo "pane_sizes: the docked pane is $want"; return 1; }
  check only "$want" || { echo "pane_sizes: the agents saw $(saw docked) and $(saw shelved), not only the Desk's $want"; return 1; }
  (cd "$repo" && kanna task new Backlogged "go" --agent "$KANNA_STATE_DIR/backlogged") >/dev/null && kanna task start $(tid 2) >/dev/null || { echo "pane_sizes line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(tail -1 "$KANNA_STATE_DIR/backlogged.sizes" 2>/dev/null)" == "$want" ]]' || { echo "pane_sizes: the agent running in the backlog is $(tail -1 "$KANNA_STATE_DIR/backlogged.sizes" 2>/dev/null), not the Desk's $want"; return 1; }
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
  check wait_for 5 eval '[[ "$(last docked):$(last shelved):$(last backlogged)" == "$wider:$wider:$wider" ]]' || { echo "pane_sizes: in a bigger window the agents are $(last docked), $(last shelved) and $(last backlogged), not $wider"; return 1; }
  kanna task pull Shelved >/dev/null && sleep 1 || { echo "pane_sizes line $LINENO"; return 1; }
  check test "$(view Shelved)" == "$wider" || { echo "pane_sizes: pulled onto the Desk, the agent's view is $(view Shelved), not $wider"; return 1; }
  check test "$(tail -1 "$KANNA_STATE_DIR/shelved.sizes")" == "$wider" || return 1
}

scenario backlog_rename M4
backlog_rename() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_STATE_DIR/repo"
  mkdir -p "$repo/.kanna" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "backlog_rename line $LINENO"; return 1; }
  printf '{"version": 1, "backlog": {"runners": 0}}\n' >"$repo/.kanna/config.json"
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "backlog_rename line $LINENO"; return 1; }
  in_repo() { (cd "$repo" && kanna "$@"); }
  named() { raw '{"id":1,"cmd":"backlog","args":{"action":"list"}}' | python3 -c 'import json, sys; t = json.load(sys.stdin)["tasks"][0]; print(t["name"], "|", t["message"])'; }
  in_repo task new "Old name" "work 1" --agent "$agent" >/dev/null || { echo "backlog_rename line $LINENO"; return 1; }
  check test "$(in_repo task name $(tid 1) "New name")" == 'named '"$(sid 1)"' "New name"' || { echo "backlog_rename: task name on a backlog task"; return 1; }
  check test "$(named)" == "New name | work 1" || { echo "backlog_rename: the entry kept its old name, or its first message changed"; named; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  desk() { raw '{"id":1,"cmd":"debug.desk","args":{"window":"d1"}}' | field "$1"; }
  check wait_for 10 eval '[[ "$(desk backlog)" == "[\"$(tid 1)\"]" ]]' || { desk backlog; return 1; }
  in_repo task shelve $(tid 1) >/dev/null || { echo "backlog_rename line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(desk shelf.0.title)" == "New name" ]]' || { echo "backlog_rename: the Shelf doesn't show the new name"; desk shelf; return 1; }
  kanna workspace list --all --json | python3 -c 'import json, sys; sys.exit(not any(p.get("name") == "New name" for p in json.load(sys.stdin)["panes"]))' || { echo "backlog_rename: the started task's pane isn't named from its entry"; kanna workspace list --all; return 1; }
}

scenario done_leaves_no_window M4
done_leaves_no_window() {
  start_daemon && start_app || { echo "done_leaves_no_window line $LINENO"; return 1; }
  local repo="$KANNA_STATE_DIR/repo" agent="$repo_root/tools/mock-agent/mock-agent"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "done_leaves_no_window line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  (cd "$repo" && kanna task new "With diff" "say 1" --agent "$agent") >/dev/null || { echo "done_leaves_no_window line $LINENO"; return 1; }
  kanna task shelve "$(tid 1)" >/dev/null || { echo "done_leaves_no_window line $LINENO"; return 1; }
  local task; task="$(pane "With diff" id)"
  (cd "$repo" && KANNA_TERMINAL="$task" kanna open diff --merge-base main --name changes --tab) >/dev/null || { echo "done_leaves_no_window line $LINENO"; return 1; }
  stop_daemon; start_daemon && check wait_for 10 app_attached || { echo "done_leaves_no_window line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(pane changes type)" == diff ]]' || { echo "done_leaves_no_window: the diff tab didn't survive the daemon's restart"; kanna workspace list --all; return 1; }
  kanna task done "$(tid 1)" >/dev/null || { echo "done_leaves_no_window line $LINENO"; return 1; }
  leftovers() { kanna workspace list --all --json | python3 -c 'import json, sys; s = json.load(sys.stdin); print(len(s["windows"]) + len(s.get("hiddenWindows") or []), len(s["panes"]))'; }
  check wait_for 5 eval '[[ "$(leftovers)" == "0 0" ]]' || { echo "done_leaves_no_window: windows and panes left: $(leftovers)"; kanna workspace list --all; return 1; }
}

scenario toasts M4
toasts() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_STATE_DIR/repo"
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "toasts line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "toasts line $LINENO"; return 1; }
  check test "$(kanna toast "Early" "raised before the app")" == "raised toast-1; no app is attached, so it shows when one attaches within a minute" || return 1
  (cd "$repo" && kanna task new "Lint fixes" "say 1" --agent "$agent" && kanna task new "Docs pass" "say 2" --agent "$agent") >/dev/null || { echo "toasts line $LINENO"; return 1; }
  kanna task shelve "$(tid 1)" >/dev/null || { echo "toasts line $LINENO"; return 1; }
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
  check wait_for 15 eval '[[ "$(desk title)" == "Docs pass"* ]]' || { echo "toasts: clicking a backlog task's toast didn't shelve and show it: $(desk title)"; desk note; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 focused)" == "$(pane "Docs pass" id)" ]]' || { echo "toasts: the backlog task's agent isn't focused"; return 1; }
  kanna toast "Kept" "the pointer is on it" >/dev/null && kanna toast "Fades" >/dev/null || { echo "toasts line $LINENO"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"hover":"toast-5"}}' >/dev/null
  check wait_for 12 eval '[[ "$(toasted)" == "d1 toast-5" ]]' || { echo "toasts: after 6s only the hovered toast should be left: $(toasted)"; return 1; }
  raw '{"id":1,"cmd":"debug.toasts","args":{"hover":"toast-5","on":false}}' >/dev/null
  check wait_for 5 eval '[[ "$(toasted)" == "d1 " ]]' || { echo "toasts: the toast left by the pointer didn't fade: $(toasted)"; return 1; }
  kanna toast "Closed" >/dev/null && check wait_for 5 eval '[[ "$(toasted)" == "d1 toast-7" ]]' || return 1
  raw '{"id":1,"cmd":"debug.toasts","args":{"click":"toast-7","close":true}}' >/dev/null
  check wait_for 2 eval '[[ "$(toasted)" == "d1 " ]]' || { echo "toasts: × didn't close it"; return 1; }
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
  if [[ $result -eq 0 ]]; then
    printf '%-20s PASS (%s)\n' "$s" "$m"; passed=$((passed + 1))
  else
    printf '%-20s FAIL (%s)  %s\n' "$s" "$m" "$(tail -1 "$out/$s.log")"; failed=$((failed + 1))
  fi
done
echo "$passed passed, $failed failed"
[[ $failed -eq 0 ]]
