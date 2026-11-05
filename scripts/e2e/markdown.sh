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
  mdattop() { python3 -c 'import sys; sys.exit(0 if abs(float(sys.argv[1])) < 1 else 1)' "$(mdo scroll)"; }
  mdwin 1200
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == True ]]' || { echo "no overview: wanted $(mdo overview.wanted), width $(mdo width)"; return 1; }
  check eval '(( $(mdo overview.box_height | cut -d. -f1) > 10 ))' || { echo "box $(mdo overview.box_y) $(mdo overview.box_height)"; return 1; }
  check eval '(( $(mdo pictures) >= 1 ))' || return 1
  pictures="$(mdo pictures)"
  mdo scroll ',"scroll":2000' >/dev/null
  check wait_for 5 eval '[[ "$(mdo overview.top | cut -d. -f1)" == 2000 ]]' || { echo "the box didn't follow the scroll: top $(mdo overview.top)"; return 1; }
  sleep 0.3
  check test "$(mdo pictures)" == "$pictures" || { echo "a scroll drew the picture again: $pictures, then $(mdo pictures)"; return 1; }
  local box; box="$(mdo overview.box_height)"
  raw '{"id":1,"cmd":"debug.pinch","args":{"pane":"longdoc"}}' >/dev/null; sleep 0.3
  check test "$(mdo pictures)" == "$pictures" || { echo "a pinch drew the picture again: $pictures, then $(mdo pictures)"; return 1; }
  check python3 -c 'import sys; assert float(sys.argv[2]) < float(sys.argv[1]) * 0.8, sys.argv[1:]' "$box" "$(mdo overview.box_height)" || { echo "the box didn't follow the pinch"; return 1; }
  mdo zoom ',"page_zoom":1' >/dev/null
  mdo zoom ',"zoom":1' >/dev/null
  check wait_for 5 eval '(( $(mdo pictures) > pictures ))' || { echo "a text size step didn't draw the picture again: $pictures, then $(mdo pictures)"; return 1; }
  mdo zoom ',"zoom":0' >/dev/null; sleep 0.3
  pictures="$(mdo pictures)"
  mdo scroll ',"overview_click":3' >/dev/null
  check wait_for 5 eval 'mdattop' || { echo "a click at the top of the thumbnail left the scroll at $(mdo scroll)"; return 1; }
  mdo scroll ',"overview_marks_click":690' >/dev/null
  check wait_for 5 eval '(( $(mdo scroll | cut -d. -f1) > 2000 ))' || { echo "a click at the foot of the marks left the scroll at $(mdo scroll)"; return 1; }
  mdo scroll ',"overview_drag":[10,-400]' >/dev/null
  check wait_for 5 eval 'mdattop' || { echo "dragging the box up left the scroll at $(mdo scroll)"; return 1; }
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
  mdwin 1500
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == True ]]' || return 1
  local page_width page_lines; page_width="$(mdo layout_width)"; page_lines="$(mdo lines ',"lines":true')"
  kanna markup longdoc >/dev/null || return 1
  check wait_for 5 eval '[[ "$(mdo markup)" == True && "$(mdo overview.shown)" == True && "$(mdo threads_shown)" == True ]]' || { echo "at 1500 pt with comments: overview $(mdo overview.shown), list $(mdo threads_shown)"; return 1; }
  check test "$(mdo layout_width)" == "$page_width" || { echo "the comments list moved the page: width $page_width, then $(mdo layout_width)"; return 1; }
  check test "$(mdo lines ',"lines":true')" == "$page_lines" || { echo "the comments list changed the line breaks"; return 1; }
  check wait_for 5 eval '[[ "$(mdo overview.marks_by_kind.comment)" == 1 ]]' || { echo "comment marks: $(mdo overview.marks_by_kind)"; return 1; }
  mdo overview.shown ',"overview_hover":true' >/dev/null
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null; sleep 0.5
  mdshot markdown_overview-threads-light
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null; sleep 0.5
  mdshot markdown_overview-threads-dark
  mdo overview.shown ',"overview_hover":false' >/dev/null
  mdwin 1000
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == True && "$(mdo threads_shown)" == True ]]' || { echo "at 1000 pt both stay: overview $(mdo overview.shown), list $(mdo threads_shown)"; return 1; }
  mdshot markdown_overview-narrow
  mdwin 500
  check wait_for 5 eval '[[ "$(mdo overview.shown)" == False && "$(mdo threads_shown)" == False ]]' || { echo "at 500 pt: overview $(mdo overview.shown), list $(mdo threads_shown)"; return 1; }
}

