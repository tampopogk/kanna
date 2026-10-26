#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE' >&2
usage: scripts/build-linux.sh [production|staging|dev] [--arch x86_64|aarch64] [--out DIR]

Builds Kanna's Linux tarball on this Mac or a Linux machine: the daemon, keeper and CLI
cross-compiled with Zig (baseline CPU, glibc 2.35), Kanna's skills and release.json, as
DIR/Kanna-VERSION-linux-ARCH.tar.gz (default DIR: target/release-linux). KANNA_VERSION sets
the version. docs/specs/fleet-update.md says what's in it.
USAGE
  exit 2
}

channel="${KANNA_CHANNEL:-production}" arch=x86_64 out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    production|staging|dev) channel="$1"; shift ;;
    --arch) arch="${2:-}"; shift 2 ;;
    --out) out="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
case "$arch" in x86_64|aarch64) ;; *) usage ;; esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$repo_root/scripts/env.sh"
kanna_targets "$repo_root" || exit 1
target="$arch-unknown-linux-gnu"
glibc=2.35
out="${out:-$repo_root/target/release-linux}"
fail() { echo "build-linux: $*" >&2; exit 1; }
step() { echo "==> $*"; }

commit="$(git -C "$repo_root" rev-parse HEAD)"
[[ -z "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]] || commit="$commit-dirty"
version="${KANNA_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo_root/Cargo.toml" | head -1)}"
case "$channel" in staging) name=Kanna-Staging ;; *) name=Kanna ;; esac

zig="$(kanna_zig "$repo_root")"
if command -v rustup >/dev/null 2>&1 && ! rustup target list --installed | grep -qx "$target"; then
  step "Rust's $target target"
  rustup target add "$target"
fi
if [[ ! -f "$repo_root/target/ghostty-vt/$target/lib/libghostty-vt.a" ]] || [[ "$(cat "$repo_root/target/ghostty-vt/$target/ghostty.rev" 2>/dev/null)" != "$KANNA_GHOSTTY_COMMIT" ]]; then
  step "libghostty-vt for $target"
  "$repo_root/scripts/build-ghostty-vt.sh" "$target"
fi

step "Build Kanna $version ($channel, $commit) for $target"
wrappers="$repo_root/target/zig-$target"
mkdir -p "$wrappers"
for tool in cc c++ ar; do
  case "$tool" in
    ar) body="exec \"$zig\" ar \"\$@\"" ;;
    *) body="args=(); for a in \"\$@\"; do case \"\$a\" in --target=*|-target|-Wl,-O1|-Wl,--fix-cortex-a53-843419) ;; *) args+=(\"\$a\") ;; esac; done; exec \"$zig\" $tool -target $arch-linux-gnu.$glibc -mcpu=baseline \"\${args[@]}\"" ;;
  esac
  printf '#!/usr/bin/env bash\n%s\n' "$body" > "$wrappers/$tool.new"
  chmod +x "$wrappers/$tool.new"
  mv "$wrappers/$tool.new" "$wrappers/$tool"
done
triple_env="$(echo "$target" | tr 'a-z-' 'A-Z_')"
triple_lower="$(echo "$target" | tr '-' '_')"
env "CC_$triple_lower=$wrappers/cc" "CXX_$triple_lower=$wrappers/c++" "AR_$triple_lower=$wrappers/ar" \
  "CARGO_TARGET_${triple_env}_LINKER=$wrappers/cc" \
  CARGO_PROFILE_RELEASE_STRIP=debuginfo KANNA_CHANNEL="$channel" KANNA_VERSION="$version" KANNA_COMMIT="$commit" \
  cargo build --release --manifest-path "$repo_root/Cargo.toml" --target "$target" -p kanna -p kanna-daemon -p kanna-keeper
built="$repo_root/target/$target/release"

step "Tarball"
stage="$out/.stage-$arch"
rm -rf "$stage"
mkdir -p "$stage/kanna/kanna-skills"
cp "$built/kanna" "$built/kanna-daemon" "$built/kanna-keeper" "$repo_root/apps/kanna/Resources/planner.sh" "$stage/kanna/"
for skill in kanna-tasks kanna-planning kanna-new-project kanna-markup kanna-explain kanna-quiz; do cp -RL "$repo_root/.agents/kanna-skills/skills/$skill" "$stage/kanna/kanna-skills/"; done
protocols="$(sed -n 's/^pub const PROTOCOLS: \[u32; 2\] = \[\([0-9]*\), \([0-9]*\)\];/[\1, \2]/p' "$repo_root/crates/kanna-build/src/lib.rs")"
[[ -n "$protocols" ]] || fail "can't read PROTOCOLS from crates/kanna-build/src/lib.rs"
cat > "$stage/kanna/release.json" <<JSON
{"commit": "$commit", "version": "$version", "channel": "$channel", "os": "linux", "arch": "$arch", "protocols": $protocols}
JSON
tarball="$out/$name-$version-linux-$arch.tar.gz"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$tarball.new" -C "$stage" kanna
mv "$tarball.new" "$tarball"
rm -rf "$stage"
echo "    $tarball"
