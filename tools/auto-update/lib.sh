repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
. "$repo_root/scripts/env.sh"
port=38471
bundle="dev.kanna.app.dev.$(basename "$repo_root")"
state="$HOME/Library/Application Support/kanna-dev-$(basename "$repo_root")"
sparkle_bin="$repo_root/target/kanna-build/artifacts/sparkle/Sparkle/bin"
install="$T/install/Kanna.app"
served="$T/served"
export KANNA_SOCKET="$state/kanna.sock"
fail() { echo "FAIL: $*" >&2; exit 1; }
note() { echo "==> $*"; }
raw() { python3 -I -c 'import socket, sys
s = socket.socket(socket.AF_UNIX); s.settimeout(30); s.connect(sys.argv[1])
s.sendall(sys.argv[2].encode() + b"\n"); print(s.makefile().readline().strip())' "$KANNA_SOCKET" "$1"; }
installed_build() { /usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$install/Contents/Info.plist"; }
installer_running() { pgrep -fl 'Autoupdate|Updater' | grep -q "$bundle"; }
toast_field() { raw '{"id":1,"cmd":"debug.toasts"}' | python3 -I -c 'import json, sys
for t in json.load(sys.stdin).get("toasts", []):
    if t.get("id") == "update": print(t.get(sys.argv[1]))' "$1"; }
press() { raw "{\"id\":2,\"cmd\":\"debug.toasts\",\"args\":{\"click\":\"update\",\"action\":\"$1\"}}" >/dev/null; }
wait_for() { local tries="$1"; shift; local i; for ((i = 0; i < tries * 4; i++)); do "$@" && return 0; sleep 0.25; done; return 1; }
titled() { [[ "$(toast_field title 2>/dev/null)" == *"$1"* ]]; }
launch() {
  defaults delete "$bundle" >/dev/null 2>&1 || true
  env -u KANNA_SOCKET -u KANNA_TERMINAL -u KANNA_PANE -u KANNA_STATE_DIR KANNA_DEBUG=1 KANNA_IGNORE_OCCLUSION=1 "$install/Contents/MacOS/Kanna" --bg >>"$T/app.log" 2>&1 &
  app_pid=$!
  wait_for 30 app_attached || fail "the app didn't attach: $(tail -5 "$T/app.log")"
}
stop_app() { kill "${1:--TERM}" "$app_pid" 2>/dev/null || true; wait "$app_pid" 2>/dev/null || true; }
app_attached() { [[ "$(raw '{"id":1,"cmd":"debug.app"}' 2>/dev/null)" == *'"ok":true'* || "$(raw '{"id":1,"cmd":"debug.app"}' 2>/dev/null)" == *'"ok": true'* ]]; }
reinstall() { rm -rf "$install"; mkdir -p "$(dirname "$install")"; ditto "$T/A/Kanna.app" "$install"; }
feed_for() {
  local dmg="$1" key="$2" name="$3"
  local signature; signature="$("$sparkle_bin/sign_update" --ed-key-file "$key" -p "$dmg")"
  "$repo_root/scripts/write-appcast.sh" --title Kanna --version 4.91.0 --build "$build_b" --minimum 15.0.0 --protocol "${speaks:-$(sed -n 's/^pub const PROTOCOL: u32 = \([0-9]*\);/\1/p' "$repo_root/crates/kanna-build/src/lib.rs")}" \
    --url "http://127.0.0.1:$port/$name" --length "$(stat -f %z "$dmg")" --signature "$signature"
}
stop_instance() {
  "$T/A/Kanna.app/Contents/Helpers/kanna" instances stop >/dev/null 2>&1 || true
  wait_for 30 eval '! pgrep -f "^$install/" >/dev/null' || fail "the last instance didn't stop"
  rm -rf "$state"
}
