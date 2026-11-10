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
  mkdir -p "$repo/src" && cat >"$repo/src/lib.rs" <<'RUST'
pub fn busy(a: u8, b: bool) -> u8 {
    if a > 1 && b {
        if a > 2 {
            return 1;
        }
    } else if a > 3 || !b {
        for i in 0..a {
            if i == 4 {
                return 2;
            }
        }
    }
    match a {
        1 => 1,
        2 => 2,
        3 => 3,
        _ => 4,
    }
}
RUST
  git -C "$repo" add src && dated $((now - 4 * day)) commit -q --amend --allow-empty --no-edit --only && dated $((now - 3 * day)) commit -q -m plain || { echo "stats_tab line $LINENO"; return 1; }
  git -C "$repo" checkout -q -b feature && dated $((now - 2 * day)) commit -q --allow-empty -m one && dated $((now - 2 * day + 60)) commit -q --allow-empty -m two || { echo "stats_tab line $LINENO"; return 1; }
  git -C "$repo" checkout -q main && dated "$now" merge -q --no-ff -m "Merge feature" feature || { echo "stats_tab line $LINENO"; return 1; }
  start_daemon || { echo "stats_tab line $LINENO"; return 1; }
  quiz() { (cd "$repo" && kanna quiz "$@"); }
  quiz start >/dev/null || { echo "stats_tab line $LINENO"; return 1; }
  quiz answer r1 --kind why --file docs/specs/a.md:1 --wrong >/dev/null && quiz answer r1 --kind why --file docs/specs/a.md:2 --right >/dev/null \
    && quiz answer r1 --kind bug --file crates/one/src/a.rs:3 --right >/dev/null && quiz answer r1 --kind where --file README.md:1 --right >/dev/null || { echo "stats_tab line $LINENO"; return 1; }
  local one two three
  one="$(new_task "Stats one" "" stats)" && two="$(new_task "Stats two" "" stats)" && three="$(new_task "Stats three" "exec $mock_agent" stats)" || { echo "stats_tab line $LINENO"; return 1; }
  kanna task start "$one" >/dev/null && kanna task start "$three" >/dev/null || { echo "stats_tab line $LINENO"; return 1; }
  kanna task done "$one" >/dev/null || { echo "stats_tab line $LINENO"; return 1; }
  check wait_for 10 shows "Stats three" "mock-agent" || { echo "stats_tab line $LINENO"; return 1; }
  kanna send "Stats three" turn || { echo "stats_tab line $LINENO"; return 1; }
  local status_rows="select group_concat(s, ' ') from (select json_extract(doc, '\$.status') as s from task_events where what = 'status' and task = '$three' order by id)"
  check wait_for 10 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "$status_rows")" == *needs-you ]]' || { echo "stats_tab: the agent never asked: $(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "$status_rows")"; return 1; }
  stop_daemon
  start_daemon || { echo "stats_tab line $LINENO"; return 1; }
  kanna send "Stats three" 1 || { echo "stats_tab line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "$status_rows")" == *"working needs-you working idle" ]]' || { echo "stats_tab: the status rows are $(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "$status_rows")"; return 1; }
  local json
  sampled() { json="$(cd "$repo" && kanna stats --json)" && python3 -c 'import json, sys; c = [s for s in json.loads(sys.argv[1])["sections"] if s["id"] == "code"][0]; sys.exit(0 if json.loads(sys.argv[1])["status"] is None and any(t["id"] == "erosion" and t["value"] is not None for t in c["tiles"]) else 1)' "$json"; }
  check wait_for 20 sampled || { echo "stats_tab: kanna stats --json never had the code sampled: $json"; return 1; }
  check python3 -c 'import json, sys
