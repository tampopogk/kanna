#!/usr/bin/env bash
set -uo pipefail

usage() {
  cat <<'USAGE'
usage: scripts/test-volume-drop.sh

A task's private Kanna keeps running when its build volume drops out
(docs/specs/build-store.md#6-running-off-the-volume). A disk image stands in for
the volume: a fake repo's worktree gets its target/ from kanna_targets with the
image as targets, this checkout's target/Kanna.app is put where
build-kanna-app.sh puts it, and an instance (daemon, keeper, app --bg) runs from
there on a private socket. The image is force-detached and attached again, and
the instance must open and show a terminal each time. The same app copied onto
the image (the old layout) runs beside it as a control, which must break:
otherwise the drop wasn't one the test could see.
Run it unsandboxed after scripts/build.sh: it needs hdiutil, ps and signals.
USAGE
}
case "${1:-}" in -h|--help) usage; exit 0 ;; esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scripts="$repo_root/scripts"
built="$repo_root/target/Kanna.app"
[[ -x "$built/Contents/MacOS/Kanna" ]] || { echo "test-volume-drop: build first: scripts/build.sh" >&2; exit 2; }
unset KANNA_PANE KANNA_TERMINAL KANNA_INSTANCE KANNA_SOCKET KANNA_STATE_DIR KANNA_MUX

dir="$(mktemp -d /tmp/kanna-drop.XXXXXX)" && dir="$(cd "$dir" && pwd -P)" || exit 1
vhs="$dir/vhs"
pids=()
cleanup() {
  local p
  for p in ${pids[@]+"${pids[@]}"}; do kill "$p" 2>/dev/null; done
  sleep 1
  for p in ${pids[@]+"${pids[@]}"}; do kill -9 "$p" 2>/dev/null; done
  hdiutil detach -force -quiet "$vhs" 2>/dev/null
  chmod -R u+w "$dir" 2>/dev/null
  rm -rf "$dir"
}
trap cleanup EXIT
failed=0
fail() { echo "FAIL: $*"; failed=1; }

attach() { hdiutil attach -nobrowse -noverify -mountpoint "$vhs" -quiet "$dir/vhs.dmg"; }
hdiutil create -size 600m -fs APFS -volname kanna-drop-test -quiet "$dir/vhs.dmg" && attach || { echo "test-volume-drop: hdiutil can't make or attach a disk image here" >&2; exit 2; }

main="$dir/repo"
git init -q -b main "$main" && git -C "$main" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || exit 1
printf '{"targets": "%s"}\n' "$vhs/kanna-targets" >"$main/build.local.json"
git -C "$main" worktree add -q "$main/.kanna-worktrees/task-drop" -b task-drop >/dev/null 2>&1 || exit 1
wt="$main/.kanna-worktrees/task-drop"
(. "$scripts/env.sh" && kanna_targets "$wt") 2>/dev/null || fail "kanna_targets: exit $?"
app_dir="$(. "$scripts/env.sh" && kanna_arch_dir "$wt" "$(kanna_host_triple)")"
[[ "$(readlink "$wt/target/debug")" == "$vhs/kanna-targets/repo/task-drop/debug" ]] || fail "layout: debug/ isn't on the image: $(ls -l "$wt/target")"
cp -R "$built" "$app_dir/Kanna.app" && mkdir -p "$app_dir/e2e-kanna" || exit 1
[[ "$(stat -f %d "$app_dir/Kanna.app")" != "$(stat -f %d "$vhs")" ]] || fail "layout: target/Kanna.app is on the image"
[[ "$(stat -f %d "$app_dir/e2e-kanna")" != "$(stat -f %d "$vhs")" ]] || fail "layout: target/e2e-kanna is on the image"
cp -R "$built" "$vhs/kanna-targets/repo/task-drop/Kanna.app" || exit 1

