#!/usr/bin/env python3
import json
import os
import select
import signal
import sys
import time

BOOT = [
    "ESP-ROM:esp32s3-20210327",
    "\x1b[0;32mI (24) boot: ESP-IDF v5.3.1 2nd stage bootloader\x1b[0m",
    "\x1b[0;32mI (27) boot: chip revision: v0.2\x1b[0m",
    "\x1b[0;32mI (232) main_task: Calling app_main()\x1b[0m",
    "\x1b[0;33mW (201) spi_flash: Detected size(8192k) larger than the size in the binary image header(2048k).\x1b[0m",
    "\x1b[0;32mI (342) wifi_station: connecting to \"kanna-lab\"\x1b[0m",
    "\x1b[0;31mE (5341) wifi_station: connect failed: reason 201 (NO_AP_FOUND)\x1b[0m",
    "\x1b[0;33mW (5342) wifi_station: retry 1/5 in 1000 ms\x1b[0m",
    "\x1b[0;32mI (6402) wifi_station: got ip:192.168.1.47\x1b[0m",
    "D (6410) mqtt: sending CONNECT",
    "\x1b[0;32mI (6522) app: ready, free heap 241732\x1b[0m",
]

CRASH = [
    "Guru Meditation Error: Core  0 panic'ed (LoadProhibited). Exception was unhandled.",
    "",
    "Core  0 register dump:",
    "PC      : 0x42008a1e  PS      : 0x00060230  A0      : 0x82008c51  A1      : 0x3fca3e70",
    "Backtrace: 0x42008a1b:0x3fca3e70 0x42008c4e:0x3fca3ea0 0x4037c1d5:0x3fca3ec0",
    "Rebooting...",
]


def main():
    folder = sys.argv[1]
    master, slave = os.openpty()
    path = os.ttyname(slave)
    os.close(slave)
    os.set_inheritable(master, False)
    ports = [{"path": path, "vid": "303a", "pid": "1001", "serial": "48:27:E2:4C:91:A0", "product": "USB JTAG/serial debug unit", "vendor": "Espressif"}]
    with open(os.path.join(folder, "ports.json.new"), "w") as out:
        json.dump(ports, out)
    os.rename(os.path.join(folder, "ports.json.new"), os.path.join(folder, "ports.json"))
    print(path, flush=True)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))

    def say(lines, gap=0.03):
        for line in lines:
            os.write(master, (line + "\r\n").encode())
            time.sleep(gap)

    pending = b""
    while True:
        ready, _, _ = select.select([master], [], [], 0.5)
        if not ready:
            continue
        try:
            data = os.read(master, 1024)
        except OSError:
            time.sleep(0.1)
            continue
        pending += data
        while b"\n" in pending or b"\r" in pending:
            cut = min(i for i in (pending.find(b"\n"), pending.find(b"\r")) if i >= 0)
            line, pending = pending[:cut].decode(errors="replace").strip(), pending[cut + 1:]
            if not line:
                continue
            if line == "reboot":
                say(BOOT)
            elif line == "crash":
                say(CRASH, 0)
            elif line == "binary":
                os.write(master, b"\x00\x01\x02\xfe\xffraw\r\n")
            elif line == "help":
                say(["Commands: help, wifi status, reboot, crash, binary"])
            elif line == "wifi status":
                say(["\x1b[0;32mI (9120) wifi_station: connected to kanna-lab, rssi -58\x1b[0m"])
            else:
                say(["unknown command: " + line])
            os.write(master, b"esp> ")


if __name__ == "__main__":
    main()
