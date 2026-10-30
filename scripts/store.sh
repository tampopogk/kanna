#!/bin/sh

kanna_store_root() {
  root="$(kanna_targets_root)"
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

kanna_store_writable() {
  chmod -R u+w "$1" 2>/dev/null
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
      kanna_store_writable "$tmp"
      echo "==> Building $name/$key into the store ($store)" >&2
      if (cd "$store/$name" && "$builder" "$tmp" "$@") >&2 && [ -d "$tmp" ]; then
        find "$tmp" -mindepth 1 -exec chmod a-w {} + && mv "$tmp" "$entry" && chmod a-w "$entry" || { kanna_store_writable "$tmp"; kanna_lock_release "$entry.lock"; return 1; }
      else
        echo "==> Building $name/$key failed" >&2
        kanna_store_writable "$tmp"
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
  if kanna_main_checkout "$3" >/dev/null; then
    [ -L "$here" ] && [ "$(readlink "$here")" = "$entry" ] && return 0
    if [ -L "$here" ]; then rm "$here"; elif [ -e "$here" ]; then kanna_store_writable "$here"; fi
    mkdir -p "$(dirname "$here")" && ln -s "$entry" "$here"
    return
  fi
  [ ! -L "$here" ] && [ "$(cat "$here/.store-key" 2>/dev/null)" = "$entry" ] && return 0
  rm -rf "$here.copying"
  mkdir -p "$(dirname "$here")" || return 1
  kanna_clone "$entry" "$here.copying" && chmod -R u+w "$here.copying" && echo "$entry" > "$here.copying/.store-key" || { kanna_store_writable "$here.copying"; return 1; }
  if [ -L "$here" ]; then rm "$here"; elif [ -e "$here" ]; then kanna_store_writable "$here"; fi
  mv "$here.copying" "$here"
}

kanna_store_refs() {
  root="$(kanna_targets_root)"
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
        kanna_pid_alive "${pid%%.*}" || kanna_store_writable "$entry"
        continue ;;
    esac
    printf '%s\n' "$refs" | grep -qxF "$entry" && continue
    [ -n "$(find "$entry" -maxdepth 0 -mmin "-$grace" 2>/dev/null)" ] && continue
    kanna_lock_try "$entry.lock" || continue
    echo "==> Pruning $entry: nothing links to it" >&2
    kanna_store_writable "$entry"
    kanna_lock_release "$entry.lock"
  done
}

kanna_targets_free_gb() {
  df -k "$1" 2>/dev/null | awk 'NR == 2 { print int($4 / 1048576) }'
}

kanna_targets_floor_gb() {
  config="$(kanna_targets_config)"
  floor=""
  [ -f "$config" ] && floor="$(plutil -extract build.free raw -o - "$config" 2>/dev/null)"
  case "$floor" in ""|*[!0-9]*) floor=100 ;; esac
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
  root="$(kanna_targets_root)"
  case "$root" in /*) ;; *) return 0 ;; esac
  kanna_targets_unmounted "$root" >/dev/null && return 0
  if main="$(kanna_main_checkout "$1")"; then self="$(basename "$1")"; else main="$1"; self=""; fi
  repo="$root/$(basename "$main")"
  [ -d "$repo" ] || return 0
  floor="$(kanna_targets_floor_gb)"
  free="$(kanna_targets_free_gb "$root")"
  [ -n "$free" ] && [ "$free" -lt "$floor" ] || return 0
  mkdir -p "$root/_shared"
  kanna_lock_try "$root/_shared/.evict.lock" || return 0
  echo "==> $root has ${free} GB free, under build.free (${floor} GB): removing build folders of stopped tasks, least recently used first" >&2
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
