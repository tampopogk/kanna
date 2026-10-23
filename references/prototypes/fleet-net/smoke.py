#!/usr/bin/env python3
"""Two real local endpoints: six kinds, both directions, and wrong-path rejection."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
from summarize import summarize

root = Path(__file__).resolve().parent
binary = root / "target/release/fleet-net"
with tempfile.TemporaryDirectory(prefix="fleet-net-smoke-") as temp:
    temp = Path(temp)
    server = subprocess.Popen(
        [str(binary), "--mdns", "serve"], stderr=subprocess.PIPE,
        stdout=subprocess.DEVNULL, text=True, env={**os.environ, "RUST_LOG": "off"},
    )
    try:
        event = json.loads(server.stderr.readline())
        peer = event["id"]
        common = [str(binary), "--mdns", "probe", "--peer", peer,
                  "--mib", "1", "--samples", "10", "--settle-seconds", "1"]
        good = temp / "good.json"
        subprocess.run(common + ["--expect", "direct", "--output", str(good)],
                       stdout=subprocess.DEVNULL, check=True, timeout=60)
        summarize(good)
        bad = temp / "wrong-path.json"
        result = subprocess.run(common + ["--expect", "relay", "--output", str(bad)],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                timeout=60)
        assert result.returncode != 0
        assert "did not reach Relay" in json.loads(bad.read_text())["measurement"]["error"]
        print("PASS: six stream kinds, payload integrity in both directions, concurrent echoes, wrong-path rejection")
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()
