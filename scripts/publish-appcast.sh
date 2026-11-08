#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
feed_repo="tampopogk/kanna-web"
. "$repo_root/scripts/env.sh"
fail() { echo "publish-appcast: $*" >&2; exit 1; }
[[ $# -eq 2 && -d "$1" ]] || { echo "usage: scripts/publish-appcast.sh FEEDS-FOLDER BUILD-NUMBER" >&2; exit 2; }
feeds="$1"
build="$2"

for feed in "$feeds"/*.xml; do
  url="$(sed -n 's#.*<enclosure url="\([^"]*\)".*#\1#p' "$feed")"
  length="$(sed -n 's#.*<enclosure [^>]*length="\([0-9]*\)".*#\1#p' "$feed")"
  [[ -n "$url" && -n "$length" ]] || fail "$feed names no enclosure"
  got="$(curl -fsSL "$url" | wc -c | tr -d ' ')" || fail "can't download $url"
  [[ "$got" == "$length" ]] || fail "$url is $got bytes, the feed says $length"
done

for feed in "$feeds"/*.xml; do
  name="appcast/$(basename "$feed")"
  sha="$(gh api "repos/$feed_repo/contents/$name" --jq .sha 2>/dev/null || true)"
  args=(-X PUT "repos/$feed_repo/contents/$name" -f "message=Appcast: $(basename "$feed" .xml) build $build" -f "content=$(base64 <"$feed" | tr -d '\n')")
  [[ -n "$sha" ]] && args+=(-f "sha=$sha")
  gh api "${args[@]}" --silent
done

for feed in "$feeds"/*.xml; do
  url="$KANNA_UPDATE_FEEDS/$(basename "$feed")"
  for _ in $(seq 80); do
    curl -fsSL "$url?build=$build" 2>/dev/null | grep -q "<sparkle:version>$build</sparkle:version>" && { echo "    $url"; continue 2; }
    sleep 15
  done
  fail "$url doesn't serve build $build yet: check tampopogk/kanna-web's Pages build"
done
