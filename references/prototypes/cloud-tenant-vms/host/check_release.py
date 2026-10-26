import json
import secrets
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENV = dict(line.split("=", 1) for line in (HERE / "env.sh").read_text().split() if "=" in line and "$" not in line)
POOL, NET, PROFILE, IMAGE = ENV["POOL"], ENV["NET"], ENV["PROFILE"], ENV["IMAGE"]
VM = sys.argv[1] if len(sys.argv) > 1 else "ctv-a"
VOL = f"{VM}-data"
LV = VM.replace("-", "--")
HOST_DIRS = [f"/var/lib/incus/virtual-machines/{VM}", f"/var/lib/incus/storage-pools/{POOL}/virtual-machines/{VM}",
             f"/var/lib/incus/storage-pools/{POOL}/virtual-machines-snapshots/{VM}", f"/var/log/incus/{VM}", f"/run/incus/{VM}"]
results = []


def run(cmd, check=False):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    if check and r.returncode:
        raise SystemExit(f"{cmd}: {r.stderr.strip()}")
    return r.stdout.strip()


def check(name, ok, detail):
    results.append(dict(check=name, ok=bool(ok), detail=detail))
    print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}", flush=True)


def info(name, detail):
    print(f"info {name}: {detail}", flush=True)


def guest(*args):
    return json.loads(run(f"incus exec {VM} -- python3 -I /root/probe.py " + " ".join(f"'{a}'" for a in args), check=True))


def exists():
    return run(f"incus info {VM} >/dev/null 2>&1 && echo yes") == "yes"


def volume_exists():
    return VOL in run(f"incus storage volume list {POOL} -f csv -c n type=custom").split()


def launch():
    run(f"incus init {IMAGE} {VM} --vm --no-profiles --profile {PROFILE}", check=True)
    if not volume_exists():
        run(f"incus storage volume create {POOL} {VOL} --type=block size=2GiB", check=True)
    run(f"incus storage volume attach {POOL} {VOL} {VM}", check=True)
    run(f"incus start {VM}", check=True)
    for _ in range(120):
        if run(f"incus exec {VM} -- true && echo up") == "up":
            break
        time.sleep(2)
    run(f"incus exec {VM} -- cloud-init status --wait")
    run(f"incus file push {HERE.parent / 'guest' / 'probe.py'} {VM}/root/probe.py", check=True)


def addrs():
    state = json.loads(run(f"incus query /1.0/instances/{VM}/state"))
    return [a["address"] for n in (state["network"] or {}).values() for a in n["addresses"] if a["scope"] == "global"]


def conntrack(ips):
    return run(" ; ".join(f"conntrack -L -f {'ipv6' if ':' in ip else 'ipv4'} -{d} {ip} 2>/dev/null"
                          for ip in ips for d in "sd")).splitlines()


def inventory(mac, ips, marker):
    return {
        "lvm": run(f"lvs --noheadings -o lv_name {POOL} | grep -E '_{LV}(\\.|-|$)' || true").split(),
        "volumes": run(f"incus storage volume list {POOL} -f csv -c tn | grep -E ',({VM}|{VOL})(/|$)' || true").split(),
        "snapshots": run(f"incus snapshot list {VM} -f csv -c n 2>/dev/null || true").split(),
        "dirs": [d for d in HOST_DIRS if run(f"test -e {d} && echo y") == "y"],
        "logs_with_marker": run(f"grep -rlF '{marker}' /var/log/incus/{VM} /run/incus/{VM} 2>/dev/null || true").split(),
        "dhcp_hosts": run(f"ls /var/lib/incus/networks/{NET}/dnsmasq.hosts/ | grep '^{VM}\\.' || true").split(),
        "dhcp_leases": run(f"grep -iE '{mac}|{' |'.join(ips)} ' /var/lib/incus/networks/{NET}/dnsmasq.leases || true").splitlines(),
        "nft": run(f"nft list table bridge incus | grep -iE 'chain .*\\.{VM}\\.|{mac}' || true").splitlines(),
        "neigh": run(f"ip neigh show dev {NET} | grep -i '{mac}' || true").splitlines(),
        "conntrack": conntrack(ips),
    }


