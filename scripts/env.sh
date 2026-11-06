#!/bin/sh
KANNA_GHOSTTY_FORK="${KANNA_GHOSTTY_FORK:-https://github.com/jemdiggity/ghostty.git}"
KANNA_GHOSTTY_BRANCH="kanna4"
KANNA_GHOSTTY_COMMIT="4e28443e32fe8b0f88934d6a1159c42561f1931a"

KANNA_UPDATE_FEEDS="https://kanna.build/appcast"
KANNA_UPDATE_PUBLIC_KEY="${KANNA_UPDATE_PUBLIC_KEY:-hjHTuszAUmqmKN6ANvmQQhTlzLs3Tm15WPEudwaMwgU=}"

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

kanna_rust_profile() {
  case "${KANNA_RUST_PROFILE:-}" in
    debug|release) echo "$KANNA_RUST_PROFILE"; return ;;
    "") ;;
    *) echo "KANNA_RUST_PROFILE is $KANNA_RUST_PROFILE: use debug or release" >&2; return 1 ;;
  esac
  if kanna_main_checkout "$1" >/dev/null; then echo debug; else echo release; fi
}

kanna_cargo_profile() {
  case "$(kanna_rust_profile "$1")" in debug) echo dev ;; release) echo release ;; *) return 1 ;; esac
}

kanna_rust_dir() {
  profile="$(kanna_rust_profile "$1")" || return 1
  echo "$(kanna_arch_dir "$1" "${2:-$(kanna_host_triple)}")/$profile"
}

kanna_cargo_stamp() {
  cargo_triple="${3:-$(kanna_host_triple)}"
  cargo_dir="$(kanna_rust_dir "$1" "$cargo_triple")" || return 1
  cargo_stamp="$cargo_dir/macos-deployment-target"
  [ "$(cat "$cargo_stamp" 2>/dev/null)" = "$2" ] && return
  if [ -d "$cargo_dir" ]; then
    echo "==> $cargo_dir was built for macOS $(cat "$cargo_stamp" 2>/dev/null || echo "the default"), not $2: cleaning it" >&2
    if [ "$cargo_triple" = "$(kanna_host_triple)" ]; then
      cargo clean --manifest-path "$1/Cargo.toml" --profile "$(kanna_cargo_profile "$1")" >&2 || return 1
    else
      cargo clean --manifest-path "$1/Cargo.toml" --profile "$(kanna_cargo_profile "$1")" --target "$cargo_triple" >&2 || return 1
    fi
  fi
  mkdir -p "$cargo_dir" && echo "$2" > "$cargo_stamp"
}

kanna_zig_build() {
  arch="$2" os="$3"
  name="zig-$arch-$os-$KANNA_ZIG_VERSION"
  eval "sum=\${KANNA_ZIG_SHA256_${arch}_$os:-}"
  [ -n "$sum" ] || { echo "no Zig download for $arch-$os" >&2; return 1; }
  echo "Downloading Zig $KANNA_ZIG_VERSION ($(zig version 2>/dev/null || echo "no zig") on PATH)"
  mkdir -p "$1.download" || return 1
  curl -fsSL "https://ziglang.org/download/$KANNA_ZIG_VERSION/$name.tar.xz" -o "$1.download/$name.tar.xz" &&
    echo "$sum  $1.download/$name.tar.xz" | shasum -a 256 -c - >/dev/null &&
    tar -xJf "$1.download/$name.tar.xz" -C "$1.download" &&
    mv "$1.download/$name" "$1"
  status=$?
  rm -rf "$1.download"
  [ "$status" = 0 ] || echo "Zig download failed or its checksum didn't match" >&2
  return "$status"
}

kanna_zig() {
  if command -v zig >/dev/null 2>&1 && [ "$(zig version)" = "$KANNA_ZIG_VERSION" ]; then
    command -v zig
    return
  fi
  arch="$(uname -m)"; [ "$arch" = arm64 ] && arch=aarch64
  case "$(uname -s)" in Darwin) os=macos ;; Linux) os=linux ;; *) echo "no Zig download for $(uname -s)" >&2; return 1 ;; esac
  entry="$(kanna_store "$1" zig "$(kanna_store_key "$KANNA_ZIG_VERSION" "$arch" "$os")" kanna_zig_build "$arch" "$os")" || return 1
  echo "$entry/zig"
}

kanna_ghostty_source_build() {
  zig="$(kanna_zig "$2")" || return 1
  git clone -q --no-checkout --filter=blob:none --branch "$KANNA_GHOSTTY_BRANCH" "$KANNA_GHOSTTY_FORK" "$1" &&
    git -C "$1" checkout -q --detach "$KANNA_GHOSTTY_COMMIT" &&
    (cd "$1" && "$zig" build --fetch=all --cache-dir "$1.fetch-cache")
  status=$?
  rm -rf "$1.fetch-cache"
  return "$status"
}

