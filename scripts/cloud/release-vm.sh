#!/usr/bin/env bash
set -euo pipefail
exec </dev/null

vm="${1:?usage: release-vm.sh INSTANCE}"
pool=kcloud
net=kcloud0
[[ "$(id -u)" == 0 ]] || { echo "release-vm: run as root" >&2; exit 1; }
[[ "$vm" =~ ^kc-[0-9a-f]{8}$ ]] || { echo "release-vm: $vm isn't a Kanna Cloud instance (kc-XXXXXXXX)" >&2; exit 1; }

ips=()
if incus info "$vm" >/dev/null 2>&1; then
  while read -r ip; do [[ -n "$ip" ]] && ips+=("$ip"); done < <(
    incus query "/1.0/instances/$vm/state" 2>/dev/null | python3 -c 'import json, sys
for nic in (json.load(sys.stdin).get("network") or {}).values():
    for a in nic.get("addresses", []):
        if a.get("scope") == "global": print(a["address"])' || true)
  incus delete --force "$vm"
fi
for vol in $(incus storage volume list "$pool" -f csv -c tn type=custom | grep -E "^custom,$vm(-|$)" | cut -d, -f2); do
  incus storage volume delete "$pool" "$vol"
done
rm -rf "/var/log/incus/$vm" "/run/incus/$vm"
for ip in "${ips[@]}"; do
  family=ipv4; [[ "$ip" == *:* ]] && family=ipv6
  conntrack -D -f "$family" -s "$ip" >/dev/null 2>&1 || true
  conntrack -D -f "$family" -d "$ip" >/dev/null 2>&1 || true
  ip neigh del "$ip" dev "$net" 2>/dev/null || true
done
leases="/var/lib/incus/networks/$net/dnsmasq.leases"
if [[ -f "$leases" ]]; then
  server="$(awk '$1 == "duid" { print $2; exit }' "$leases")"
  for ip in "${ips[@]}"; do
    [[ "$ip" == *:* && -n "$server" ]] || continue
    awk -v ip="$ip" 'NF == 5 && $3 == ip { print $2, $5 }' "$leases" | while read -r iaid client; do
      dhcp_release6 --iface "$net" --server-id "$server" --client-id "$client" --iaid "$iaid" --ip "$ip" || true
    done
  done
fi
sleep 3
left="$(incus list -f csv -c n | grep -x "$vm" || true; incus storage volume list "$pool" -f csv -c n | grep -E "^$vm(-|/|$)" || true; ls -d "/var/log/incus/$vm" "/run/incus/$vm" 2>/dev/null || true)"
[[ -z "$left" ]] || { echo "release-vm: left behind: $left" >&2; exit 1; }
echo "$vm destroyed with its disks, snapshots, logs, leases and flows (${ips[*]:-no addresses})"
