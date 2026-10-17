#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="$repo_root/target/Kanna.app"
kanna="$repo_root/target/release/kanna"

case "${1:-}" in
  -h|--help)
    cat <<'USAGE'
usage: scripts/run-kanna.sh                 open this checkout's Kanna (target/Kanna.app)
       scripts/run-kanna.sh COMMAND [ARGS]  run this checkout's kanna CLI against it

The Kanna built here (scripts/build.sh) runs as its own app beside any other:
a dev build in a worktree is "Kanna Dev <worktree>", with its own instance.
KANNA_SOCKET, KANNA_INSTANCE and KANNA_PANE are cleared first, so from a terminal
inside another Kanna this still reaches this checkout's Kanna, not that one.
USAGE
    exit 0 ;;
esac
[[ -x "$kanna" && -d "$app" ]] || { echo "run-kanna: build first: scripts/build.sh" >&2; exit 1; }
if [[ $# -eq 0 ]]; then
  exec /usr/bin/open "$app"
fi
exec env -u KANNA_SOCKET -u KANNA_INSTANCE -u KANNA_PANE -u KANNA_MUX "$kanna" "$@"
