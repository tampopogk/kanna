#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(env -i HOME="$HOME" PATH="$HOME/.cargo/bin:$PATH" TMPDIR="${TMPDIR:-/tmp}" KANNA_VERSION="${KANNA_VERSION:-}" \
  bash -c '. "$1/scripts/env.sh" && kanna_version "$1"' _ "$repo_root")"
[[ -n "$version" ]] || { echo "ios-version: kanna-build printed no version" >&2; exit 1; }
plist="$TARGET_BUILD_DIR/$INFOPLIST_PATH"
plutil -replace CFBundleShortVersionString -string "${version%%-*}" "$plist"
plutil -replace CFBundleVersion -string "$(git -C "$repo_root" rev-list --count HEAD)" "$plist"
plutil -replace KannaVersion -string "$version" "$plist"
echo "Kanna $version"
