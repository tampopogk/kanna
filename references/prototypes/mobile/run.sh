#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
DEVICE="${KANNA_PROTO_DEVICE:-iPhone 17 Pro}"
BUNDLE=dev.kanna.prototype.mobile
UDID=$(xcrun simctl list devices available | grep -F "$DEVICE (" | head -1 | sed -E 's/.*\(([0-9A-F-]{36})\).*/\1/')
[ -n "$UDID" ] || { echo "No simulator named $DEVICE; set KANNA_PROTO_DEVICE" >&2; exit 1; }
xcrun simctl boot "$UDID" 2>/dev/null || true
xcrun simctl bootstatus "$UDID" -b >/dev/null
xcodebuild -project KannaMobile.xcodeproj -scheme KannaMobile -destination "platform=iOS Simulator,id=$UDID" -derivedDataPath build/dd -quiet build
xcrun simctl install "$UDID" build/dd/Build/Products/Debug-iphonesimulator/KannaMobile.app
[ "${KANNA_PROTO_NO_OPEN:-}" = 1 ] || open -a Simulator --args -CurrentDeviceUDID "$UDID"
xcrun simctl launch --terminate-running-process "$UDID" "$BUNDLE" "$@"
