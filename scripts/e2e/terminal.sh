ghostty_config() { printf '%s\n' "$@" >"$KANNA_GHOSTTY_CONFIG"; }

scenario terminal_colours M4
terminal_colours() {
  ghostty_config 'palette = 1=#0102fe' 'cursor-style = bar' \
    'theme = light:'"$KANNA_STATE_DIR"'/light,dark:'"$KANNA_STATE_DIR"'/dark'
  printf 'background = #f0f0e0\nforeground = #202020\n' >"$KANNA_STATE_DIR/light"
  printf 'background = #123456\nforeground = #e5d4b0\n' >"$KANNA_STATE_DIR/dark"
  start_daemon && start_app || return 1
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
  check near "$drawn" '#f0f0e0' || return 1
  colours_theme 'light:GitHub Light Default,dark:GitHub Dark Default' '#ffffff' '#0d1117' none || return 1
  local ghostty="${KANNA_E2E_GHOSTTY:-/Applications/Ghostty.app}"
  if [[ -f "$ghostty/Contents/Resources/ghostty/themes/Dracula" ]]; then
    colours_theme 'light:iTerm2 Solarized Light,dark:Dracula' '#fdf6e3' '#282a36' "$ghostty" || return 1
  else
    echo "terminal_colours: no Ghostty.app here; not checking themes from it"
  fi
  colours_theme 'Dracula' '' '' none
}
colours_theme() {
  local from light="$2" dark="$3"
  stop_app
  ghostty_config "theme = $1"
  mkdir -p "$KANNA_STATE_DIR/xdg" && check test ! -e "$KANNA_STATE_DIR/xdg/ghostty/themes" || return 1
  from=$(($(wc -l <"$out/app.log" 2>/dev/null || echo 0) + 1))
  XDG_CONFIG_HOME="$KANNA_STATE_DIR/xdg" KANNA_GHOSTTY_APP="$4" start_app || return 1
  if [[ -z "$2" ]]; then
    check wait_for 5 eval 'tail -n +$from "$out/app.log" | grep -q "Ghostty config: theme \"Dracula\" not found"' || return 1
    check eval 'tail -n +$from "$out/app.log" | grep -q "Kanna ships only these themes: .*GitHub Dark Default"'
    return
  fi
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"dark"}}' >/dev/null || return 1
  check wait_for 5 eval '[[ "$(terminal settings.background "{\"pane\":\"terminal_colours\"}")" == "$dark" ]]' || { terminal settings.scheme '{"pane":"terminal_colours"}'; terminal settings.background '{"pane":"terminal_colours"}'; return 1; }
  raw '{"id":1,"cmd":"debug.app","args":{"appearance":"light"}}' >/dev/null || return 1
  check wait_for 5 eval '[[ "$(terminal settings.background "{\"pane\":\"terminal_colours\"}")" == "$light" ]]' || { terminal settings.scheme '{"pane":"terminal_colours"}'; terminal settings.background '{"pane":"terminal_colours"}'; return 1; }
  check eval '! tail -n +$from "$out/app.log" | grep -q "Ghostty config:"'
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
  ghostty_config 'clipboard-read = allow'
  start_daemon && start_app || return 1
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
  stop_app; start_app || return 1
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

