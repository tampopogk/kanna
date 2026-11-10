#!/bin/sh
set -eu
repo_root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$repo_root/target/kanna-vm/kanna-vm}
mkdir -p "$(dirname "$out")"
xcrun swiftc -O "$repo_root/apps/kanna-vm/kanna-vm.swift" -o "$out"
codesign --force --sign - --entitlements "$repo_root/apps/kanna-vm/kanna-vm.entitlements" "$out"
echo "$out"
