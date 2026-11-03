#!/usr/bin/env python3
import os
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent / "xcodebuild-shim"))
import client

root = Path(sys.argv[1]).resolve()
token = Path(sys.argv[2]).read_text().strip()
dest = Path(sys.argv[3]).resolve()
script = sys.argv[4]
prelude = 'R="$HOME/.kanna/jobs/$KANNA_JOB/results"; '
job = client.submit(root, ["/bin/sh", "-c", prelude + script], ui="--ui" in sys.argv[5:], token=token)
print("job=" + job, file=sys.stderr)
status = client.attach(job, dest, token=token)
print("\nstate=%s exit=%s vm=%s" % (status.get("state"), status.get("build_exit"), status.get("vm")), file=sys.stderr)
