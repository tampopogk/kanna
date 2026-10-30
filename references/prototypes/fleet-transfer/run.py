#!/usr/bin/env python3
"""Run the real Mac <-> Linux matrix over SSH (transport only)."""
import argparse
import datetime
import json
from pathlib import Path
import shlex
import subprocess
import uuid


SCRIPT = Path(__file__).with_name("transfer.py").resolve()


class Machine:
    def __init__(self, host=None):
        self.host = host
        self.home = self.command(["python3", "-c", "from pathlib import Path; print(Path.home())"]).strip()

    def command(self, args, expected=0):
        argv = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", self.host,
                shlex.join(map(str, args))] if self.host else list(map(str, args))
        r = subprocess.run(argv, stdin=subprocess.DEVNULL, text=True, capture_output=True)
        if r.returncode != expected:
            raise RuntimeError(f"{shlex.join(argv)}: exit {r.returncode}\n{r.stderr}\n{r.stdout}")
        return r.stdout

    def step(self, action, root, *args, expected=0):
        return self.command(["python3", self.script, action, root, *args], expected)

    def read(self, path):
        return json.loads(self.command(["cat", path]))


def copy(source, source_path, destination, destination_path):
    def arg(machine, path):
        return f"{machine.host}:{path}" if machine.host else str(path)
    subprocess.run(["scp", "-q", "-o", "BatchMode=yes", arg(source, source_path),
                    arg(destination, destination_path)], check=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--host", default="jeremyhale@172.16.0.105")
    p.add_argument("--agent", choices=["claude", "codex", "both"], default="both")
    p.add_argument("--direction", choices=["out", "back", "both"], default="both")
    args = p.parse_args()
    local, remote = Machine(), Machine(args.host)
    assert local.home != remote.home, "This probe requires different home paths"
    run_id = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ-") + uuid.uuid4().hex[:8]
    for machine in (local, remote):
        machine.root = Path(machine.home) / ".local/share/kanna-fleet-transfer" / run_id
        machine.command(["mkdir", "-p", machine.root])
        machine.script = machine.root / "transfer.py"
        copy(local, SCRIPT, machine, machine.script)
    print(f"Evidence: {local.root} and {args.host}:{remote.root}", flush=True)
    rows = []
    directions = [("mac-to-linux", local, remote), ("linux-to-mac", remote, local)]
    for direction, source, destination in directions:
        if args.direction != "both" and args.direction != ("out" if source is local else "back"):
            continue
        for agent in (["claude", "codex"] if args.agent == "both" else [args.agent]):
            name = direction + "-" + agent
            sender, receiver = source.root / (name + "-sender"), destination.root / (name + "-receiver")
            print(name + ": seed and stop", flush=True)
            source.step("init", sender, "--agent", agent)
            source.step("seed", sender)
            source.step("pack", sender, "--stop-after", "prepared", expected=75)
            source.step("pack", sender)
            source.step("pack", sender)  # duplicate transfer request
            destination.command(["mkdir", "-p", receiver])
            for file in ("base.bundle", "task.bundle", "transcript.jsonl", "manifest.json"):
                copy(source, sender / file, destination, receiver / file)
            print(name + ": bundle, soft reset, transcript", flush=True)
            for phase in ("received", "fetched", "restored", "installed"):
                destination.step("receive", receiver, "--stop-after", phase, expected=75)
            destination.step("receive", receiver)
            print(name + ": resume same session", flush=True)
            destination.step("resume", receiver, "--stop-after", "answered", expected=75)
            destination.step("resume", receiver)
            destination.step("resume", receiver)  # must not send another agent turn
            destination.step("check-workdir", receiver)
            copy(destination, receiver / "receipt.json", source, sender / "receipt.json")
            assert source.read(sender / "state.json")["phase"] == "packed"
            source.step("ack", sender, "--stop-after", "done", expected=75)
            source.step("ack", sender)
            s, r = source.read(sender / "state.json"), destination.read(receiver / "state.json")
            row = dict(direction=direction, agent=agent, session=s["session"],
                       source_home=source.home, receiver_home=destination.home,
                       sender=str(sender), receiver=str(receiver),
                       transcript_sha256=s["transcript_sha256"], tree=s["tree"],
                       before=s["before"], wip=s["wip"],
                       source_phase=s["phase"], receiver_phase=r["phase"],
                       memory_recalled=True, same_session=True, receiver_workdir_proved=True,
                       source_version=source.command([str(Path(source.home) / ".local/bin" / agent), "--version"]).strip(),
                       receiver_version=destination.command([str(Path(destination.home) / ".local/bin" / agent), "--version"]).strip())
            rows.append(row)
            (local.root / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
            print(name + ": PASS", flush=True)
    print(f"Results: {local.root / 'results.json'}", flush=True)


if __name__ == "__main__":
    main()