start() {
  local name="$1" app="$2" state="$dir/$1" logs="$3"
  mkdir -p "$state/home/.kanna-v4" "$logs"
  echo '{"version": 2, "setup": false}' >"$state/home/.kanna-v4/agents.json"
  echo '{"version": 1, "shelf": {"runners": 0}, "agents": {"prompts": "skip"}}' >"$state/home/.kanna-v4/config.json"
  (
    cd "$wt" || exit 1
    export KANNA_STATE_DIR="$state" KANNA_SOCKET="$state/kanna.sock" KANNA_AGENTS_HOME="$state/home"
    export KANNA_IGNORE_OCCLUSION=1 KANNA_DEBUG=1 KANNA_NO_INITIAL_WINDOW=1 KANNA_AGENT_RULES_URL=none KANNA_PLANNING_AGENTS=none
    KANNA_KEEPER="$app/Contents/Helpers/kanna-keeper" "$app/Contents/Helpers/kanna-daemon" >>"$logs/$name-daemon.log" 2>&1 &
    echo $! >"$state/started.daemon"
    for _ in $(seq 50); do [[ -S "$KANNA_SOCKET" ]] && break; sleep 0.1; done
    "$app/Contents/MacOS/Kanna" --bg >>"$logs/$name-app.log" 2>&1 &
    echo $! >"$state/started.app"
  )
  pids+=("$(cat "$state/started.daemon")" "$(cat "$state/started.app")")
  for _ in $(seq 100); do [[ -f "$state/keeper.pid" ]] && break; sleep 0.1; done
  [[ -f "$state/keeper.pid" ]] && pids+=("$(cat "$state/keeper.pid")")
}
cli() { local name="$1"; shift; KANNA_SOCKET="$dir/$name/kanna.sock" KANNA_STATE_DIR="$dir/$name" perl -e 'alarm shift; exec @ARGV' 10 "$app_dir/Kanna.app/Contents/Helpers/kanna" "$@"; }
alive() {
  local name="$1" p
  for p in "$(cat "$dir/$name/started.daemon")" "$(cat "$dir/$name/started.app")" "$(cat "$dir/$name/keeper.pid" 2>/dev/null)"; do
    [[ -n "$p" ]] && kill -0 "$p" 2>/dev/null || return 1
  done
}
works() {
  local name="$1" mark="$1-$2" i
  alive "$name" || { echo "a process is gone"; return 1; }
  cli "$name" open --window new --name "$mark" --cmd "echo $mark-\$((40+2)); exec sleep 600" >/dev/null 2>&1 || { echo "kanna open failed"; return 1; }
  for i in $(seq 100); do
    cli "$name" read "$mark" 2>/dev/null | grep -q "$mark-42" && break
    sleep 0.1
  done
  cli "$name" read "$mark" 2>/dev/null | grep -q "$mark-42" || { echo "the terminal didn't show its output"; return 1; }
  [[ "$(cli "$name" capabilities 2>/dev/null | tail -1)" == "Kanna.app: attached" ]] || { echo "the app isn't attached"; return 1; }
  alive "$name" || { echo "a process is gone"; return 1; }
}

start internal "$app_dir/Kanna.app" "$app_dir/e2e-kanna"
start volume "$vhs/kanna-targets/repo/task-drop/Kanna.app" "$app_dir/e2e-kanna"
said="$(works internal before)" || fail "before the drop: the instance from target/Kanna.app: $said"
said="$(works volume before)" || fail "before the drop: the control on the image: $said"

hdiutil detach -force -quiet "$vhs" || { fail "couldn't force-detach the image"; exit 1; }
sleep 3
said="$(works internal unplugged)" || fail "volume gone: the instance from target/Kanna.app: $said"
works volume unplugged >/dev/null && fail "volume gone: the control on the image kept working, so this drop shows nothing"

attach || { fail "couldn't attach the image again"; exit 1; }
sleep 2
said="$(works internal replugged)" || fail "volume back: the instance from target/Kanna.app: $said"
(. "$scripts/env.sh" && kanna_targets "$wt") 2>/dev/null || fail "kanna_targets after the drop: exit $?"
[[ -d "$wt/target/debug/" ]] || fail "after the drop: target/debug doesn't reach the image again"

[[ $failed == 0 ]] && echo "volume drop: ok"
exit "$failed"
