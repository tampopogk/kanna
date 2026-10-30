#!/bin/sh
KANNA_GHOSTTY_FORK="${KANNA_GHOSTTY_FORK:-https://github.com/jemdiggity/ghostty.git}"
KANNA_GHOSTTY_BRANCH="kanna4"
KANNA_GHOSTTY_COMMIT="4e28443e32fe8b0f88934d6a1159c42561f1931a"

KANNA_ZIG_VERSION="0.17.0"
KANNA_ZIG_SHA256_aarch64_macos="b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a"
KANNA_ZIG_SHA256_x86_64_macos="4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6"
KANNA_ZIG_SHA256_aarch64_linux="9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8"
KANNA_ZIG_SHA256_x86_64_linux="1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026"

kanna_macos_target() {
  cargo_target="$(sed -n 's/^MACOSX_DEPLOYMENT_TARGET *= *{ *value *= *"\([0-9.]*\)".*/\1/p' "$1/.cargo/config.toml" 2>/dev/null)"
  swift_target="$(sed -n -e 's/.*\.macOS(\.v\([0-9]*\)_\([0-9]*\)).*/\1.\2/p' -e 's/.*\.macOS(\.v\([0-9]*\)).*/\1.0/p' -e 's/.*\.macOS("\([0-9.]*\)").*/\1/p' "$1/apps/kanna/Package.swift")"
  if [ -z "$cargo_target" ] || [ "$cargo_target" != "$swift_target" ]; then
    echo "macOS deployment target: .cargo/config.toml has ${cargo_target:-none}, apps/kanna/Package.swift has ${swift_target:-none}; make them the same" >&2
    return 1
  fi
  echo "$cargo_target"
}

kanna_host_triple() {
  rustc -vV | sed -n 's/^host: //p'
}

kanna_arch_triple() {
  case "$1" in
    arm64|aarch64) echo aarch64-apple-darwin ;;
    x86_64) echo x86_64-apple-darwin ;;
    *) echo "unknown architecture $1: use arm64 or x86_64" >&2; return 1 ;;
  esac
}

kanna_triple_arch() {
  case "$1" in aarch64-*) echo arm64 ;; *) echo "${1%%-*}" ;; esac
}

kanna_arch_dir() {
  if [ "$2" = "$(kanna_host_triple)" ]; then echo "$1/target"; else echo "$1/target/$2"; fi
}

kanna_rust_target() {
  [ "$1" = "$(kanna_host_triple)" ] && return
  rustup target list --installed 2>/dev/null | grep -qx "$1" && return
  echo "==> rustup target add $1" >&2
  rustup target add "$1" >&2
}

kanna_cargo_release_for() {
  cargo_triple="${3:-$(kanna_host_triple)}"
  cargo_release="$(kanna_arch_dir "$1" "$cargo_triple")/release"
  stamp="$cargo_release/macos-deployment-target"
  [ "$(cat "$stamp" 2>/dev/null)" = "$2" ] && return
  if [ -d "$cargo_release" ]; then
    echo "==> $cargo_release was built for macOS $(cat "$stamp" 2>/dev/null || echo "the default"), not $2: cleaning it" >&2
    if [ "$cargo_triple" = "$(kanna_host_triple)" ]; then
      cargo clean --manifest-path "$1/Cargo.toml" --release >&2 || return 1
    else
      cargo clean --manifest-path "$1/Cargo.toml" --release --target "$cargo_triple" >&2 || return 1
    fi
  fi
  mkdir -p "$cargo_release" && echo "$2" > "$stamp"
}

kanna_zig() {
  if command -v zig >/dev/null 2>&1 && [ "$(zig version)" = "$KANNA_ZIG_VERSION" ]; then
    command -v zig
    return
  fi
  arch="$(uname -m)"; [ "$arch" = arm64 ] && arch=aarch64
  case "$(uname -s)" in Darwin) os=macos ;; Linux) os=linux ;; *) echo "no Zig download for $(uname -s)" >&2; return 1 ;; esac
  dir="$1/target/zig-$KANNA_ZIG_VERSION"
  if [ ! -x "$dir/zig" ]; then
    name="zig-$arch-$os-$KANNA_ZIG_VERSION"
    eval "sum=\${KANNA_ZIG_SHA256_${arch}_$os:-}"
    [ -n "$sum" ] || { echo "no Zig download for $arch-$os" >&2; return 1; }
    echo "Downloading Zig $KANNA_ZIG_VERSION ($(zig version 2>/dev/null || echo "no zig") on PATH)" >&2
    mkdir -p "$1/target"
    curl -fsSL "https://ziglang.org/download/$KANNA_ZIG_VERSION/$name.tar.xz" -o "$1/target/$name.tar.xz" || return 1
    echo "$sum  $1/target/$name.tar.xz" | shasum -a 256 -c - >/dev/null || { echo "Zig download checksum mismatch" >&2; return 1; }
    tar -xJf "$1/target/$name.tar.xz" -C "$1/target" && rm "$1/target/$name.tar.xz"
    mv "$1/target/$name" "$dir"
  fi
  echo "$dir/zig"
}

