#!/usr/bin/env bash
set -uo pipefail

scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
dir="$(mktemp -d "${TMPDIR:-/tmp}/kanna-store.XXXXXX")" && dir="$(cd "$dir" && pwd -P)" || exit 1
pids=()
trap 'for p in ${pids[@]+"${pids[@]}"}; do kill "$p" 2>/dev/null; done; chmod -R u+w "$dir"; rm -rf "$dir"' EXIT
failed=0
fail() { echo "FAIL: $*"; failed=1; }

export KANNA_LOCK_POLL=0.1
main="$dir/repo"
git init -q -b main "$main" && git -C "$main" -c user.email=e@e -c user.name=e commit -q --allow-empty -m first || exit 1
config() { printf '{"targets": "%s"%s}\n' "$1" "${2:+, \"free\": $2}" >"$main/build.local.json"; }
root="$dir/volume/kanna-targets"
config "$root"
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

nested() { mkdir -p "$1/include/ghostty/vt" && echo "$2" >"$1/include/ghostty/vt/h.h" && echo "$2" >"$1/out"; }
kanna_store "$wt" thing k7 nested seven >/dev/null 2>&1
mkdir -p "$main/target/thing.copying/include/ghostty/vt" && echo stale >"$main/target/thing.copying/include/ghostty/vt/h.h"
chmod -R a-w "$main/target/thing.copying"
kanna_store_use "$store/thing/k7" "$main/target/thing" "$main" 2>"$dir/said" || fail "leftover .copying: exit $?: $(cat "$dir/said")"
[[ "$(cat "$main/target/thing/include/ghostty/vt/h.h" 2>/dev/null)" == seven && ! -e "$main/target/thing.copying" ]] || fail "leftover .copying: $(ls -R "$main/target/thing")"

cp() {
  if [[ "$1" == -c ]]; then
    local from="${*: -2:1}" to="${*: -1}"
    (cd "$from" && find . -type d) | while IFS= read -r d; do mkdir -p "$to/$d"; done
    chmod -R a-w "$to"
    echo "cp: $to/out: clonefile failed: Cross-device link" >&2
    return 1
  fi
  command cp "$@"
}
kanna_store "$wt" thing k8 nested eight >/dev/null 2>&1
kanna_store_use "$store/thing/k8" "$main/target/thing" "$main" 2>"$dir/said" || fail "no clones: exit $?: $(cat "$dir/said")"
[[ "$(cat "$main/target/thing/include/ghostty/vt/h.h" 2>/dev/null)" == eight && "$(cat "$main/target/thing/.store-key")" == "$store/thing/k8" ]] || fail "no clones: didn't copy: $(ls -R "$main/target/thing")"
[[ -w "$main/target/thing/include/ghostty/vt" && -w "$main/target/thing/out" && ! -e "$main/target/thing.copying" ]] || fail "no clones: the copy is read-only, or .copying is left"
unset -f cp

if hdiutil create -size 16m -fs APFS -volname kanna-store-test -quiet "$dir/other.dmg" 2>/dev/null &&
   hdiutil attach -nobrowse -noverify -mountpoint "$dir/other" -quiet "$dir/other.dmg" 2>/dev/null; then
  trap 'hdiutil detach -force -quiet "$dir/other" 2>/dev/null; for p in ${pids[@]+"${pids[@]}"}; do kill "$p" 2>/dev/null; done; chmod -R u+w "$dir"; rm -rf "$dir"' EXIT
  far="$dir/other/_shared/thing/k1"
  mkdir -p "$far" && nested "$far" far && chmod -R a-w "$far"
  kanna_store_use "$far" "$main/target/thing" "$main" 2>"$dir/said" || fail "another volume: exit $?: $(cat "$dir/said")"
  [[ "$(cat "$main/target/thing/include/ghostty/vt/h.h" 2>/dev/null)" == far && -w "$main/target/thing/include/ghostty/vt" ]] || fail "another volume: $(ls -R "$main/target/thing")"
  chmod -R u+w "$dir/other/_shared"
else
  echo "skip: no disk image (hdiutil can't attach here); the cp -c failure above stands in for it"
fi

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

