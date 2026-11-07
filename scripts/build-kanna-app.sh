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
kanna_ghosttykit "$repo_root" || exit 1
built="$(cat "$repo_root/target/ghosttykit/ghostty.rev" 2>/dev/null || true)"
if [[ "$built" != "$KANNA_GHOSTTY_COMMIT" ]]; then
  if [[ -z "${GHOSTTY_SOURCE_DIR:-}" ]]; then
    echo "build-kanna-app: GhosttyKit is Ghostty ${built:-unknown}, but scripts/env.sh pins $KANNA_GHOSTTY_COMMIT" >&2
    exit 1
  fi
  echo "note: GhosttyKit is Ghostty $built from GHOSTTY_SOURCE_DIR, not the pinned $KANNA_GHOSTTY_COMMIT" >&2
fi

if [[ -z "${KANNA_GHOSTTY_VT_DIR:-}" ]]; then
  kanna_ghostty_vt "$repo_root" "$triple" || exit 1
fi

kanna_cargo_seed "$repo_root" "" "$macos_target"
kanna_cargo_stamp "$repo_root" "$macos_target"
if [[ "$triple" == "$host" ]]; then
  (cd "$repo_root" && cargo build --profile "$cargo_profile" -p kanna-ffi -p kanna-daemon -p kanna-keeper -p kanna -p kanna-build)
else
  kanna_cargo_seed "$repo_root" "$triple" "$macos_target"
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

identity="$(kanna_dev_identity)"
if [[ -n "$identity" ]]; then signing="signed as Kanna Dev Signing"; else identity=-; signing="ad-hoc signed: macOS asks for privacy permissions again after each build; scripts/dev-signing-identity.sh makes an identity that keeps them"; fi
signed="$arch_dir/signed"
mkdir -p "$signed"
for helper in kanna-daemon kanna-keeper kanna; do
  stamp="$(shasum -a 256 "$rust_dir/$helper" | cut -d' ' -f1) $bundle_id.$helper $identity"
  if [[ ! -f "$signed/$helper" || "$(cat "$signed/$helper.stamp" 2>/dev/null)" != "$stamp" ]]; then
    cp "$rust_dir/$helper" "$signed/$helper"
    kanna_sign_helper "$bundle_id" "$identity" "$signed/$helper"
    echo "$stamp" >"$signed/$helper.stamp"
  fi
done

app="$arch_dir/Kanna.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp "$product_dir/Kanna" "$app/Contents/MacOS/Kanna"
cp "$signed/kanna-daemon" "$signed/kanna-keeper" "$signed/kanna" "$app/Contents/Helpers/"
ditto "$product_dir/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
rm -rf "$app/Contents/Frameworks/Sparkle.framework/XPCServices" "$app/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices"
cp "$repo_root/assets/Kanna.icns" "$app/Contents/Resources/Kanna.icns"
cp "$repo_root/LICENSE" "$app/Contents/Resources/"
install -m 755 "$repo_root/apps/kanna/Resources/planner.sh" "$app/Contents/Resources/"
mkdir -p "$app/Contents/Resources/kanna-skills"
for skill in kanna-tasks kanna-planning kanna-new-project kanna-markup kanna-explain kanna-quiz; do cp -RL "$repo_root/.agents/kanna-skills/skills/$skill" "$app/Contents/Resources/kanna-skills/"; done

case "$channel" in
  production|staging) update_feed="$KANNA_UPDATE_FEEDS/$channel-$(kanna_triple_arch "$triple").xml" ;;
  *) update_feed="" ;;
esac
update_feed="${KANNA_UPDATE_FEED:-$update_feed}"
update_plist=""
if [[ -n "$update_feed" ]]; then
  update_plist="<key>SUFeedURL</key><string>$update_feed</string>
<key>SUPublicEDKey</key><string>$KANNA_UPDATE_PUBLIC_KEY</string>
<key>SUEnableAutomaticChecks</key><true/>
<key>SUScheduledCheckInterval</key><integer>21600</integer>
<key>SUAutomaticallyUpdate</key><false/>
<key>SUAllowsAutomaticUpdates</key><false/>
<key>SUVerifyUpdateBeforeExtraction</key><true/>
<key>SURequireSignedFeed</key><true/>"
  [[ "$update_feed" == http://127.0.0.1* ]] && update_plist+="
<key>NSAppTransportSecurity</key><dict><key>NSAllowsLocalNetworking</key><true/></dict>"
fi

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
<key>KannaVersion</key><string>$version</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>${version%%-*}</string>
<key>CFBundleVersion</key><string>$build_number</string>
<key>NSAppleEventsUsageDescription</key><string>A program running in Kanna would like to use AppleScript.</string>
<key>NSMicrophoneUsageDescription</key><string>A program running in Kanna would like to use your microphone.</string>
<key>NSLocalNetworkUsageDescription</key><string>Kanna connects to your other machines on this network to run and show their tasks.</string>
<key>NSBonjourServices</key><array><string>_kanna._udp</string></array>
<key>LSMinimumSystemVersion</key><string>$macos_target</string>
<key>NSHighResolutionCapable</key><true/>
$update_plist
</dict></plist>
PLIST
kanna_sign_app "$repo_root" "$app" "$identity"
echo "Built $app: $app_name $version ($bundle_id), $channel channel, $(kanna_triple_arch "$triple"), instance $instance in $folder, $signing"
