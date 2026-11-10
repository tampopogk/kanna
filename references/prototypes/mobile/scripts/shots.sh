#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
DEVICE="${KANNA_PROTO_DEVICE:-iPhone 17 Pro}"
BUNDLE=dev.kanna.prototype.mobile
OUT="${1:?usage: shots.sh OUTDIR [scene...]}"
shift
UDID=$(xcrun simctl list devices available | grep -F "$DEVICE (" | head -1 | sed -E 's/.*\(([0-9A-F-]{36})\).*/\1/')
SCENES=("$@")
if [ ${#SCENES[@]} -eq 0 ]; then
  SCENES=($(grep -oE 'key: "[a-z-]+"' KannaMobile/Scenes.swift | sed -E 's/key: "(.*)"/\1/'))
fi
xcrun simctl status_bar "$UDID" override --time 9:41 --batteryState charged --batteryLevel 100 --wifiBars 3 --cellularBars 4
for theme in ${THEMES:-dark light}; do
  mkdir -p "$OUT/$theme"
  for scene in "${SCENES[@]}"; do
    xcrun simctl launch --terminate-running-process "$UDID" "$BUNDLE" -scene "$scene" -hold YES -appearance "$theme" -score "${SCORE:-both}" >/dev/null
    sleep "${WAIT:-2}"
    xcrun simctl io "$UDID" screenshot --type=png "$OUT/$theme/$scene.png" 2>/dev/null
  done
done
xcrun simctl status_bar "$UDID" clear
