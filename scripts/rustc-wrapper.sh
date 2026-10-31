#!/bin/sh
case "${KANNA_SCCACHE:-1}" in 0) exec "$@" ;; esac
command -v sccache >/dev/null 2>&1 || exec "$@"
off="${TMPDIR:-/tmp}/kanna-sccache-off.$PPID"
[ -e "$off" ] && exec "$@"
if [ -z "${SCCACHE_DIR:-}" ]; then
  scripts="$(cd "$(dirname "$0")" && pwd -P)"
  . "$scripts/env.sh"
  . "$scripts/store.sh"
  SCCACHE_DIR="$(kanna_store_root "$(dirname "$scripts")")/sccache"
  export SCCACHE_DIR
fi
export SCCACHE_SERVER_PORT="${KANNA_SCCACHE_PORT:-4227}" SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-30G}"
sccache "$@"
status=$?
[ "$status" = 2 ] || exit "$status"
echo "kanna: sccache can't run here: compiling without it" >&2
: > "$off"
exec "$@"
