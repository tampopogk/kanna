#!/usr/bin/env bash
set -uo pipefail

scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
dir="$(mktemp -d "${TMPDIR:-/tmp}/kanna-store.XXXXXX")" && dir="$(cd "$dir" && pwd -P)" || exit 1
pids=()
trap 'for p in ${pids[@]+"${pids[@]}"}; do kill "$p" 2>/dev/null; done; chmod -R u+w "$dir"; rm -rf "$dir"' EXIT
failed=0
fail() { echo "FAIL: $*"; failed=1; }

export KANNA_AGENTS_HOME="$dir/home" KANNA_LOCK_POLL=0.1
mkdir -p "$KANNA_AGENTS_HOME/.kanna-v4"
config() { printf '{"version": 1, "build": {"targets": "%s"%s}}\n' "$1" "${2:+, \"free\": $2}" >"$KANNA_AGENTS_HOME/.kanna-v4/config.json"; }
root="$dir/volume/kanna-targets"
config "$root"

main="$dir/repo"
git init -q -b main "$main" && git -C "$main" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || exit 1
worktree() { git -C "$main" worktree add -q "$main/.kanna-worktrees/$1" -b "$1" >/dev/null 2>&1 && echo "$main/.kanna-worktrees/$1"; }

. "$scripts/env.sh"
. "$scripts/store.sh"

slow() { sleep 1; echo built >>"$dir/count"; mkdir "$1" && echo "$2" >"$1/out"; }
broken() { mkdir "$1" && echo half >"$1/out"; return 1; }
store="$root/_shared"
wt="$(worktree task-a)"

entry="$(kanna_store "$wt" thing k1 slow one 2>/dev/null)" || fail "publish: exit $?"
[[ "$entry" == "$store/thing/k1" && "$(cat "$entry/out")" == one ]] || fail "publish: got $entry"
[[ ! -w "$entry" && ! -w "$entry/out" ]] || fail "publish: the entry is writable"
[[ -z "$(ls -d "$store/thing/"*.tmp-* "$store/thing/"*.lock 2>/dev/null)" ]] || fail "publish: left $(ls "$store/thing")"

rm -f "$dir/count"
(kanna_store "$wt" thing k2 slow two >"$dir/first" 2>/dev/null) & first=$!
(kanna_store "$wt" thing k2 slow two >"$dir/second" 2>/dev/null) & second=$!
wait "$first" "$second"
[[ "$(wc -l <"$dir/count" | tr -d ' ')" == 1 ]] || fail "two at once: built $(wc -l <"$dir/count") times"
[[ "$(cat "$dir/first")" == "$store/thing/k2" && "$(cat "$dir/second")" == "$store/thing/k2" ]] || fail "two at once: $(cat "$dir/first") and $(cat "$dir/second")"

kanna_store "$wt" thing k3 broken >/dev/null 2>&1 && fail "failed build: exit 0"
[[ -z "$(ls "$store/thing" | grep k3)" ]] || fail "failed build: left $(ls "$store/thing" | grep k3)"
[[ "$(kanna_store "$wt" thing k3 slow three 2>/dev/null)" == "$store/thing/k3" ]] || fail "after a failed build: didn't build"

sh -c 'exit 0' & dead=$!; wait "$dead"
mkdir "$store/thing/k4.lock" && echo "$dead" >"$store/thing/k4.lock/pid"
[[ "$(kanna_store "$wt" thing k4 slow four 2>/dev/null)" == "$store/thing/k4" ]] || fail "stale lock: didn't take it over"

sleep 30 & live=$!; disown "$live"; pids+=("$live")
mkdir "$store/thing/k5.lock" && echo "$live" >"$store/thing/k5.lock/pid"
(kanna_store "$wt" thing k5 slow five >"$dir/waited" 2>"$dir/said") & waiter=$!
sleep 1
[[ ! -d "$store/thing/k5" ]] || fail "live lock: built while it was held"
grep -q "Waiting for pid $live" "$dir/said" || fail "live lock: didn't say who it waits for: $(cat "$dir/said")"
kill "$live"
wait "$waiter"
[[ "$(cat "$dir/waited")" == "$store/thing/k5" ]] || fail "live lock: didn't build once it was free"

