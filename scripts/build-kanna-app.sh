#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$repo_root/scripts/env.sh"
. "$repo_root/scripts/store.sh"
kanna_targets "$repo_root" || exit 1
usage() { echo "usage: scripts/build-kanna-app.sh [production|staging|dev] [--arch arm64|x86_64]   (default: dev, or \$KANNA_CHANNEL; this Mac's architecture)" >&2; exit 2; }
channel="${KANNA_CHANNEL:-dev}"
arch=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    production|staging|dev) channel="$1" ;;
    --arch) [[ $# -ge 2 ]] || usage; arch="$2"; shift ;;
    --arch=*) arch="${1#--arch=}" ;;
    *) usage ;;
  esac
  shift
done
case "$channel" in production|staging|dev) export KANNA_CHANNEL="$channel" ;; *) usage ;; esac
macos_target="$(kanna_macos_target "$repo_root")"
host="$(kanna_host_triple)"
if [[ -n "$arch" ]]; then triple="$(kanna_arch_triple "$arch")" || usage; else triple="$host"; fi
arch_dir="$(kanna_arch_dir "$repo_root" "$triple")"
rust_dir="$(kanna_rust_dir "$repo_root" "$triple")"
host_rust_dir="$(kanna_rust_dir "$repo_root")"
cargo_profile="$(kanna_cargo_profile "$repo_root")"
kanna_rust_target "$triple"
if [[ -n "${GHOSTTY_SOURCE_DIR:-}" || ! -d "$repo_root/target/ghosttykit/GhosttyKit.xcframework" || "$(cat "$repo_root/target/ghosttykit/ghostty.rev" 2>/dev/null)" != "$KANNA_GHOSTTY_COMMIT" ]]; then
  kanna_ghosttykit "$repo_root"
fi
built="$(cat "$repo_root/target/ghosttykit/ghostty.rev" 2>/dev/null || true)"
if [[ "$built" != "$KANNA_GHOSTTY_COMMIT" ]]; then
  if [[ -z "${GHOSTTY_SOURCE_DIR:-}" ]]; then
    echo "build-kanna-app: GhosttyKit is Ghostty ${built:-unknown}, but scripts/env.sh pins $KANNA_GHOSTTY_COMMIT" >&2
    exit 1
  fi
  echo "note: GhosttyKit is Ghostty $built from GHOSTTY_SOURCE_DIR, not the pinned $KANNA_GHOSTTY_COMMIT" >&2
fi

if [[ -z "${KANNA_GHOSTTY_VT_DIR:-}" ]]; then
  ghostty_vt="$repo_root/target/ghostty-vt/$triple"
  if [[ -n "${GHOSTTY_SOURCE_DIR:-}" || ! -f "$ghostty_vt/lib/libghostty-vt.a" || "$(cat "$ghostty_vt/ghostty.rev" 2>/dev/null)" != "$KANNA_GHOSTTY_COMMIT" ]]; then
    kanna_ghostty_vt "$repo_root" "$triple"
  fi
fi

kanna_cargo_stamp "$repo_root" "$macos_target"
if [[ "$triple" == "$host" ]]; then
  (cd "$repo_root" && cargo build --profile "$cargo_profile" -p kanna-ffi -p kanna-daemon -p kanna-keeper -p kanna -p kanna-build)
else
  kanna_cargo_stamp "$repo_root" "$macos_target" "$triple"
  (cd "$repo_root" && cargo build --profile "$cargo_profile" -p kanna-build &&
    cargo build --profile "$cargo_profile" --target "$triple" -p kanna-ffi -p kanna-daemon -p kanna-keeper -p kanna)
fi

if [[ "$KANNA_CHANNEL" == dev ]]; then configuration="${KANNA_CONFIGURATION:-debug}"; else configuration="${KANNA_CONFIGURATION:-release}"; fi
while IFS='=' read -r key value; do
  case "$key" in
    channel) channel="$value" ;; name) app_name="$value" ;; bundle_id) bundle_id="$value" ;; instance) instance="$value" ;; folder) folder="$value" ;; version) version="$value" ;; commit) commit="$value" ;;
  esac
done < <("$host_rust_dir/kanna-build")
build_number="$(git -C "$repo_root" rev-list --count HEAD 2>/dev/null || echo 0)"
kanna_borrow_swiftpm "$repo_root" "$triple"
build=(swift build --package-path "$repo_root/apps/kanna" --configuration "$configuration" --scratch-path "$arch_dir/kanna-build")
[[ "$triple" == "$host" ]] || build+=(--arch "$(kanna_triple_arch "$triple")")
export KANNA_RUST_DIR="$rust_dir"
product_dir="$("${build[@]}" --show-bin-path)"
[[ "$rust_dir/libkanna_ffi.a" -nt "$product_dir/Kanna" ]] && rm -f "$product_dir/Kanna"
"${build[@]}"

app="$arch_dir/Kanna.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Resources"
cp "$product_dir/Kanna" "$app/Contents/MacOS/Kanna"
cp "$rust_dir/kanna-daemon" "$rust_dir/kanna-keeper" "$rust_dir/kanna" "$app/Contents/Helpers/"
cp "$repo_root/assets/Kanna.icns" "$app/Contents/Resources/Kanna.icns"
cp "$repo_root/LICENSE" "$app/Contents/Resources/"
install -m 755 "$repo_root/apps/kanna/Resources/planner.sh" "$app/Contents/Resources/"
mkdir -p "$app/Contents/Resources/kanna-skills"
for skill in kanna-tasks kanna-planning kanna-new-project kanna-markup kanna-explain kanna-quiz; do cp -RL "$repo_root/.agents/kanna-skills/skills/$skill" "$app/Contents/Resources/kanna-skills/"; done

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Kanna</string>
<key>CFBundleIconFile</key><string>Kanna</string>
<key>CFBundleIdentifier</key><string>$bundle_id</string>
<key>CFBundleName</key><string>$app_name</string>
<key>CFBundleDisplayName</key><string>$app_name</string>
<key>KannaChannel</key><string>$channel</string>
<key>KannaInstance</key><string>$instance</string>
<key>KannaCommit</key><string>$commit</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>${version%%-*}</string>
<key>CFBundleVersion</key><string>$build_number</string>
<key>NSAppleEventsUsageDescription</key><string>A program running in Kanna would like to use AppleScript.</string>
<key>NSMicrophoneUsageDescription</key><string>A program running in Kanna would like to use your microphone.</string>
<key>NSLocalNetworkUsageDescription</key><string>Kanna connects to your other machines on this network to run and show their tasks.</string>
<key>NSBonjourServices</key><array><string>_kanna._udp</string></array>
<key>LSMinimumSystemVersion</key><string>$macos_target</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
echo "Built $app: $app_name $version ($bundle_id), $channel channel, $(kanna_triple_arch "$triple"), instance $instance in $folder"
