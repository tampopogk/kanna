#!/usr/bin/env python3
"""Run as the logged-in job user; installs only this prototype's LaunchAgent."""
import argparse
import os
from pathlib import Path
import plistlib
import subprocess
import sys

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("state", type=Path)
p.add_argument("--token-file", type=Path, required=True)
p.add_argument("--port", type=int, default=8765)
args = p.parse_args()
if os.getuid() != os.stat("/dev/console").st_uid:
    sys.exit("Log in as the job user first: it must own /dev/console.")
state = args.state.resolve()
state.mkdir(parents=True, exist_ok=True)
plist = state / "dev.kanna.shim-model.plist"
plist.write_bytes(plistlib.dumps({
    "Label": "dev.kanna.shim-model",
    "ProgramArguments": [sys.executable, str(Path(__file__).resolve().with_name("model.py")),
        "serve", str(state), "--token-file", str(args.token_file.resolve()), "--port", str(args.port)],
    "RunAtLoad": True,
    "LimitLoadToSessionType": "Aqua",
    "StandardOutPath": str(state / "agent.out"),
    "StandardErrorPath": str(state / "agent.err"),
}))
subprocess.run(["launchctl", "bootstrap", "gui/" + str(os.getuid()), str(plist)], check=True)
print("Started dev.kanna.shim-model in gui/" + str(os.getuid()))
print("Remove: launchctl bootout gui/" + str(os.getuid()) + "/dev.kanna.shim-model")
