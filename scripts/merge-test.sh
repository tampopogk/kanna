#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scripts="$repo_root/scripts"
. "$scripts/env.sh"
. "$scripts/store.sh"
kanna_targets "$repo_root" || exit 1
macos_target="$(kanna_macos_target "$repo_root")"
triple="$(kanna_host_triple)"
kanna_ghostty_vt "$repo_root" "$triple"
kanna_cargo_seed "$repo_root" "$triple" "$macos_target"
kanna_cargo_stamp "$repo_root" "$macos_target" "$triple"
exec cargo test --manifest-path "$repo_root/Cargo.toml" --workspace --no-fail-fast "$@"