r = json.loads(sys.argv[1])
assert r["project"] == sys.argv[2], r["project"]
assert len(r["days"]) == 30, r["days"]
s = {x["id"]: x for x in r["sections"]}
assert list(s) == ["quiz", "main", "tasks", "agents", "code", "pipelines"], list(s)
assert s["pipelines"]["empty"].startswith("No pipeline runs yet"), s["pipelines"]
t = {(sid, x["id"]): x for sid, sec in s.items() for x in sec["tiles"]}
assert t["quiz", "right"]["value"] == 75 and t["quiz", "right"]["note"] == "of 4 answers", t["quiz", "right"]
assert t["quiz", "misses"]["value"] == 1, t["quiz", "misses"]
assert [x["label"] for x in t["quiz", "areas"]["chart"]["rows"]] == ["README.md", "crates/one", "docs/specs"], t["quiz", "areas"]
assert t["main", "commits"]["value"] == 2 and t["main", "commits"]["note"] == "today · 4 in 30 days", t["main", "commits"]
assert t["main", "landed"]["value"] == 1, t["main", "landed"]
assert [x["id"] for x in s["tasks"]["tiles"]] == ["open", "made", "started", "ended", "lead"], s["tasks"]
o = t["tasks", "open"]
assert o["value"] == 2 and o["note"] == "+2 in 30 days" and o["span"] == 6, o
assert o["chart"]["kind"] == "flow" and o["chart"]["values"] == [0] * 29 + [2], o["chart"]
assert o["chart"]["made"][-1] == 3 and o["chart"]["done"][-1] == 1 and sum(o["chart"]["made"]) == 3 and sum(o["chart"]["done"]) == 1, o["chart"]
assert t["tasks", "made"]["note"] == "today · 3 in 30 days", t["tasks", "made"]
assert t["tasks", "started"]["note"] == "today · 2 in 30 days", t["tasks", "started"]
assert t["tasks", "ended"]["note"] == "today · 1 in 30 days", t["tasks", "ended"]
assert t["tasks", "lead"]["note"] == "median of 1" and t["tasks", "lead"]["unit"] == "s", t["tasks", "lead"]
assert [x["id"] for x in s["agents"]["tiles"]] == ["agent-time", "waiting", "answer", "unread", "turns", "turn-length"], s["agents"]
assert t["agents", "agent-time"]["unit"] == "h" and t["agents", "agent-time"]["note"].startswith("in 30 days · "), t["agents", "agent-time"]
assert t["agents", "answer"]["note"] == "median of 1" and t["agents", "answer"]["unit"] == "s", t["agents", "answer"]
assert t["agents", "unread"]["note"] == "1 not looked at yet", t["agents", "unread"]
assert t["agents", "turns"]["value"] == 2 and t["agents", "turns"]["note"] == "2 turns · 1 task", t["agents", "turns"]
assert t["agents", "turn-length"]["note"] == "median of 2" and t["agents", "turn-length"]["value"] >= 1, t["agents", "turn-length"]
assert "placeholder" not in s["code"], s["code"]
assert t["code", "erosion"]["value"] == 100 and t["code", "erosion"]["unit"] == "%", t["code", "erosion"]
assert t["code", "high"]["value"] == 100 and t["code", "high"]["unit"] == "%" and t["code", "high"]["note"].startswith("1 of 1 · ") and t["code", "lines"]["value"] == 13, (t["code", "high"], t["code", "lines"])
w = t["code", "worst"]["chart"]["rows"][0]
assert w["label"] == "busy" and w["open"] == {"path": "src/lib.rs", "line": 1}, w
assert t["code", "lines-covered"]["note"].startswith("not set up"), t["code", "lines-covered"]' "$json" "$repo" || return 1
  [[ "$(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select count(*), sum(value) from samples where metric = 'commits'")" == "3|4.0" ]] || { echo "stats_tab: the samples are $(sqlite3 -readonly "$KANNA_STATE_DIR/kanna.db" "select metric, value from samples")"; return 1; }
  (cd "$repo" && kanna stats) | grep -qE '^Main +Commits +2  today · 4 in 30 days ' || { echo "stats_tab: kanna stats prints"; (cd "$repo" && kanna stats); return 1; }
  (cd "$repo" && kanna stats) | grep -qE '^Tasks +Open +2  \+2 in 30 days ' || { echo "stats_tab: kanna stats prints the open tasks"; (cd "$repo" && kanna stats); return 1; }
  start_app || { echo "stats_tab line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  local opened d; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "stats_tab line $LINENO: $opened"; return 1; }
  d="${BASH_REMATCH[1]}"
  check wait_for 10 eval '[[ "$(desk pages "$d")" == '"'"'["Desk", "Machines", "Builds", "Stats", "Schedules", "Merge queue"]'"'"' ]]' || { echo "stats_tab: the Desk view's fixed tabs"; desk pages "$d"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$d\",\"size\":[1300,900]}}" >/dev/null
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"Machines\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(desk machinesPage "$d")" == True ]]' || { echo "stats_tab: the Machines tab"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Next Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk buildsPage "$d")" == True && "$(desk machinesPage "$d")" == False ]]' || { echo "stats_tab: Next Tab from Machines didn't show Builds"; return 1; }
  raw '{"id":1,"cmd":"debug.menu","args":{"item":"Next Tab"}}' >/dev/null
  check wait_for 5 eval '[[ "$(desk statsPage "$d")" == True && "$(desk buildsPage "$d")" == False ]]' || { echo "stats_tab: Next Tab from Builds didn't show Stats"; return 1; }
  check wait_for 10 eval '[[ "$(desk stats "$d")" != "[]" ]]' || { echo "stats_tab: the Stats page drew no tiles"; desk statsSections "$d"; return 1; }
  check python3 -c 'import json, sys
shown = {(x["section"], x["id"]): x for x in json.loads(sys.argv[1])}
r = json.loads(sys.argv[2])
for sec in r["sections"]:
    for x in sec["tiles"]:
        got = shown[sec["id"], x["id"]]
        assert got["label"] == x["label"] and got["note"] == x["note"] and got["tip"] == x.get("tip"), (got, x)
        assert got["frame"] and got["frame"]["width"] > 100 and got["frame"]["height"] > 30, got
assert shown["quiz", "right"]["figure"] == "75%" and shown["main", "commits"]["figure"] == "2", shown
assert shown["tasks", "lead"]["figure"] not in ("", "—"), shown["tasks", "lead"]
assert shown["tasks", "open"]["figure"] == "2", shown["tasks", "open"]
assert shown["agents", "turns"]["figure"] == "2.0" and shown["agents", "agent-time"]["figure"].endswith(("m", "h")), shown["agents", "turns"]
assert shown["agents", "agent-time"]["frame"]["y"] == shown["agents", "answer"]["frame"]["y"], shown["agents", "answer"]
assert shown["tasks", "open"]["frame"]["width"] > 2.5 * shown["tasks", "made"]["frame"]["width"] and shown["tasks", "open"]["frame"]["height"] > 150, shown["tasks", "open"]
code = [s for s in json.loads(sys.argv[3]) if s["id"] == "code"][0]
assert code["placeholder"] is False and code["tiles"] > 10, code
assert shown["code", "high"]["tip"].startswith("CC is cyclomatic complexity"), shown["code", "high"]
assert shown["code", "erosion"]["figure"] == "100%" and shown["code", "worst"]["rows"][0] == "busy", (shown["code", "erosion"], shown["code", "worst"])' "$(desk stats "$d")" "$json" "$(desk statsSections "$d")" || return 1
  local clicked; clicked="$(raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"stats\":\"worst\",\"row\":0,\"window\":\"$d\"}}")"
  check wait_for 10 eval 'kanna workspace list --all --json | python3 -c "import json, sys; panes = json.load(sys.stdin)[\"panes\"]; sys.exit(0 if any(p.get(\"type\") == \"code\" and p.get(\"path\", \"\").endswith(\"/src/lib.rs\") and p.get(\"line\") == 1 for p in panes) else 1)"' || { echo "stats_tab: clicking busy opened no code tab at src/lib.rs:1: $clicked $(state | tr -d '\n' | cut -c1-400)"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"Stats\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(desk statsPage "$d")" == True ]]' || { echo "stats_tab: back to the Stats tab"; return 1; }
  python3 -c '