config "$root"
[[ -z "$(kanna_targets_floor_gb "$wt")" ]] || fail "settings: a threshold with no build.json or build.local.json"
printf '{"free": 40, "targets": "%s"}\n' "$dir/elsewhere" >"$wt/build.json"
[[ "$(kanna_targets_floor_gb "$wt")" == 40 ]] || fail "settings: build.json's default isn't used"
[[ "$(kanna_targets_root "$wt")" == "$root" ]] || fail "settings: targets came from build.json"
config "$root" 70
[[ "$(kanna_targets_floor_gb "$wt")" == 70 ]] || fail "settings: build.local.json doesn't override build.json"
rm "$wt/build.json"

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

builds=0
cargo() {
  [[ "$1" == build ]] && builds=$((builds + 1)) && echo built >>"$dir/cargo-builds"
  mkdir -p "$CARGO_TARGET_DIR/debug/.fingerprint/dep-x" "$CARGO_TARGET_DIR/debug/.fingerprint/kanna-daemon-1" "$CARGO_TARGET_DIR/debug/deps" "$CARGO_TARGET_DIR/debug/incremental/kanna"
  echo dep >"$CARGO_TARGET_DIR/debug/deps/libdep_x.rlib"; echo ours >"$CARGO_TARGET_DIR/debug/deps/libkanna_mux.rlib"; echo bin >"$CARGO_TARGET_DIR/debug/kanna-daemon"
}
printf 'lock 1\n' >"$main/Cargo.lock" && printf '[workspace]\n' >"$main/Cargo.toml"
git -C "$main" add Cargo.lock Cargo.toml && git -C "$main" -c user.email=e@e -c user.name=e commit -q -m lock
config "$root"
seeded="$(worktree task-seeded)" && (kanna_targets "$seeded") 2>/dev/null
rm -f "$dir/cargo-builds"
kanna_cargo_seed "$seeded" "" 15.0 2>/dev/null
[[ "$(wc -l <"$dir/cargo-builds" | tr -d ' ')" == 1 ]] || fail "seed: main's Cargo.lock didn't build one"
[[ -f "$seeded/target/debug/deps/libdep_x.rlib" && -d "$seeded/target/debug/.fingerprint/dep-x" ]] || fail "seed: the worktree didn't get the dependencies"
[[ ! -e "$seeded/target/debug/deps/libkanna_mux.rlib" && ! -e "$seeded/target/debug/.fingerprint/kanna-daemon-1" && ! -e "$seeded/target/debug/incremental" && ! -e "$seeded/target/debug/kanna-daemon" ]] || fail "seed: carried our own crates: $(ls -R "$seeded/target/debug")"
[[ "$(cat "$seeded/target/debug/macos-deployment-target")" == 15.0 && -w "$seeded/target/debug/deps/libdep_x.rlib" ]] || fail "seed: no stamp, or read-only"
second="$(worktree task-second)" && (kanna_targets "$second") 2>/dev/null
kanna_cargo_seed "$second" "" 15.0 2>/dev/null
[[ "$(wc -l <"$dir/cargo-builds" | tr -d ' ')" == 1 && -f "$second/target/debug/deps/libdep_x.rlib" ]] || fail "seed: a second worktree built again, or got nothing"
bumped="$(worktree task-bumped)" && (kanna_targets "$bumped") 2>/dev/null && printf 'lock 2\n' >"$bumped/Cargo.lock"
kanna_cargo_seed "$bumped" "" 15.0 2>/dev/null
[[ "$(wc -l <"$dir/cargo-builds" | tr -d ' ')" == 1 ]] || fail "seed: a worktree's own Cargo.lock built a seed"
[[ -f "$bumped/target/debug/deps/libdep_x.rlib" ]] || fail "seed: a worktree's own Cargo.lock didn't start from main's seed"
echo mine >"$second/target/debug/deps/libdep_x.rlib"
kanna_cargo_seed "$second" "" 15.0 2>/dev/null
[[ "$(cat "$second/target/debug/deps/libdep_x.rlib")" == mine ]] || fail "seed: replaced a worktree's own build"
kanna_cargo_seed "$main" "" 15.0 2>/dev/null
[[ ! -e "$main/target/debug" && ! -e "$main/target/release" ]] || fail "seed: seeded the main checkout"

[[ $failed == 0 ]] && echo "store: ok"
exit "$failed"
