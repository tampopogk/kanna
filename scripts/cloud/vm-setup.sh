#!/usr/bin/env bash
set -euo pipefail
exec </dev/null

name="${1:?usage: vm-setup.sh NAME TARBALL|latest AGENTS RELAY-URL}"
tarball="${2:?}"
agents="${3:?}"
relay="${4:?}"
dir=/var/lib/kanna-cloud
fail() { echo "vm-setup: $*" >&2; exit 1; }
step() { echo "==> $*"; }

[[ "$(id -un)" == kanna ]] || fail "run as kanna"
export HOME=/home/kanna XDG_RUNTIME_DIR="/run/user/$(id -u)"
cd "$HOME"

if [[ "$tarball" == latest ]]; then
  step "The latest Kanna release"
  url="$(curl -fsSL https://api.github.com/repos/tampopogk/kanna/releases/latest | python3 -c 'import json, sys
print(next(a["browser_download_url"] for a in json.load(sys.stdin)["assets"] if a["name"].endswith("-linux-x86_64.tar.gz")))')" || fail "no Linux tarball in the latest release"
  tarball="$dir/$(basename "$url")"
  curl -fsSL -o "$tarball" "$url"
fi

bash "$dir/server-setup.sh" --tarball "$tarball" --agents "$agents"
rm -f "$tarball"

kanna() { KANNA_SOCKET="$HOME/.local/share/kanna-prod/kanna.sock" /opt/kanna/current/kanna "$@"; }
step "This machine: $name, meeting your machines through $relay"
kanna machine name "$name" >/dev/null
if [[ -s "$dir/relay-token" ]]; then
  kanna machine relay "$relay" --token-file "$dir/relay-token" >/dev/null
else
  kanna machine relay "$relay" >/dev/null
fi
rm -f "$dir/relay-token"
for _ in $(seq 100); do
  kanna machine list --json | python3 -c 'import json, sys; sys.exit(0 if json.load(sys.stdin).get("home_relay") else 1)' && break
  sleep 0.3
done
kanna machine list --json | python3 -c 'import json, sys; sys.exit(0 if json.load(sys.stdin).get("home_relay") else 1)' || fail "the daemon never reached the relay $relay"
step "Ready to enroll"