scenario markdown_zoom_anchor M4
markdown_zoom_anchor() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR/mdza" start width
  mkdir -p "$dir"
  {
    printf '# Anchors\n\n'
    for i in $(seq 60); do
      printf '## Section %s\n\n' "$i"
      for j in $(seq 6); do printf 'Sentence %s.%s says enough about the session, the token and the redirect to wrap across the line. ' "$i" "$j"; done
      printf '\n\n'
      (( i % 5 == 0 )) && printf '```mermaid\nflowchart TD\n    A%s[app] --> P%s[provider] --> R%s[redirect] --> S%s[session]\n```\n\n' "$i" "$i" "$i" "$i"
    done
  } >"$dir/anchors.md"
  kanna open --window new --name zaleft --cmd 'exec sleep 600' >/dev/null || return 1
  kanna open --window win-1 md --name zadoc --split right "$dir/anchors.md" >/dev/null || return 1
  check wait_for 5 eval 'doc_text zadoc "Section 60"' || return 1
  check wait_for 10 eval '[[ "$(raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"zadoc\"}}" | field diagrams | grep -c flowchart)" -ge 1 ]]' || return 1
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[1400,800]}}' >/dev/null
  mdz() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"zadoc\"${2:-}}}" | field "$1"; }
  place() { echo "$(mdz scroll) | $(mdz top.text) | $(mdz top.fraction)"; }
  sleep 0.5
  mdz scroll ',"scroll":7000' >/dev/null; sleep 0.3
  start="$(place)"
  width="$(mdz layout_width)"
  check test "$(mdz top.text)" != "" || { echo "no text at the top: $start"; return 1; }
  for _ in $(seq 20); do
    kanna zoom zadoc >/dev/null || return 1
    check wait_for 5 eval '[[ "$(mdz layout_width)" != "$width" ]]' || { echo "the pane zoom didn't widen the doc"; return 1; }
    kanna zoom zadoc >/dev/null || return 1
    check wait_for 5 eval '[[ "$(mdz layout_width)" == "$width" ]]' || { echo "the pane unzoom didn't narrow the doc"; return 1; }
  done
  check test "$(place)" == "$start" || { echo "20 zooms moved the view from $start to $(place)"; return 1; }
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"zadoc","at":[300,300]}}' >/dev/null; sleep 0.3
  mdz scroll ',"scroll":7000' >/dev/null; sleep 0.3
  start="$(place)"
  key cmd+shift+=
  check python3 -c 'import sys; assert float(sys.argv[1]) > 1, sys.argv[1]' "$(mdz zoom)" || return 1
  check test "$(mdz top.text)" == "$(cut -d'|' -f2 <<<"$start" | xargs)" || { echo "⌘+ moved the text at the top from $start to $(place)"; return 1; }
  key cmd+-
  check test "$(mdz zoom)" == 1 || return 1
  check test "$(place)" == "$start" || { echo "⌘+ then ⌘− moved the view from $start to $(place)"; return 1; }
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[900,800]}}' >/dev/null; sleep 0.5
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[1400,800]}}' >/dev/null; sleep 0.5
  check test "$(place)" == "$start" || { echo "a window resize moved the view from $start to $(place)"; return 1; }
  kanna markup zadoc >/dev/null || return 1
  check wait_for 5 eval '[[ "$(mdz markup)" == True ]]' || return 1
  kanna markup zadoc --off >/dev/null || return 1
  check wait_for 5 eval '[[ "$(mdz markup)" == False ]]' || return 1
  check test "$(place)" == "$start" || { echo "turning comments on and off moved the view from $start to $(place)"; return 1; }
}

