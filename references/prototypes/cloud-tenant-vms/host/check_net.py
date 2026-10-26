import json
import subprocess
import sys
import threading
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENV = dict(line.split("=", 1) for line in (HERE / "env.sh").read_text().split() if "=" in line and "$" not in line)
GW4, GW6, HOST_LAN, IDRAC = ENV["GW4"], ENV["GW6"], ENV["HOST_LAN4"], ENV["IDRAC4"]
RELAY_HOST, RELAY_PORT = ENV["RELAY"].split(":")
OTHER_NET = "10.199.64.1"
A, B = (sys.argv[1:3] if len(sys.argv) > 2 else ("ctv-a", "ctv-b"))
results = []


def run(cmd):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True, stdin=subprocess.DEVNULL).stdout.strip()


def guest(vm, *args):
    out = run("incus exec " + vm + " -- python3 -I /root/probe.py " + " ".join(f"'{a}'" for a in args))
    return json.loads(out)


def check(name, ok, detail):
    results.append(dict(check=name, ok=bool(ok), detail=detail))
    print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}", flush=True)


def prepare(vm):
    run(f"incus file push {HERE.parent / 'guest' / 'probe.py'} {vm}/root/probe.py")
    run(f"incus exec {vm} -- sh -c 'command -v scapy >/dev/null || (apt-get -qq update && "
        f"DEBIAN_FRONTEND=noninteractive apt-get -qq install -y python3-scapy >/dev/null)'")
    guest(vm, "listen")


def host_neigh(ip):
    return run(f"ip neigh show {ip} dev {ENV['NET']} | grep -o 'lladdr [^ ]*' | cut -d' ' -f2")


def gw_ll():
    return run(f"ip -6 -o addr show dev {ENV['NET']} scope link | awk '{{print $4}}' | cut -d/ -f1")


def reachability(src, dst, ident_dst):
    r = guest(src, "reach", json.dumps(ident_dst), GW4, GW6, gw_ll(), HOST_LAN, IDRAC, RELAY_HOST, RELAY_PORT, OTHER_NET)
    tag = f"{src}->"
    check(tag + "internet_v4", r["internet_v4_https"].startswith("HTTP/") and r["internet_v4_ping"] == "reply",
          f"https {r['internet_v4_https']}, ping {r['internet_v4_ping']}")
    check(tag + "dns", r["dns_via_gateway"] not in ("", "none"), r["dns_via_gateway"])
    check(tag + "ipv6_to_gateway", r["gw_v6_dns_tcp53"] == "open", f"[{GW6}]:53 {r['gw_v6_dns_tcp53']}")
    check(tag + "iroh_relay", r["relay_https"].startswith("HTTP/"), f"{RELAY_HOST}:{RELAY_PORT} {r['relay_https']}")
    blocked = {k: v for k, v in r.items() if k.startswith(("peer_", "gw_", "host_lan", "idrac", "lan_router", "other_net", "metadata"))
               and k not in ("gw_v4_dns_tcp53", "gw_v6_dns_tcp53")}
    leaks = {k: v for k, v in blocked.items() if v in ("open", "refused", "reply", "no_address")}
    check(tag + "no_peer_host_lan_idrac", not leaks, "leaks: " + json.dumps(leaks) if leaks else
          f"{len(blocked)} probes all dropped (peer v4/v6/link-local, gateway v4/v6/link-local :22/:8443, "
          f"host {HOST_LAN}, iDRAC {IDRAC}, router, {OTHER_NET}, metadata)")
    check(tag + "link_local_neighbours", r["ff02_1_responders"] in ("0", "1"),
          f"{r['ff02_1_responders']} answer ff02::1 (the gateway at most)")
    print("  info internet_v6:", r["internet_v6_tcp"], flush=True)
    return r