kanna_ghostty_source() {
  if [ -n "${GHOSTTY_SOURCE_DIR:-}" ]; then
    echo "$GHOSTTY_SOURCE_DIR"
    return
  fi
  if main="$(kanna_main_checkout "$1")" && kanna_ghostty_pinned "$main/target/ghostty-source"; then
    echo "$main/target/ghostty-source"
    return
  fi
  dir="$1/target/ghostty-source"
  if [ ! -f "$dir/build.zig" ]; then
    mkdir -p "$1/target"
    git clone -q --no-checkout --filter=blob:none --branch "$KANNA_GHOSTTY_BRANCH" "$KANNA_GHOSTTY_FORK" "$dir" >&2 || return 1
  fi
  if [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" != "$KANNA_GHOSTTY_COMMIT" ] || [ ! -f "$dir/build.zig" ]; then
    git -C "$dir" remote set-url origin "$KANNA_GHOSTTY_FORK" >&2 || return 1
    git -C "$dir" fetch -q origin "$KANNA_GHOSTTY_BRANCH" >&2 || return 1
    git -C "$dir" checkout -q --detach "$KANNA_GHOSTTY_COMMIT" >&2 || return 1
  fi
  echo "$dir"
}

kanna_ghostty_pinned() {
  [ -f "$1/build.zig" ] &&
    [ "$(git -C "$1" rev-parse HEAD 2>/dev/null)" = "$KANNA_GHOSTTY_COMMIT" ] &&
    [ -z "$(git -C "$1" status --porcelain 2>/dev/null)" ]
}

kanna_ghostty_rev() {
  rev="$(git -C "$1" rev-parse HEAD 2>/dev/null)" || { echo unknown; return; }
  if [ -n "$(git -C "$1" status --porcelain 2>/dev/null)" ]; then rev="$rev-dirty"; fi
  echo "$rev"
}

# A worktree uses the main checkout's prebuilt Ghostty pieces (target/ghosttykit, target/ghostty-vt/HOST)
# instead of building its own, when the main checkout's were built from the pinned commit. It links them
# into its own target/, so nothing there is copied and a rebuild here replaces the link, never the original.
kanna_borrow_built() {
  here="$1/target/$2"
  [ -e "$here" ] && return
  common="$(git -C "$1" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || return 0
  main="$(dirname "$common")/target/$2"
  [ "$main" != "$here" ] && [ -d "$main" ] || return 0
  [ "$(cat "$main/ghostty.rev" 2>/dev/null)" = "$KANNA_GHOSTTY_COMMIT" ] || return 0
  mkdir -p "$(dirname "$here")" && ln -s "$main" "$here"
}

kanna_main_checkout() {
  common="$(git -C "$1" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || return 1
  main="$(dirname "$common")"
  [ "$(cd "$main" && pwd -P)" != "$(cd "$1" && pwd -P)" ] || return 1
  echo "$main"
}

kanna_targets() {
  if main="$(kanna_main_checkout "$1")"; then worktree=1; else main="$1"; worktree=""; fi
  config="${KANNA_AGENTS_HOME:-$HOME}/.kanna-v4/config.json"
  root=""
  if [ -f "$config" ] && command -v plutil >/dev/null 2>&1 && [ "$(plutil -extract version raw -o - "$config" 2>/dev/null || echo 1)" -le 1 ] 2>/dev/null; then
    root="$(plutil -extract build.targets raw -o - "$config" 2>/dev/null)" || root=""
  fi
  root="${root%/}"
  case "$root" in
    ""|/*) ;;
    *) echo "==> build.targets in $config is $root, not an absolute folder: building in target/" >&2; root="" ;;
  esac
  case "$root" in
    /Volumes/*)
      volume="${root#/Volumes/}"; volume="/Volumes/${volume%%/*}"
      if [ ! -d "$volume" ] || [ "$(stat -L -f %d "$volume")" = "$(stat -f %d /Volumes)" ]; then
        echo "==> The volume ${volume#/Volumes/} isn't mounted: building in target/ on this disk until it is" >&2
        root=""
      fi
      ;;
  esac
  repo="$root/$(basename "$main")"
  if [ -n "$worktree" ]; then
    kanna_target_link "$1/target" "$repo/$(basename "$1")" "$root"
    return
  fi
  [ -n "$root" ] && mkdir -p "$1/target"
  for entry in "$1"/target/* "$1"/target/.[!.]*; do
    case "$(basename "$entry")" in *.app|ghosttykit|ghostty-vt|.offline) continue ;; esac
    if [ -L "$entry" ] || [ -d "$entry" ]; then
      kanna_target_link "$entry" "$repo/_main/$(basename "$entry")" "$root" || return 1
    fi
  done
  offline="$1/target/.offline"
  if [ -z "$root" ]; then
    if [ ! -e "$1/target/release" ] && [ -d "$offline/release" ]; then
      kanna_clone "$offline/release" "$1/target/release" && echo "==> Restored target/release from target/.offline" >&2
    fi
    if [ ! -e "$1/target/kanna-build" ] && [ -f "$offline/kanna-build/workspace-state.json" ]; then
      kanna_clone "$offline/kanna-build" "$1/target/kanna-build" && echo "==> Restored target/kanna-build's packages from target/.offline" >&2
    fi
  fi
  return 0
}

kanna_targets_offline() {
  kanna_main_checkout "$1" >/dev/null && return 0
  release="$1/target/release" kanna_build="$1/target/kanna-build" offline="$1/target/.offline"
  [ -L "$release" ] && [ -d "$release/.fingerprint" ] || return 0
  mkdir -p "$offline/kanna-build"
  rsync -a --delete "$release/" "$offline/release/" || return 1
  if [ -L "$kanna_build" ] && [ -f "$kanna_build/workspace-state.json" ]; then
    rsync -a --delete "$kanna_build/repositories" "$kanna_build/checkouts" "$offline/kanna-build/" &&
      cp -p "$kanna_build/workspace-state.json" "$offline/kanna-build/" || return 1
  fi
}

kanna_target_link() {
  if [ -z "$3" ]; then
    if [ -L "$1" ] && [ ! -e "$1" ]; then rm "$1"; fi
    return 0
  fi
  if [ -L "$1" ] && [ "$(readlink "$1")" = "$2" ]; then mkdir -p "$2"; return; fi
  mkdir -p "$(dirname "$2")" || return 1
  if [ -L "$1" ] && [ ! -e "$1" ]; then rm "$1"; fi
  if [ -e "$1" ]; then
    echo "==> Moving ${1#"$(dirname "$(dirname "$1")")/"} to $2" >&2
    if [ -L "$1" ]; then moving="$(readlink "$1")"; else moving="$1"; fi
    rm -rf "$2.moving" "$2.old"
    mv "$moving" "$2.moving" || return 1
    if [ -e "$2" ]; then mv "$2" "$2.old" || return 1; fi
    mv "$2.moving" "$2" || return 1
    if [ -L "$1" ]; then rm "$1"; fi
    rm -rf "$2.old"
  fi
  mkdir -p "$2" && ln -s "$2" "$1"
}

kanna_clone() {
  cp -c -R -p "$1" "$2" 2>/dev/null && return
  rm -rf "$2"
  cp -R -p "$1" "$2"
}

kanna_borrow_cargo() {
  borrow_triple="${2:-$(kanna_host_triple)}"
  here="$(kanna_arch_dir "$1" "$borrow_triple")/release"
  [ -e "$here" ] && return
  main="$(kanna_main_checkout "$1")" || return 0
  from="$(kanna_arch_dir "$main" "$borrow_triple")/release"
  if [ ! -d "$from/.fingerprint" ] && [ "$borrow_triple" = "$(kanna_host_triple)" ]; then from="$main/target/.offline/release"; fi
  [ -d "$from/.fingerprint" ] || return 0
  from="$(cd "$from" && pwd -P)"
  mkdir -p "$(dirname "$here")"
  if kanna_clone "$from" "$here.borrowing"; then
    rm -rf "$here.borrowing"/.fingerprint/kanna-*
    mv "$here.borrowing" "$here"
    echo "Borrowed $from's dependencies"
  else
    rm -rf "$here.borrowing"
  fi
}

kanna_borrow_swiftpm() {
  borrow_triple="${2:-$(kanna_host_triple)}"
  here="$(kanna_arch_dir "$1" "$borrow_triple")/kanna-build"
  [ -e "$here" ] && return
  main="$(kanna_main_checkout "$1")" || return 0
  from="$main/target/kanna-build"
  [ -f "$from/workspace-state.json" ] || from="$main/target/.offline/kanna-build"
  [ -f "$from/workspace-state.json" ] && [ -d "$from/checkouts" ] && [ -d "$from/repositories" ] || return 0
  mkdir -p "$here.borrowing"
  if kanna_clone "$from/repositories" "$here.borrowing/repositories" && kanna_clone "$from/checkouts" "$here.borrowing/checkouts" &&
     sed "s#$main/#$1/#g" "$from/workspace-state.json" > "$here.borrowing/workspace-state.json"; then
    mv "$here.borrowing" "$here"
    echo "Borrowed $from's Swift packages"
  else
    rm -rf "$here.borrowing"
  fi
}
