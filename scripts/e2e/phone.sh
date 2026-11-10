phone_agent() {
  phone_fake="$KANNA_STATE_DIR/agents" phone_dir="$KANNA_STATE_DIR/phone"
  mkdir -p "$phone_fake/bin" || return 1
  ln -s "$repo_root/tools/mock-agent/mock-agent" "$phone_fake/bin/$1" || return 1
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$phone_fake/bin/$1" >"$phone_fake/$1" && chmod +x "$phone_fake/$1"
}

phone_rig() {
  export KANNA_MACHINE_MDNS=0 KANNA_MACHINE_PORT=0
  phone_agent "$1" || return 1
  phone_repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo"
  mkdir -p "$phone_repo/src" || return 1
  printf 'one\ntwo\nthree\n' >"$phone_repo/src/a.txt"
  git -C "$phone_repo" init -q -b main && git -C "$phone_repo" add . && git -C "$phone_repo" -c user.email=e@e -c user.name=e commit -q -m first || return 1
  phone_repo="$(cd "$phone_repo" && pwd -P)"
  KANNA_PLANNING_AGENTS="$phone_fake/$1" start_daemon || return 1
  raw "{\"id\":1,\"cmd\":\"projects\",\"args\":{\"action\":\"touch\",\"folder\":\"$phone_repo\"}}" >/dev/null || return 1
  phone_pair
}

phone_pair() {
  python3 - "$KANNA_SOCKET" "$(sed -n 's/^pub const PROTOCOL: u32 = \([0-9]*\);/\1/p' "$repo_root/crates/kanna-build/src/lib.rs")" <<'PY' &
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f = s.makefile("rw")
f.write(json.dumps({"id": 0, "cmd": "app.hello", "args": {"pid": 1, "protocol": int(sys.argv[2])}}) + "\n"); f.flush()
for line in f:
    m = json.loads(line)
    if m.get("cmd") == "app.machine.confirm":
        f.write(json.dumps({"id": m["id"], "ok": True, "confirmed": True}) + "\n"); f.flush()
        break
PY
  local confirmer=$!
  kanna phone pair --json >"$KANNA_STATE_DIR/offer" &
  local waiting=$!
  check wait_for 5 test -s "$KANNA_STATE_DIR/offer" || return 1
  local offer; offer="$(head -1 "$KANNA_STATE_DIR/offer" | python3 -c 'import json, sys; o = json.load(sys.stdin); o["addrs"] = ["127.0.0.1:%s" % o["port"]]; print(json.dumps(o))')"
  phone pair "$offer" >/dev/null || return 1
  wait "$waiting" "$confirmer"
}