def main():
    for vm in (A, B):
        prepare(vm)
    ia, ib = guest(A, "ident"), guest(B, "ident")
    print("ident", json.dumps({A: ia, B: ib}), flush=True)
    check("addresses", all(i[k] for i in (ia, ib) for k in ("mac", "v4", "v6", "ll", "gw_mac")),
          f"{A} {ia['v4']} {ia['v6']}, {B} {ib['v4']} {ib['v6']}")
    reachability(A, B, ib)
    reachability(B, A, ia)

    run(f"incus exec {B} -- sh -c 'ping -c1 -W2 1.1.1.1; ping -6 -c1 -W2 {GW6}'")
    before = dict(host_b4=host_neigh(ib["v4"]), host_b6=host_neigh(ib["v6"]), b=guest(B, "state", GW4, GW6))
    sniffed = {}
    t = threading.Thread(target=lambda: sniffed.update(guest(B, "sniff", "110", json.dumps(ia))))
    t.start()
    spoof = guest(A, "spoof", json.dumps(ib), GW4, GW6)
    t.join()
    spoofed = dict(host_b4=host_neigh(ib["v4"]), host_b6=host_neigh(ib["v6"]))
    run(f"incus exec {B} -- sh -c 'ping -c1 -W2 1.1.1.1; ping -6 -c1 -W2 {GW6}'")
    after = dict(host_b4=host_neigh(ib["v4"]), host_b6=host_neigh(ib["v6"]), b=guest(B, "state", GW4, GW6))
    print("spoof", json.dumps(spoof), flush=True)

    check("l2_nothing_from_a_reaches_b", sniffed["frames_from_peer"] == 0,
          f"{spoof['frames_sent']} forged frames sent (ARP, NA, RA, DHCP offer, VLAN-tagged, peer-MAC, routed via gateway); "
          f"{sniffed['frames_from_peer']} seen on {B} {sniffed['sample']}")
    check("arp_spoof_host_table", ia["mac"] not in (spoofed["host_b4"], after["host_b4"]) and after["host_b4"] == ib["mac"],
          f"host's ARP for {B} {before['host_b4']} -> {spoofed['host_b4'] or 'aged out'} -> {after['host_b4']}")
    check("nd_spoof_host_table", ia["mac"] not in (spoofed["host_b6"], after["host_b6"]),
          f"host's ND for {B} {before['host_b6']} -> {spoofed['host_b6'] or 'aged out'} -> {after['host_b6'] or 'none'}")
    check("arp_nd_spoof_peer_table", ia["mac"] not in after["b"]["neigh"],
          f"{A}'s MAC {ia['mac']} in none of {B}'s neighbour entries (gateway v4, v6, link-local)")
    check("rogue_ra_dhcp", after["b"]["rogue_prefix"] == "0" and after["b"]["default_v4"] == before["b"]["default_v4"],
          f"{B} rogue prefixes {after['b']['rogue_prefix']}, default {after['b']['default_v4']}")
    for k in ("mac_changed_internet", "mac_as_peer_internet", "v4_other_addr_internet", "v4_peer_addr_internet",
              "v4_gateway_addr_peer", "v6_other_addr_gateway_dns", "v6_other_ll_gateway_ping"):
        check("spoof_" + k, spoof[k] in ("none", ""), spoof[k])
    check("spoof_restored", spoof["restored_internet"] == "reply", f"{A} back online: {spoof['restored_internet']}")

    tap = run(f"incus config get {A} volatile.eth0.host_name")
    control = {}
    run(f"bridge link set dev {tap} isolated off")
    try:
        t = threading.Thread(target=lambda: control.update(guest(B, "sniff", "12", json.dumps(ia))))
        t.start()
        guest(A, "hello", json.dumps(ib))
        t.join()
    finally:
        run(f"bridge link set dev {tap} isolated on")
    check("control_isolation_off_is_seen", control["frames_from_peer"] > 0,
          f"with {tap} isolation off for 12 s, {B} saw {control['frames_from_peer']} of {A}'s ARP requests; isolation back on")

    rules = run("nft list table bridge incus") + run("nft list table inet incus")
    check("enforced_on_host", f"fwd.{A}.eth0" in rules and "acl.ctv0" in rules,
          "bridge incus fwd.<vm>.eth0 (MAC/IP/RA filters) and inet incus acl.ctv0 on the host; port isolation: "
          + run(f"bridge -d link show | grep -A1 $(incus config get {A} volatile.eth0.host_name) | grep -o 'isolated [a-z]*'"))
    (HERE / "net-results.json").write_text(json.dumps(results, indent=1))
    print(f"{sum(r['ok'] for r in results)}/{len(results)} passed", flush=True)
    sys.exit(0 if all(r["ok"] for r in results) else 1)


main()
