#!/bin/sh

kanna_store_root() {
  root="$(kanna_targets_root "$1")"
  case "$root" in
    /*) if ! kanna_targets_unmounted "$root" >/dev/null; then echo "$root/_shared"; return; fi ;;
  esac
  main="$(kanna_main_checkout "$1")" || main="$(cd "$1" && pwd -P)"
  echo "$main/target/_shared"
}

kanna_store_key() {
  printf '%s\n' "$@" | shasum -a 256 | cut -c1-16
}

kanna_file_hash() {
  shasum -a 256 "$1" | cut -c1-64
}

kanna_pid_alive() {
  [ -n "$1" ] && kill -0 "$1" 2>/dev/null
}

kanna_lock_try() {
  if mkdir "$1" 2>/dev/null; then
    echo "$$" > "$1/pid"
    return 0
  fi
  holder="$(cat "$1/pid" 2>/dev/null)"
  if [ -n "$holder" ] && ! kanna_pid_alive "$holder"; then
    echo "==> Taking over $(basename "$1"): pid $holder is gone" >&2
    rm -rf "$1"
    if mkdir "$1" 2>/dev/null; then
      echo "$$" > "$1/pid"
      return 0
    fi
  fi
  return 1
}

kanna_lock_wait() {
  said=""
  until kanna_lock_try "$1"; do
    if [ -z "$said" ]; then
      echo "==> Waiting for pid $(cat "$1/pid" 2>/dev/null || echo "?") to finish ${1%.lock}" >&2
      said=1
    fi
    sleep "${KANNA_LOCK_POLL:-2}"
  done
}

kanna_lock_release() {
  rm -rf "$1"
}

kanna_store() {
  store="$(kanna_store_root "$1")"
  name="$2" key="$3" builder="$4"
  shift 4
  entry="$store/$name/$key"
  if [ ! -d "$entry" ]; then
    mkdir -p "$store/$name" || return 1
    kanna_lock_wait "$entry.lock"
    if [ ! -d "$entry" ]; then
      tmp="$entry.tmp-$$"
      kanna_remove "$tmp"
      echo "==> Building $name/$key into the store ($store)" >&2
      if (cd "$store/$name" && "$builder" "$tmp" "$@") >&2 && [ -d "$tmp" ]; then
        find "$tmp" -mindepth 1 -exec chmod a-w {} + && mv "$tmp" "$entry" && chmod a-w "$entry" || { kanna_remove "$tmp"; kanna_lock_release "$entry.lock"; return 1; }
      else
        echo "==> Building $name/$key failed" >&2
        kanna_remove "$tmp"
        kanna_lock_release "$entry.lock"
        return 1
      fi
    fi
    kanna_lock_release "$entry.lock"
  fi
  touch "$entry" 2>/dev/null
  echo "$entry"
}

kanna_store_use() {
  entry="$1" here="$2"
  [ -L "$here" ] && [ "$(readlink "$here")" = "$entry" ] && return 0
  kanna_remove "$here" && mkdir -p "$(dirname "$here")" && ln -s "$entry" "$here"
}

kanna_store_refs() {
  root="$(kanna_targets_root "$1")"
  case "$root" in /*) [ -d "$root" ] && find "$root" -maxdepth 4 -type l -exec readlink {} \; 2>/dev/null ;; esac
  git -C "$1" worktree list --porcelain 2>/dev/null | sed -n 's/^worktree //p' | while IFS= read -r wt; do
    [ -d "$wt/target" ] && find "$wt/target/" -maxdepth 2 -type l -exec readlink {} \; 2>/dev/null
  done
}

kanna_store_prune() {
  store="$(kanna_store_root "$1")"
  [ -d "$store" ] || return 0
  refs="$(kanna_store_refs "$1")"
  grace="${KANNA_STORE_GRACE_MIN:-1440}"
  for entry in "$store"/*/*; do
    [ -d "$entry" ] || continue
    base="$(basename "$entry")"
    case "$(basename "$(dirname "$entry")")" in sccache) continue ;; esac
    case "$base" in
      *.lock) continue ;;
      *.tmp-*)
        pid="${base##*.tmp-}"
        kanna_pid_alive "${pid%%.*}" || kanna_remove "$entry"
        continue ;;
    esac
    printf '%s\n' "$refs" | grep -qxF "$entry" && continue
    [ -n "$(find "$entry" -maxdepth 0 -mmin "-$grace" 2>/dev/null)" ] && continue
    kanna_lock_try "$entry.lock" || continue
    echo "==> Pruning $entry: nothing links to it" >&2
    kanna_remove "$entry"
    kanna_lock_release "$entry.lock"
  done
}