scenario markdown_zoom M4
markdown_zoom() {
  start_daemon && start_app || return 1
  local dir="$KANNA_STATE_DIR/mdzoom" before pinched after stepped
  mkdir -p "$dir"
  {
    printf '# Zoom\n\n'
    for i in $(seq 60); do printf '## Section %s\n\nParagraph %s about the session, the token and how the redirect is checked against the allow list before the app trusts it and opens the session.\n\n' "$i" "$i"; done
  } >"$dir/zoom.md"
  kanna open --window new md "$dir/zoom.md" --name zdoc >/dev/null || return 1
  mdz() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"zdoc\",\"lines\":true${1:-}}}"; }
  check wait_for 5 eval 'mdz | field text | grep -q "Section 60"' || return 1
  mdz ',"scroll":1500' >/dev/null; sleep 0.5
  before="$(mdz)"
  pinched="$(raw '{"id":1,"cmd":"debug.pinch","args":{"pane":"zdoc"}}')"
  sleep 0.5
  after="$(mdz)"
  check python3 -c 'import json, sys
before, pinch, after = (json.loads(a) for a in sys.argv[1:])
z = pinch["zooms"]
assert z and z[-1] > 1.2, ("the pinch did not zoom", z)
assert abs(after["zoom"] - z[-1]) < 0.001, ("the zoom snapped on release", z[-1], after["zoom"])
assert abs(after["page_zoom"] - z[-1]) < 0.001 and after["text_size"] == 1, ("a pinch is page zoom", after["text_size"], after["page_zoom"])
assert after["top_line"] == pinch["top_lines"][-1], ("the top line moved on release", pinch["top_lines"][-1], after["top_line"])
assert set(pinch["layout_widths"]) == {before["layout_width"]} and after["layout_width"] == before["layout_width"], ("a pinch laid the page out again", before["layout_width"], pinch["layout_widths"], after["layout_width"])
assert after["lines"] == before["lines"], "a pinch changed the line breaks"' "$before" "$pinched" "$after" || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"zdoc","at":[40,200]}}' >/dev/null; sleep 0.3
  key cmd+shift+=
  stepped="$(mdz)"
  check python3 -c 'import json, sys
after, stepped = (json.loads(a) for a in sys.argv[1:])
assert stepped["text_size"] == 1.1, ("⌘+ steps the text size", stepped["text_size"])
assert abs(stepped["page_zoom"] - after["page_zoom"]) < 0.001, ("⌘+ kept the page zoom", after["page_zoom"], stepped["page_zoom"])
assert stepped["layout_width"] < after["layout_width"] - 10, ("⌘+ re-wraps to the pane", after["layout_width"], stepped["layout_width"])
assert stepped["lines"] != after["lines"], "⌘+ kept the line breaks"
reading = after["top_line"]
holding = max(l for l in stepped["lines"] if l <= reading)
assert stepped["top_line"] == holding, ("⌘+ lost the reading line", reading, holding, stepped["top_line"])' "$after" "$stepped" || return 1
  sleep 1
  stop_app
  start_app || return 1
  check wait_for 10 eval 'mdz | field text | grep -q "Section 60"' || return 1
  check python3 -c 'import json, sys
stepped, back = (json.loads(a) for a in sys.argv[1:])
assert back["text_size"] == stepped["text_size"], ("the text size came back", stepped["text_size"], back["text_size"])
assert abs(back["page_zoom"] - stepped["page_zoom"]) < 0.001, ("the page zoom came back", stepped["page_zoom"], back["page_zoom"])' "$stepped" "$(mdz)" || return 1
  raw '{"id":1,"cmd":"debug.click","args":{"pane":"zdoc","at":[40,200]}}' >/dev/null; sleep 0.3
  key cmd+0
  check python3 -c 'import json, sys
back = json.loads(sys.argv[1])
assert back["text_size"] == 1 and back["page_zoom"] > 1.2, ("⌘0 resets the text size only", back["text_size"], back["page_zoom"])' "$(mdz)" || return 1
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

