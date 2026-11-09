#!/bin/sh
set -eu

usage() {
  echo "usage: scripts/move-instance.sh TO [--dry-run] [--serve]

Moves the Kanna that \$KANNA_SOCKET names (the one this terminal runs in) to the
instance folder TO: a folder name in ~/Library/Application Support (kanna-staging)
or a path. Its tasks, records, machine key, windows and terminals move, and its
agents run on (docs/specs/instance-move.md).

  --dry-run  what would move, and why it would be refused; changes nothing
  --serve    after the move, keep TO's daemon running from launchd
             (kanna machine serve), instead of opening TO's Kanna.app" >&2
  exit 2
}

to="" dry=false serve=false
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry=true ;;
    --serve) serve=true ;;
    -h|--help) usage ;;
    -*) usage ;;
    *) [ -z "$to" ] || usage; to="$arg" ;;
  esac
done
[ -n "$to" ] || usage
[ -n "${KANNA_SOCKET:-}" ] || { echo "KANNA_SOCKET isn't set: run this in a terminal of the Kanna to move, or set it to that Kanna's socket" >&2; exit 2; }

kanna="${KANNA:-kanna}"
request="$(jq -cn --arg to "$to" --argjson dry "$dry" '{cmd: "instance", args: {action: "move", to: $to, dry: $dry}}')"
reply="$("$kanna" raw "$request")"

echo "$reply" | jq -r '
  "From \(.from) to \(.to) (\(.folder)):",
  "  tasks: \(.tasks.waiting) waiting, \(.tasks.started) started, \(.tasks.desk) on a Desk",
  "  terminals: \(.terminals), their agents running on",
  "  records: version \(.records)",
  (if (.accounts | length) > 0 then "  agent accounts to add again in \(.to): \(.accounts | join(", "))" else empty end),
  (.refused[]? | "  refused (\(.code)): \(.message)"),
  (if .moved then "Moved (\(.id)): \(.moved.waiting) waiting, \(.moved.started) started, \(.moved.desk) on a Desk" else empty end),
  (.warnings[]? | "  warning: \(.)")'

$dry && exit 0

case "$to" in
  */*) folder="$to" ;;
  *) folder="$HOME/Library/Application Support/$to" ;;
esac
app="$(jq -r '.app // empty' "$folder/instance.json" 2>/dev/null || true)"
bundle="$(jq -r '.bundle // empty' "$folder/instance.json" 2>/dev/null || true)"
for name in $(env | sed -n 's/^\(KANNA_[A-Za-z0-9_]*\)=.*/\1/p'); do
  unset "$name"
done
if $serve; then
  [ -n "$app" ] && [ -x "$app/Contents/Helpers/kanna" ] || { echo "$folder/instance.json names no app: run its kanna machine serve yourself" >&2; exit 1; }
  "$app/Contents/Helpers/kanna" machine serve
elif [ -n "$app" ] && [ -d "$app" ]; then
  open "$app"
elif [ -n "$bundle" ]; then
  open -b "$bundle"
else
  echo "$folder/instance.json names no app: open $to's Kanna yourself" >&2
fi
