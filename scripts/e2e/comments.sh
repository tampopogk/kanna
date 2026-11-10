long_comment="A long comment that runs on for a while, so that it wraps over many lines in the comment box and then some more past its cap. "
long_comment="$long_comment$long_comment$long_comment$long_comment$long_comment$long_comment"
box_fits() { python3 -c 'import json, sys
r = json.load(sys.stdin)
p = {k: (x, y, x + w, y + h) for k, (x, y, w, h) in r["box_parts"].items()}
box, f = p["box"], p["field"]
inside = lambda a: a[0] >= box[0] and a[1] >= box[1] and a[2] <= box[2] and a[3] <= box[3]
assert f[3] - f[1] == 128, ("the field should grow to its cap of 8 lines", r["box_parts"])
assert p["text"][3] - p["text"][1] > 128, ("past its cap the text should scroll inside the field", r["box_parts"])
for k in ("box", "field", "hint", "post"):
    if k in p: assert inside(p[k]), (k, "is outside the box", r["box_parts"])
for k in ("hint", "post"):
    if k in p: assert p[k][1] >= f[3], (k, "overlaps the field", r["box_parts"])
assert r["box_shown"], ("the box should stay in view", r["box_parts"])'; }

comment_agents() {
  comment_repo="$(e2e_repo comments)" || return 1
  comment_fake="$KANNA_STATE_DIR/comment-agents"
  mkdir -p "$comment_fake/bin" "$comment_repo/.kanna" || return 1
  machine_runners 2
  ln -sf "$repo_root/tools/mock-agent/mock-agent" "$comment_fake/bin/claude"
  ln -sf "$repo_root/tools/mock-agent/mock-agent" "$comment_fake/copilot"
  printf '#!/bin/sh\nKANNA_AGENTS_HOME=%s MOCK_AGENT_REPORTS=1 exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$comment_fake/bin/claude" >"$comment_fake/claude" && chmod +x "$comment_fake/claude"
  mkdir -p "$comment_fake/slow"
  printf '#!/bin/sh\nKANNA_MOCK_INBOX_DELAY=10 KANNA_AGENTS_HOME=%s MOCK_AGENT_REPORTS=1 exec %s "$@"\n' "$KANNA_AGENTS_HOME" "$comment_fake/bin/claude" >"$comment_fake/slow/claude" && chmod +x "$comment_fake/slow/claude"
}
comment_held() { raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
t = [t for t in json.load(sys.stdin)["shelf"] if t["id"] == sys.argv[1]][0]
print((t.get("held") or "-") if t.get("running") else "stopped")' "$1"; }
comment_claude() {
  comment_task="$(cd "$comment_repo" && kanna --json task new "$1" "work ${3:-12}" --agent "${2:-$comment_fake/claude}" | field task.id)" || return 1
  check wait_for 15 eval '[[ "$(comment_held "$comment_task")" == term-* ]]' || { comment_held "$comment_task"; return 1; }
  comment_term="$(comment_held "$comment_task")"
  check wait_for 10 is "$comment_term" status working || { kanna status explain "$comment_term"; return 1; }
  kanna send-key "$comment_term" d r a f t >/dev/null
}
comment_waits() {
  local thread="$1" status="$2" screen
  check wait_for 5 eval '[[ "$($status)" == waiting ]]' || return 1
  screen="$(kanna read "$comment_term")"
  [[ "$screen" != *"kanna comment $thread"* ]] || { echo "a comment was typed mid-turn"; echo "$screen"; return 1; }
  is "$comment_term" status working || { echo "the agent's turn ended before the check"; return 1; }
}
comment_arrived() {
  local thread="$1" status="$2" screen
  check wait_for 25 shows "$comment_term" "peer: \[kanna comment $thread · " || { kanna read "$comment_term"; tail -5 "$out/daemon.log"; return 1; }
  check wait_for 5 eval '[[ "$($status)" == sent ]]' || return 1
  screen="$(kanna read "$comment_term")"
  [[ "$(grep -F "kanna comment" <<<"$screen" | grep -cv "peer: ")" == 0 ]] || { echo "a comment went into the prompt's draft"; echo "$screen"; return 1; }
  [[ "$(cd "$comment_repo" && kanna task messages "$comment_task")" == *"user"*"kanna comment $thread"* ]] || { echo "task messages should list the comment from you"; (cd "$comment_repo" && kanna task messages "$comment_task"); return 1; }
}

scenario comment_before_socket M4
comment_before_socket() {
  start_daemon && start_app || return 1
  comment_agents && comment_claude "No socket yet" "$comment_fake/slow/claude" 18 || return 1
  comment_program() { raw "{\"id\":1,\"cmd\":\"status\",\"args\":{\"action\":\"explain\",\"pane\":\"$comment_term\"}}" | field agent; }
  check wait_for 5 eval '[[ "$(comment_program)" == claude ]]' || { kanna status explain "$comment_term"; return 1; }
  local doc="$comment_repo/notes.md" said
  mkdir -p "$comment_repo/.kanna/docs"
  printf '# Notes\n\nThe strip scrolls.\n' >"$doc"
  printf '{"file":"%s","agent":null,"socket":null,"next":2,"threads":[{"id":"c1","quote":"The strip scrolls.","line":3,"state":"open","status":"waiting","messages":[{"who":"you","text":"Say when","at":"2026-01-01T00:00:00Z"}]}]}\n' "$doc" >"$comment_repo/.kanna/docs/notes.md.json"
  comment_status() { python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["threads"][0]["status"])' "$comment_repo/.kanna/docs/notes.md.json"; }
  said="$(raw "{\"id\":1,\"cmd\":\"tasks\",\"args\":{\"action\":\"comment\",\"pane\":\"$comment_term\",\"text\":\"[kanna comment c1 · notes.md:3]\\nSay when\",\"doc\":\"$doc\",\"thread\":\"c1\"}}")"
  check test "$(field state <<<"$said")" == waiting || { echo "$said"; kanna read "$comment_term"; return 1; }
  comment_waits c1 comment_status || return 1
  comment_arrived c1 comment_status || return 1
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
  key cmd+return
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
  xy="$(md_comments_md ',"mark":"c3"')"
  md_comments_md ",\"scroll\":$(python3 -c 'import sys; print(max(0, float(sys.argv[1]) + float(sys.argv[2]) - 34 - 60))' "$(md_comments_md | field scroll)" "$(echo "$xy" | field y)")" >/dev/null
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
  check wait_for 5 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"doc2\"}}" | field markup)" == True ]]' || return 1
  comment_agents && comment_claude "Claude reads comments" || return 1
  printf '# Notes\n\nThe strip scrolls, TELLCLAUDE when it must.\n\nAnd TELLCOPILOT too.\n' >"$comment_repo/notes.md"
  kanna markup "$comment_repo/notes.md" --to "$comment_term" --name notes --window new >/dev/null || return 1
  md_comments_md() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"notes\"${1:-}}}"; }
  check wait_for 5 eval '[[ "$(md_comments_md | field agent)" == "$comment_term" ]]' || { md_comments_md; return 1; }
  md_comments_post() {
    kanna focus notes >/dev/null; sleep 0.3
    at="$(md_comments_md | field text | python3 -c 'import sys; print(sys.stdin.read().index(sys.argv[1]))' "$1")"
    md_comments_md ",\"select\":[$at,${#1}]" >/dev/null
    key cmd+return; check test "$(md_comments_md | field box)" == new || return 1
    raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$2\"}}" >/dev/null
    key cmd+return
  }
  is "$comment_term" status working || { echo "the agent's turn ended before the comment"; return 1; }
  md_comments_post TELLCLAUDE "Say when"
  comment_waits c1 "md_comments_thread c1 status" || { md_comments_md; return 1; }
  check test "$(md_comments_md | field cards.c1.waiting)" == "Waiting · it goes to the agent when its turn is over" || { md_comments_md; return 1; }
  md_comments_shot md_comments-waiting
  check wait_for 15 shows "$comment_term" "› draft" || { kanna read "$comment_term"; return 1; }
  comment_arrived c1 "md_comments_thread c1 status" || return 1
  shows "$comment_term" "peer: > TELLCLAUDE" && shows "$comment_term" "peer: Say when" || { kanna read "$comment_term"; return 1; }
  check wait_for 20 eval '[[ "$(comment_held "$comment_task")" == stopped ]]' || { echo "the Shelf task should stop after the comment's turn"; return 1; }
  xy="$(md_comments_md ',"mark":"c1"')"
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"notes\",\"at\":[$(echo "$xy" | field x),$(echo "$xy" | field y)]}}" >/dev/null
  check test "$(md_comments_md | field box)" == c1 || return 1
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"While you slept"}}' >/dev/null
  key cmd+return
  check wait_for 5 eval '[[ "$(md_comments_thread c1 status)" == waiting ]]' || { md_comments_md; return 1; }
  key escape
  check wait_for 20 eval '[[ "$(comment_held "$comment_task")" == term-* ]]' || { echo "a comment should wake the stopped Shelf task"; comment_held "$comment_task"; return 1; }
  comment_term="$(comment_held "$comment_task")"
  check wait_for 25 shows "$comment_term" "peer: While you slept" || { kanna read "$comment_term"; return 1; }
  check wait_for 5 eval '[[ "$(md_comments_thread c1 status)" == sent ]]' || return 1
  kanna open --window new --name copilot --cmd "$comment_fake/copilot" >/dev/null || return 1
  check wait_for 10 shows copilot "mock-agent:" || return 1
  kanna markup notes --to copilot >/dev/null || return 1
  check wait_for 5 eval '[[ "$(md_comments_md | field agent)" == "$(pane copilot id)" ]]' || { md_comments_md; return 1; }
  md_comments_post TELLCOPILOT "Typed for you"
  check wait_for 5 eval '[[ "$(md_comments_thread c2 status)" == sent ]]' || { md_comments_md; return 1; }
  check wait_for 5 shows copilot "echo: \[kanna comment c2 · notes.md:5\]" || { kanna read copilot; return 1; }
  check test "$(cd "$comment_repo" && kanna task messages | grep -c "kanna comment c2")" == 0
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