kanna_ghostty_source() {
  if [ -n "${GHOSTTY_SOURCE_DIR:-}" ]; then
    echo "$GHOSTTY_SOURCE_DIR"
    return
  fi
  kanna_store "$1" ghostty-source "$(kanna_store_key "$KANNA_GHOSTTY_FORK" "$KANNA_GHOSTTY_COMMIT" "$KANNA_ZIG_VERSION")" kanna_ghostty_source_build "$1"
}

kanna_ghostty_rev() {
  rev="$(git -C "$1" rev-parse HEAD 2>/dev/null)" || { echo unknown; return; }
  if [ -n "$(git -C "$1" status --porcelain 2>/dev/null)" ]; then rev="$rev-dirty"; fi
  echo "$rev"
}

kanna_ghostty_key() {
  kanna_store_key "$KANNA_GHOSTTY_FORK" "$KANNA_GHOSTTY_COMMIT" "$KANNA_ZIG_VERSION" "${KANNA_GHOSTTY_OPTIMIZE:-ReleaseSafe}" "$(kanna_file_hash "$1/scripts/$2")" "${3:-}"
}

kanna_ghosttykit_store_build() {
  "$2/scripts/build-ghosttykit.sh" --into "$1"
}

kanna_ghostty_vt_store_build() {
  "$2/scripts/build-ghostty-vt.sh" "$3" --into "$1"
}

kanna_ghosttykit() {
  here="$1/target/ghosttykit"
  if [ -n "${GHOSTTY_SOURCE_DIR:-}" ]; then
    if [ -L "$here" ]; then rm "$here"; fi
    "$1/scripts/build-ghosttykit.sh" --into "$here"
    return
  fi
  entry="$(kanna_store "$1" ghosttykit "$(kanna_ghostty_key "$1" build-ghosttykit.sh)" kanna_ghosttykit_store_build "$1")" || return 1
  kanna_store_use "$entry" "$here"
}

kanna_ghostty_vt() {
  here="$1/target/ghostty-vt/$2"
  if [ -n "${GHOSTTY_SOURCE_DIR:-}" ]; then
    if [ -L "$here" ]; then rm "$here"; fi
    "$1/scripts/build-ghostty-vt.sh" "$2" --into "$here"
    return
  fi
  entry="$(kanna_store "$1" ghostty-vt "$(kanna_ghostty_key "$1" build-ghostty-vt.sh "$2")" kanna_ghostty_vt_store_build "$1" "$2")" || return 1
  kanna_store_use "$entry" "$here"
}

kanna_main_checkout() {
  common="$(git -C "$1" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || return 1
  main="$(dirname "$common")"
  [ "$(cd "$main" && pwd -P)" != "$(cd "$1" && pwd -P)" ] || return 1
  echo "$main"
}

kanna_targets_config() {
  checkout="$(kanna_main_checkout "$1")" || checkout="$(cd "$1" && pwd -P)"
  echo "$checkout/build.local.json"
}

kanna_targets_setting() {
  command -v plutil >/dev/null 2>&1 || return 1
  config="$(kanna_targets_config "$1")"
  if [ -f "$config" ]; then
    if git -C "$(dirname "$config")" ls-files --error-unmatch build.local.json >/dev/null 2>&1; then
      echo "==> $config is committed to git: ignoring it, since it says what builds may delete" >&2
    elif value="$(plutil -extract "$2" raw -o - "$config" 2>/dev/null)"; then
      echo "$value"
      return 0
    fi
  fi
  [ "$2" != targets ] && [ -f "$1/build.json" ] && plutil -extract "$2" raw -o - "$1/build.json" 2>/dev/null
}

kanna_targets_root() {
  root="$(kanna_targets_setting "$1" targets)" || root=""
  echo "${root%/}"
}

