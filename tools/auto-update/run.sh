#!/usr/bin/env bash
set -euo pipefail
T="$1"; shift
. "$(dirname "$0")/lib.sh"
build_b="$(cat "$T/build-b")"
build_a="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$T/A/Kanna.app/Contents/Info.plist")"
python3 -I "$repo_root/tools/auto-update/serve.py" "$served" "$port" 2>>"$T/serve.log" &
server=$!
trap 'kill $server 2>/dev/null || true' EXIT
wait_for 5 curl -fs -o /dev/null "http://127.0.0.1:$port/Kanna-B.dmg" -r 0-1 || fail "the feed server didn't start"

good_feed() { rm -f "$served/rate"; feed_for "$served/Kanna-B.dmg" "$T/test.key" Kanna-B.dmg >"$served/feed.xml"; "$sparkle_bin/sign_update" --ed-key-file "$T/test.key" "$served/feed.xml"; }
untouched() {
  [[ "$(installed_build)" == "$build_a" ]] || fail "$1: the update installed"
  sleep 3
  [[ "$(installed_build)" == "$build_a" ]] || fail "$1: the update installed after the app ended"
  ! installer_running || fail "$1: an installer is running"
  note "ok: $1 left A in place, no installer"
}
quit_daemon() { :; }

ask_first() {
  good_feed
  for how in -KILL -TERM; do
    reinstall; launch
    wait_for 40 titled "is available" || fail "no toast: $(raw '{"id":1,"cmd":"debug.toasts"}')"
    stop_app "$how"; untouched "quit ($how) at Found"
    reinstall; launch
    wait_for 40 titled "is available" || fail "no toast"
    press Later
    wait_for 5 eval '! titled "is available"' || fail "Later left the toast"
    stop_app "$how"; untouched "quit ($how) after Later"
    reinstall; launch
    wait_for 40 titled "is available" || fail "no toast"
    echo 500000 >"$served/rate"
    press "Install and Relaunch"
    wait_for 10 titled "Downloading" || fail "no download toast"
    sleep 2
    stop_app "$how"; untouched "quit ($how) while downloading"
    rm -f "$served/rate"
  done
}

bad_updates() {
  reinstall
  head -c 1000000 "$served/Kanna-B.dmg" >"$served/short.dmg"
  feed_for "$served/Kanna-B.dmg" "$T/test.key" short.dmg >"$served/feed.xml"
  "$sparkle_bin/sign_update" --ed-key-file "$T/test.key" "$served/feed.xml"
  launch; wait_for 40 titled "is available" || fail "no toast"
  press "Install and Relaunch"
  wait_for 60 titled "didn't install" || fail "truncated DMG: no failure toast: $(raw '{"id":1,"cmd":"debug.toasts"}')"
  note "truncated: $(toast_field detail)"
  stop_app -TERM; untouched "a truncated DMG"

  feed_for "$served/Kanna-B.dmg" "$T/other.key" Kanna-B.dmg >"$served/feed.xml"
  "$sparkle_bin/sign_update" --ed-key-file "$T/test.key" "$served/feed.xml"
  reinstall; launch; wait_for 40 titled "is available" || fail "no toast"
  press "Install and Relaunch"
  wait_for 60 titled "didn't install" || fail "wrong archive key: no failure toast: $(raw '{"id":1,"cmd":"debug.toasts"}')"
  note "wrong archive key: $(toast_field detail)"
  stop_app -TERM; untouched "a DMG signed by another key"

  feed_for "$served/Kanna-B.dmg" "$T/test.key" Kanna-B.dmg >"$served/feed.xml"
  "$sparkle_bin/sign_update" --ed-key-file "$T/other.key" "$served/feed.xml"
  reinstall; launch
  ! wait_for 15 titled "is available" || fail "a feed signed by another key was believed"
  stop_app -TERM; untouched "a feed signed by another key"
}

real_update() {
  . "$(dirname "$0")/real.sh"
}

machine_left_behind() {
  . "$(dirname "$0")/machine.sh"
}

for phase in "$@"; do
  note "phase $phase"
  "$phase"
done
note "passed: $*"
