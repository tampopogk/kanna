#!/usr/bin/env bash
set -euo pipefail
. "$(dirname "$0")/env.sh"
exec </dev/null

systemctl stop "ctv-rollback*.timer" 2>/dev/null || true
systemctl reset-failed ctv-rollback.service ctv-rollback.timer 2>/dev/null || true
systemd-run --unit=ctv-rollback --on-active="${ROLLBACK_MIN}min" "$DIR/teardown.sh" >/dev/null
echo "rollback armed: teardown in ${ROLLBACK_MIN} min (systemctl stop ctv-rollback.timer to keep)"

lsmod | grep -q "^br_netfilter" || { modprobe br_netfilter; touch /run/ctv-loaded-br_netfilter; }

incus storage show "$POOL" >/dev/null 2>&1 || incus storage create "$POOL" lvm size=60GiB

incus network acl show "$ACL" >/dev/null 2>&1 || {
  incus network acl create "$ACL"
  incus network acl rule add "$ACL" egress action=drop \
    destination=0.0.0.0/8,10.0.0.0/8,100.64.0.0/10,127.0.0.0/8,169.254.0.0/16,172.16.0.0/12,192.0.0.0/24,192.168.0.0/16,198.18.0.0/15,224.0.0.0/4,240.0.0.0/4
  incus network acl rule add "$ACL" egress action=drop \
    destination=::/128,::1/128,64:ff9b::/96,100::/64,fc00::/7,fe80::/10,fec0::/10,ff00::/8
  incus network acl rule add "$ACL" egress action=allow
}

incus network show "$NET" >/dev/null 2>&1 || incus network create "$NET" \
  ipv4.address="$V4" ipv4.nat=true ipv4.dhcp=true \
  ipv6.address="$V6" ipv6.nat=true ipv6.dhcp=true ipv6.dhcp.stateful=true \
  dns.mode=none \
  security.acls="$ACL" \
  security.acls.default.egress.action=reject \
  security.acls.default.ingress.action=drop

incus profile show "$PROFILE" >/dev/null 2>&1 || {
  incus profile create "$PROFILE"
  incus profile set "$PROFILE" limits.cpu=2 limits.memory=2GiB security.secureboot=false
  incus profile device add "$PROFILE" root disk pool="$POOL" path=/ size=12GiB
  incus profile device add "$PROFILE" eth0 nic network="$NET" name=eth0 \
    security.mac_filtering=true security.ipv4_filtering=true security.ipv6_filtering=true \
    security.port_isolation=true
}

for vm in "$@"; do
  incus info "$vm" >/dev/null 2>&1 && continue
  incus init "$IMAGE" "$vm" --vm --no-profiles --profile "$PROFILE" >/dev/null
  incus start "$vm"
done
for vm in "$@"; do
  for _ in $(seq 90); do incus exec "$vm" -- true 2>/dev/null && break; sleep 2; done
  incus exec "$vm" -- cloud-init status --wait >/dev/null 2>&1 || true
done
incus list -f csv -c n46 | grep '^ctv-' || true