scenario agent_focus M4
agent_focus() {
  local agent="$repo_root/tools/mock-agent/mock-agent" repo="$KANNA_AGENTS_HOME/.kanna-v4/repos/local/repo" log="$KANNA_STATE_DIR/focus.log" task other
  mkdir -p "$repo" && git -C "$repo" init -q -b main && git -C "$repo" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || { echo "agent_focus line $LINENO"; return 1; }
  repo="$(cd "$repo" && pwd -P)"
  start_daemon || { echo "agent_focus line $LINENO"; return 1; }
  env -u KANNA_NO_INITIAL_WINDOW KANNA_START_DIR="$repo" "$app" --bg >>"$out/app.log" 2>&1 &
  app_pid=$!
  wait_for 10 app_attached || { echo "the app did not attach"; return 1; }
  check wait_for 10 eval '[[ "$(dwin d1 lead.pane)" == term-* ]]' || { echo "agent_focus: no Desk"; return 1; }
  focus_now() { [[ "$(tail -1 "$log" 2>/dev/null)" == "$1" ]]; }
  focus_was() { check wait_for 5 focus_now "$1" || { echo "agent_focus: $2: the agent was last told focus $(tail -1 "$log" 2>/dev/null || echo nothing), not $1 ($(tr '\n' ' ' <"$log" 2>/dev/null))"; kanna workspace list --all; return 1; }; }
  (cd "$repo" && kanna task new "Focus task" "focus $log" --agent "$agent") >/dev/null && kanna task pick $(tid 1) >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  kanna task activate "Focus task" >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  task="$(dwin d1 dock.pane)"
  check wait_for 10 test -f "$log" || { echo "agent_focus: the agent didn't turn on focus reports"; kanna read "$task"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 key):$(dwin d1 focused)" == "True:$task" ]]' || { echo "agent_focus: the task's agent isn't focused in the key window"; kanna workspace list --all; return 1; }
  focus_was in "made active" || return 1
  KANNA_TERMINAL="$task" kanna open --name focus_tab --cmd "exec sleep 600" >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  focus_was out "a tab of its task focused" || return 1
  kanna zoom "$task" >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 zoomed)" == "$task" ]]' || { echo "agent_focus: kanna zoom didn't zoom the agent"; kanna workspace list --all; return 1; }
  focus_was in "zoomed on its agent" || return 1
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock)" == None ]]' || { echo "agent_focus line $LINENO"; return 1; }
  focus_was out "parked" || return 1
  key "cmd+."
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane):$(dwin d1 zoomed)" == "$task:$task" ]]' || { echo "agent_focus line $LINENO"; return 1; }
  focus_was in "shown again" || return 1
  key cmd+shift+return
  check wait_for 5 eval '[[ "$(dwin d1 zoomed)" == None ]]' || { echo "agent_focus line $LINENO"; return 1; }
  focus_was in "unzoomed" || return 1
  kanna focus focus_tab >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  focus_was out "its tab focused" || return 1
  kanna focus "$task" >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  focus_was in "focused again" || return 1
  key cmd+shift+return
  check wait_for 5 eval '[[ "$(dwin d1 zoomed)" == "$task" ]]' || { echo "agent_focus line $LINENO"; return 1; }
  focus_was in "zoomed again" || return 1
  (cd "$repo" && kanna task new "Other task" "hello" --agent "$agent") >/dev/null && kanna task pick $(tid 2) >/dev/null && kanna task activate "Other task" >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  other="$(dwin d1 dock.pane)"
  check test "$other" != "$task" || { echo "agent_focus line $LINENO"; return 1; }
  focus_was out "another task made active" || return 1
  kanna task activate "Focus task" >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 dock.pane)" == "$task" ]]' || { echo "agent_focus line $LINENO"; return 1; }
  focus_was in "made active again" || return 1
  kanna open --window new --name focus_window >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 key)" == False ]]' || { echo "agent_focus line $LINENO"; return 1; }
  focus_was out "another window key" || return 1
  kanna close focus_window >/dev/null || { echo "agent_focus line $LINENO"; return 1; }
  check wait_for 5 eval '[[ "$(dwin d1 key)" == True ]]' || { echo "agent_focus: the Desk didn't become key again"; return 1; }
  focus_was in "its window key again" || return 1
  check test "$(dwin d1 zoomed)" == "$task" || { echo "agent_focus line $LINENO"; return 1; }
  stop_app
  printf 'restart\n' >>"$log"
  start_app || { echo "agent_focus line $LINENO"; return 1; }
  check wait_for 10 eval '[[ "$(dwin d1 zoomed)" == "$task" ]]' || { echo "agent_focus: the task isn't zoomed after a relaunch"; kanna workspace list --all; return 1; }
  focus_was in "after a relaunch with the task zoomed" || return 1
  key "cmd+."
  focus_was out "parked after a relaunch" || return 1
  key "cmd+."
  focus_was in "shown again after a relaunch" || return 1
  stop_agents
}
