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
. "$scripts/store.sh"
kanna_targets "$repo_root" || exit 1
macos_target="$(kanna_macos_target "$repo_root")"
echo "==> macOS deployment target: $macos_target"
host="$(kanna_host_triple)"
if [[ -n "$arch" ]]; then triple="$(kanna_arch_triple "$arch")" || usage; else triple="$host"; fi
arch_dir="$(kanna_arch_dir "$repo_root" "$triple")"
echo "==> Architecture: $(kanna_triple_arch "$triple") (${arch_dir#"$repo_root/"}/Kanna.app)"
kanna_rust_target "$triple"
rust_dir="$(kanna_rust_dir "$repo_root" "$triple")"
cargo_profile="$(kanna_cargo_profile "$repo_root")"

echo "==> libghostty-vt and GhosttyKit for Ghostty $KANNA_GHOSTTY_COMMIT, from the build store"
kanna_ghostty_vt "$repo_root" "$triple"
kanna_ghosttykit "$repo_root"

echo "==> kanna, kanna-daemon, kanna-keeper, libkanna_ffi (cargo build --workspace --profile $cargo_profile into ${rust_dir#"$repo_root/"})"
kanna_cargo_seed "$repo_root" "$triple" "$macos_target"
kanna_cargo_stamp "$repo_root" "$macos_target" "$triple"
if [[ "$triple" == "$host" ]]; then
  cargo build --manifest-path "$repo_root/Cargo.toml" --workspace --profile "$cargo_profile"
else
  cargo build --manifest-path "$repo_root/Cargo.toml" --workspace --profile "$cargo_profile" --target "$triple"
fi

echo "==> Swift app and ${arch_dir#"$repo_root/"}/Kanna.app (scripts/build-kanna-app.sh)"
"$scripts/build-kanna-app.sh" "$KANNA_CHANNEL" --arch "$(kanna_triple_arch "$triple")"
kanna_targets_offline "$repo_root" || echo "==> Couldn't sync target/.offline: a build with the volume unplugged starts cold" >&2