import json, sys
sections = json.loads(sys.argv[1])
bands = [s["band"] for s in sections]
assert len(bands) >= 4 and all(b and b["height"] >= 28 and b["width"] > 200 for b in bands), bands
assert len({b["x"] for b in bands}) == 1 and all(a["y"] + a["height"] < b["y"] for a, b in zip(bands, bands[1:])), bands' "$(desk statsSections "$d")" || { echo "stats_tab: the section headings' bands"; return 1; }
  sleep 0.5; snap "$d" stats_tab-dark >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.5
  snap "$d" stats_tab-light >/dev/null
  [[ -s "$out/stats_tab-dark.png" && -s "$out/stats_tab-light.png" ]] || { echo "stats_tab: no snapshots"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"Desk\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(desk statsPage "$d")" == False && "$(desk page "$d")" == True ]]' || { echo "stats_tab: the Desk tab didn't leave the Stats page"; return 1; }
}

scenario stats_updating M4
stats_updating() {
  local repo; repo="$(e2e_repo updating)" || { echo "stats_updating line $LINENO"; return 1; }
  local holds="$KANNA_STATE_DIR/holds" real; real="$(command -v git)"
  mkdir -p "$holds/bin" "$repo/src"
  cat >"$holds/bin/git" <<SHIM
#!/bin/sh
case " \$* " in
  *" diff --numstat "*) while [ -e "$holds/reading" ]; do sleep 0.05; done ;;
  *" ls-tree "*) while [ -e "$holds/measuring" ]; do sleep 0.05; done; [ -e "$holds/slow" ] && sleep 0.5 ;;