kanna_targets_free_gb() {
  df -k "$1" 2>/dev/null | awk 'NR == 2 { print int($4 / 1048576) }'
}

kanna_targets_floor_gb() {
  floor="$(kanna_targets_setting "$1" free)" || return 1
  case "$floor" in ""|*[!0-9]*) return 1 ;; esac
  echo "$floor"
}

kanna_running_paths() {
  lsof -a -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' || return 1
  ps -axww -o command= || return 1
}

kanna_mentions() {
  printf '%s\n' "$1" | awk -v p="$2" '$0 == p || index($0, p "/") || index($0, " " p " ") || substr($0, length($0) - length(p)) == " " p { found = 1 } END { exit !found }'
}

kanna_evict() {
  [ -n "${KANNA_EVICT_DONE:-}" ] && return 0
  export KANNA_EVICT_DONE=1
  kanna_store_prune "$1"
  root="$(kanna_targets_root "$1")"
  case "$root" in /*) ;; *) return 0 ;; esac
  kanna_targets_unmounted "$root" >/dev/null && return 0
  if main="$(kanna_main_checkout "$1")"; then self="$(basename "$1")"; else main="$1"; self=""; fi
  repo="$root/$(basename "$main")"
  [ -d "$repo" ] || return 0
  floor="$(kanna_targets_floor_gb "$1")" || return 0
  free="$(kanna_targets_free_gb "$root")"
  [ -n "$free" ] && [ "$free" -lt "$floor" ] || return 0
  mkdir -p "$root/_shared"
  kanna_lock_try "$root/_shared/.evict.lock" || return 0
  echo "==> $root has ${free} GB free, under free in $(kanna_targets_config "$1") (${floor} GB): removing build folders of stopped tasks, least recently used first" >&2
  if ! running="$(kanna_running_paths)" || [ -z "$running" ]; then
    echo "==> Can't list running processes: removing nothing" >&2
    kanna_lock_release "$root/_shared/.evict.lock"
    return 0
  fi
  worktrees="$(git -C "$main" worktree list --porcelain 2>/dev/null | sed -n 's/^worktree //p')"
  candidates=""
  for folder in "$repo"/*; do
    [ -d "$folder" ] && [ ! -L "$folder" ] || continue
    name="$(basename "$folder")"
    case "$name" in _*|.*) continue ;; esac
    [ "$name" = "$self" ] || [ "$name" = "$(basename "$main")" ] && continue
    wt="$(printf '%s\n' "$worktrees" | while IFS= read -r w; do [ "$(basename "$w")" = "$name" ] && echo "$w" && break; done)"
    kanna_mentions "$running" "$folder" && continue
    [ -n "$wt" ] && kanna_mentions "$running" "$wt" && continue
    if [ -e "$folder/.used" ]; then used="$(stat -f %m "$folder/.used")"; else used="$(stat -f %m "$folder")"; fi
    if [ -z "$wt" ]; then used=0; fi
    candidates="$candidates$used $name
"
  done
  for name in $(printf '%s' "$candidates" | sort -n | cut -d' ' -f2); do
    free="$(kanna_targets_free_gb "$root")"
    [ -n "$free" ] && [ "$free" -lt "$floor" ] || break
    echo "==> Removing $repo/$name" >&2
    rm -rf "${repo:?}/$name"
  done
  kanna_lock_release "$root/_shared/.evict.lock"
}

kanna_targets_ready() {
  [ -n "${SCCACHE_DIR:-}" ] || { SCCACHE_DIR="$(kanna_store_root "$1")/sccache"; export SCCACHE_DIR; }
  kanna_evict "$1"
}

kanna_cargo_family() {
  kanna_store_key "$(rustc -vV)" "$2" "$(kanna_cargo_profile "$1")" | cut -c1-8
}

kanna_cargo_inputs() {
  for file in Cargo.lock Cargo.toml .cargo/config.toml; do
    if [ -n "${2:-}" ]; then git -C "$1" show "HEAD:$file" 2>/dev/null; else cat "$1/$file" 2>/dev/null; fi
    echo "--- $file"
  done
}

kanna_cargo_seed_build() {
  out="$1" repo="$2" triple="$3" macos="$4"
  profile="$(kanna_cargo_profile "$repo")"
  [ "$profile" = dev ] && folder=debug || folder="$profile"
  [ "$triple" = "$(kanna_host_triple)" ] || folder="$triple/$folder"
  set -- --manifest-path "$repo/Cargo.toml" --workspace --profile "$profile"
  [ "$triple" = "$(kanna_host_triple)" ] || set -- "$@" --target "$triple"
  (cd "$repo" && CARGO_TARGET_DIR="$out" cargo build "$@" && CARGO_TARGET_DIR="$out" cargo test --no-run "$@") || return 1
  dir="$out/$folder"
  [ -d "$dir/.fingerprint" ] || { echo "no $dir/.fingerprint" >&2; return 1; }
  rm -rf "$dir/incremental" "$dir/examples" "$dir/.fingerprint"/kanna-* "$dir/build"/kanna-* "$dir/deps"/*kanna*
  find "$dir" -maxdepth 1 -type f -delete
  echo "$macos" > "$dir/macos-deployment-target"
  echo "$folder" > "$out/seed"
}

kanna_cargo_seed() {
  kanna_main_checkout "$1" >/dev/null || return 0
  triple="${2:-$(kanna_host_triple)}"
  here="$(kanna_rust_dir "$1" "$triple")" || return 1
  [ -e "$here" ] && return 0
  store="$(kanna_store_root "$1")"
  family="$(kanna_cargo_family "$1" "$triple")"
  key="$family-$(kanna_cargo_inputs "$1" | shasum -a 256 | cut -c1-8)"
  main="$(kanna_main_checkout "$1")" || main="$1"
  if [ -d "$store/cargo/$key" ]; then
    entry="$(kanna_store "$1" cargo "$key" kanna_cargo_seed_build "$1" "$triple" "$3")" || return 0
  elif [ "$(kanna_cargo_inputs "$1")" = "$(kanna_cargo_inputs "$main" head)" ]; then
    echo "==> Building the Rust dependencies' seed for main's Cargo.lock ($key)" >&2
    entry="$(kanna_store "$1" cargo "$key" kanna_cargo_seed_build "$1" "$triple" "$3")" || return 0
  else
    entry="$(cd "$store/cargo" 2>/dev/null && ls -dt "$family"-* 2>/dev/null | grep -v '\.' | head -1)"
    [ -n "$entry" ] && entry="$store/cargo/$entry"
    [ -n "$entry" ] || return 0
    touch "$entry" 2>/dev/null
  fi
  from="$entry/$(cat "$entry/seed" 2>/dev/null)"
  [ -f "$entry/seed" ] && [ -d "$from" ] || return 0
  mkdir -p "$(dirname "$here")" || return 1
  if kanna_clone "$from" "$here.seeding"; then
    mv "$here.seeding" "$here"
    echo "==> Started ${here#"$1/"} from the store's $(basename "$entry")" >&2
  else
    kanna_remove "$here.seeding"
  fi
}