kanna_targets_unmounted() {
  case "$1" in
    /Volumes/*)
      volume="${1#/Volumes/}"; volume="/Volumes/${volume%%/*}"
      if [ ! -d "$volume" ] || [ "$(stat -L -f %d "$volume")" = "$(stat -f %d /Volumes)" ]; then
        echo "${volume#/Volumes/}"
        return 0
      fi
      ;;
  esac
  return 1
}

kanna_targets() {
  if main="$(kanna_main_checkout "$1")"; then worktree=1; else main="$1"; worktree=""; fi
  root="$(kanna_targets_root "$1")"
  case "$root" in
    ""|/*) ;;
    *) echo "==> targets in $(kanna_targets_config "$1") is $root, not an absolute folder: building in target/" >&2; root="" ;;
  esac
  if [ -n "$root" ] && volume="$(kanna_targets_unmounted "$root")"; then
    echo "==> The volume $volume isn't mounted: building in target/ on this disk until it is" >&2
    root=""
  fi
  repo="$root/$(basename "$main")"
  if [ -n "$worktree" ]; then
    kanna_target_link "$1/target" "$repo/$(basename "$1")" "$root" || return 1
    if [ -n "$root" ]; then touch "$repo/$(basename "$1")/.used"; fi
    if command -v kanna_targets_ready >/dev/null 2>&1; then kanna_targets_ready "$1"; fi
    return 0
  fi
  [ -n "$root" ] && mkdir -p "$1/target"
  for entry in "$1"/target/* "$1"/target/.[!.]*; do
    case "$(basename "$entry")" in *.app|ghosttykit|ghostty-vt|_shared) continue ;; esac
    if [ -L "$entry" ] || [ -d "$entry" ]; then
      kanna_target_link "$entry" "$repo/_main/$(basename "$entry")" "$root" || return 1
    fi
  done
  if command -v kanna_targets_ready >/dev/null 2>&1; then kanna_targets_ready "$1"; fi
  return 0
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

kanna_remove() {
  if [ -L "$1" ]; then rm "$1"; return; fi
  [ -e "$1" ] || return 0
  chmod -R u+w "$1" 2>/dev/null
  rm -rf "$1"
}

kanna_clone() {
  kanna_remove "$2" || return 1
  if cp -c -R -p "$1" "$2" 2>/dev/null || { kanna_remove "$2" && cp -R -p "$1" "$2"; }; then
    chmod -R u+w "$2" && return
  fi
  kanna_remove "$2"
  return 1
}

kanna_borrow_swiftpm() {
  borrow_triple="${2:-$(kanna_host_triple)}"
  here="$(kanna_arch_dir "$1" "$borrow_triple")/kanna-build"
  [ -e "$here" ] && return
  main="$(kanna_main_checkout "$1")" || return 0
  from="$main/target/kanna-build"
  [ -f "$from/workspace-state.json" ] && [ -d "$from/checkouts" ] && [ -d "$from/repositories" ] && [ -d "$from/artifacts" ] || return 0
  mkdir -p "$here.borrowing"
  if kanna_clone "$from/repositories" "$here.borrowing/repositories" && kanna_clone "$from/checkouts" "$here.borrowing/checkouts" && kanna_clone "$from/artifacts" "$here.borrowing/artifacts" &&
     sed -e "s#$main/#$1/#g" -e "s#$1/target/kanna-build/#$here/#g" "$from/workspace-state.json" > "$here.borrowing/workspace-state.json"; then
    mv "$here.borrowing" "$here"
    echo "Borrowed $from's Swift packages"
  else
    rm -rf "$here.borrowing"
  fi
}

kanna_dev_identity() {
  security find-identity -p codesigning 2>/dev/null | sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Kanna Dev Signing".*/\1/p' | head -1
}

kanna_signed() {
  [ "$3" != - ] && sign_requirement="$(codesign -d -r- "$1" 2>&1)" && printf '%s\n' "$sign_requirement" | grep -qx "designated => identifier \"$2\" and certificate leaf = H\"$(printf '%s' "$3" | tr A-F a-f)\""
}

kanna_sign_helper() {
  sign_bundle_id="$1" sign_identity="$2" sign_helper="$3"
  shift 3
  sign_identifier="$sign_bundle_id.$(basename "$sign_helper")"
  if [ $# -eq 0 ] && kanna_signed "$sign_helper" "$sign_identifier" "$sign_identity"; then return 0; fi
  codesign --force --sign "$sign_identity" --identifier "$sign_identifier" "$@" "$sign_helper"
}

kanna_sign_app() {
  sign_root="$1" sign_app="$2" sign_identity="$3"
  shift 3
  sign_bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$sign_app/Contents/Info.plist")" || return 1
  for sign_file in "$sign_app"/Contents/Helpers/*; do
    file -b "$sign_file" | grep -q Mach-O || continue
    kanna_sign_helper "$sign_bundle_id" "$sign_identity" "$sign_file" "$@" || return 1
  done
  sign_sparkle="$sign_app/Contents/Frameworks/Sparkle.framework"
  if [ -d "$sign_sparkle" ]; then
    for sign_file in "$sign_sparkle/Versions/B/Autoupdate" "$sign_sparkle/Versions/B/Updater.app" "$sign_sparkle"; do
      codesign --force --sign "$sign_identity" "$@" "$sign_file" || return 1
    done
  fi
  codesign --force --sign "$sign_identity" --identifier "$sign_bundle_id" "$@" --entitlements "$sign_root/apps/kanna/Resources/Kanna.entitlements" "$sign_app"
}
