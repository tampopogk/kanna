import json
import socket
import ssl
import subprocess
import sys
import time

IFACE = subprocess.run("ip -o link | awk -F': ' '$2 != \"lo\" {print $2; exit}'", shell=True,
                       capture_output=True, text=True).stdout.strip()


def sh(cmd, check=False):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if check and r.returncode:
        raise SystemExit(f"{cmd}: {r.stderr.strip()}")
    return r.stdout.strip()


def tcp(host, port, timeout=3):
    if not host or host == "0":
        return "no_address"
    family = socket.AF_INET6 if ":" in host else socket.AF_INET
    addr = (host, port, 0, socket.if_nametoindex(IFACE)) if host.startswith("fe80") else (host, port)
    s = socket.socket(family, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(addr)
        return "open"
    except ConnectionRefusedError:
        return "refused"
    except (socket.timeout, TimeoutError):
        return "timeout"
    except OSError as e:
        return f"error:{e.errno}"
    finally:
        s.close()


def https(host, port=443, path="/"):
    try:
        raw = socket.create_connection((host, port), timeout=5)
        tls = ssl.create_default_context().wrap_socket(raw, server_hostname=host)
        tls.sendall(f"GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n".encode())
        line = tls.recv(64).split(b"\r\n")[0].decode(errors="replace")
        tls.close()
        return line or "empty"
    except Exception as e:
        return f"error:{type(e).__name__}"


def ping(host, count=2):
    if not host or host == "0":
        return "no_address"
    flag = "-6" if ":" in host else "-4"
    target = f"{host}%{IFACE}" if host.startswith("fe80") else host
    out = sh(f"ping {flag} -c {count} -W 2 {target} 2>&1")
    return "reply" if " 0% packet loss" in out or ", 0% packet loss" in out or "1 received" in out or "2 received" in out else "none"


def ident():
    return dict(
        mac=sh(f"cat /sys/class/net/{IFACE}/address"),
        v4=sh(f"ip -4 -o addr show {IFACE} | awk '{{print $4}}' | cut -d/ -f1 | head -1"),
        v6=sh(f"ip -6 -o addr show {IFACE} scope global | awk '{{print $4}}' | cut -d/ -f1 | head -1"),
        ll=sh(f"ip -6 -o addr show {IFACE} scope link | awk '{{print $4}}' | cut -d/ -f1 | head -1"),
        gw_mac=sh(f"ip neigh show $(ip -4 route show default | awk '{{print $3}}') | awk '{{print $5}}'"),
    )


def listen():
    sh("pkill -f 'ctv-listen' || true")
    subprocess.Popen(["setsid", "python3", "-c",
                      "import http.server,socket\n"
                      "class S(http.server.ThreadingHTTPServer): address_family=socket.AF_INET6\n"
                      "S(('::',8000),http.server.SimpleHTTPRequestHandler).serve_forever()",
                      "ctv-listen"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, cwd="/tmp")
    time.sleep(1)
    return tcp("127.0.0.1", 8000)


def reach(peer, gw4, gw6, gw_ll, lan_host, idrac, relay_host, relay_port, other_net):
    p = json.loads(peer)
    out = {}
    out["internet_v4_https"] = https("example.com")
    out["internet_v4_ping"] = ping("1.1.1.1")
    out["dns_via_gateway"] = sh("getent ahostsv4 example.com | head -1 | awk '{print $1}'") or "none"
    out["relay_https"] = https(relay_host, int(relay_port), "/ping")
    out["internet_v6_tcp"] = tcp("2606:4700:4700::1111", 443)
    for name, host in [("peer_v4", p["v4"]), ("peer_v6", p["v6"]), ("peer_ll", p["ll"])]:
        out[f"{name}_tcp8000"] = tcp(host, 8000)
        out[f"{name}_ping"] = ping(host)
    for name, host in [("gw_v4", gw4), ("gw_v6", gw6), ("gw_ll", gw_ll)]:
        for port in (22, 8443):
            out[f"{name}_tcp{port}"] = tcp(host, port)
    out["gw_v4_dns_tcp53"] = tcp(gw4, 53)
    out["gw_v6_dns_tcp53"] = tcp(gw6, 53)
    for name, host, ports in [("host_lan", lan_host, (22, 8443)), ("idrac", idrac, (22, 443)),
                              ("lan_router", "172.16.0.1", (53, 80)), ("other_net", other_net, (22,)),
                              ("metadata", "169.254.169.254", (80,))]:
        for port in ports:
            out[f"{name}_tcp{port}"] = tcp(host, port)
        out[f"{name}_ping"] = ping(host)
    out["ff02_1_responders"] = sh(f"ping -6 -c 2 -W 2 ff02::1%{IFACE} 2>&1 | grep -o 'from [^ ]*' | sort -u | wc -l")
    return out


def with_addr(cmd_set, cmd_restore, test):
    sh(cmd_set)
    time.sleep(2)
    try:
        return test()
    finally:
        sh(cmd_restore)
        time.sleep(3)


def spoof(peer, gw4, gw6):
    from scapy.all import ARP, Ether, ICMPv6ND_NA, ICMPv6ND_RA, ICMPv6NDOptPrefixInfo, ICMPv6NDOptSrcLLAddr, \
        IPv6, IP, UDP, BOOTP, DHCP, ICMP, Dot1Q, sendp, conf
    conf.verb = 0
    p = json.loads(peer)
    me = ident()
    out = {}
    bcast = "ff:ff:ff:ff:ff:ff"
    frames = [
        Ether(dst=bcast, src=me["mac"]) / ARP(op=2, psrc=gw4, hwsrc=me["mac"], pdst=p["v4"], hwdst=bcast),
        Ether(dst=p["mac"], src=me["mac"]) / ARP(op=2, psrc=gw4, hwsrc=me["mac"], pdst=p["v4"], hwdst=p["mac"]),
        Ether(dst=me["gw_mac"], src=me["mac"]) / ARP(op=2, psrc=p["v4"], hwsrc=me["mac"], pdst=gw4, hwdst=me["gw_mac"]),
        Ether(dst=bcast, src=me["mac"]) / ARP(op=1, psrc=p["v4"], hwsrc=me["mac"], pdst=gw4),
        Ether(dst="33:33:00:00:00:01", src=me["mac"]) / IPv6(src=me["ll"], dst="ff02::1") /
        ICMPv6ND_NA(tgt=gw6, R=1, S=0, O=1) / ICMPv6NDOptSrcLLAddr(lladdr=me["mac"]),
        Ether(dst=me["gw_mac"], src=me["mac"]) / IPv6(src=me["ll"], dst="ff02::1") /
        ICMPv6ND_NA(tgt=p["v6"], R=0, S=0, O=1) / ICMPv6NDOptSrcLLAddr(lladdr=me["mac"]),
        Ether(dst="33:33:00:00:00:01", src=me["mac"]) / IPv6(src=me["ll"], dst="ff02::1") /
        ICMPv6ND_RA(routerlifetime=1800) / ICMPv6NDOptSrcLLAddr(lladdr=me["mac"]) /
        ICMPv6NDOptPrefixInfo(prefix="2001:db8:bad::", prefixlen=64, L=1, A=1, validlifetime=3600, preferredlifetime=3600),
        Ether(dst=bcast, src=me["mac"]) / IP(src=gw4, dst="255.255.255.255") / UDP(sport=67, dport=68) /
        BOOTP(op=2, yiaddr=p["v4"], siaddr=me["v4"], chaddr=bytes.fromhex(p["mac"].replace(":", ""))) /
        DHCP(options=[("message-type", "offer"), ("server_id", me["v4"]), ("router", me["v4"]), "end"]),
        Ether(dst=me["gw_mac"], src=me["mac"]) / IP(src=me["v4"], dst=p["v4"]) / ICMP(),
        Ether(dst=p["mac"], src=me["mac"]) / IP(src=me["v4"], dst=p["v4"]) / ICMP(),
        Ether(dst=p["mac"], src=me["mac"]) / Dot1Q(vlan=1) / IP(src=me["v4"], dst=p["v4"]) / ICMP(),
        Ether(dst=p["mac"], src=p["mac"]) / IP(src=p["v4"], dst=p["v4"]) / ICMP(),
        Ether(dst=me["gw_mac"], src="02:00:5e:10:00:01") / IP(src=me["v4"], dst="1.1.1.1") / ICMP(),
        Ether(dst=me["gw_mac"], src=me["mac"]) / IP(src=p["v4"], dst="1.1.1.1") / ICMP(),
        Ether(dst=me["gw_mac"], src=me["mac"]) / IPv6(src=p["v6"], dst="2606:4700:4700::1111") / ICMP(),
    ]
    for _ in range(3):
        sendp(frames, iface=IFACE)
        time.sleep(1)
    out["frames_sent"] = len(frames) * 3

    out["mac_changed_internet"] = with_addr(
        f"ip link set {IFACE} down; ip link set {IFACE} address 02:00:5e:10:00:01; ip link set {IFACE} up",
        f"ip link set {IFACE} down; ip link set {IFACE} address {me['mac']}; ip link set {IFACE} up",
        lambda: ping("1.1.1.1"))
    out["mac_as_peer_internet"] = with_addr(
        f"ip link set {IFACE} down; ip link set {IFACE} address {p['mac']}; ip link set {IFACE} up",
        f"ip link set {IFACE} down; ip link set {IFACE} address {me['mac']}; ip link set {IFACE} up",
        lambda: ping("1.1.1.1"))
    out["v4_other_addr_internet"] = with_addr(
        f"ip addr add 10.231.0.250/24 dev {IFACE}; ip route replace default via {gw4} src 10.231.0.250",
        f"ip addr del 10.231.0.250/24 dev {IFACE}; networkctl reconfigure {IFACE}",
        lambda: ping("1.1.1.1"))
    out["v4_peer_addr_internet"] = with_addr(
        f"ip addr add {p['v4']}/32 dev {IFACE}; ip route replace default via {gw4} src {p['v4']}",
        f"ip addr del {p['v4']}/32 dev {IFACE}; networkctl reconfigure {IFACE}",
        lambda: ping("1.1.1.1"))
    out["v4_gateway_addr_peer"] = with_addr(
        f"ip addr add {gw4}/32 dev {IFACE}",
        f"ip addr del {gw4}/32 dev {IFACE}",
        lambda: ping(p["v4"]))
    out["v6_other_addr_gateway_dns"] = with_addr(
        f"ip -6 addr add fd5a:1c3e:7a10:1::dead/64 dev {IFACE} nodad",
        f"ip -6 addr del fd5a:1c3e:7a10:1::dead/64 dev {IFACE}",
        lambda: sh(f"timeout 4 python3 -c \"import socket;s=socket.socket(socket.AF_INET6);s.bind(('fd5a:1c3e:7a10:1::dead',0));s.settimeout(3);s.connect(('{gw6}',53));print('open')\" 2>/dev/null") or "none")
    out["v6_other_ll_gateway_ping"] = with_addr(
        f"ip -6 addr add fe80::dead/64 dev {IFACE} nodad",
        f"ip -6 addr del fe80::dead/64 dev {IFACE}",
        lambda: "reply" if "1 received" in sh(f"ping -6 -c 1 -W 2 -I fe80::dead {gw6} 2>&1") else "none")
    sh(f"networkctl reconfigure {IFACE}")
    time.sleep(4)
    out["restored_internet"] = ping("1.1.1.1")
    return out


def hello(peer):
    from scapy.all import ARP, Ether, sendp, conf
    conf.verb = 0
    p, me = json.loads(peer), ident()
    for _ in range(5):
        sendp(Ether(dst="ff:ff:ff:ff:ff:ff", src=me["mac"]) / ARP(op=1, psrc=me["v4"], hwsrc=me["mac"], pdst=p["v4"]), iface=IFACE)
        time.sleep(1)
    return dict(sent=5)


def sniff(seconds, peer):
    from scapy.all import sniff as scapy_sniff, Ether, IP, IPv6, conf
    conf.verb = 0
    p = json.loads(peer)
    foreign = {p["mac"].lower(), "02:00:5e:10:00:01"}
    bad = []

    def keep(pkt):
        if Ether not in pkt:
            return
        src = pkt[Ether].src.lower()
        ip_src = pkt[IP].src if IP in pkt else (pkt[IPv6].src if IPv6 in pkt else None)
        if src in foreign or ip_src in (p["v4"], p["v6"], p["ll"], "10.231.0.250", "fd5a:1c3e:7a10:1::dead", "fe80::dead"):
            bad.append(pkt.summary())
    scapy_sniff(iface=IFACE, prn=keep, store=False, timeout=float(seconds))
    return dict(frames_from_peer=len(bad), sample=bad[:5])


def scan(marker):
    needle = marker.encode()
    counts = {}
    for dev in sh("lsblk -dnpo NAME,TYPE | awk '$2 == \"disk\" {print $1}'").split():
        n, tail = 0, b""
        with open(dev, "rb", buffering=0) as f:
            while chunk := f.read(1 << 24):
                buf = tail + chunk
                n += buf.count(needle)
                tail = buf[-(len(needle) - 1):]
        counts[dev] = n
    return counts


def fill(marker, mib):
    line = (marker + "\n").encode()
    block = line * ((1 << 20) // len(line))
    with open("/root/fill", "wb") as f:
        for _ in range(int(mib)):
            f.write(block)
    root = "/dev/" + sh("lsblk -no PKNAME $(findmnt -no SOURCE /)")
    others = [d for d in sh("lsblk -dnpo NAME,TYPE | awk '$2 == \"disk\" {print $1}'").split() if d != root]
    for dev in others:
        with open(dev, "r+b", buffering=0) as f:
            for _ in range(int(mib)):
                f.write(block)
    with open("/dev/ttyS0", "w") as tty:
        tty.write(marker + "\n")
    sh("sync")
    return dict(root=root, data=others)


def state(gw4, gw6):
    me = ident()
    return dict(
        me=me,
        neigh=sh("ip neigh show; ip -6 neigh show"),
        default_v4=sh("ip -4 route show default"),
        default_v6=sh("ip -6 route show default"),
        rogue_prefix=sh(f"ip -6 addr show {IFACE} | grep -c 2001:db8:bad || true"),
    )


if __name__ == "__main__":
    cmd, args = sys.argv[1], sys.argv[2:]
    result = dict(ident=ident, listen=listen, hello=hello, scan=scan, fill=fill, reach=reach, spoof=spoof, sniff=sniff, state=state)[cmd](*args)
    print(json.dumps(result, indent=1))
