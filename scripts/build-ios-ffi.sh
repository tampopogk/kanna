#!/bin/bash
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
profile="${1:-release}"
out="${2:-$repo_root/target/ios/KannaIOS.xcframework}"
export IPHONEOS_DEPLOYMENT_TARGET=18.0
target_dir="$(cargo metadata --manifest-path "$repo_root/Cargo.toml" --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
args=()
for triple in aarch64-apple-ios aarch64-apple-ios-sim; do
  cargo build --manifest-path "$repo_root/Cargo.toml" --profile "$profile" --target "$triple" -p kanna-ios-ffi
  args+=(-library "$target_dir/$triple/$profile/libkanna_ios_ffi.a" -headers "$repo_root/crates/kanna-ios-ffi/include")
done
rm -rf "$out"
mkdir -p "$(dirname "$out")"
xcodebuild -create-xcframework "${args[@]}" -output "$out"