phone_place() { kanna workspace list --all --json | python3 -c 'import json, sys
w = [w for w in json.load(sys.stdin)["workspaces"] if (w.get("where") or {}).get("planner")]
print(w[0]["where"]["planner"] if w else "")'; }
phone_planner() { kanna workspace list --all --json | python3 -c 'import json, sys
w = [w for w in json.load(sys.stdin)["workspaces"] if (w.get("where") or {}).get("planner")]
print(w[0]["dock"]["pane"] if w else "")'; }

scenario phone_chat M4
phone_chat() {
  phone_chat_with claude
}

scenario phone_machines M4
phone_machines() {
  local vm vm_pid
  phone_agent claude || { echo "phone_machines line $LINENO"; return 1; }
  export KANNA_PLANNING_AGENTS="$phone_fake/claude"
  local vm_home; vm_home="$(mktemp -d "$KANNA_STATE_DIR/vm-home.XXXX")"
  machine_paired_vm phone_machines "$vm_home" || return 1
  local here="$KANNA_AGENTS_HOME/.kanna-v4/repos/team/notes" there="$vm_home/.kanna-v4/repos/team/notes" alone="$vm_home/.kanna-v4/repos/team/alone" me; me="$bin/kanna"
  local spot
  for spot in "$here" "$there" "$alone"; do
    mkdir -p "$spot" && git -C "$spot" init -q -b main && git -C "$spot" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "phone_machines line $LINENO"; return 1; }
  done
  git -C "$here" remote add origin https://example.com/team/notes.git && git -C "$there" remote add origin https://example.com/team/notes.git && git -C "$alone" remote add origin https://example.com/team/alone.git || { echo "phone_machines line $LINENO"; return 1; }
  here="$(cd "$here" && pwd -P)" there="$(cd "$there" && pwd -P)" alone="$(cd "$alone" && pwd -P)"
  raw "{\"id\":1,\"cmd\":\"projects\",\"args\":{\"action\":\"touch\",\"folder\":\"$here\"}}" >/dev/null || { echo "phone_machines line $LINENO"; return 1; }
  vraw "{\"id\":1,\"cmd\":\"projects\",\"args\":{\"action\":\"touch\",\"folder\":\"$there\"}}" >/dev/null || { echo "phone_machines line $LINENO"; return 1; }
  vraw "{\"id\":1,\"cmd\":\"projects\",\"args\":{\"action\":\"touch\",\"folder\":\"$alone\"}}" >/dev/null || { echo "phone_machines line $LINENO"; return 1; }
  phone_pair || { echo "phone_machines: no phone"; return 1; }
  rows() { python3 -c 'import json, sys
rows = json.load(sys.stdin)
for key in sys.argv[1].split("."): rows = rows[int(key)] if key.isdigit() else rows[key]
print(next((r.get(sys.argv[3]) for r in rows if r.get("name") == sys.argv[2]), "absent"))' "$@"; }
  listed() { phone hello | rows projects "$1" "$2"; }
  on() { phone machines notes | rows machines "$1" "$2"; }
  check wait_for 20 eval '[[ "$(listed alone planner)" == vm && "$(listed notes planner)" == mbp ]]' || { echo "phone_machines: two machines don't read as one list: $(phone hello)"; return 1; }
  check test "$(phone hello | python3 -c 'import json, sys; print(sum(p["name"] == "notes" for p in json.load(sys.stdin)["projects"]))')" == 1 || { echo "phone_machines: notes shows twice: $(phone hello)"; return 1; }

  check wait_for 15 eval '[[ "$(on vm has)" == True && "$(on vm up)" == True ]]' || { echo "phone_machines: vm's clone isn't on the Machines tab: $(phone machines notes)"; return 1; }
  check test "$(on vm path)" == direct && check test "$(on vm planner)" == False && check test "$(on mbp planner)" == True && check test "$(on mbp me)" == True || { echo "phone_machines: the machines: $(phone machines notes)"; return 1; }
  check test "$(phone machines notes | python3 -c 'import json, sys; r = json.load(sys.stdin); print(r["from"], [m for m in r["machines"] if m["name"] == "vm"][0]["version"] is not None)')" == "mbp True" || { echo "phone_machines: who answered: $(phone machines notes)"; return 1; }

  local stats; stats="$(phone stats notes)" || { echo "phone_machines: no stats: $stats"; return 1; }
  check test "$(field project <<<"$stats")" == "$here" && check test "$(field sections.1.id <<<"$stats")" == main || { echo "phone_machines: the stats: $stats"; return 1; }

  phone send notes m1 "mock: run $me phone say first on mbp" >/dev/null || { echo "phone_machines line $LINENO"; return 1; }
  check wait_for 25 eval 'phone read notes | grep -q "first on mbp"' || { echo "phone_machines: no answer on mbp: $(phone read notes)"; return 1; }
  check test "$(listed notes plannerKey)" == "$(kanna machine --json | field key)" || { echo "phone_machines: the first message didn't keep its planner machine"; return 1; }

  local moved; moved="$(phone move notes vm)"
  check test "$(field planner <<<"$moved")" == vm || { echo "phone_machines: the move: $moved"; return 1; }
  check test "$(listed notes planner)" == vm || { echo "phone_machines: mbp still says $(listed notes planner)"; return 1; }
  check test "$(listed alone planner)" == vm || { echo "phone_machines: another repo moved"; return 1; }
  check wait_for 15 eval '[[ "$(KANNA_SOCKET=$vm/kanna.sock $bin/kanna workspace list --all --json | grep -c "\"planner\": *\"daemon\"\|\"planner\":\"daemon\"")" != 0 ]]' || { echo "phone_machines: no planner on vm"; vmk workspace list --all --json; return 1; }
  local late; late="$(phone send notes m2 "for the old planner" 2>&1)"
  grep -q '"code":"moved"' <<<"$late" || { echo "phone_machines: mbp took a message for vm's planner: $late"; return 1; }

  export PHONE_MACHINE=vm
  phone hello >/dev/null || { echo "phone_machines: the link didn't move to vm"; return 1; }
  check test "$(listed notes planner)" == vm || { echo "phone_machines: vm doesn't say it's the planner machine: $(phone hello)"; return 1; }
  phone read notes | grep -q "first on mbp" && { echo "phone_machines: mbp's chat went to vm"; return 1; }
  phone read notes | grep -q "one is starting on vm" || { echo "phone_machines: no word of the start: $(phone read notes)"; return 1; }
  phone send notes m3 "mock: run $me phone say first on vm" >/dev/null || { echo "phone_machines line $LINENO"; return 1; }
  check wait_for 30 eval 'phone read notes | grep -q "first on vm"' || { echo "phone_machines: no answer on vm: $(phone read notes)"; vmk workspace list --all; return 1; }
  check test "$(field project <<<"$(phone stats notes)")" == "$there" || { echo "phone_machines: Stats didn't move with the planner"; return 1; }
  unset PHONE_MACHINE

  kill "$vm_pid"; wait_for 10 eval '! kill -0 '"$vm_pid" || { echo "phone_machines: vm's daemon didn't stop"; return 1; }
  check test "$(listed notes planner)" == vm && check test "$(listed alone planner)" == vm || { echo "phone_machines: an asleep machine's repos left the list: $(phone hello)"; return 1; }
  check wait_for 15 eval '[[ "$(on vm up)" == False ]]' || { echo "phone_machines: vm reads as up: $(phone machines notes)"; return 1; }
  check test "$(on vm has)" == True || { echo "phone_machines: an asleep machine lost its clone"; return 1; }
  local seen; seen="$(phone machines notes | rows machines vm seen)"
  [[ "$seen" != None && "$seen" != absent ]] || { echo "phone_machines: no last seen"; return 1; }
  moved="$(phone move notes mbp)"
  check test "$(field planner <<<"$moved")" == mbp || { echo "phone_machines: moving back past a sleeping planner machine: $moved"; return 1; }
  check test "$(listed notes planner)" == mbp || { echo "phone_machines: the move didn't stick"; return 1; }
  kill "$(cat "$vm/state/keeper.pid")"
}

scenario phone_pair_page M4
phone_pair_page() {
  export KANNA_MACHINE_MDNS=0 KANNA_MACHINE_PORT=0
  start_daemon || return 1
  start_app || return 1
  kanna open --window new --name pair_page >/dev/null || { echo "phone_pair_page line $LINENO"; return 1; }
  sleep 1
  local key; key="$(kanna machine list --json | field key)"
  raw '{"id":1,"cmd":"debug.pairPhone"}' >/dev/null || { echo "phone_pair_page line $LINENO"; return 1; }
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.pairPhone\"}" | field sheet | grep -q True' || { echo "phone_pair_page: no sheet"; return 1; }
  local code; code="$(raw '{"id":1,"cmd":"debug.pairPhone"}' | field code)"
  check test "$(printf '%s' "$code" | field key)" == "$key" || { echo "phone_pair_page: the QR reads $code, not this machine's key $key"; return 1; }
  check test "$(printf '%s' "$code" | field code | wc -c | tr -d ' ')" == 33 || { echo "phone_pair_page: the QR's code: $code"; return 1; }
  raw '{"id":1,"cmd":"debug.pairPhone","args":{"cancel":true}}' >/dev/null || { echo "phone_pair_page line $LINENO"; return 1; }
  check wait_for 5 eval 'raw "{\"id\":1,\"cmd\":\"debug.pairPhone\",\"args\":{\"cancel\":true}}" | field sheet | grep -q False' || { echo "phone_pair_page: Cancel left the sheet up"; return 1; }
}

scenario phone_shelf M4
phone_shelf() {
  phone_rig claude || { echo "phone_shelf line $LINENO"; return 1; }
  task_of() { (cd "$phone_repo" && kanna task new "$@" --agent "$phone_fake/claude") | grep -o 'task-[0-9a-f]*' | head -1; }
  shelf() { phone tasks repo | python3 -c 'import json, sys
r = json.load(sys.stdin); key, want = sys.argv[1], sys.argv[2]
t = [t for t in r["tasks"] if t["id"].startswith(sys.argv[1])]
print(t[0].get(want) if t else None)' "$1" "$2"; }
  pane_of() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
print([t["pane"] for t in json.load(sys.stdin)["desk"] if t["id"].startswith(sys.argv[1])][0])' "$1"; }
  local me; me="$(kanna machine list --json | field name)"
  local one; one="$(task_of Later "First words of the brief")"
  [[ -n "$one" ]] || { echo "phone_shelf: no task"; return 1; }
  local full; full="$(phone tasks repo)"
  check test "$(shelf "$one" name)" == Later && check test "$(shelf "$one" status)" == waiting && check test "$(shelf "$one" desk)" == None || { echo "phone_shelf: the list said $full"; return 1; }
  check test "$(field desks.0.machine <<<"$full")" == "$me" && check test "$(field desks.0.open <<<"$full")" == False || { echo "phone_shelf: the Desks: $full"; return 1; }
  local told; told="$(phone tell "$one" n1 "mind the gap")"
  check test "$(field state <<<"$told")" == waiting || { echo "phone_shelf: a note wasn't taken: $told"; return 1; }
  phone tell "$one" n1 "mind the gap" >/dev/null || { echo "phone_shelf line $LINENO"; return 1; }
  local read; read="$(phone task "$one")"
  check test "$(field task.message <<<"$read")" == "First words of the brief" && check test "$(field thread.0.text <<<"$read")" == "mind the gap" && check test "$(field thread.0.read <<<"$read")" == None && check test "$(field thread.1 <<<"$read" 2>/dev/null)" == "" || { echo "phone_shelf: the task read $read"; return 1; }
  check test "$(kanna task messages "$one" --json | field messages.0.text)" == "mind the gap" || { echo "phone_shelf: kanna task messages doesn't list the note"; return 1; }
  check test "$(shelf "$one" status)" == waiting || { echo "phone_shelf: a note started the task"; return 1; }
  phone listen 6 >"$KANNA_STATE_DIR/notices" &
  local listening=$!
  sleep 1
  phone pick "$one" "$me" >/dev/null || { echo "phone_shelf: the pick was refused: $(phone pick "$one" "$me" 2>&1)"; return 1; }
  wait "$listening"
  grep -q '"notice":"tasks"' "$KANNA_STATE_DIR/notices" || { echo "phone_shelf: no tasks notice"; return 1; }
  check test "$(shelf "$one" desk)" == "$me" || { echo "phone_shelf: the pick isn't on $me's Desk: $(phone tasks repo)"; return 1; }
  local agent; agent="$(pane_of "$one")"
  check wait_for 10 unwrapped "$agent" "[message from phone] mind the gap" || { echo "phone_shelf: the note didn't go with the first message"; kanna read "$agent"; return 1; }
  unwrapped "$agent" "First words of the brief" || { echo "phone_shelf: the first message changed"; kanna read "$agent"; return 1; }
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys, os
print([w["where"] for w in json.load(sys.stdin)["workspaces"] if w.get("task") and w["task"].startswith(sys.argv[1])][0])' "$one")" == "{'desk': 'mac'}" || { echo "phone_shelf: no claim for mac"; kanna workspace list --all; return 1; }
  check test "$(raw '{"id":1,"cmd":"replica","args":{"action":"list"}}' | python3 -c 'import json, sys
print([r.get("claim") for r in json.load(sys.stdin)["records"] if r["task"].startswith(sys.argv[1])][0])' "$one")" == "$me" || { echo "phone_shelf: no claim in the replica"; return 1; }
  check test "$(phone task "$one" | field thread.0.read)" != None || { echo "phone_shelf: a note read as waiting once it started"; return 1; }
  kanna send "$agent" "say All done, nothing left" >/dev/null && kanna send-key "$agent" enter >/dev/null || return 1
  check wait_for 8 eval '[[ "$(phone task "$one" | field said.0.text)" == "All done, nothing left" ]]' || { echo "phone_shelf: no last words: $(phone task "$one" | cut -c1-500)"; return 1; }
  kanna send "$agent" "question Which way?" >/dev/null && kanna send-key "$agent" enter >/dev/null || return 1
  check wait_for 8 eval '[[ "$(phone task "$one" | field said.1.asking)" == True ]]' || { echo "phone_shelf: the question isn't its last words: $(phone task "$one" | cut -c1-500)"; return 1; }
  check test "$(phone task "$one" | field said.1.text | head -1)" == "Which way?" && check test "$(phone task "$one" | field said.0.text)" == "All done, nothing left" || { echo "phone_shelf: asked $(phone task "$one")"; return 1; }
  check wait_for 8 eval '[[ "$(shelf "$one" status)" == idle ]]' || { echo "phone_shelf: status $(shelf "$one" status)"; return 1; }
  phone tell "$one" n2 "Keep it" >/dev/null || return 1
  check wait_for 15 shows "$agent" "peer: \[message from phone\] Keep it" || { echo "phone_shelf: the message didn't reach the agent"; kanna read "$agent"; return 1; }
  check wait_for 5 eval '[[ "$(phone task "$one" | field thread.1.read)" != None ]]' || { echo "phone_shelf: no receipt: $(phone task "$one")"; return 1; }
  check wait_for 8 eval '[[ "$(shelf "$one" status)" == idle ]]' || { echo "phone_shelf: status $(shelf "$one" status)"; return 1; }
  local spoke; spoke="$(phone task "$one" | field said)"
  (cd "$phone_repo" && printf 'one\ntwo\n' >code.txt && git add code.txt && git -c user.email=e@e -c user.name=e commit -qm code) || return 1
  phone tell "$one" n3 "mock: run $bin/kanna phone say Done, see below" >/dev/null || return 1
  check wait_for 15 eval '[[ "$(phone task "$one" | field items.0.doc.text)" == "Done, see below" ]]' || { echo "phone_shelf: the task's say didn't reach its page: $(phone task "$one" | cut -c1-500)"; kanna read "$agent"; return 1; }
  check wait_for 5 eval '[[ "$(shelf "$one" status)" == idle ]]' && sleep 3
  check test "$(phone task "$one" | field said)" == "$spoke" || { echo "phone_shelf: a turn that spoke on the phone also showed its session text: $(phone task "$one" | field said)"; return 1; }
  test -z "$(phone read repo | python3 -c 'import json, sys; print([i for i in json.load(sys.stdin)["items"] if "Done, see below" in str(i)] or "")')" || { echo "phone_shelf: a task's say reached the planner's chat"; return 1; }
  phone tell "$one" n4 "mock: run $bin/kanna phone ask Which? --choice Left --choice Right" >/dev/null || return 1
  check wait_for 15 eval '[[ "$(phone task "$one" | field items.1.doc.choices.0.label)" == Left ]]' || { echo "phone_shelf: no question: $(phone task "$one" | cut -c1-500)"; return 1; }
  check wait_for 8 eval '[[ "$(shelf "$one" status)" == needs ]]' || { echo "phone_shelf: an open question isn't needs: $(shelf "$one" status)"; return 1; }
  phone choosetask "$one" n5 2 Left | grep -q '"ok":true' || { echo "phone_shelf: the tap was refused: $(phone choosetask "$one" n5 2 Left)"; return 1; }
  check wait_for 15 shows "$agent" "peer: \[message from phone\] I chose: Left" || { echo "phone_shelf: the tap didn't reach the task"; kanna read "$agent"; return 1; }
  check test "$(phone task "$one" | field items.1.doc.chosen)" == Left || { echo "phone_shelf: the choice wasn't kept"; return 1; }
  check wait_for 8 eval '[[ "$(shelf "$one" status)" == idle ]]' || { echo "phone_shelf: answered, status $(shelf "$one" status)"; return 1; }
  local two three four; two="$(task_of Two "b")" three="$(task_of Three "c")" four="$(task_of Four "d")"
  phone pick "$two" "$me" >/dev/null && phone pick "$three" "$me" >/dev/null || { echo "phone_shelf line $LINENO"; return 1; }
  local refused; refused="$(phone pick "$four" "$me")"
  [[ "$refused" == *task_limit* ]] || { echo "phone_shelf: a fourth pick: $refused"; return 1; }
  phone pick "$four" "$me" "$three" | grep -q '"ok":true' || { echo "phone_shelf: a pick that makes room was refused: $(phone tasks repo)"; return 1; }
  check test "$(shelf "$four" desk)" == "$me" && check test "$(shelf "$three" desk)" == None || { echo "phone_shelf: the Desk after making room: $(phone tasks repo)"; return 1; }
  kill -0 "$daemon_pid" 2>/dev/null
}

scenario phone_relay M4
phone_relay() {
  (cd "$repo_root" && cargo build -q -p kanna-net --example test-relay) || { echo "phone_relay: the test relay didn't build"; return 1; }
  phone_rig claude || { echo "phone_relay line $LINENO"; return 1; }
  local me="$bin/kanna"
  local relay_out="$KANNA_STATE_DIR/relay.out" token="$KANNA_STATE_DIR/relay.token"
  printf 's3cret-phone' >"$token"
  "$rust_dir/examples/test-relay" s3cret-phone >"$relay_out" &
  local relay=$!
  trap "kill $relay 2>/dev/null" RETURN
  check wait_for 10 test -s "$relay_out" || { echo "phone_relay: the relay didn't start"; return 1; }
  local url; url="$(cat "$relay_out")"
  ! PHONE_RELAY_ONLY=1 phone hello >/dev/null 2>&1 || { echo "phone_relay: reached it with no relay"; return 1; }
  kanna machine relay "$url" --token-file "$token" >/dev/null || { echo "phone_relay line $LINENO"; return 1; }
  check wait_for 15 eval '[[ -n "$(kanna machine list --json | field home_relay)" ]]' || { echo "phone_relay: the daemon never reached the relay"; kanna machine list; return 1; }
  check test "$(phone hello | field relay)" == "$url" || { echo "phone_relay: hello didn't refresh the relay"; return 1; }
  check test "$(phone path | field path)" == direct || { echo "phone_relay: with a relay set the LAN isn't direct: $(phone path)"; return 1; }
  check test "$(phone path | field lan)" == True || { echo "phone_relay: the LAN doesn't read as LAN: $(phone path)"; return 1; }
  ! PHONE_RELAY_ONLY=1 PHONE_RELAY_TOKEN=wrong phone hello >/dev/null 2>&1 || { echo "phone_relay: a wrong token got through"; return 1; }
  check test "$(PHONE_RELAY_ONLY=1 phone path | field path)" == relay || { echo "phone_relay: not through the relay: $(PHONE_RELAY_ONLY=1 phone path)"; return 1; }
  PHONE_RELAY_ONLY=1 phone send repo r1 "mock: run $me phone say answered through the relay" >/dev/null || { echo "phone_relay line $LINENO"; return 1; }
  check test "$(PHONE_RELAY_ONLY=1 phone wait repo say 30 | field doc.text)" == "answered through the relay" || { echo "phone_relay: no answer through the relay"; phone read repo; return 1; }
  phone_dir="$KANNA_STATE_DIR/phone2"
  mkdir -p "$phone_dir"
  phone_pair || { echo "phone_relay: a second phone didn't pair"; return 1; }
  check test "$(field relay <"$phone_dir/machine.json")" == "$url" || { echo "phone_relay: the handoff had no relay"; return 1; }
  check test "$(field relay_token <"$phone_dir/machine.json")" == s3cret-phone || { echo "phone_relay: the handoff had no token"; return 1; }
  check test "$(PHONE_RELAY_ONLY=1 phone path | field path)" == relay || { echo "phone_relay: a new phone can't use the relay before its first hello"; return 1; }
  ! grep -q s3cret-phone "$KANNA_STATE_DIR/offer" || { echo "phone_relay: the token is in the QR"; return 1; }
}

scenario phone_chat_codex M4
phone_chat_codex() {
  phone_chat_with codex
}

scenario phone_quiz M4
phone_quiz() {
  phone_rig claude || { echo "phone_quiz line $LINENO"; return 1; }
  local me="$bin/kanna" round_file="$KANNA_STATE_DIR/round.json" map_file="$KANNA_STATE_DIR/map.json" bad_file="$KANNA_STATE_DIR/bad.json" lines
  mkdir -p "$phone_repo/net" "$phone_repo/desk" "$(dirname "$KANNA_AGENTS_HOME/.kanna-v4/config.json")" || return 1
  printf '{"version": 1, "shelf": {"runners": 0}, "tasks": {"agent": "%s"}, "agents": {"prompts": "skip"}}\n' "$phone_fake/claude" >"$KANNA_AGENTS_HOME/.kanna-v4/config.json" || return 1
  seq 1 40 >"$phone_repo/net/a.rs"; seq 1 10 >"$phone_repo/desk/b.rs"
  git -C "$phone_repo" add . && git -C "$phone_repo" -c user.email=e@e -c user.name=e commit -qm code || return 1
  local head; head="$(git -C "$phone_repo" rev-parse main)"
  python3 - "$head" "$round_file" "$map_file" "$bad_file" <<'PY' || return 1
import json, sys
head, round_file, map_file, bad_file = sys.argv[1:5]
choices = [{"label": "A"}, {"label": "B"}, {"label": "C"}, {"label": "D"}]
q = lambda kind, f, n: {"kind": kind, "file": f, "line": n, "text": f"What is at {f}:{n}?", "why": "It says so", "topic": f"line {n}", "source": head[:7], "choices": choices, "answer": 1}
bug = dict(q("bug", "net/a.rs", 3), lines=[{"n": n, "text": str(n)} for n in (1, 2, 3, 4)])
del bug["choices"]
qs = [q("architecture", "net/a.rs", 2), q("behaviour", "net/a.rs", 5), q("why", "desk/b.rs", 1), q("where", "desk/b.rs", 2), bug]
json.dump({"head": head, "title": "Today", "commits": [head], "questions": qs}, open(round_file, "w"))
bad = dict(q("why", "net/a.rs", 999))
json.dump({"head": head, "questions": [bad] * 5}, open(bad_file, "w"))
json.dump({"subsystems": [{"name": "Net", "paths": ["net/**"]}, {"name": "Desk", "paths": ["desk/**"]}, {"name": "S1", "paths": ["s1/**"]}, {"name": "S2", "paths": ["s2/**"]}, {"name": "S3", "paths": ["s3/**"]}, {"name": "S4", "paths": ["s4/**"]}]}, open(map_file, "w"))
PY
  writer() { kanna workspace list --all --json | python3 -c 'import json, sys
w = [w for w in json.load(sys.stdin)["workspaces"] if (w.get("where") or {}).get("quiz")]
print(w[0]["dock"]["pane"] if w else "")'; }
  type_in() { kanna send "$1" "$2" >/dev/null && kanna send-key "$1" enter >/dev/null; }
  check test "$(phone quiz-rounds repo | field rounds)" == "[]" || { echo "phone_quiz: a new repo has rounds: $(phone quiz-rounds repo)"; return 1; }
  check test "$(phone quiz-map repo | field subsystems)" == "[]" || { echo "phone_quiz: a new repo has a map"; return 1; }

  check test "$(phone quiz-ahead repo | field writing.about)" == None || { echo "phone_quiz: an empty store started no writer: $(phone quiz-rounds repo)"; return 1; }
  check wait_for 10 eval '[[ -n "$(writer)" ]]' || { echo "phone_quiz: no writer"; kanna workspace list --all; return 1; }
  local pane; pane="$(writer)"
  check wait_for 10 unwrapped "$pane" "You are the Kanna quiz writer" || { echo "phone_quiz: the writer wasn't told"; kanna read "$pane"; return 1; }
  [[ "$(phone_planner)" == "" ]] || { echo "phone_quiz: the writer started a planner"; return 1; }
  check test "$(cd "$phone_repo" && kanna task list --json | field tasks)" == "[]" || { echo "phone_quiz: the writer is a task"; return 1; }
  phone quiz-ahead repo >/dev/null
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys
print(len([w for w in json.load(sys.stdin)["workspaces"] if (w.get("where") or {}).get("quiz")]))')" == 1 || { echo "phone_quiz: a second writer started"; return 1; }

  type_in "$pane" "run cd $phone_repo; $me quiz map $map_file"
  check wait_for 10 shows "$pane" "6 subsystems" || { echo "phone_quiz: the map wasn't kept"; kanna read "$pane"; return 1; }
  type_in "$pane" "run cd $phone_repo; $me quiz ready $bad_file"
  check wait_for 10 unwrapped "$pane" "no line 999" || { echo "phone_quiz: a bad round wasn't told"; kanna read "$pane"; return 1; }
  check test "$(phone quiz-rounds repo | field rounds)" == "[]" || { echo "phone_quiz: a bad round was kept"; return 1; }
  phone listen 12 >"$KANNA_STATE_DIR/quiz-notices" &
  local listening=$!
  sleep 0.5
  type_in "$pane" "run cd $phone_repo; $me quiz ready $round_file"
  check wait_for 10 eval '[[ "$(phone quiz-rounds repo | field rounds.0.round)" == r1 ]]' || { echo "phone_quiz: no round: $(phone quiz-rounds repo)"; kanna read "$pane"; return 1; }
  check wait_for 15 eval '[[ -z "$(writer)" ]]' || { echo "phone_quiz: the writer didn't quit"; kanna read "$pane"; return 1; }
  wait "$listening"
  grep -q '"notice":"quiz".*"ready":"r1"' "$KANNA_STATE_DIR/quiz-notices" || { echo "phone_quiz: no ready notice: $(cat "$KANNA_STATE_DIR/quiz-notices")"; return 1; }
  local rounds; rounds="$(phone quiz-rounds repo)"
  check test "$(field rounds.0.questions.0.subsystem <<<"$rounds")" == Net && check test "$(field rounds.0.questions.0.code.lines.1 <<<"$rounds")" == 2 && check test "$(field rounds.0.questions.0.code.commit <<<"$rounds")" == "$head" || { echo "phone_quiz: the round reads $rounds"; return 1; }
  check test "$(field writing <<<"$rounds")" == None || { echo "phone_quiz: a written round still reads writing"; return 1; }

  check test "$(phone quiz-map repo | field subsystems.0.mastery)" == None || { echo "phone_quiz: mastery before any answer: $(phone quiz-map repo)"; return 1; }
  check test "$(phone quiz-map repo | field subsystems.0.lines)" == 40 || { echo "phone_quiz: lines: $(phone quiz-map repo)"; return 1; }
  phone quiz-answer r1 0 1 >/dev/null || { echo "phone_quiz line $LINENO"; return 1; }
  check wait_for 10 eval '[[ -n "$(writer)" ]]' || { echo "phone_quiz: the first answer started no writer"; return 1; }
  pane="$(writer)"
  phone quiz-answer r1 0 1 >/dev/null
  phone quiz-answer r1 1 1 >/dev/null
  phone quiz-answer r1 2 0 >/dev/null
  phone quiz-answer r1 3 1 >/dev/null
  phone quiz-answer r1 4 1 >/dev/null
  check test "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*), sum(right) from quiz_answers")" == "5|4" || { echo "phone_quiz: a resend counted: $(sqlite3 "$KANNA_STATE_DIR/kanna.db" "select * from quiz_answers")"; return 1; }
  local map; map="$(phone quiz-map repo)"
  check test "$(field subsystems.0.mastery <<<"$map")" == 60 && check test "$(field subsystems.1.mastery <<<"$map")" == 25 || { echo "phone_quiz: the map didn't move: $map"; return 1; }
  check test "$(field subsystems.1.misses.0.file <<<"$map")" == "desk/b.rs:1" || { echo "phone_quiz: the miss isn't on its subsystem: $map"; return 1; }
  check test "$(cd "$phone_repo" && kanna quiz history --misses --json | field rounds.0.answers.0.answer)" == a3 || { echo "phone_quiz: a phone miss doesn't come back on the Mac"; return 1; }
  check test "$(cd "$phone_repo" && kanna stats --json | python3 -c 'import json, sys
print([t for s in json.load(sys.stdin)["sections"] if s["id"] == "quiz" for t in s["tiles"] if t["id"] == "right"][0]["value"])')" == 80 || { echo "phone_quiz: Stats doesn't count phone answers"; return 1; }

  (cd "$phone_repo" && kanna quiz start >/dev/null && kanna quiz answer r2 --kind why --file desk/b.rs:3 --right >/dev/null) || { echo "phone_quiz line $LINENO"; return 1; }
  check test "$(phone quiz-map repo | field subsystems.1.answers)" == 3 || { echo "phone_quiz: a Mac round doesn't count: $(phone quiz-map repo)"; return 1; }
  check test "$(phone quiz-map repo | field subsystems.1.mastery)" == 40 || { echo "phone_quiz: a Mac round didn't move the map: $(phone quiz-map repo)"; return 1; }
  check test "$(phone quiz-map repo | field subsystems.1.moved)" == 0 || { echo "phone_quiz: moved since the last right answer: $(phone quiz-map repo)"; return 1; }
  printf 'x\n' >>"$phone_repo/desk/b.rs"
  git -C "$phone_repo" -c user.email=e@e -c user.name=e commit -qam "desk moved" || return 1
  check test "$(phone quiz-map repo | field subsystems.1.moved)" == 1 && check test "$(phone quiz-map repo | field subsystems.1.commits.0.subject)" == "desk moved" || { echo "phone_quiz: a commit didn't move its subsystem: $(phone quiz-map repo)"; return 1; }

  type_in "$pane" "run cd $phone_repo; $me quiz ahead --about 'the relay'"
  check wait_for 10 eval 'phone quiz-rounds repo | grep -q "\"queued\":\[\"the relay\"\]"' || { echo "phone_quiz: the subject wasn't queued: $(phone quiz-rounds repo)"; return 1; }
  type_in "$pane" "run cd $phone_repo; $me quiz ready $round_file"
  check wait_for 15 eval '[[ "$(writer)" != "" && "$(writer)" != "$pane" ]]' || { echo "phone_quiz: the queued writer didn't start: $(kanna read "$pane")"; return 1; }
  pane="$(writer)"
  check wait_for 10 unwrapped "$pane" "Its subject: the relay" || { echo "phone_quiz: the subject wasn't told"; kanna read "$pane"; return 1; }
  type_in "$pane" "run cd $phone_repo; $me quiz ready $round_file"
  local card; card="$(phone wait repo round 15)" || { echo "phone_quiz: no round card in the chat"; phone read repo; return 1; }
  check test "$(field doc.about <<<"$card")" == "the relay" || { echo "phone_quiz: the card was $card"; return 1; }
  check wait_for 15 eval '[[ -z "$(writer)" ]]' || { echo "phone_quiz: the second writer didn't quit"; return 1; }
  check test "$(phone quiz-rounds repo | python3 -c 'import json, sys
print([r["about"] for r in json.load(sys.stdin)["rounds"]])')" == "[None, 'the relay']" || { echo "phone_quiz: the rounds: $(phone quiz-rounds repo)"; return 1; }

  sqlite3 "$KANNA_STATE_DIR/kanna.db" "update quiz_ready set taken = 1" || return 1
  phone quiz-ahead repo >/dev/null
  check wait_for 10 eval '[[ -n "$(writer)" ]]' || { echo "phone_quiz: no writer for the failure"; return 1; }
  pane="$(writer)"
  type_in "$pane" "exit"
  check wait_for 15 eval 'phone quiz-rounds repo | grep -q "quit without writing"' || { echo "phone_quiz: a failed writer wasn't told: $(phone quiz-rounds repo)"; return 1; }
  phone quiz-ahead repo >/dev/null
  check test "$(phone quiz-rounds repo | field writing)" == None || { echo "phone_quiz: a failure retried at once"; return 1; }
  check test "$(phone quiz-again repo | field writing.about)" == None || { echo "phone_quiz: Try again didn't start one: $(phone quiz-rounds repo)"; return 1; }
  check wait_for 10 eval '[[ -n "$(writer)" ]]' || { echo "phone_quiz: no writer after Try again"; return 1; }
}

phone_chat_with() {
  phone_rig "$1" || { echo "phone_chat line $LINENO"; return 1; }
  local me="$bin/kanna"
  check test "$(kanna machine list --json | field phones.0.name | cut -c1-7)" == iphone- || { echo "phone_chat: the phone isn't listed"; kanna machine list; return 1; }
  check test "$(phone hello | python3 -c 'import json, sys; print(json.load(sys.stdin)["projects"][0]["status"])')" == not_running || { echo "phone_chat: a planner shows before any started: $(phone hello 2>&1 | cut -c1-700)"; return 1; }
  phone send repo m1 "mock: run $me phone say hello from the planner" >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check test "$(phone wait repo sys 5 | field doc.text | cut -c1-30)" == "No planner was running for rep" || { echo "phone_chat: the chat didn't say a planner is starting"; phone read repo; return 1; }
  check phone wait repo say 30 >/dev/null || { local p; p="$(phone_planner)"; echo "phone_chat: no answer from the windowless planner in [$p]: $(kanna read "$p" 2>&1 | tail -12 | tr '\n' '~' | cut -c1-800)"; return 1; }
  check test "$(phone wait repo say 1 | field doc.text)" == "hello from the planner" || { phone read repo; return 1; }
  local planner; planner="$(phone_planner)"
  [[ -n "$planner" ]] || { echo "phone_chat: no planner pane"; kanna workspace list --all; return 1; }
  shows "$planner" "peer: \[message from phone\]" || { echo "phone_chat: the message wasn't from phone"; kanna read "$planner"; return 1; }
  check test "$(kanna workspace list --all --json | python3 -c 'import json, sys
print(json.load(sys.stdin)["workspaces"][0]["where"]["planner"])')" == daemon || { echo "phone_chat: the planner isn't windowless"; return 1; }
  check phone status repo idle 10 >/dev/null || { echo "phone_chat: the status didn't come back to idle"; return 1; }
  local seq; seq="$(phone send repo m1 "again" | field item.seq)"
  check test "$seq" == 1 || { echo "phone_chat: a resend made a new item ($seq)"; phone read repo; return 1; }
  check test "$(kanna read "$planner" | grep -c "peer: \[message from phone\]")" == 1 || { echo "phone_chat: a resend delivered twice"; kanna read "$planner"; return 1; }
  local refused; refused="$(cd "$phone_repo" && kanna phone say nope 2>&1)" && { echo "phone_chat: a stranger spoke on the phone"; return 1; }
  [[ "$refused" == *not_an_agent* ]] || { echo "phone_chat: refused with: $refused"; return 1; }
  phone send repo m2 "mock: run $me phone show src/a.txt:2-3 --hot 2-2" >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  local card; card="$(phone wait repo show 20)" || { echo "phone_chat: no code card"; kanna read "$planner"; return 1; }
  check test "$(field doc.lines.1 <<<"$card")" == three && check test "$(field doc.file <<<"$card")" == src/a.txt || { echo "phone_chat: the card was $card"; return 1; }
  phone send repo m3 "mock: run $me phone ask Merge? --choice Yes --choice No" >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  local asked; asked="$(phone wait repo ask 20 | field seq)" || { echo "phone_chat: no question"; kanna read "$planner"; return 1; }
  check phone status repo asks 10 >/dev/null || { echo "phone_chat: an open question isn't Asks you"; return 1; }
  phone choose repo m4 "$asked" Yes >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check wait_for 15 shows "$planner" "peer: \[message from phone\] I chose: Yes" || { echo "phone_chat: the tap didn't reach the planner"; kanna read "$planner"; return 1; }
  kanna send "$planner" "work 6" >/dev/null && kanna send-key "$planner" enter >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check wait_for 10 is "$planner" status working || { echo "phone_chat: the planner didn't get busy"; return 1; }
  phone send repo m5 "mock: run $me phone say after the busy turn" >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check phone status repo elsewhere 5 >/dev/null || { echo "phone_chat: a busy planner isn't Working on something else"; phone read repo 1000; return 1; }
  ! shows "$planner" "after the busy turn" || { echo "phone_chat: the phone interrupted the turn"; return 1; }
  check wait_for 3 eval 'sleep 8; phone read repo | grep -q "\"text\":\"after the busy turn"' || { echo "phone_chat: the waiting message never got its answer"; kanna read "$planner"; return 1; }
  sleep 5
  ! shows "$planner" "The phone is waiting" || { echo "phone_chat: an answer given before the turn's end was taken for silence"; kanna read "$planner"; return 1; }
  phone send repo m6 "mock: work 0" >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check wait_for 25 shows "$planner" "peer: \[message from kanna\] The phone is waiting" || { echo "phone_chat: a silent turn wasn't nudged"; kanna read "$planner"; return 1; }
  check wait_for 25 eval 'phone read repo | grep -q "The planner finished without answering"' || { echo "phone_chat: a second silent turn wasn't told"; phone read repo; return 1; }
  check test "$(cd "$phone_repo" && KANNA_TERMINAL="$planner" kanna phone history --last 1 | cut -c1-3)" == sys || { echo "phone_chat: history was wrong"; return 1; }
  check test "$(phone_place)" == daemon || { echo "phone_chat: the planner isn't windowless: $(phone_place)"; return 1; }
  kanna send "$planner" ask >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check wait_for 5 is "$planner" status needs-you || { echo "phone_chat: the mock didn't ask"; kanna read "$planner"; return 1; }
  check phone status repo blocked 10 >/dev/null || { echo "phone_chat: a planner that needs you reads as $(phone read repo 1000)"; return 1; }
  kanna send-key "$planner" enter >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check wait_for 5 is "$planner" status idle || { echo "phone_chat: the question didn't end"; kanna read "$planner"; return 1; }
  local adopted; adopted="$(raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"plan\",\"app\":\"mac\",\"folder\":\"$phone_repo\"}}")"
  check test "$(printf '%s' "$adopted" | field pane)" == "$planner" || { echo "phone_chat: a Desk didn't adopt the windowless planner: $adopted"; return 1; }
  check test "$(phone_place)" == mac || { echo "phone_chat: the adopted planner is still $(phone_place)"; return 1; }
  raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"let-go\",\"task\":\"$planner\"}}" >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check test "$(phone_place)" == daemon || { echo "phone_chat: closing the Desk ended the planner: $(phone_place)"; return 1; }
  check test "$(phone_planner)" == "$planner" || { echo "phone_chat: the planner changed when its window closed"; return 1; }
  local keeper; keeper="$(cat "$KANNA_STATE_DIR/keeper.pid")"
  [[ "$keeper" =~ ^[0-9]+$ ]] && [[ "$(ps -o command= -p "$keeper")" == *"$KANNA_STATE_DIR"* ]] || { echo "phone_chat: $keeper isn't this run's keeper"; return 1; }
  stop_daemon
  kill "$keeper"
  check wait_for 5 eval '! kill -0 $keeper' || { echo "phone_chat: this run's keeper didn't stop"; return 1; }
  KANNA_MACHINE_PORT="$(head -1 "$KANNA_STATE_DIR/offer" | field port)" KANNA_PLANNING_AGENTS="$phone_fake/$1" start_daemon || { echo "phone_chat line $LINENO"; return 1; }
  check test "$(phone_place)" == "" || { echo "phone_chat: a planner with no terminal came back: $(kanna workspace list --all --json | cut -c1-500)"; return 1; }
  check phone status repo not_running 10 >/dev/null || { echo "phone_chat: a planner lost with its keeper still shows: $(phone hello | cut -c1-500)"; return 1; }
  local answered; answered="$(phone read repo | grep -c '"text":"back from the keeper')"
  phone send repo m7 "mock: run $me phone say back from the keeper" >/dev/null || { echo "phone_chat line $LINENO"; return 1; }
  check wait_for 40 eval '[[ "$(phone read repo | grep -c "\"text\":\"back from the keeper")" -gt $answered ]]' || { echo "phone_chat: no answer after the keeper died: $(phone read repo | tail -3 | cut -c1-400); $(kanna workspace list --all | tail -5)"; return 1; }
  check test "$(phone_place)" == daemon || { echo "phone_chat: the new planner isn't windowless: $(phone_place)"; return 1; }
}