scenario markdown_overview_height M4
markdown_overview_height() {
  start_daemon && start_app || return 1
  local repo="$KANNA_STATE_DIR/mdovh" git=(git -C "$KANNA_STATE_DIR/mdovh" -c user.name=e2e -c user.email=e2e@example.invalid) name
  mkdir -p "$repo" && "${git[@]}" init -q -b main || return 1
  {
    printf '# Factory\n\nHow the factory runs.\n\n'
    for i in $(seq 12); do
      printf '## Part %s\n\nPart %s explains a stage of the pipeline and what it hands to the next one.\n\n' "$i" "$i"
      printf '```mermaid\nflowchart TD\n    A%s[plan] --> B%s[build] --> C%s[check]\n    C%s --> D%s[review] --> E%s[merge]\n    C%s --> B%s\n```\n\n' "$i" "$i" "$i" "$i" "$i" "$i" "$i" "$i"
      for j in $(seq 4); do printf 'Paragraph %s.%s says what the stage reads, writes and checks before it passes the work on.\n\n' "$i" "$j"; done
    done
  } >"$repo/factory.md"
  "${git[@]}" add factory.md && "${git[@]}" commit -qm base || return 1
  perl -0pi -e 's/(Paragraph 2\.2 [^\n]*\n\n)/$1## Added\n\nA new part.\n\n```mermaid\nflowchart LR\n    X[new] --> Y[stage] --> Z[done]\n```\n\n/; s/Paragraph 9\.3 says/Paragraph 9.3 now says/' "$repo/factory.md"
  mdo() { raw "{\"id\":1,\"cmd\":\"debug.md\",\"args\":{\"pane\":\"$1\"${3:-}}}" | field "$2"; }
  covers() {
    python3 -c 'import sys; p, c, s, d = map(float, sys.argv[1:]); sys.exit(0 if c > 0 and abs(p - c) * s <= 1 and d >= c - 1 else 1)' "$(mdo "$1" overview.picture_height)" "$(mdo "$1" overview.content_height)" "$(mdo "$1" overview.scale)" "$(mdo "$1" overview.document_height)"
  }
  kanna open --window new md "$repo/factory.md" --name plain >/dev/null || return 1
  raw '{"id":1,"cmd":"debug.desk","args":{"window":"win-1","size":[2200,800]}}' >/dev/null
  kanna open --window win-1 md "$repo/factory.md" --name diffed --diff main --split right >/dev/null || return 1
  for name in plain diffed; do
    check wait_for 5 eval "[[ \"\$(mdo $name overview.shown)\" == True ]]" || { echo "$name: no overview: wanted $(mdo $name overview.wanted), width $(mdo $name width)"; return 1; }
    check wait_for 5 covers $name || { echo "$name: picture $(mdo $name overview.picture_height) for content $(mdo $name overview.content_height), document $(mdo $name overview.document_height)"; return 1; }
  done
  kanna zoom diffed >/dev/null || return 1
  check wait_for 5 covers diffed || { echo "pane zoomed with the diff: picture $(mdo diffed overview.picture_height) for content $(mdo diffed overview.content_height)"; return 1; }
  mdo diffed zoom ',"zoom":1' >/dev/null
  mdo diffed zoom ',"zoom":1' >/dev/null
  check wait_for 5 covers diffed || { echo "text zoomed with the diff: picture $(mdo diffed overview.picture_height) for content $(mdo diffed overview.content_height)"; return 1; }
  mdo diffed scroll ',"scroll":0' >/dev/null; sleep 0.3
  check python3 -c 'import sys; y = float(sys.argv[1]); assert y < 20, y' "$(mdo diffed overview.box_y)" || { echo "at the top the box is at $(mdo diffed overview.box_y)"; return 1; }
  kanna zoom diffed >/dev/null || return 1
  check wait_for 5 covers diffed || { echo "unzoomed with the diff: picture $(mdo diffed overview.picture_height) for content $(mdo diffed overview.content_height)"; return 1; }
  kanna zoom plain >/dev/null || return 1
  mdo plain zoom ',"zoom":-1' >/dev/null
  check wait_for 5 covers plain || { echo "zoomed without the diff: picture $(mdo plain overview.picture_height) for content $(mdo plain overview.content_height)"; return 1; }
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