esac
exec "$real" "\$@"
SHIM
  chmod +x "$holds/bin/git"
  commit() { git -C "$repo" -c user.email=e@e -c user.name=e "$@"; }
  grow() { printf 'pub fn f%s(a: u8) -> u8 {\n    if a > %s { 1 } else { 0 }\n}\n' "$1" "$1" >>"$repo/src/lib.rs"; git -C "$repo" add src && commit commit -q -m "grow $1"; }
  grow 1 && grow 2 || { echo "stats_updating line $LINENO"; return 1; }
  touch "$holds/reading"
  PATH="$holds/bin:$PATH" start_daemon && start_app || { echo "stats_updating line $LINENO"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$d\"}}" | field "$1"; }
  tab() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"$1\",\"window\":\"$d\"}}" >/dev/null; }
  reply() { (cd "$repo" && kanna stats --json); }
  tile() { python3 -c 'import json, sys
r = json.loads(sys.argv[1])
print(next(t[sys.argv[3]] for s in r["sections"] for t in s["tiles"] if t["id"] == sys.argv[2]))' "$(reply)" "$1" "$2"; }
  noted() { python3 -c 'import json, sys; sys.exit(0 if any(t["id"] == sys.argv[2] and sys.argv[3] in t["note"] for t in json.loads(sys.argv[1])) else 1)' "$(desk stats)" "$1" "$2"; }
  local opened d; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "stats_updating line $LINENO: $opened"; return 1; }
  d="${BASH_REMATCH[1]}"
  raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$d\",\"size\":[1300,900]}}" >/dev/null
  tab Stats
  check wait_for 5 eval '[[ "$(desk statsPage)" == True && "$(desk statsLoadings)" != 0 ]]' || { echo "stats_updating: a repo with no data shows Loading"; return 1; }
  check wait_for 5 eval '[[ "$(desk statsStatus)" == "Updating: reading 3 new commits" ]]' || { echo "stats_updating: the first step: $(desk statsStatus)"; return 1; }
  local started=$SECONDS json; json="$(reply)"
  (( SECONDS - started <= 1 )) || { echo "stats_updating: the reply waited for the catch-up"; return 1; }
  check test "$(field status <<<"$json")" == "Updating: reading 3 new commits" || { echo "stats_updating: kanna stats --json said $json"; return 1; }
  check test "$(field sections <<<"$json")" == "[]" || { echo "stats_updating: no data at all has no sections: $json"; return 1; }
  rm "$holds/reading"
  check wait_for 10 eval '[[ "$(desk statsStatus)" == None ]] && noted commits "3 in 30 days"' || { echo "stats_updating: the page didn't refresh by itself: $(desk statsStatus) $(desk stats)"; return 1; }
  local loadings; loadings="$(desk statsLoadings)"
  [[ "$(tile lines value)" == 4* ]] || { echo "stats_updating: the code lines: $(tile lines value)"; return 1; }

  tab Desk
  check wait_for 5 eval '[[ "$(desk statsPage)" == False ]]' || { echo "stats_updating line $LINENO"; return 1; }
  grow 3 && grow 4 && grow 5 || { echo "stats_updating line $LINENO"; return 1; }
  touch "$holds/reading" "$holds/measuring"
  tab Stats
  check wait_for 5 eval '[[ "$(desk statsStatus)" == "Updating: reading 3 new commits" ]]' || { echo "stats_updating: the new commits: $(desk statsStatus)"; return 1; }
  noted commits "3 in 30 days" || { echo "stats_updating: the old numbers went: $(desk stats)"; return 1; }
  check test "$(field status <<<"$(reply)")" == "Updating: reading 3 new commits" || { echo "stats_updating: kanna stats --json said $(reply)"; return 1; }
  check test "$(tile commits note)" == "today · 3 in 30 days" || { echo "stats_updating: the CLI's old numbers: $(tile commits note)"; return 1; }
  sleep 0.3; snap "$d" stats_updating-reading >/dev/null
  rm "$holds/reading"
  check wait_for 5 eval '[[ "$(desk statsStatus)" == "Updating: measuring code, 0 of 3 commits" ]] && noted commits "6 in 30 days"' || { echo "stats_updating: measuring: $(desk statsStatus) $(desk stats)"; return 1; }
  [[ "$(tile lines value)" == 4* ]] || { echo "stats_updating: the code's old numbers went: $(tile lines value)"; return 1; }
  rm "$holds/measuring"
  check wait_for 8 eval '[[ "$(desk statsStatus)" == None ]]' || { echo "stats_updating: the status stayed: $(desk statsStatus)"; return 1; }
  [[ "$(tile lines value)" == 10* ]] || { echo "stats_updating: the first catch-up's code: $(tile lines value)"; return 1; }

  tab Desk
  check wait_for 5 eval '[[ "$(desk statsPage)" == False ]]' || { echo "stats_updating line $LINENO"; return 1; }
  for n in 6 7 8 9 10 11 12 13; do grow "$n" || { echo "stats_updating line $LINENO"; return 1; }; done
  touch "$holds/slow"
  tab Stats
  check wait_for 8 eval '[[ "$(desk statsStatus)" =~ ^Updating:\ measuring\ code,\ [1-7]\ of\ 8\ commits$ ]]' || { echo "stats_updating: no step partway through: $(desk statsStatus)"; return 1; }
  sleep 0.2; snap "$d" stats_updating-measuring >/dev/null
  check wait_for 8 eval '[[ "$(desk statsStatus)" == None ]] && [[ "$(tile lines value)" == 26* ]]' || { echo "stats_updating: the page didn't end: $(desk statsStatus) $(tile lines value)"; return 1; }
  rm "$holds/slow"

  mkdir -p "$KANNA_AGENTS_HOME/.kanna-v4" && python3 -c 'import json, sys; c = json.load(open(sys.argv[3])); c["stats"] = {"coverage": {sys.argv[1]: "while [ -e " + sys.argv[2] + " ]; do sleep 0.05; done; printf \"SF:$PWD/src/lib.rs\\nDA:1,1\\nend_of_record\\n\" >\"$KANNA_COVERAGE\""}}; json.dump(c, open(sys.argv[3], "w"))' "$repo" "$holds/coverage" "$KANNA_AGENTS_HOME/.kanna-v4/config.json"
  touch "$holds/coverage"
  tab Desk
  check wait_for 5 eval '[[ "$(desk statsPage)" == False ]]' || { echo "stats_updating line $LINENO"; return 1; }
  tab Stats
  check wait_for 5 eval '[[ "$(desk statsStatus)" == "Updating: running coverage" ]]' || { echo "stats_updating: the coverage run: $(desk statsStatus)"; return 1; }
  snap "$d" stats_updating-coverage >/dev/null
  rm "$holds/coverage"
  check wait_for 10 eval '[[ "$(desk statsStatus)" == None ]] && [[ "$(tile lines-covered value)" != None ]]' || { echo "stats_updating: the run's end: $(desk statsStatus) $(tile lines-covered value) $(tail -3 "$out/daemon.log")"; return 1; }
  check test "$(desk statsLoadings)" == "$loadings" || { echo "stats_updating: the page went back to Loading: $(desk statsLoadings) after $loadings"; return 1; }
  [[ -s "$out/stats_updating-reading.png" && -s "$out/stats_updating-measuring.png" ]] || { echo "stats_updating: no snapshots"; return 1; }
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

