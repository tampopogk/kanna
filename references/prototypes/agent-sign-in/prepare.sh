#!/usr/bin/env bash
# Run from the Mac. Dedicated model resources only; no existing profiles changed.
set -euo pipefail
host=${1:-jeremyhale@172.16.0.105}
here=$(cd -- "$(dirname -- "$0")" && pwd)
ssh "$host" 'bash -s' <<'REMOTE'
set -euo pipefail
sudo -n incus storage show auth-probe >/dev/null 2>&1 || sudo -n incus storage create auth-probe dir </dev/null
sudo -n incus network show auth-probe0 >/dev/null 2>&1 || sudo -n incus network create auth-probe0 \
  ipv4.address=10.199.64.1/24 ipv4.nat=true ipv6.address=none </dev/null
for vm in agent-sign-in-a agent-sign-in-b; do
  if ! sudo -n incus info "$vm" >/dev/null 2>&1; then
    sudo -n incus launch images:ubuntu/26.04/cloud "$vm" --vm \
      --storage auth-probe --network auth-probe0 \
      -c limits.cpu=2 -c limits.memory=4GiB -d root,size=12GiB </dev/null
  fi
  ready=false
  for attempt in $(seq 1 90); do
    if sudo -n incus exec "$vm" -- true </dev/null 2>/dev/null; then ready=true; break; fi
    sleep 2
  done
  "$ready" || { echo "VM agent did not start: $vm" >&2; exit 1; }
  sudo -n incus exec "$vm" -- bash -s <<'GUEST'
set -euo pipefail
cloud-init status --wait
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends python3 curl ca-certificates nodejs npm
id probe >/dev/null 2>&1 || useradd --create-home --shell /bin/bash probe
mkdir -p /opt/agent-sign-in
npm install -g @openai/codex@0.163.0 @anthropic-ai/claude-code@2.1.310
codex --version
claude --version
GUEST
done
REMOTE
for vm in agent-sign-in-a agent-sign-in-b; do
  ssh "$host" sudo -n incus exec "$vm" -- tee /opt/agent-sign-in/worker.py < "$here/worker.py" >/dev/null
  ssh "$host" sudo -n incus exec "$vm" -- tee /opt/agent-sign-in/external.py < "$here/external.py" >/dev/null
done
ssh "$host" sudo -n incus list --format csv -c ns4
