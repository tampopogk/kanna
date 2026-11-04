#!/usr/bin/env bash
set -euo pipefail
exec </dev/null

pool_size="${KCLOUD_POOL_SIZE:-400GiB}"
pool=kcloud
net=kcloud0
acl=kcloud-tenant
profile=kcloud-tenant

[[ "$(id -u)" == 0 ]] || { echo "host-setup: run as root" >&2; exit 1; }
step() { echo "==> $*"; }

missing=()
command -v incus >/dev/null || missing+=(incus)
command -v conntrack >/dev/null || missing+=(conntrack)
command -v dhcp_release6 >/dev/null || missing+=(dnsmasq-utils)
command -v lvm >/dev/null || missing+=(lvm2 thin-provisioning-tools)
if [[ ${#missing[@]} -gt 0 ]]; then
  step "Packages: ${missing[*]}"
  apt-get update -q
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q "${missing[@]}"
fi

if ! incus info >/dev/null 2>&1; then
  step "Incus: first start"
  incus admin init --minimal
fi

step "br_netfilter, now and at boot"
lsmod | grep -q '^br_netfilter' || modprobe br_netfilter
echo br_netfilter > /etc/modules-load.d/kanna-cloud.conf

if ! incus storage show "$pool" >/dev/null 2>&1; then
  step "Storage pool $pool: LVM thin, $pool_size, new blocks zeroed"
  incus storage create "$pool" lvm size="$pool_size" lvm.use_thinpool=true
fi
[[ "$(incus storage show "$pool" | sed -n 's/^driver: //p')" == lvm ]] || { echo "host-setup: pool $pool isn't LVM" >&2; exit 1; }

if ! incus network acl show "$acl" >/dev/null 2>&1; then
  step "Network ACL $acl"
  incus network acl create "$acl"
  incus network acl rule add "$acl" egress action=drop \
    destination=0.0.0.0/8,10.0.0.0/8,100.64.0.0/10,127.0.0.0/8,169.254.0.0/16,172.16.0.0/12,192.0.0.0/24,192.168.0.0/16,198.18.0.0/15,224.0.0.0/4,240.0.0.0/4
  incus network acl rule add "$acl" egress action=drop \
    destination=::/128,::1/128,64:ff9b::/96,100::/64,fc00::/7,fe80::/10,fec0::/10,ff00::/8
  incus network acl rule add "$acl" egress action=allow
fi

if ! incus network show "$net" >/dev/null 2>&1; then
  step "Tenant bridge $net"
  if ! incus network create "$net" \
    ipv4.address=10.232.0.1/24 ipv4.nat=true ipv4.dhcp=true \
    ipv6.address=fd5a:1c3e:7a10:2::1/64 ipv6.nat=true ipv6.dhcp=true ipv6.dhcp.stateful=true \
    dns.mode=none \
    security.acls="$acl" \
    security.acls.default.egress.action=reject \
    security.acls.default.ingress.action=drop; then
    pidfile="/var/lib/incus/networks/$net/dnsmasq.pid"
    [[ -f "$pidfile" ]] && kill "$(cat "$pidfile")" 2>/dev/null || true
    echo "host-setup: couldn't make $net (a dnsmasq it left was stopped); run it again" >&2
    exit 1
  fi
fi

if ! incus profile show "$profile" >/dev/null 2>&1; then
  step "Profile $profile"
  incus profile create "$profile"
  incus profile set "$profile" limits.cpu=4 limits.memory=8GiB security.secureboot=false
  incus profile device add "$profile" root disk pool="$pool" path=/ size=40GiB
  incus profile device add "$profile" eth0 nic network="$net" name=eth0 \
    security.mac_filtering=true security.ipv4_filtering=true security.ipv6_filtering=true \
    security.port_isolation=true
fi

step "Ready: $(incus --version), pool $pool, bridge $net, profile $profile"
