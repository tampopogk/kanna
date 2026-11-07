#!/usr/bin/env bash
set -euo pipefail
T="$1"
. "$(dirname "$0")/lib.sh"
identity="$(security find-identity -v -p codesigning | sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Developer ID Application: .*/\1/p' | head -1)"
build="$(( $(cat "$T/build-b") + 1 ))"
restore() { git -C "$repo_root" checkout -- crates/kanna-build/src/lib.rs apps/kanna/Sources/KannaMachines/Protocols.swift; }
trap restore EXIT
sed -i '' 's/^pub const PROTOCOL: u32 = 4;/pub const PROTOCOL: u32 = 5;/' "$repo_root/crates/kanna-build/src/lib.rs"
sed -i '' 's/public static let mine = 4/public static let mine = 5/' "$repo_root/apps/kanna/Sources/KannaMachines/Protocols.swift"
note "build C: protocol 5, build number $build"
env -u KANNA_SOCKET -u KANNA_TERMINAL -u KANNA_PANE KANNA_VERSION=4.92.0 KANNA_UPDATE_FEED="http://127.0.0.1:$port/feed.xml" KANNA_UPDATE_PUBLIC_KEY="$(cat "$T/test.key.pub")" "$repo_root/scripts/build.sh" >"$T/build-C.log" 2>&1
mkdir -p "$T/C"
rm -rf "$T/C/Kanna.app"
ditto "$repo_root/target/Kanna.app" "$T/C/Kanna.app"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build" "$T/C/Kanna.app/Contents/Info.plist"
kanna_sign_app "$repo_root" "$T/C/Kanna.app" "$identity" --options runtime --timestamp
stage="$T/stage-c"
rm -rf "$stage"; mkdir -p "$stage"
ditto "$T/C/Kanna.app" "$stage/Kanna.app"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname Kanna -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$served/Kanna-C.dmg"
codesign --force --sign "$identity" --timestamp "$served/Kanna-C.dmg"
echo "$build" >"$T/build-c"
