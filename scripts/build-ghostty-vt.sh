#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$repo_root/scripts/env.sh"
kanna_targets "$repo_root" || exit 1
source_dir="$(kanna_ghostty_source "$repo_root")"
zig="$(kanna_zig "$repo_root")"

host="$(rustc -vV | sed -n 's/^host: //p')"
target="${1:-$host}"
zig_target=()
if [ "$target" != "$host" ] || [ "$(uname -s)" = Linux ]; then
  case "$target" in
    x86_64-unknown-linux-gnu) zig_target=(-Dtarget=x86_64-linux-gnu -Dcpu=baseline) ;;
    aarch64-unknown-linux-gnu) zig_target=(-Dtarget=aarch64-linux-gnu -Dcpu=baseline) ;;
    x86_64-unknown-linux-musl) zig_target=(-Dtarget=x86_64-linux-musl -Dcpu=baseline) ;;
    aarch64-unknown-linux-musl) zig_target=(-Dtarget=aarch64-linux-musl -Dcpu=baseline) ;;
    x86_64-apple-darwin) zig_target=(-Dtarget=x86_64-macos -Dcpu=baseline) ;;
    aarch64-apple-darwin) zig_target=(-Dtarget=aarch64-macos -Dcpu=baseline) ;;
    *) echo "build-ghostty-vt: no Zig target for $target" >&2; exit 2 ;;
  esac
fi

out="$repo_root/target/ghostty-vt/$target"
stage="$repo_root/target/ghostty-vt/.stage-$target"
rm -rf "$stage"
optimize="${KANNA_GHOSTTY_OPTIMIZE:-ReleaseSafe}"
(cd "$source_dir" && "$zig" build --cache-dir "$repo_root/target/ghostty-vt/.zig-cache-$target" \
  -Demit-lib-vt=true -Demit-xcframework=false \
  -Doptimize="$optimize" ${zig_target[@]+"${zig_target[@]}"} -p "$stage")

rm -rf "$out"
mkdir -p "$out/lib"
cp -R "$stage/include" "$out/"
cp "$stage/lib/libghostty-vt.a" "$out/lib/"
rm -rf "$stage"
kanna_ghostty_rev "$source_dir" > "$out/ghostty.rev"
echo "Built $out (Ghostty $(cat "$out/ghostty.rev"))"
