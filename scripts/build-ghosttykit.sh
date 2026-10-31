#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$repo_root/scripts/env.sh"
. "$repo_root/scripts/store.sh"
kanna_targets "$repo_root" || exit 1
if [[ "${1:-}" != --into ]]; then
  kanna_ghosttykit "$repo_root"
  echo "GhosttyKit: $repo_root/target/ghosttykit -> $(cd "$repo_root/target/ghosttykit" && pwd -P)"
  exit 0
fi
[[ $# == 2 ]] || { echo "usage: scripts/build-ghosttykit.sh [--into DIR]" >&2; exit 2; }
out="$2"
source_dir="$(kanna_ghostty_source "$repo_root")"
zig="$(kanna_zig "$repo_root")"

optimize="${KANNA_GHOSTTY_OPTIMIZE:-ReleaseSafe}"
if [[ -n "${GHOSTTY_SOURCE_DIR:-}" ]]; then
  work="$repo_root/target/ghosttykit-build"
else
  work="$out.work"
  rm -rf "$work"
  trap 'rm -rf "$work"' EXIT
fi
mkdir -p "$work/macos"
(cd "$work" && "$zig" build --build-file "$source_dir/build.zig" --cache-dir "$work/.zig-cache" -p "$work/zig-out" \
  -Demit-xcframework=true -Demit-macos-app=false -Doptimize="$optimize" -Dsentry=false)

xcframework="$out/GhosttyKit.xcframework"
mkdir -p "$out"
rm -rf "$xcframework"
cp -R "$work/macos/GhosttyKit.xcframework" "$out/"
kanna_ghostty_rev "$source_dir" > "$out/ghostty.rev"

add_lib_prefix_swiftpm_needs() {
  local plist="$xcframework/Info.plist"
  local i=0 library identifier
  while library=$(/usr/libexec/PlistBuddy -c "Print :AvailableLibraries:$i:LibraryPath" "$plist" 2>/dev/null); do
    identifier=$(/usr/libexec/PlistBuddy -c "Print :AvailableLibraries:$i:LibraryIdentifier" "$plist")
    if [[ "$library" != lib* ]]; then
      mv "$xcframework/$identifier/$library" "$xcframework/$identifier/lib$library"
      /usr/libexec/PlistBuddy -c "Set :AvailableLibraries:$i:LibraryPath lib$library" "$plist"
    fi
    i=$((i + 1))
  done
}
add_lib_prefix_swiftpm_needs
grep -q GHOSTTY_IO_MODE_EXTERNAL "$xcframework"/*/Headers/ghostty.h \
  || { echo "GhosttyKit has no external I/O: $source_dir isn't the fork's kanna4 branch" >&2; exit 1; }
echo "Built $xcframework"