def release(mac, ips):
    if exists():
        run(f"incus delete --force {VM}", check=True)
    for vol in run(f"incus storage volume list {POOL} -f csv -c tn type=custom | grep -E '^custom,{VM}-' | cut -d, -f2").split():
        run(f"incus storage volume delete {POOL} {vol}", check=True)
    run(f"rm -rf /var/log/incus/{VM} /run/incus/{VM}")
    for ip in ips:
        fam = "ipv6" if ":" in ip else "ipv4"
        run(f"conntrack -D -f {fam} -s {ip}; conntrack -D -f {fam} -d {ip}; ip neigh del {ip} dev {NET}")
    leases = Path(f"/var/lib/incus/networks/{NET}/dnsmasq.leases").read_text().splitlines()
    server = next((line.split()[1] for line in leases if line.startswith("duid ")), None)
    for line in leases:
        f = line.split()
        if server and len(f) == 5 and f[2] in ips and ":" in f[2]:
            run(f"dhcp_release6 --iface {NET} --server-id {server} --client-id {f[4]} --iaid {f[1]} --ip {f[2]}")
    time.sleep(3)


def tenant(marker):
    if not exists():
        launch()
    run(f"incus file push {HERE.parent / 'guest' / 'probe.py'} {VM}/root/probe.py", check=True)
    written = guest("fill", marker, "512")
    run(f"incus exec {VM} -- curl -s -o /dev/null https://example.com")
    run(f"incus snapshot create {VM} s1", check=True)
    hits = guest("scan", marker)
    check("tenant_wrote_marker", all(hits.get(d, 0) > 1000 for d in [written["root"]] + written["data"]),
          f"marker on root and data disks {hits}, the console, snapshot s1, a flow to example.com")
    return run(f"incus config get {VM} volatile.eth0.hwaddr"), addrs()


def main():
    run("command -v conntrack dhcp_release6 >/dev/null || "
        "DEBIAN_FRONTEND=noninteractive apt-get -qq install -y conntrack dnsmasq-utils >/dev/null")
    if exists():
        release(run(f"incus config get {VM} volatile.eth0.hwaddr"), addrs())
    marker = "CTV-RELEASE-" + secrets.token_hex(8)

    mac, ips = tenant(marker)
    run(f"incus delete --force {VM}", check=True)
    left = {k: v for k, v in inventory(mac, ips, marker).items() if v}
    info("incus_delete_alone_leaves", json.dumps(left))
    launch()
    inherited = dict(volume_reattached=guest("scan", marker), logs=inventory(mac, ips, marker)["logs_with_marker"],
                     conntrack=conntrack(ips))
    info("same_name_vm_after_incus_delete_inherits", json.dumps(inherited))

    release(run(f"incus config get {VM} volatile.eth0.hwaddr"), addrs())
    release(mac, ips)
    mac, ips = tenant(marker)
    release(mac, ips)
    gone = {k: v for k, v in inventory(mac, ips, marker).items() if v}
    check("release_leaves_nothing", not gone,
          "no LVs, volumes, snapshots, dirs, logs, DHCP hosts or leases, filters, neighbours or flows" if not gone
          else "left: " + json.dumps(gone))

    launch()
    new_mac = run(f"incus config get {VM} volatile.eth0.hwaddr")
    hits = guest("scan", marker)
    after = inventory(new_mac, ips, marker)
    online = run(f"incus exec {VM} -- curl -s -m 10 -o /dev/null -w '%{{http_code}}' https://example.com")
    check("new_vm_online", online == "200", f"the new VM reaches the internet from the address Incus gave it: {online}")
    check("new_vm_new_mac", new_mac != mac, f"same name {VM}, MAC {mac} -> {new_mac}, addresses {ips} -> {addrs()}")
    check("new_vm_no_snapshots", not after["snapshots"], "no snapshots")
    check("new_vm_no_old_data", not any(hits.values()), f"marker hits on the new VM's disks: {hits}")
    check("new_vm_no_old_logs", not after["logs_with_marker"], "no console or QEMU log of the old VM")
    release(new_mac, addrs())
    (HERE / "release-results.json").write_text(json.dumps(results, indent=1))
    print(f"{sum(r['ok'] for r in results)}/{len(results)} passed", flush=True)
    sys.exit(0 if all(r["ok"] for r in results) else 1)


main()
