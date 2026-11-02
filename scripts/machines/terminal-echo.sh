#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/machines/terminal-echo.sh DIR LABEL [--remote SSH-HOST [--lan-proxy]] [--relay URL [--relay-token-file FILE] [--relay-only]] [--samples N]

Starts a private daemon on this Mac in DIR/mbp and a second one, in DIR/peer here or in
~/kanna-echo on SSH-HOST (which needs kanna-daemon, kanna-keeper and kanna built for it
in ~/kanna-echo/bin), pairs them, and runs the terminal-echo example from this Mac against
the second: link round trips, keystroke echo idle and while another terminal floods, and
attach sizes. The JSON report is DIR/LABEL.json. Neither daemon uses your own Kanna's
state, HOME or socket; both are stopped at the end.
--lan-proxy sends this Mac's side through a python UDP forwarder on 127.0.0.1, for binaries
macOS Local Network privacy hasn't allowed to reach the LAN ("No route to host").
EOF
  exit 2
}

[[ $# -ge 2 ]] || usage
dir="$1" label="$2"; shift 2
remote="" relay="" token="" relay_only="" samples=100 proxy=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --remote) remote="$2"; shift ;;
    --lan-proxy) proxy=1 ;;
    --relay) relay="$2"; shift ;;
    --relay-token-file) token="$2"; shift ;;
    --relay-only) relay_only=1 ;;
    --samples) samples="$2"; shift ;;
    *) usage ;;
  esac
  shift
done

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cargo build -q --release --manifest-path "$repo/Cargo.toml" -p kanna-daemon -p kanna-keeper -p kanna
cargo build -q --release --manifest-path "$repo/Cargo.toml" -p kanna-net --example terminal-echo
bin="$(cd "$repo" && cargo metadata --format-version 1 --no-deps | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/release"

mkdir -p "$dir"
dir="$(cd "$dir" && pwd)"
pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; done
  for state in "$dir"/mbp/state "$dir"/peer/state; do
    [[ -f "$state/keeper.pid" ]] && kill "$(cat "$state/keeper.pid")" 2>/dev/null || true
  done
  [[ -n "$remote" ]] && ssh "$remote" 'cd ~/kanna-echo && kill $(cat daemon.pid) $(cat state/keeper.pid) 2>/dev/null; rm -f relay.token; true' || true
}
trap cleanup EXIT
exec 3>&2

daemon() {
  local home="$1"
  mkdir -p "$home/state" "$home/home" && chmod 700 "$home"
  HOME="$home/home" SHELL=/bin/sh KANNA_SOCKET="$home/kanna.sock" KANNA_STATE_DIR="$home/state" KANNA_KEEPER="$bin/kanna-keeper" \
    KANNA_AGENT_RULES_URL=none KANNA_MACHINE_MDNS=0 KANNA_MACHINE_PORT=0 "$bin/kanna-daemon" >>"$home/daemon.log" 2>&1 &
  pids+=($!)
  for _ in $(seq 50); do [[ -S "$home/kanna.sock" ]] && return 0; sleep 0.1; done
  echo "the daemon in $home didn't open its socket" >&2; return 1
}
mbp() { KANNA_SOCKET="$dir/mbp/kanna.sock" KANNA_STATE_DIR="$dir/mbp/state" "$bin/kanna" "$@"; }
if [[ -n "$remote" ]]; then
  ssh "$remote" 'cd ~/kanna-echo && rm -rf state home kanna.sock && mkdir -p state home && chmod 700 . &&
    (HOME=$PWD/home SHELL=/bin/sh KANNA_SOCKET=$PWD/kanna.sock KANNA_STATE_DIR=$PWD/state KANNA_KEEPER=$PWD/bin/kanna-keeper \
     KANNA_AGENT_RULES_URL=none KANNA_MACHINE_MDNS=0 KANNA_MACHINE_PORT=7471 nohup bin/kanna-daemon >>daemon.log 2>&1 & echo $! >daemon.pid) &&
    for i in $(seq 50); do [ -S kanna.sock ] && exit 0; sleep 0.1; done; exit 1'
  peer() { ssh "$remote" "cd ~/kanna-echo && KANNA_SOCKET=\$PWD/kanna.sock KANNA_STATE_DIR=\$PWD/state bin/kanna $(printf '%q ' "$@")"; }
  host="$(ssh -G "$remote" | awk '/^hostname /{print $2}')"
  if [[ -n "$proxy" ]]; then
    python3 -u - "$host" 7471 >"$dir/proxy.port" <<'PY' &
