#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scripts="$repo_root/scripts"
usage() { echo "usage: scripts/build.sh [production|staging|dev] [--arch arm64|x86_64]   (default: dev, or \$KANNA_CHANNEL; this Mac's architecture)" >&2; exit 2; }
channel="${KANNA_CHANNEL:-dev}"
arch=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    production|staging|dev) channel="$1" ;;
    --arch) [[ $# -ge 2 ]] || usage; arch="$2"; shift ;;
    --arch=*) arch="${1#--arch=}" ;;
    *) usage ;;
  esac
  shift
done
case "$channel" in production|staging|dev) export KANNA_CHANNEL="$channel" ;; *) usage ;; esac
echo "==> Channel: $KANNA_CHANNEL"
. "$scripts/env.sh"
kanna_targets "$repo_root" || exit 1
macos_target="$(kanna_macos_target "$repo_root")"
echo "==> macOS deployment target: $macos_target"
host="$(kanna_host_triple)"
if [[ -n "$arch" ]]; then triple="$(kanna_arch_triple "$arch")" || usage; else triple="$host"; fi
arch_dir="$(kanna_arch_dir "$repo_root" "$triple")"
echo "==> Architecture: $(kanna_triple_arch "$triple") (${arch_dir#"$repo_root/"}/Kanna.app)"
kanna_rust_target "$triple"
pinned() { [[ -z "${GHOSTTY_SOURCE_DIR:-}" ]] && kanna_ghostty_built "$1"; }

echo "==> Ghostty: using the main checkout's pinned builds, if this is a worktree"
kanna_borrow_built "$repo_root" ghosttykit
kanna_borrow_built "$repo_root" "ghostty-vt/$triple"

if pinned "$repo_root/target/ghostty-vt/$triple"; then
  echo "==> libghostty-vt: already Ghostty $KANNA_GHOSTTY_COMMIT"
else
  echo "==> libghostty-vt (scripts/build-ghostty-vt.sh $triple)"
  "$scripts/build-ghostty-vt.sh" "$triple"
fi

if pinned "$repo_root/target/ghosttykit"; then
  echo "==> GhosttyKit: already Ghostty $KANNA_GHOSTTY_COMMIT"
else
  echo "==> GhosttyKit (scripts/build-ghosttykit.sh)"
  "$scripts/build-ghosttykit.sh"
fi

if [[ "$triple" == "$host" ]]; then
  echo "==> kanna, kanna-daemon, kanna-keeper, libkanna_ffi (cargo build --workspace --release)"
  kanna_borrow_cargo "$repo_root"
  kanna_cargo_release_for "$repo_root" "$macos_target"
  cargo build --manifest-path "$repo_root/Cargo.toml" --workspace --release
else
  echo "==> kanna, kanna-daemon, kanna-keeper, libkanna_ffi (cargo build --workspace --release --target $triple)"
  kanna_borrow_cargo "$repo_root" "$triple"
  kanna_cargo_release_for "$repo_root" "$macos_target" "$triple"
  cargo build --manifest-path "$repo_root/Cargo.toml" --workspace --release --target "$triple"
fi

echo "==> Swift app and ${arch_dir#"$repo_root/"}/Kanna.app (scripts/build-kanna-app.sh)"
"$scripts/build-kanna-app.sh" "$KANNA_CHANNEL" --arch "$(kanna_triple_arch "$triple")"
kanna_targets_offline "$repo_root" || echo "==> Couldn't sync target/.offline: a build with the volume unplugged starts cold" >&2
