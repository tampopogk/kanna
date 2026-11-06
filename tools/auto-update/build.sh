#!/usr/bin/env bash
set -euo pipefail
T="$1"
. "$(dirname "$0")/lib.sh"
mkdir -p "$T/A" "$T/B" "$served"
cat >"$T/key.swift" <<'SWIFT'
import CryptoKit
import Foundation
let key = Curve25519.Signing.PrivateKey()
print(key.rawRepresentation.base64EncodedString())
print(key.publicKey.rawRepresentation.base64EncodedString())
SWIFT
keypair() { (umask 077; swift "$T/key.swift" >"$1.both"); sed -n 1p "$1.both" >"$1"; sed -n 2p "$1.both" >"$1.pub"; rm "$1.both"; }
keypair "$T/test.key"
keypair "$T/other.key"
identity="$(security find-identity -v -p codesigning | sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Developer ID Application: .*/\1/p' | head -1)"
[[ -n "$identity" ]] || fail "no Developer ID identity"
count="$(git -C "$repo_root" rev-list --count HEAD)"
one() {
  local name="$1" version="$2" build="$3"
  note "build $name: Kanna $version, build number $build"
  env -u KANNA_SOCKET -u KANNA_TERMINAL -u KANNA_PANE KANNA_VERSION="$version" KANNA_UPDATE_FEED="http://127.0.0.1:$port/feed.xml" KANNA_UPDATE_PUBLIC_KEY="$(cat "$T/test.key.pub")" "$repo_root/scripts/build.sh" >"$T/build-$name.log" 2>&1
  ditto "$repo_root/target/Kanna.app" "$T/$name/Kanna.app"
  /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build" "$T/$name/Kanna.app/Contents/Info.plist"
  kanna_sign_app "$repo_root" "$T/$name/Kanna.app" "$identity" --options runtime --timestamp
  codesign --verify --strict --deep "$T/$name/Kanna.app"
}
one A 4.90.0 "$count"
one B 4.91.0 "$((count + 1))"
for helper in kanna-daemon kanna-keeper; do
  [[ "$(shasum -a 256 <"$T/A/Kanna.app/Contents/Helpers/$helper")" != "$(shasum -a 256 <"$T/B/Kanna.app/Contents/Helpers/$helper")" ]] || fail "$helper is the same build in A and B"
done
note "DMG of B"
stage="$T/stage"
rm -rf "$stage"; mkdir -p "$stage"
ditto "$T/B/Kanna.app" "$stage/Kanna.app"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname Kanna -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$served/Kanna-B.dmg"
codesign --force --sign "$identity" --timestamp "$served/Kanna-B.dmg"
echo "$((count + 1))" >"$T/build-b"
ls -la "$served"
