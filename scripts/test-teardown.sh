#!/usr/bin/env bash
set -uo pipefail

scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
dir="$(mktemp -d "${TMPDIR:-/tmp}/kanna-teardown.XXXXXX")" && dir="$(cd "$dir" && pwd -P)" || exit 1
trap 'rm -rf "$dir"' EXIT
failed=0
fail() { echo "FAIL: $*"; failed=1; }

export KANNA_AGENTS_HOME="$dir/home"
mkdir -p "$KANNA_AGENTS_HOME/.kanna-v4"
targets() { printf '{"version": 1, "build": {"targets": "%s"}}\n' "$1" >"$KANNA_AGENTS_HOME/.kanna-v4/config.json"; }
root="$dir/volume/kanna-targets"
targets "$root"

main="$dir/repo"
git init -q -b main "$main" && git -C "$main" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || exit 1
worktree() { git -C "$main" worktree add -q "$main/.kanna-worktrees/$1" -b "$1" >/dev/null 2>&1 && echo "$main/.kanna-worktrees/$1"; }
link() { (. "$scripts/env.sh" && kanna_targets "$1") 2>/dev/null; echo built >"$1/target/out"; }
teardown() { (cd "$1" && "$scripts/teardown.sh") >"$dir/said" 2>&1; }

mkdir -p "$root/repo/_main/release" "$root/repo/.offline/release" "$root/repo/task-other"

wt="$(worktree task-a)" && link "$wt"
[[ "$(readlink "$wt/target")" == "$root/repo/task-a" ]] || fail "kanna_targets didn't link task-a into ROOT/REPO: $(readlink "$wt/target")"
teardown "$wt" || fail "linked worktree: exit $?: $(cat "$dir/said")"
[[ ! -e "$root/repo/task-a" ]] || fail "linked worktree: its build folder is left"
[[ ! -L "$wt/target" ]] || fail "linked worktree: target/ link is left"
[[ -d "$root/repo/_main/release" && -d "$root/repo/.offline/release" && -d "$root/repo/task-other" ]] || fail "linked worktree: removed another folder"

wt="$(worktree task-real)" && mkdir -p "$wt/target" && echo built >"$wt/target/out"
teardown "$wt" || fail "real target/: exit $?"
[[ -f "$wt/target/out" ]] || fail "real target/: removed it"

wt="$(worktree task-none)"
teardown "$wt" || fail "no target/: exit $?"

wt="$(worktree task-elsewhere)" && ln -s "$root/repo/task-other" "$wt/target"
teardown "$wt" || fail "link to another worktree's folder: exit $?"
[[ -d "$root/repo/task-other" && -L "$wt/target" ]] || fail "link to another worktree's folder: removed it"

wt="$(worktree _main)" && ln -s "$root/repo/_main" "$wt/target"
teardown "$wt" || fail "a worktree named _main: exit $?"
[[ -d "$root/repo/_main/release" ]] || fail "a worktree named _main: removed main's builds"

wt="$(worktree task-outside)" && mkdir -p "$dir/outside/task-outside" && ln -s "$dir/outside/task-outside" "$wt/target"
teardown "$wt" || fail "link outside ROOT: exit $?"
[[ -d "$dir/outside/task-outside" ]] || fail "link outside ROOT: removed it"

wt="$(worktree task-redirected)" && ln -s "$dir/outside" "$root/repo/task-redirected" && ln -s "$root/repo/task-redirected" "$wt/target"
teardown "$wt" || fail "ROOT/REPO/WORKTREE a link: exit $?"
[[ -d "$dir/outside/task-outside" && -L "$root/repo/task-redirected" ]] || fail "ROOT/REPO/WORKTREE a link: followed it"

wt="$(worktree task-gone)" && link "$wt" && rm -rf "$root/repo/task-gone"
teardown "$wt" || fail "folder already gone: exit $?"
[[ ! -L "$wt/target" ]] || fail "folder already gone: target/ link is left"

wt="$(worktree task-unset)" && link "$wt"
targets ""
teardown "$wt" || fail "build.targets unset: exit $?"
[[ -f "$root/repo/task-unset/out" ]] || fail "build.targets unset: removed the folder"

missing="/Volumes/kanna-teardown-missing-$$/kanna-targets"
targets "$missing"
wt="$(worktree task-unmounted)" && ln -s "$missing/repo/task-unmounted" "$wt/target"
teardown "$wt" || fail "unmounted volume: exit $?"
grep -q "isn't mounted" "$dir/said" || fail "unmounted volume: didn't say so: $(cat "$dir/said")"
[[ -L "$wt/target" ]] || fail "unmounted volume: removed the link"

targets "$root"
mkdir -p "$root/repo/repo" && ln -s "$root/repo/repo" "$main/target"
teardown "$main" || fail "main checkout: exit $?"
[[ -d "$root/repo/repo" && -L "$main/target" ]] || fail "main checkout: removed its target/"

[[ $failed == 0 ]] && echo "teardown: ok"
exit "$failed"