import select, socket, sys
target = (sys.argv[1], int(sys.argv[2]))
front = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); front.bind(("127.0.0.1", 0))
back = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); back.bind(("0.0.0.0", 0))
print(front.getsockname()[1], flush=True)
client = None
while True:
    for s in select.select([front, back], [], [])[0]:
        data, addr = s.recvfrom(65536)
        if s is front:
            client = addr
            back.sendto(data, target)
        elif client and addr == target:
            front.sendto(data, client)
PY
    pids+=($!)
    for _ in $(seq 50); do [[ -s "$dir/proxy.port" ]] && break; sleep 0.1; done
  fi
else
  daemon "$dir/peer"
  peer() { KANNA_SOCKET="$dir/peer/kanna.sock" KANNA_STATE_DIR="$dir/peer/state" "$bin/kanna" "$@"; }
  host=127.0.0.1
fi
daemon "$dir/mbp"
mbp machine name mbp >/dev/null
peer machine name peer >/dev/null

python3 - "$dir/mbp/kanna.sock" <<'PY' &
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f = s.makefile("rw")
f.write(json.dumps({"id": 0, "cmd": "app.hello", "args": {"pid": 1, "protocols": [1, 99]}}) + "\n"); f.flush()
for line in f:
    m = json.loads(line)
    if m.get("cmd") == "app.machine.confirm":
        f.write(json.dumps({"id": m["id"], "ok": True, "confirmed": True}) + "\n"); f.flush()
        break
PY
confirmer=$!
peer machine pair --json >"$dir/offer" &
waiting=$!
for _ in $(seq 50); do [[ -s "$dir/offer" ]] && break; sleep 0.1; done
code="$(head -1 "$dir/offer" | python3 -c 'import json, sys; print(json.load(sys.stdin)["code"])')"
key="$(head -1 "$dir/offer" | python3 -c 'import json, sys; print(json.load(sys.stdin)["key"])')"
port="$(head -1 "$dir/offer" | python3 -c 'import json, sys; print(json.load(sys.stdin)["port"])')"
if [[ -n "$proxy" ]]; then host=127.0.0.1 port="$(cat "$dir/proxy.port")"; fi
mbp machine pair "$code" --peer "$key@$host:$port"
wait "$waiting" "$confirmer"
kill "$confirmer" 2>/dev/null || true

addr=(--addr "$host:$port")
relayed=()
if [[ -n "$relay" ]]; then
  if [[ -n "$token" ]]; then
    mbp machine relay "$relay" --token-file "$token" >/dev/null
    if [[ -n "$remote" ]]; then
      ssh "$remote" 'umask 077 && cat >~/kanna-echo/relay.token' <"$token"
      peer machine relay "$relay" --token-file relay.token >/dev/null
    else
      peer machine relay "$relay" --token-file "$token" >/dev/null
    fi
    relayed=(--relay "$relay" --relay-token-file "$token")
  else
    mbp machine relay "$relay" >/dev/null
    peer machine relay "$relay" >/dev/null
    relayed=(--relay "$relay")
  fi
  [[ -n "$relay_only" ]] && relayed+=(--relay-only) && addr=()
  sleep 2
fi
"$bin/examples/terminal-echo" --socket "$dir/mbp/kanna.sock" --peer "$key" ${addr[@]+"${addr[@]}"} ${relayed[@]+"${relayed[@]}"} \
  --samples "$samples" --label "$label" --output "$dir/$label.json"
