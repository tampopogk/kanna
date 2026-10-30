#!/usr/bin/env bash
set -u
. "$(dirname "$0")/env.sh"
exec </dev/null
for vm in $(incus list -f csv -c n | grep '^ctv-'); do incus delete --force "$vm"; done
for vol in $(incus storage volume list "$POOL" -f csv -c tn type=custom 2>/dev/null | cut -d, -f2); do
  incus storage volume delete "$POOL" "$vol"
done
incus profile delete "$PROFILE" 2>/dev/null
incus network delete "$NET" 2>/dev/null
incus network acl delete "$ACL" 2>/dev/null
incus storage delete "$POOL" 2>/dev/null
nft delete table bridge ctv_guard 2>/dev/null
nft delete table inet ctv_guard 2>/dev/null
[ -e /run/ctv-loaded-br_netfilter ] && rmmod br_netfilter && rm /run/ctv-loaded-br_netfilter
systemctl stop 'ctv-rollback*.timer' 2>/dev/null
echo "teardown done: $(incus list -f csv -c n | grep -c '^ctv-') ctv VMs left"
