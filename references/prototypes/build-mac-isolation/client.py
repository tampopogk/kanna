#!/usr/bin/env python3
import argparse
import base64
import json
import os
from pathlib import Path
import shutil
import sys
import time
import urllib.error
import urllib.request

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "xcodebuild-shim"))
import intake
import model

FINAL = {"finished", "cancelled", "error", "lost", "retired"}


def api(action, body, token=None, url=None):
    token = token or Path(os.environ["ISOLATION_TOKEN_FILE"]).read_text().strip()
    url = url or os.environ.get("ISOLATION_URL", "http://127.0.0.1:48800")
    req = urllib.request.Request(url + "/" + action, data=json.dumps(body).encode(),
                                 headers={"Authorization": "Bearer " + token})
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.load(error)


def submit(root, argv, ui=False, app=None, task=None, cwd=".", extra=None, token=None):
    payload, snap = model.snapshot(root)
    request = {"identity": [os.uname().nodename, model.repository_id(root), task or model.digest(str(root).encode())[:24]],
               "root": str(root), "cwd": cwd, "snapshot_hash": snap["hash"], "argv": argv,
               "developer_dir": "/Applications/Xcode.app/Contents/Developer", "ui": ui, "app": app}
    request.update(extra or {})
    code, result = api("submit", {"request": request, "snapshot": base64.b64encode(payload).decode()}, token)
    if code != 200:
        raise RuntimeError("submit refused %s: %s" % (code, result))
    return result["job"]


def attach(job, destination, token=None, echo=True):
    offsets = {"stdout.raw": 0, "stderr.raw": 0}
    while True:
        code, status = api("status", {"job": job, "offsets": offsets}, token)
        if code != 200:
            raise RuntimeError("status refused %s: %s" % (code, status))
        for name, data in status.pop("streams").items():
            chunk = base64.b64decode(data)
            if echo:
                (sys.stdout.buffer if name == "stdout.raw" else sys.stderr.buffer).write(chunk)
            offsets[name] += len(chunk)
        if status["state"] in FINAL:
            break
        time.sleep(0.5)
    if destination.exists():
        shutil.rmtree(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    store = intake.Store(destination)
    _, listing = api("artifacts", {"job": job}, token)
    for item in listing["manifest"]:
        _, blob = api("artifact", {"job": job, "path": item["path"]}, token)
        store.put(item["path"], base64.b64decode(blob["data"]), item["sha256"])
    store.close()
    model.save(destination / "status.json", status)
    diag = destination / "diagnostics.txt"
    if echo and diag.exists():
        sys.stderr.write(diag.read_text())
    return status


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="mode", required=True)
    p = sub.add_parser("run")
    p.add_argument("--root", type=Path, default=Path.cwd())
    p.add_argument("--ui", action="store_true")
    p.add_argument("--app")
    p.add_argument("argv", nargs=argparse.REMAINDER)
    p = sub.add_parser("attach")
    p.add_argument("job")
    p.add_argument("destination", type=Path)
    p = sub.add_parser("cancel")
    p.add_argument("job")
    args = parser.parse_args()
    if args.mode == "cancel":
        print(api("cancel", {"job": args.job}))
        return 0
    if args.mode == "attach":
        status = attach(args.job, args.destination)
    else:
        root = args.root.resolve()
        argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
        job = submit(root, argv, args.ui, args.app)
        print("job=" + job, file=sys.stderr, flush=True)
        status = attach(job, root / ".kanna/build" / job)
    code = status.get("build_exit")
    if code is None:
        print("isolation transport/model error: %s" % status.get("error"), file=sys.stderr)
        return 125
    return code if code >= 0 else 128 - code


if __name__ == "__main__":
    sys.exit(main())
