#!/bin/sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/vmrun}
xcrun swiftc -O "$here/vmrun.swift" -o "$out"
codesign --force --sign - --entitlements "$here/vmrun.entitlements" "$out"