link="$(cd "$wt" && kanna_targets "$wt" 2>/dev/null; echo "$wt/target")"
kanna_store_use "$store/thing/k1" "$wt/target/thing" "$wt"
[[ "$(readlink "$wt/target/thing")" == "$store/thing/k1" ]] || fail "worktree: didn't link the entry: $(ls -l "$wt/target/thing")"
kanna_store_use "$store/thing/k1" "$main/target/thing" "$main"
[[ ! -L "$main/target/thing" && "$(cat "$main/target/thing/out")" == one && -w "$main/target/thing/out" ]] || fail "main checkout: didn't copy the entry"
[[ "$(cat "$main/target/thing/.store-key")" == "$store/thing/k1" ]] || fail "main checkout: didn't mark the copy"
kanna_store_use "$store/thing/k2" "$main/target/thing" "$main"
[[ "$(cat "$main/target/thing/out")" == two ]] || fail "main checkout: didn't replace the copy for a new key"

old() { touch -h -t 202001010000 "$1"; }
mkdir -p "$store/sccache" && old "$store/sccache"
for k in k1 k2 k3 k4 k5; do old "$store/thing/$k"; done
mkdir "$store/thing/k3.lock" && echo $$ >"$store/thing/k3.lock/pid"
chmod u+w "$store/thing" && mkdir "$store/thing/k9.tmp-$dead" "$store/thing/k8.tmp-$$"
kanna_store "$wt" thing k6 slow six >/dev/null 2>&1
kanna_store_prune "$wt" 2>/dev/null
[[ -d "$store/thing/k1" ]] || fail "prune: removed an entry a worktree links to"
[[ ! -e "$store/thing/k2" && ! -e "$store/thing/k4" && ! -e "$store/thing/k5" ]] || fail "prune: kept unlinked old entries: $(ls "$store/thing")"
[[ -d "$store/thing/k3" ]] || fail "prune: removed a locked entry"
[[ -d "$store/thing/k6" ]] || fail "prune: removed an entry used today"
[[ ! -e "$store/thing/k9.tmp-$dead" && -d "$store/thing/k8.tmp-$$" ]] || fail "prune: temp folders: $(ls "$store/thing")"
[[ -d "$store/sccache" ]] || fail "prune: removed sccache"
rm -rf "$store/thing/k3.lock"

repo="$root/repo"
make_folder() { mkdir -p "$repo/$1/debug" && echo built >"$repo/$1/debug/out" && touch -t "$2" "$repo/$1/.used"; }
for name in task-old task-new task-run; do worktree "$name" >/dev/null; done
make_folder task-old 202001010000
make_folder task-new 202501010000
make_folder task-run 202001010000
make_folder task-orphan 202501010000
mkdir -p "$repo/_main/release" "$repo/.offline/release"
(cd "$main/.kanna-worktrees/task-run" && exec sleep 60) & pids+=("$!"); disown "$!"
kanna_targets_free_gb() { echo $((250 - 50 * $(ls "$repo" | grep -cv '^_'))); }
config "$root" 100
unset KANNA_EVICT_DONE
(cd "$wt" && kanna_evict "$wt") 2>"$dir/said"
[[ ! -e "$repo/task-orphan" ]] || fail "evict: kept the folder of a worktree that's gone"
[[ ! -e "$repo/task-old" ]] || fail "evict: kept the least recently used folder"
[[ -d "$repo/task-new" ]] || fail "evict: removed more than it needed: $(cat "$dir/said")"
unset KANNA_EVICT_DONE
config "$root" 100000
(cd "$wt" && kanna_evict "$wt") 2>"$dir/said"
[[ ! -e "$repo/task-new" ]] || fail "evict: kept a stopped task's folder under the threshold"
[[ -d "$repo/task-run" ]] || fail "evict: removed a running task's folder"
[[ -d "$repo/task-a" ]] || fail "evict: removed the building worktree's folder"
[[ -d "$repo/_main/release" && -d "$repo/.offline/release" && -d "$store/thing/k1" ]] || fail "evict: removed _main, .offline or the store"
[[ ! -e "$store/.evict.lock" ]] || fail "evict: left its lock"

unset KANNA_EVICT_DONE
mkdir -p "$repo/task-held" && mkdir "$store/.evict.lock" && echo $$ >"$store/.evict.lock/pid"
(kanna_evict "$wt") 2>/dev/null
[[ -d "$repo/task-held" ]] || fail "evict: ran while another eviction held the lock"
rm -rf "$store/.evict.lock"

unset KANNA_EVICT_DONE
config "$root" 100000
kanna_running_paths() { return 1; }
(kanna_evict "$wt") 2>"$dir/said"
[[ -d "$repo/task-held" ]] || fail "evict: removed a folder without knowing what runs"
grep -q "removing nothing" "$dir/said" || fail "evict: didn't say why it removed nothing: $(cat "$dir/said")"

[[ $failed == 0 ]] && echo "store: ok"
exit "$failed"
