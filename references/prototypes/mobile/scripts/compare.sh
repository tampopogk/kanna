#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${1:?usage: compare.sh OUTDIR, with shots in OUTDIR/shots}"
for theme in dark light; do
  python3 scripts/mock_grid.py ../../mockups/mobile.html "$theme" "$OUT/mock-$theme.html"
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu --user-data-dir="$OUT/chrome" --hide-scrollbars --force-device-scale-factor=1 --window-size=3312,4340 --screenshot="$OUT/mock-$theme.png" "file://$(cd "$OUT" && pwd)/mock-$theme.html" 2>/dev/null
  python3 scripts/compare.py "$OUT/mock-$theme.png" "$OUT/shots/$theme" "$OUT" "$theme"
done
