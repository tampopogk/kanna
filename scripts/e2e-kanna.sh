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
Waits, speed bounds and that limit stretch by $KANNA_E2E_SLOW (load per core,
1 to 6). The run wakes the display and keeps it on: with it asleep, windows
don't draw and clicks, scrolls and web panes go wrong. A locked screen is fine.
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
real_agent_tasks, real_agent_messages, real_agent_planners, real_agent_last_words, real_agent_program_status and keeper_upgrade run the real claude and codex (on PATH,
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

slowness() { python3 -c 'import math, os; print(max(1, min(6, math.ceil(os.getloadavg()[0] / os.cpu_count()))))'; }
stretch="${KANNA_E2E_SLOW:-$(slowness)}"
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
  export KANNA_GHOSTTY_CONFIG="$KANNA_STATE_DIR/ghostty/config"
  mkdir -p "${KANNA_GHOSTTY_CONFIG%/*}" && : >"$KANNA_GHOSTTY_CONFIG"
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
  local tries=$(($1 * 10 * stretch)); shift
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

says() { local got; got="$("$@" 2>&1)"; [[ "$got" == "$expected" ]] || { echo "kanna ${*:2}: expected \"$expected\", got \"$got\""; return 1; }; }

app_text() { raw "{\"id\":1,\"cmd\":\"debug.text\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q -- "$2"; }

doc_text() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$1\"}}" | field text | grep -q -- "$2"; }

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
drew() { check wait_for 10 redrawn "$1" "$2" "$3" || { echo "$shot"; return 1; }; }
click_tab() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"tab\":\"$1\",\"clicks\":${2:-1}}}" >/dev/null; sleep 0.3; }
drag() { last_drag="$1 → $(raw "{\"id\":1,\"cmd\":\"debug.drag\",\"args\":$1}")"; sleep 0.3; }

near() { python3 -c 'import sys
a, b = (bytes.fromhex(x.lstrip("#")) for x in sys.argv[1:3])
sys.exit(0 if max(abs(x - y) for x, y in zip(a, b)) <= 16 else 1)' "$1" "$2"; }

phone() { "$rust_dir/phone-client" "$phone_dir" "$@"; }

machine_paired_vm() {
  export KANNA_MACHINE_MDNS=0 KANNA_MACHINE_PORT=0
  start_daemon || { echo "$1 line $LINENO"; return 1; }
  vm="$KANNA_STATE_DIR/machine-vm"
  mkdir -p "$vm/state" && chmod 700 "$vm"
  vmk() { KANNA_SOCKET="$vm/kanna.sock" KANNA_STATE_DIR="$vm/state" "$bin/kanna" "$@"; }
  vraw() { KANNA_SOCKET="$vm/kanna.sock" raw "$@"; }
  KANNA_AGENTS_HOME="${2:-$KANNA_AGENTS_HOME}" KANNA_SOCKET="$vm/kanna.sock" KANNA_STATE_DIR="$vm/state" KANNA_KEEPER="$bin/kanna-keeper" "$bin/kanna-daemon" >>"$out/daemon-vm.log" 2>&1 &
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
}

for area in "$repo_root"/scripts/e2e/*.sh; do . "$area"; done
in_run_order() {
  local names order="$repo_root/scripts/e2e/run-order"
  names="$(printf '%s\n' "${scenarios[@]%%:*}")"
  grep -xFf <(echo "$names") "$order"
  grep -vxFf "$order" <<<"$names"
}

for s in ${only[@]+"${only[@]}"}; do due "$s" >/dev/null || { echo "e2e-kanna: no scenario named $s (see --help)" >&2; exit 2; }; done
if [[ ${#only[@]} -gt 0 ]]; then run=("${only[@]}"); else run=($(in_run_order)); fi
passed=0 failed=0 limit="${KANNA_E2E_TIMEOUT:-600}"
caffeinate -d -w $$ &
for s in "${run[@]}"; do
  m="$(due "$s")"
  if [[ "$m" > "$through" ]]; then printf '%-20s --   not due (%s)\n' "$s" "$m"; continue; fi
  cleanup
  new_state_dir
  stretch="${KANNA_E2E_SLOW:-$(slowness)}"
  caffeinate -u -t 5 &
  run_scenario "$s" >"$out/$s.log" 2>&1 &
  scenario_pid=$! ends=$((SECONDS + limit * stretch))
  echo "$scenario_pid" >"$KANNA_STATE_DIR/e2e-scenario.pid"
  while kill -0 "$scenario_pid" 2>/dev/null && [[ $SECONDS -lt $ends ]]; do sleep 0.2; done
  if kill -0 "$scenario_pid" 2>/dev/null; then
    echo "$s didn't finish in $((limit * stretch))s (KANNA_E2E_TIMEOUT)" >>"$out/$s.log"; result=1
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
