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
  sampled() { json="$(cd "$repo" && kanna stats --json)" && python3 -c 'import json, sys; c = [s for s in json.loads(sys.argv[1])["sections"] if s["id"] == "code"][0]; sys.exit(0 if "sampling" not in c["note"] and any(t["id"] == "erosion" and t["value"] is not None for t in c["tiles"]) else 1)' "$json"; }
  check wait_for 20 sampled || { echo "stats_tab: kanna stats --json never had the code sampled: $json"; return 1; }
  check python3 -c 'import json, sys
r = json.loads(sys.argv[1])
assert r["project"] == sys.argv[2], r["project"]
assert len(r["days"]) == 30, r["days"]
s = {x["id"]: x for x in r["sections"]}
assert list(s) == ["quiz", "main", "tasks", "agents", "code"], list(s)
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
  check wait_for 10 eval '[[ "$(desk pages "$d")" == '"'"'["Desk", "Machines", "Stats", "Schedules"]'"'"' ]]' || { echo "stats_tab: the Desk view's fixed tabs"; desk pages "$d"; return 1; }
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
