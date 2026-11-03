#!/usr/bin/env bash
set -euo pipefail
exec </dev/null

vm="${1:?usage: release-vm.sh INSTANCE}"
pool=kcloud
net=kcloud0
leases="/var/lib/incus/networks/$net/dnsmasq.leases"
fail() { echo "release-vm: $*" >&2; exit 1; }
[[ "$(id -u)" == 0 ]] || fail "run as root"
[[ "$vm" =~ ^kc-[0-9a-f]{8}$ ]] || fail "$vm isn't a Kanna Cloud instance (kc-XXXXXXXX)"

instances="$(incus list -f csv -c n)" || fail "can't list instances"
ips=()
if grep -qx "$vm" <<<"$instances"; then
  mac="$(incus config get "$vm" volatile.eth0.hwaddr)" || fail "can't read $vm's MAC"
  state="$(incus query "/1.0/instances/$vm/state")" || fail "can't read $vm's state"
  while read -r ip; do [[ -n "$ip" ]] && ips+=("$ip"); done < <(python3 -c 'import json, sys
for nic in (json.loads(sys.argv[1]).get("network") or {}).values():
    for a in nic.get("addresses", []):
        if a.get("scope") == "global": print(a["address"])' "$state")
  hosts="/var/lib/incus/networks/$net/dnsmasq.hosts/$vm.eth0"
  if [[ -f "$hosts" ]]; then
    while read -r ip; do [[ -n "$ip" && " ${ips[*]} " != *" $ip "* ]] && ips+=("$ip"); done < <(
      tr ',' '\n' < "$hosts" | sed -n 's/^\[\(.*\)\]$/\1/p; /^[0-9]*\.[0-9]*\.[0-9]*\.[0-9]*$/p')
  fi
  if [[ -n "$mac" && -f "$leases" ]]; then
    while read -r ip; do [[ -n "$ip" && " ${ips[*]} " != *" $ip "* ]] && ips+=("$ip"); done < <(
      awk -v mac="$mac" 'NF == 5 && $2 == mac { print $3 }' "$leases")
  fi
  incus delete --force "$vm"
fi
volumes="$(incus storage volume list "$pool" -f csv -c tn type=custom)" || fail "can't list $pool's volumes"
for vol in $(awk -F, -v vm="$vm" '$1 == "custom" && ($2 == vm || index($2, vm "-") == 1) { print $2 }' <<<"$volumes"); do
  incus storage volume delete "$pool" "$vol"
done
rm -rf "/var/log/incus/$vm" "/run/incus/$vm"
for ip in "${ips[@]}"; do
  family=ipv4; [[ "$ip" == *:* ]] && family=ipv6
  conntrack -D -f "$family" -s "$ip" >/dev/null 2>&1 || true
  conntrack -D -f "$family" -d "$ip" >/dev/null 2>&1 || true
  ip neigh del "$ip" dev "$net" 2>/dev/null || true
done
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

left=()
instances="$(incus list -f csv -c n)" || fail "can't list instances to check"
if grep -qx "$vm" <<<"$instances"; then left+=("the instance"); fi
volumes="$(incus storage volume list "$pool" -f csv -c n)" || fail "can't list $pool's volumes to check"
if awk -v vm="$vm" '$0 == vm || index($0, vm "-") == 1 || index($0, vm "/") == 1 { found = 1 } END { exit !found }' <<<"$volumes"; then left+=("volumes"); fi
for dir in "/var/log/incus/$vm" "/run/incus/$vm"; do if [[ -e "$dir" ]]; then left+=("$dir"); fi; done
for ip in "${ips[@]}"; do
  family=ipv4; [[ "$ip" == *:* ]] && family=ipv6
  if [[ "$(conntrack -L -f "$family" -s "$ip" 2>/dev/null | wc -l)" -gt 0 ]]; then left+=("flows from $ip"); fi
  if ip neigh show "$ip" dev "$net" 2>/dev/null | grep -qv -e FAILED -e INCOMPLETE; then left+=("neighbour $ip"); fi
done
[[ ${#left[@]} -eq 0 ]] || fail "left behind: ${left[*]}"
echo "$vm destroyed with its disks, snapshots, logs, leases and flows (${ips[*]:-no addresses})"
