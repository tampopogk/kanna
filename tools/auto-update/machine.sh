kanna_a="$T/A/Kanna.app/Contents/Helpers"
stop_instance
rm -f "$T/offer" "$T/confirmed" "$T/peer-ticks"
reinstall
peer="$(mktemp -d /tmp/claude/kau.XXXX)"
mkdir -p "$peer/state" && chmod 700 "$peer"
export KANNA_MACHINE_MDNS=0 KANNA_MACHINE_PORT=0
peerk() { KANNA_SOCKET="$peer/kanna.sock" KANNA_STATE_DIR="$peer/state" "$kanna_a/kanna" "$@"; }
start_peer() {
  KANNA_MACHINE_PORT=38472 KANNA_SOCKET="$peer/kanna.sock" KANNA_STATE_DIR="$peer/state" KANNA_KEEPER="$1/kanna-keeper" "$1/kanna-daemon" >>"$T/peer.log" 2>&1 &
  peer_pid=$!
  wait_for 10 test -S "$peer/kanna.sock" || fail "the peer didn't open its socket"
}
trap 'kill $server $peer_pid 2>/dev/null || true' EXIT
mkdir -p "$state"
env -u KANNA_STATE_DIR KANNA_KEEPER="$install/Contents/Helpers/kanna-keeper" "$install/Contents/Helpers/kanna-daemon" >>"$T/daemon-solo.log" 2>&1 &
solo=$!
wait_for 10 test -S "$KANNA_SOCKET" || fail "no daemon"
start_peer "$kanna_a"
"$kanna_a/kanna" machine name mbp >/dev/null; peerk machine name linux-1 >/dev/null
python3 -I - "$KANNA_SOCKET" 4 >"$T/confirmed" <<'PY' &
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
confirmer=$!
peerk machine pair --json >"$T/offer" &
waiting=$!
wait_for 10 test -s "$T/offer" || fail "no pairing code"
code="$(head -1 "$T/offer" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["code"])')"
peer_at="$(head -1 "$T/offer" | python3 -I -c 'import json, sys; o = json.load(sys.stdin); print("%s@127.0.0.1:%s" % (o["key"], o["port"]))')"
"$kanna_a/kanna" machine pair "$code" --peer "$peer_at"
wait "$waiting" "$confirmer" || true
[[ "$(cat "$T/confirmed")" == linux-1 ]] || fail "the pairing wasn't confirmed"
opened="$(peerk open --window new --name remote --cmd "while :; do date +%s >>'$T/peer-ticks'; sleep 0.2; done" --json)"
remote_pid="$(echo "$opened" | python3 -I -c 'import json, sys; p = json.load(sys.stdin)["pane"]; print(p.get("pid", ""))')"
launch_keep() {
  env -u KANNA_SOCKET -u KANNA_TERMINAL -u KANNA_PANE -u KANNA_STATE_DIR KANNA_DEBUG=1 KANNA_IGNORE_OCCLUSION=1 "$install/Contents/MacOS/Kanna" --bg >>"$T/app.log" 2>&1 &
  app_pid=$!
  wait_for 30 app_attached || fail "the app didn't attach"
}
machines() { raw '{"id":1,"cmd":"debug.machines"}' | python3 -I -c 'import json, sys
m = json.load(sys.stdin)["links"][0]
print(m["machine"], m["attached"], (m.get("failure") or {}).get("code"))'; }
speaks=5
build_b="$(cat "$T/build-c")"
feed_for "$served/Kanna-C.dmg" "$T/test.key" Kanna-C.dmg >"$served/feed.xml"
"$sparkle_bin/sign_update" --ed-key-file "$T/test.key" "$served/feed.xml"
launch
wait_for 30 eval '[[ "$(machines)" == "linux-1 True"* ]]' || fail "the app didn't link to linux-1: $(raw '{"id":1,"cmd":"debug.machines"}')"
wait_for 40 titled "is available" || fail "no toast"
warning="$(toast_field warning)"
note "toast warning: $warning"
[[ "$warning" == "linux-1 will show Incompatible until you update it." ]] || fail "the warning is: $warning"
raw '{"id":3,"cmd":"debug.toasts","args":{"png":"auto-update-product-protocol.png"}}' >/dev/null
keeper="$(cat "$state/keeper.pid")"
press "Install and Relaunch"
wait_for 120 eval '[[ "$(installed_build)" == "$build_b" ]]' || fail "C never installed"
newapp() { pgrep -f "^$install/Contents/MacOS/Kanna" | grep -vx "$app_pid" | head -1; }
wait_for 60 eval '[[ -n "$(newapp)" ]]' || fail "C wasn't relaunched"
new="$(newapp)"
wait_for 60 eval '"$install/Contents/Helpers/kanna" workspace list --all --json >/dev/null 2>&1' || fail "no daemon after the relaunch"
note "C relaunched as pid $new, keeper $keeper -> $(cat "$state/keeper.pid")"
kill -TERM "$new"; wait_for 20 eval '! ps -p $new >/dev/null' || fail "C didn't quit"
app_pid=0
launch_keep
wait_for 30 eval '[[ "$(machines)" == "linux-1 False other_protocol" ]]' || fail "linux-1 isn't Incompatible: $(raw '{"id":1,"cmd":"debug.machines"}')"
note "after the update: $(machines)"
raw '{"id":1,"cmd":"debug.machines"}' | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["links"][0]["failure"])'
ps -p "$peer_pid" >/dev/null || fail "the peer stopped"
n="$(wc -l <"$T/peer-ticks")"; sleep 2; [[ "$(wc -l <"$T/peer-ticks")" -gt "$n" ]] || fail "the peer's program stopped"
note "the peer's daemon and program kept running"
kill -TERM "$peer_pid"; wait "$peer_pid" 2>/dev/null || true
start_peer "$install/Contents/Helpers"
wait_for 60 eval '[[ "$(machines)" == "linux-1 True"* ]]' || fail "updating linux-1 didn't bring it back: $(raw '{"id":1,"cmd":"debug.machines"}')"
note "updated linux-1 is back: $(machines)"
n="$(wc -l <"$T/peer-ticks")"; sleep 2; [[ "$(wc -l <"$T/peer-ticks")" -gt "$n" ]] || fail "the peer's program stopped across its update"
note "its program kept running"
stop_app -TERM
KANNA_SOCKET="$peer/kanna.sock" "$install/Contents/Helpers/kanna" instances stop >/dev/null 2>&1 || true