scenario pipeline_stats M4
pipeline_stats() {
  local repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/stats-factory" fake="$KANNA_STATE_DIR/agents"
  mkdir -p "$fake/bin" && ln -s "$repo_root/tools/mock-agent/mock-agent" "$fake/bin/claude"
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$fake/bin/claude" >"$fake/claude" && chmod +x "$fake/claude"
  python3 -c 'import json, sys; p = sys.argv[1]; c = json.load(open(p)); c["pipelines"] = {"agents": {"claude": sys.argv[2]}}; json.dump(c, open(p, "w"))' "$KANNA_AGENTS_HOME/.kanna-v4/config.json" "$fake/claude" || { echo "pipeline_stats line $LINENO"; return 1; }
  CLAUDE_CONFIG_DIR="$KANNA_STATE_DIR/claude" start_daemon && start_app || { echo "pipeline_stats line $LINENO"; return 1; }
  mkdir -p "$repo/.kanna/pipelines" "$repo/.kanna/prompts" && git -C "$repo" init -q -b main || { echo "pipeline_stats line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  git -C "$repo" config user.email e@e && git -C "$repo" config user.name e
  cat >"$fake/usage.sh" <<USAGE
session="$KANNA_STATE_DIR/claude/projects/-mock/\$KANNA_MOCK_SESSION.jsonl"
mkdir -p "\$(dirname "\$session")"
printf '{"type":"assistant","timestamp":"2100-01-01T00:00:00.000Z","message":{"id":"m-%s","model":"mock-model","usage":{"input_tokens":%s,"output_tokens":%s,"cache_read_input_tokens":500,"cache_creation_input_tokens":50},"content":[{"type":"tool_use","id":"t-%s"}]}}\n' "\$KANNA_MOCK_SESSION" "\$1" "\$2" "\$KANNA_MOCK_SESSION" >>"\$session"
USAGE
  cat >"$fake/review.sh" <<REVIEW
n=\$(cat "$fake/reviews" 2>/dev/null || echo 0); n=\$((n + 1)); echo "\$n" >"$fake/reviews"
sh "$fake/usage.sh" 400 100
verdict=pass; [ "\$n" -eq 1 ] && verdict=fail
printf 'Looked.\n\`\`\`json name=findings\n[{"severity": "bug", "file": "a", "line": 1, "what": "x"}, {"severity": "nit", "what": "y"}]\n\`\`\`\n' >"\$KANNA_ANSWER"
"\$KANNA_CLI" run done "\$verdict"
REVIEW
  printf '{"version": 1, "pipelines": {"pricing": {"mock-model": {"in": 2, "out": 10}}}}\n' >"$repo/.kanna/config.json"
  machine_runners 1
  printf 'budget = 9 sessions, 30m\nbuild  = claude\ncheck  = run sh check.sh\n\nbuild -> check\ncheck -[fail, max 2]-> build\ncheck -> done\n' >"$repo/.kanna/pipelines/alpha.kojo"
  printf 'budget = 9 sessions, 30m\nbuild  = claude\nreview = claude verdict\n\nbuild -> review\nreview -[fail, max 2]-> build\nreview -> done\n' >"$repo/.kanna/pipelines/beta.kojo"
  printf 'Build it.\nmock: run sh %s 3000 700 && echo "$KANNA_RUN $KANNA_STAGE" >>"built-$KANNA_RUN.txt" && git add -A . && git commit -qm build && echo Built. >"$KANNA_ANSWER" && "$KANNA_CLI" run done\n' "$fake/usage.sh" >"$repo/.kanna/prompts/build.md"
  printf 'Review it.\nmock: run sh %s\n' "$fake/review.sh" >"$repo/.kanna/prompts/review.md"
  cat >"$repo/check.sh" <<CHECK
n=\$(cat "$fake/checks" 2>/dev/null || echo 0); n=\$((n + 1)); echo "\$n" >"$fake/checks"
if [ "\$n" -eq 1 ]; then echo "test parses ... FAILED"; exit 1; fi
CHECK
  printf '.kanna/tmp\n' >"$repo/.gitignore"
  git -C "$repo" add -A && git -C "$repo" commit -qm first || { echo "pipeline_stats line $LINENO"; return 1; }
  (cd "$repo" && kanna task new "Compare" "run $bin/kanna run start alpha" --agent "$fake/claude") >/dev/null || { echo "pipeline_stats line $LINENO"; return 1; }
  local task; task="$(tid 1)"
  local folder="$repo/.kanna-worktrees/$task"
  held() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
print([t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0].get("held") or "-")' "$task"; }
  states() { kanna run list --all --json | python3 -c 'import json, sys
print(" ".join(x["state"] for x in json.load(sys.stdin)["runs"]))'; }
  next() { local said=""; said="$(cd "$folder" && KANNA_TERMINAL="$(held)" kanna run start "$1" 2>&1)" || { echo "next $1: $said" >>"$out/next.log"; return 1; }; }
  check wait_for 40 eval '[[ "$(states)" == done ]]' || { echo "pipeline_stats: the first run didn't hand back: $(states)"; tail -20 "$out/daemon.log"; return 1; }
  check wait_for 30 next alpha || { echo "pipeline_stats: the second run didn't start"; cat "$out/next.log"; return 1; }
  check wait_for 40 eval '[[ "$(states)" == "done done" ]]' || { echo "pipeline_stats: the second run: $(states)"; return 1; }
  check wait_for 30 next beta || { echo "pipeline_stats: the third run didn't start"; return 1; }
  check wait_for 40 eval '[[ "$(states)" == "done done done" ]]' || { echo "pipeline_stats: the third run: $(states)"; kanna run list --all; return 1; }
  check wait_for 30 next beta || { echo "pipeline_stats: the fourth run didn't start"; return 1; }
  check wait_for 40 eval '[[ "$(states)" == "done done done done" ]]' || { echo "pipeline_stats: the fourth run: $(states)"; return 1; }
  local first; first="$(git -C "$repo" rev-parse "run/$task-1")"
  git -C "$repo" merge -q --no-ff -m "Merge run/$task-1" "run/$task-1" && git -C "$repo" merge -q --no-ff -m "Merge run/$task-2" "run/$task-2" && git -C "$repo" merge -q --no-ff -m "Merge run/$task-3" "run/$task-3" || { echo "pipeline_stats line $LINENO"; return 1; }
  git -C "$repo" checkout -q -b fix-it && git -C "$repo" commit -q --allow-empty -m "Fix the build" -m "Fixes: $first" && git -C "$repo" checkout -q main && git -C "$repo" merge -q --no-ff -m "Merge fix-it" fix-it || { echo "pipeline_stats line $LINENO"; return 1; }
  local json
  landed() { json="$(cd "$repo" && kanna stats pipelines --json)" && python3 -c 'import json, sys; r = json.loads(sys.argv[1]); sys.exit(0 if sum(p["landed"] for p in r["pipelines"]) == 3 else 1)' "$json"; }
  check wait_for 20 landed || { echo "pipeline_stats: three landed runs never showed: $json"; return 1; }
  check wait_for 10 eval 'json="$(cd "$repo" && kanna stats pipelines --json)"; python3 -c "import json, sys; r = json.loads(sys.argv[1]); sys.exit(0 if r[\"runs\"][0][\"bugsTracedBack\"] == 1 else 1)" "$json"' || { echo "pipeline_stats: the fix wasn't traced to the first run: $json"; return 1; }
  check python3 -c 'import json, sys
r = json.loads(sys.argv[1])
assert r["days"] == 30 and "F1 is about 0.6" in r["caveat"], r["caveat"]
p = {x["pipeline"]: x for x in r["pipelines"]}
assert list(p) == ["alpha", "beta"] and [x["series"] for x in r["pipelines"]] == [0, 1], r["pipelines"]
a, b = p["alpha"], p["beta"]
assert a["runs"] == 2 and a["landed"] == 2 and a["landedShare"] == 1, a
assert b["runs"] == 2 and b["landed"] == 1 and b["landedShare"] == 0.5, b
assert len(a["hash"]) > 4 and a["hash"] != b["hash"] and a["revision"] == 1, a
assert a["medians"]["reworkLoops"] == 1 and b["medians"]["reworkLoops"] == 1, (a, b)
assert a["medians"]["timeToLand"] > 0 and a["medians"]["activeTime"] > 0 and a["medians"]["findings"] is None, a
assert b["medians"]["findings"] == 3, b
assert a["medians"]["bugsTracedBack"] == 0.5, a
runs = r["runs"]
one, two, three, four = runs
assert [x["pipeline"] for x in runs] == ["alpha", "alpha", "beta", "beta"], runs
assert [x["outcome"] for x in runs] == ["landed", "landed", "landed", "handed back"], runs
assert one["visits"] == 4 and one["sessions"] == 2 and one["failingTests"] == 1 and one["failedRuns"] == 1 and one["reworkByStage"] == {"build": 1, "check": 1}, one
assert one["tokens"] == {"total": 7500, "in": 6000, "out": 1400, "cached": 1000, "written": 100}, one["tokens"]
assert one["turns"] == 2 and one["tools"] == 2 and one["commits"] == 2 and one["added"] == 2 and one["files"] == 2, one
assert one["cost"] == 6000 * 2 / 1e6 + 1400 * 10 / 1e6 + 1000 * 2 / 1e6 + 100 * 2 / 1e6, one["cost"]
assert one["timeToLand"] >= one["timeToHandBack"] > 0 and one["landing"], one
assert three["findings"] == {"bug": 2, "nit": 2} and three["reworkLoops"] == 2 and three["visits"] == 4, three
assert four["findings"] == {"bug": 1, "nit": 1} and four["timeToLand"] is None and four["timeToHandBack"] > 0, four
assert one["qualityDelta"]["erosion"] is not None or two["qualityDelta"]["erosion"] is not None, one
machines = {(x["pipeline"], x["stage"]) for x in r["stages"]}
assert machines == {("alpha", "build"), ("alpha", "check"), ("beta", "build"), ("beta", "review")} and all(x["machine"] and x["visits"] for x in r["stages"]), r["stages"]
tiles = {x["id"]: x for x in r["sections"][0]["tiles"]}
assert [c["text"] for c in tiles["pipelines"]["chart"]["rows"][1]["cells"]][:3] == ["beta rev 1", "2", "1 (50%)"], tiles["pipelines"]["chart"]["rows"]
assert len(tiles["run-land"]["chart"]["points"]) == 3 and len(tiles["run-land"]["chart"]["medians"]) == 2, tiles["run-land"]
assert len(tiles["run-tokens"]["chart"]["points"]) == 4, tiles["run-tokens"]' "$json" || { echo "pipeline_stats: kanna stats pipelines said: $json"; return 1; }
  local printed; printed="$(cd "$repo" && kanna stats pipelines)"
  grep -qE 'alpha rev 1 +2 +2 \(100%\)' <<<"$printed" || { echo "pipeline_stats: kanna stats pipelines prints"; echo "$printed"; return 1; }
  (cd "$repo" && kanna stats pipelines --days 0 --json) | python3 -c 'import json, sys; r = json.load(sys.stdin); sys.exit(0 if r["days"] is None and len(r["runs"]) == 4 else 1)' || { echo "pipeline_stats: --days 0 isn't every run"; return 1; }
  local shown; shown="$(kanna run show "$(python3 -c 'import json, sys; print(json.loads(sys.argv[1])["runs"][0]["run"])' "$json")" --json)"
  python3 -c 'import json, sys
s = json.loads(sys.argv[1])
assert s["metrics"]["tokens"]["total"] == 7500, s["metrics"]
u = s["visits"][0]["doc"]["usage"]
assert (u["turns"], u["tools"], u["in"], u["out"], u["cached"], u["written"], u["model"], u["costBy"]) == (1, 1, 3000, 700, 500, 50, "mock-model", "pricing"), u
assert s["visits"][0]["doc"]["commits"] == 1 and s["visits"][0]["doc"]["added"] == 1 and s["visits"][1]["doc"]["failing"] == 1, s["visits"][:2]' "$shown" || { echo "pipeline_stats: kanna run show said: $shown"; return 1; }
  desk() { raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$2\"}}" | field "$1"; }
  local opened d; opened="$(cd / && kanna "$repo")" && [[ "$opened" =~ ^opened\ (d[0-9]+) ]] || { echo "pipeline_stats line $LINENO: $opened"; return 1; }
  d="${BASH_REMATCH[1]}"
  raw "{\"id\":1,\"cmd\":\"debug.desk\",\"args\":{\"window\":\"$d\",\"size\":[1900,1300]}}" >/dev/null
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"page\":\"Stats\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 10 eval '[[ "$(desk stats "$d")" == *run-land* ]]' || { echo "pipeline_stats: the Stats tab has no Pipelines section"; desk statsSections "$d"; return 1; }
  check python3 -c 'import json, sys
shown = {x["id"]: x for x in json.loads(sys.argv[1]) if x["section"] == "pipelines"}
assert [r[:3] for r in shown["pipelines"]["table"]] == [["alpha rev 1", "2", "2 (100%)"], ["beta rev 1", "2", "1 (50%)"]], shown["pipelines"]
assert shown["run-land"]["dots"] == 3 and shown["run-tokens"]["dots"] == 4 and shown["run-rework"]["dots"] == 4, shown["run-land"]
assert len(shown["recent"]["table"]) == 4 and shown["machines"]["table"], shown["recent"]
assert "F1 is about 0.6" in shown["pipelines"]["note"], shown["pipelines"]["note"]' "$(desk stats "$d")" || { desk stats "$d"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"statsChip\":\"30 days\",\"window\":\"$d\"}}" >/dev/null
  sleep 0.5; snap "$d" pipeline_stats-dark >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.5
  snap "$d" pipeline_stats-light >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"statsChip\":\"beta\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval 'python3 -c "import json, sys; s = {x[\"id\"]: x for x in json.loads(sys.argv[1]) if x[\"section\"] == \"pipelines\"}; sys.exit(0 if len(s[\"pipelines\"][\"table\"]) == 1 and s[\"run-tokens\"][\"dots\"] == 2 else 1)" "$(desk stats "$d")"' || { echo "pipeline_stats: hiding beta left its rows"; desk stats "$d"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"statsChip\":\"beta\",\"window\":\"$d\"}}" >/dev/null
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"statsChip\":\"Table view\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(desk statsSections "$d")" == *"\"tableView\": true"* ]]' || { echo "pipeline_stats: Table view didn't take"; desk statsSections "$d"; return 1; }
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"statsChip\":\"90 days\",\"window\":\"$d\"}}" >/dev/null
  check wait_for 5 eval '[[ "$(desk statsSections "$d")" == *"\"on\": ["*"90 days"* ]]' || { echo "pipeline_stats: 90 days didn't take"; desk statsSections "$d"; return 1; }
}