scenario web_pinch_comments M4
web_pinch_comments() {
  start_daemon && start_app || return 1
  kanna open --window new --name agent --cmd "$(probe '\033[?2004h' x 0)" >/dev/null || return 1
  check wait_for 5 shows agent READY || return 1
  local dir="$KANNA_STATE_DIR/pinchmark" agent_id xy
  mkdir -p "$dir/.git"
  python3 - "$dir/page.html" <<'PY'
import sys
cells = "".join(f'<div id="c{y}-{x}" style="position:absolute;left:{x * 120}px;top:{y * 120}px;width:120px;height:120px">{y}-{x}</div>' for y in range(20) for x in range(12))
open(sys.argv[1], "w").write(f'<!doctype html><html><head><meta charset="utf-8"></head><body style="margin:0;height:2400px;width:1440px;position:relative">{cells}</body></html>')
PY
  agent_id="$(pane agent id)"
  KANNA_TERMINAL="$agent_id" kanna markup --name page --tab "$dir/page.html" >/dev/null || return 1
  web_pinch_page() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"page\"${1:-}}}"; }
  check wait_for 5 eval '[[ "$(web_pinch_page | field agent)" == "$agent_id" ]]' || { web_pinch_page; return 1; }
  web_pinch_page ',"js":"scrollTo(120, 700)"' >/dev/null; sleep 0.3
  check wait_for 5 eval 'web_pinch_page ",\"find\":\"#c7-3\"" | grep -q at' || return 1
  xy="$(web_pinch_page ',"find":"#c7-3"' | field at)"
  raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"page\",\"command\":true,\"at\":$xy}}" >/dev/null
  check wait_for 5 eval '[[ "$(web_pinch_page | field box)" == new ]]' || { web_pinch_page; return 1; }
  raw '{"id":1,"cmd":"debug.type","args":{"focused":true,"text":"Here"}}' >/dev/null
  key cmd+return
  check wait_for 5 eval 'web_pinch_page | field areas.c1 >/dev/null 2>&1' || { web_pinch_page; return 1; }
  web_pinch_lined_up() {
    local mark element
    mark="$(web_pinch_page | field areas.c1)"; element="$(web_pinch_page ',"find":"#c7-3"' | field at)"
    python3 -c 'import json, sys; a, b = json.loads(sys.argv[1]), json.loads(sys.argv[2]); sys.exit(not (abs(a[0] - b[0]) < 3 and abs(a[1] - b[1]) < 3))' "$mark" "$element" || { echo "the mark left its element: $mark, $element"; return 1; }
  }
  check wait_for 3 web_pinch_lined_up || { web_pinch_lined_up; return 1; }
  raw '{"id":1,"cmd":"debug.pinch","args":{"pane":"page","at":[260,180],"steps":[0.2,0.2,0.2]}}' >/dev/null
  check wait_for 3 web_pinch_lined_up || { web_pinch_lined_up; return 1; }
  check test "$(web_pinch_page ',"find":"#c7-3"' | field at)" != "$xy" || { echo "the pinch did not move the element"; return 1; }
  raw '{"id":1,"cmd":"debug.pinch","args":{"pane":"page","at":[120,90],"steps":[-0.15,-0.1]}}' >/dev/null
  check wait_for 3 web_pinch_lined_up || { web_pinch_lined_up; return 1; }
  kanna focus page >/dev/null || return 1
  key cmd+=
  check wait_for 3 web_pinch_lined_up || { web_pinch_lined_up; return 1; }
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
  local page_width; page_width="$(page_comments_plain | field page_width)"
  KANNA_TERMINAL="$agent_id" kanna markup plain | grep -qF "comments on in" || return 1
  check wait_for 5 eval '[[ "$(page_comments_plain | field markup)" == True ]]' && check wait_for 5 eval 'page_comments_plain | grep -q threads' || { page_comments_plain; return 1; }
  check test "$(page_comments_kept)" == "$kept" || { echo "turning comments on should keep the web view, its page, its scroll and its JS state ($kept)"; return 1; }
  check test "$(page_comments_plain | field page_width)" == "$page_width" || { echo "the comments list moved the page: width $page_width, then $(page_comments_plain | field page_width)"; return 1; }
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
  key cmd+return
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
  local edge; edge="$(page_comments_web ',"js":"(() => { const r = document.getElementById(\"red\").getBoundingClientRect(), s = visualViewport.scale; return [Math.round((r.right - 8) * s), Math.round((r.top + r.height / 2) * s)]; })()"' | field value)"
  raw "{\"id\":1,\"cmd\":\"debug.pinch\",\"args\":{\"pane\":\"page\",\"at\":$edge,\"steps\":[0.15,0.1]}}" >/dev/null
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
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null
  comment_agents && comment_claude "Claude reads page comments" || return 1
  cp "$dir/page.html" "$comment_repo/page.html"
  kanna markup "$comment_repo/page.html" --to "$comment_term" --name cpage --window new >/dev/null || return 1
  page_comments_web() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"cpage\"${1:-}}}"; }
  page_comments_click() { raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"cpage\",$1}}" >/dev/null; }
  check wait_for 5 eval '[[ "$(page_comments_web | field agent)" == "$comment_term" ]]' || { page_comments_web; return 1; }
  check wait_for 5 eval 'page_comments_web ",\"find\":\"#go\"" | grep -q at' || return 1
  page_comments_post() {
    kanna focus cpage >/dev/null; sleep 0.3
    page_comments_click "\"command\":true,\"at\":$(page_comments_web ",\"find\":\"$1\"" | field at)"
    check wait_for 5 eval '[[ "$(page_comments_web | field box)" == new ]]' || { page_comments_web; return 1; }
    raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$2\"}}" >/dev/null
    key cmd+return
  }
  is "$comment_term" status working || { echo "the agent's turn ended before the comment"; return 1; }
  page_comments_post "#go" "Bigger, Claude" || return 1
  comment_waits c1 "page_comments_thread c1 status" || { page_comments_web; return 1; }
  comment_arrived c1 "page_comments_thread c1 status" || return 1
  shows "$comment_term" "peer: Bigger, Claude" && shows "$comment_term" "peer: > marked up: .kanna/docs/marks/page.html-c1.png" || { kanna read "$comment_term"; return 1; }
  kanna open --window new --name copilot --cmd "$comment_fake/copilot" >/dev/null || return 1
  check wait_for 10 shows copilot "mock-agent:" || return 1
  kanna markup cpage --to copilot >/dev/null || return 1
  check wait_for 5 eval '[[ "$(page_comments_web | field agent)" == "$(pane copilot id)" ]]' || { page_comments_web; return 1; }
  page_comments_post "#red" "Typed for you" || return 1
  check wait_for 5 eval '[[ "$(page_comments_thread c2 status)" == sent ]]' || { page_comments_web; return 1; }
  check wait_for 5 shows copilot "echo: \[kanna comment c2 · page.html:" || { kanna read copilot; return 1; }
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

durable_same() {
  local id="$1" name="$2" when="$3" task
  task="$(raw '{"id":1,"cmd":"tasks","args":{"action":"list"}}' | python3 -c 'import json, sys
for t in json.load(sys.stdin)["shelf"]:
    if t["id"] == sys.argv[1]: print(t.get("held") or "")' "$comment_task" 2>/dev/null)"
  [[ "$(pane "$name" id)" == "$id" ]] || { echo "$when: $name's pane should still be $id, not $(pane "$name" id)"; return 1; }
  [[ -z "$task" || "$task" == "$id" ]] || { echo "$when: the task should hold $id, not $task"; return 1; }
}

scenario task_pane_keeps_comments M4
task_pane_keeps_comments() {
  start_daemon && start_app || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  comment_agents && comment_claude "Durable" || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  local id="$comment_term" first second third
  printf '<!doctype html><title>Page</title><button id="one">One</button><p><button id="two">Two</button></p><p><button id="three">Three</button></p>\n' >"$comment_repo/page.html"
  KANNA_TERMINAL="$id" kanna markup --name page --tab "$comment_repo/page.html" >/dev/null || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  check wait_for 5 is page agent "$id" || { kanna workspace list --all --json; return 1; }
  durable_web() { raw "{\"id\":1,\"cmd\":\"debug.web\",\"args\":{\"pane\":\"page\"${1:-}}}"; }
  durable_thread() { durable_web | field threads | python3 -c 'import json, sys
print([t for t in json.load(sys.stdin) if t["id"] == sys.argv[1]][0][sys.argv[2]])' "$1" "$2"; }
  durable_post() {
    local xy
    kanna focus page >/dev/null
    check wait_for 5 eval 'durable_web ",\"find\":\"#$1\"" | grep -q at' || { durable_web; return 1; }
    xy="$(durable_web ",\"find\":\"#$1\"" | field at)"
    raw "{\"id\":1,\"cmd\":\"debug.click\",\"args\":{\"pane\":\"page\",\"command\":true,\"at\":$xy}}" >/dev/null
    check wait_for 5 eval '[[ "$(durable_web | field box)" == new ]]' || { durable_web; return 1; }
    raw "{\"id\":1,\"cmd\":\"debug.type\",\"args\":{\"focused\":true,\"text\":\"$2\"}}" >/dev/null
    key cmd+return
    sleep 0.2
    key escape
  }

  check wait_for 30 eval '[[ "$(comment_held "$comment_task")" == stopped ]]' || { echo "task_pane_keeps_comments: the Shelf runner should stop the task"; comment_held "$comment_task"; return 1; }
  check is "$id" state exited || { kanna workspace list --all --json; return 1; }
  durable_same "$id" Durable "stopped by a Shelf runner" || return 1
  first="$(pane "$id" terminal)"
  kanna task pick "$comment_task" >/dev/null || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  check wait_for 10 is "$id" state running || { kanna workspace list --all --json; return 1; }
  check wait_for 5 shows "$id" "--resume" || { kanna read "$id"; return 1; }
  durable_same "$id" Durable "moved to the Desk" || return 1
  second="$(pane "$id" terminal)"
  [[ "$second" != "$first" && "$second" != "$id" ]] || { echo "task_pane_keeps_comments: a new run needs a terminal of its own ($first, $second)"; return 1; }
  check is page agent "$id" || { pane page agent; return 1; }
  [[ "$(cd "$comment_repo" && KANNA_TERMINAL="$id" kanna task list "$id" | placed)" == *Durable* ]] || { echo "task_pane_keeps_comments: the task should be found by its pane"; return 1; }
  kanna task activate "$comment_task" >/dev/null || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(durable_web | field agent)" == "$id" ]]' || { durable_web; return 1; }
  durable_post one "After the move" || return 1
  comment_arrived c1 "durable_thread c1 status" || return 1

  kanna task stop "$comment_task" >/dev/null && check wait_for 5 is "$id" state exited || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  durable_post two "While it was stopped" || return 1
  check wait_for 5 eval '[[ "$(durable_thread c2 status)" == waiting ]]' || { durable_web; tail -5 "$out/daemon.log"; return 1; }
  ! shows "$id" "kanna comment c2" || { echo "task_pane_keeps_comments: a comment reached a stopped task"; return 1; }
  kanna task start "$comment_task" >/dev/null || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  comment_arrived c2 "durable_thread c2 status" || return 1
  durable_same "$id" Durable "started on the Desk" || return 1

  third="$(pane "$id" terminal)"
  stop_app; stop_daemon; start_daemon && start_app || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  check is "$id" terminal "$third" && check is "$id" state running || { echo "task_pane_keeps_comments: the keeper's run should come back behind the pane"; kanna workspace list --all --json; return 1; }
  durable_same "$id" Durable "after a daemon restart" || return 1
  kanna task activate "$comment_task" >/dev/null || { echo "task_pane_keeps_comments line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(durable_web | field agent)" == "$id" ]]' || { durable_web; return 1; }
  durable_post three "After the daemon restarted" || return 1
  comment_arrived c3 "durable_thread c3 status" || return 1
}
