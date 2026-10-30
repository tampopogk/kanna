#!/usr/bin/env python3
"""Trusted-input model, not a service to expose to a network. See README."""
import argparse
import base64
import contextlib
import fcntl
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
import uuid

EXCLUDES = {".git", ".kanna", "target", ".build", "node_modules", "__pycache__", ".shim-last-job.json"}
PATH_FLAGS = {"-project", "-workspace", "-derivedDataPath", "-resultBundlePath",
              "-archivePath", "--scratch-path"}


def run(argv, **kw):
    return subprocess.check_output(argv, **kw).decode().strip()


def save(path, value):
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(value, indent=2) + "\n")
    temp.replace(path)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def repository_id(root):
    common = Path(run(["git", "-C", str(root), "rev-parse", "--git-common-dir"]))
    return digest(str((root / common).resolve()).encode())


def pack(path):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w:gz") as archive:
        for child in sorted(path.iterdir()):
            archive.add(child, arcname=child.name)
    return out.getvalue()


def unpack(data, path):
    # Both peers and repositories are trusted in this deliberately small model.
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        archive.extractall(path)


def snapshot(root):
    """Caller must stop other writers during capture, as the shim blocks its agent."""
    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp)
        src = stage / "src"
        shutil.copytree(root, src, symlinks=True,
                        ignore=shutil.ignore_patterns(*EXCLUDES))
        entries = []
        folded = set()
        for p in sorted(src.rglob("*")):
            rel = str(p.relative_to(src))
            if rel.casefold() in folded:
                raise ValueError("case-insensitive path collision: " + rel)
            folded.add(rel.casefold())
            if p.is_symlink():
                p.resolve().relative_to(src.resolve())  # reject escaping links
                value = "link:" + os.readlink(p)
            elif p.is_file():
                value = digest(p.read_bytes())
            else:
                continue
            entries.append([rel, p.lstat().st_mode & 0o777, value])
        run(["git", "-C", str(root), "bundle", "create", str(stage / "source.bundle"), "HEAD"])
        metadata = {"commit": run(["git", "-C", str(root), "rev-parse", "HEAD"]),
                    "dirty": run(["git", "-C", str(root), "status", "--porcelain"]),
                    "files": entries}
        metadata["hash"] = digest(json.dumps(entries, separators=(",", ":")).encode())
        save(stage / "snapshot.json", metadata)
        return pack(stage), metadata


def translate(argv, linux, src, results, cache, cwd):
    mapped = list(argv)
    paths = {str(src): linux}
    for i, arg in enumerate(argv[:-1]):
        if arg not in PATH_FLAGS:
            continue
        given = Path(argv[i + 1])
        absolute = given if given.is_absolute() else Path(linux) / cwd / given
        rel = Path(os.path.normpath(absolute)).relative_to(linux)
        if arg in {"-project", "-workspace"}:
            target = src / rel
        else:
            target = (cache if arg in {"-derivedDataPath", "--scratch-path"} else results) / rel
            target.parent.mkdir(parents=True, exist_ok=True)
        mapped[i + 1] = str(target)
        paths[str(target)] = str(absolute)
    if argv[0] == "xcodebuild":
        if "-derivedDataPath" not in argv:
            mapped += ["-derivedDataPath", str(cache / "DerivedData")]
        if "test" in argv and "-resultBundlePath" not in argv:
            mapped += ["-resultBundlePath", str(results / "Tests.xcresult")]
    # Xcode may spell /private/tmp as /tmp (also /private/var as /var).
    for mac, linux_path in list(paths.items()):
        if mac.startswith(("/private/tmp/", "/private/var/")):
            paths[mac[len("/private"):]] = linux_path
    return mapped, paths


def diagnostics(raw, mappings):
    lines = []
    for line in raw.decode("utf-8", errors="replace").splitlines():
        if not re.search(r":\d+(?::\d+)?: (?:warning|error|note):", line):
            continue
        for mac, linux in sorted(mappings.items(), key=lambda x: -len(x[0])):
            if line.startswith(mac + "/") or line.startswith(mac + ":"):
                line = linux + line[len(mac):]
                break
        lines.append(line)
    return "\n".join(lines) + ("\n" if lines else "")


@contextlib.contextmanager
def lock(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        yield


def worker(state, job_id):
    job = state / "jobs" / job_id
    request = json.loads((job / "request.json").read_text())
    status = {"job": job_id, "state": "waiting", "build_exit": None,
              "artifacts": "pending", "started": time.time()}
    save(job / "status.json", status)
    results = job / "results"
    results.mkdir(exist_ok=True)
    mirror_id = digest(json.dumps(request["identity"]).encode())[:24]
    mirror = state / "mirrors" / mirror_id
    try:
        with lock(mirror / "mirror.lock"), lock(state / "heavy-slot.lock"):
            if (job / "cancel").exists():
                raise RuntimeError("cancelled before execution")
            unpack((job / "upload.tgz").read_bytes(), job / "input")
            snap = json.loads((job / "input/snapshot.json").read_text())
            src = mirror / "src"
            if not src.exists():
                run(["git", "clone", str(job / "input/source.bundle"), str(src)], stderr=subprocess.STDOUT)
            else:
                run(["git", "-C", str(src), "fetch", str(job / "input/source.bundle"), "HEAD"], stderr=subprocess.STDOUT)
            run(["git", "-C", str(src), "reset", "--hard", snap["commit"]])
            run(["rsync", "-a", "--delete", "--exclude=.git", str(job / "input/src") + "/", str(src) + "/"])
            env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(Path.home()),
                   "DEVELOPER_DIR": request["developer_dir"], "TMPDIR": tempfile.gettempdir()}
            version = run(["/usr/bin/xcodebuild", "-version"], env=env)
            cache = mirror / "derived" / digest(version.encode())[:12]
            cache.mkdir(parents=True, exist_ok=True)
            argv, paths = translate(request["argv"], request["root"], src, results, cache, request["cwd"])
            identity = {"uid": os.getuid(), "user": run(["id", "-un"]),
                        "console_uid": os.stat("/dev/console").st_uid,
                        "launchd_domain": run(["launchctl", "managername"])}
            if request["ui"] and (identity["uid"] != identity["console_uid"] or identity["launchd_domain"] != "Aqua"):
                raise RuntimeError("UI job requires its user to own the Aqua console session")
            save(results / "execution.json", {"argv": argv, "paths": paths, "snapshot": snap,
                 "identity": identity, "xcode": version,
                 "swift": run(["xcrun", "swift", "--version"], env=env),
                 "sdk": run(["xcodebuild", "-showsdks"], env=env),
                 "arch": run(["uname", "-m"]), "request": request})
            with (results / "stdout.raw").open("wb") as out, (results / "stderr.raw").open("wb") as err:
                process = subprocess.Popen(argv, cwd=src / request["cwd"], env=env,
                                           stdout=out, stderr=err, start_new_session=True)
                status.update(state="running", pid=process.pid)
                save(job / "status.json", status)
                cancelled = False
                while process.poll() is None:
                    if (job / "cancel").exists() and not cancelled:
                        os.killpg(process.pid, signal.SIGTERM)
                        cancelled = time.monotonic()
                    if cancelled and time.monotonic() - cancelled > 5:
                        with contextlib.suppress(ProcessLookupError):
                            os.killpg(process.pid, signal.SIGKILL)
                    time.sleep(0.1)
                if cancelled:
                    # Also terminate grandchildren after their parent has exited.
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(process.pid, signal.SIGKILL)
                status.update(build_exit=process.returncode, state="cancelled" if cancelled else "finished")
            raw = (results / "stdout.raw").read_bytes() + (results / "stderr.raw").read_bytes()
            (results / "diagnostics.txt").write_text(diagnostics(raw, paths))
            for bundle in list(results.rglob("*.xcresult")):
                summary = subprocess.run(["xcrun", "xcresulttool", "get", "test-results", "summary", "--path", str(bundle)],
                                         env=env, capture_output=True)
                (results / "tests.json").write_bytes(summary.stdout)
                exported = subprocess.run(["xcrun", "xcresulttool", "export", "attachments", "--path", str(bundle),
                                "--output-path", str(results / "attachments")], env=env, capture_output=True)
                save(results / "xcresult-export.json", {"summary_exit": summary.returncode,
                     "attachments_exit": exported.returncode, "stderr": (summary.stderr + exported.stderr).decode(errors="replace")})
                if summary.returncode or exported.returncode:
                    raise RuntimeError("xcresult export failed; build_exit is recorded separately")
            if request.get("app"):
                products = list(cache.rglob(request["app"]))
                if not products:
                    raise RuntimeError("required app not produced: " + request["app"])
                with tarfile.open(results / "app.tgz", "w:gz") as tar:
                    tar.add(products[0], arcname=products[0].name)
            save(results / "manifest.json", [{"path": str(p.relative_to(results)), "mac": str(p),
                 "sha256": digest(p.read_bytes()), "bytes": p.stat().st_size}
                 for p in sorted(results.rglob("*")) if p.is_file()])
            (job / "artifacts.tgz").write_bytes(pack(results))
            status["artifacts"] = "ready"
    except Exception as exc:
        status.update(state="error", error=str(exc), artifacts="failed")
    status["ended"] = time.time()
    save(job / "status.json", status)


def serve(state, port, token):
    state.mkdir(parents=True, exist_ok=True)
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            if self.headers.get("Authorization") != "Bearer " + token:
                self.send_error(403)
                return
            try:
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                job_id = str(uuid.UUID(body["job"]))
                job = state / "jobs" / job_id
                if self.path == "/submit":
                    try:
                        job.mkdir(parents=True)
                    except FileExistsError:
                        if json.loads((job / "request.json").read_text()) != body["request"]:
                            raise ValueError("request ID reused with different request")
                    else:
                        save(job / "request.json", body["request"])
                        (job / "upload.tgz").write_bytes(base64.b64decode(body["snapshot"]))
                        save(job / "status.json", {"job": job_id, "state": "queued"})
                        with (job / "supervisor.log").open("wb") as log:
                            subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "worker", str(state), job_id],
                                             start_new_session=True, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
                    result = {"job": job_id}
                elif self.path == "/cancel":
                    (job / "cancel").touch()
                    result = {"job": job_id}
                elif self.path == "/status":
                    result = json.loads((job / "status.json").read_text())
                    result["streams"] = {}
                    for name in ("stdout.raw", "stderr.raw"):
                        path = job / "results" / name
                        data = path.read_bytes() if path.exists() else b""
                        offset = body.get("offsets", {}).get(name, 0)
                        result["streams"][name] = base64.b64encode(data[offset:]).decode()
                elif self.path == "/artifacts":
                    result = {"data": base64.b64encode((job / "artifacts.tgz").read_bytes()).decode()}
                else:
                    raise ValueError("unknown operation")
                encoded = json.dumps(result).encode()
                self.send_response(200)
                self.end_headers()
                self.wfile.write(encoded)
            except Exception as exc:
                self.send_error(400, str(exc))
    # Serialized admission makes request-ID deduplication atomic in this model.
    http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()


def api(action, body):
    req = urllib.request.Request(os.environ["SHIM_URL"] + "/" + action,
          data=json.dumps(body).encode(), headers={"Authorization": "Bearer " + Path(os.environ["SHIM_TOKEN_FILE"]).read_text().strip()})
    with urllib.request.urlopen(req, timeout=30) as response:
        return json.load(response)


def attach(job_id, destination):
    offsets = {"stdout.raw": 0, "stderr.raw": 0}
    while True:
        status = api("status", {"job": job_id, "offsets": offsets})
        for name, data in status.pop("streams").items():
            chunk = base64.b64decode(data)
            (sys.stdout.buffer if name == "stdout.raw" else sys.stderr.buffer).write(chunk)
            offsets[name] += len(chunk)
        sys.stdout.flush()
        sys.stderr.flush()
        if status["state"] in {"finished", "cancelled", "error"}:
            destination.mkdir(parents=True, exist_ok=True)
            save(destination / "status.json", status)
            if status["artifacts"] == "ready":
                unpack(base64.b64decode(api("artifacts", {"job": job_id})["data"]), destination)
                for item in json.loads((destination / "manifest.json").read_text()):
                    if digest((destination / item["path"]).read_bytes()) != item["sha256"]:
                        raise RuntimeError("artifact checksum mismatch")
                execution = json.loads((destination / "execution.json").read_text())
                request = execution["request"]
                for i, arg in enumerate(request["argv"][:-1]):
                    if arg in {"-resultBundlePath", "-archivePath"}:
                        given = Path(request["argv"][i + 1])
                        target = given if given.is_absolute() else Path(request["root"]) / request["cwd"] / given
                        rel = Path(os.path.normpath(target)).relative_to(request["root"])
                        source = destination / rel
                        if source.is_dir():
                            shutil.copytree(source, target, symlinks=True, dirs_exist_ok=True)
                        elif source.is_file():
                            target.parent.mkdir(parents=True, exist_ok=True)
                            shutil.copy2(source, target)
                sys.stderr.write((destination / "diagnostics.txt").read_text())
            else:
                raise RuntimeError("remote job error: " + json.dumps(status))
            return status["build_exit"] if status["build_exit"] >= 0 else 128 - status["build_exit"]
        time.sleep(0.3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="mode", required=True)
    p = sub.add_parser("serve")
    p.add_argument("state", type=Path)
    p.add_argument("--port", type=int, default=8765)
    p.add_argument("--token-file", type=Path, required=True)
    p = sub.add_parser("worker")
    p.add_argument("state", type=Path)
    p.add_argument("job")
    p = sub.add_parser("run")
    p.add_argument("--root", type=Path, default=Path.cwd())
    p.add_argument("--cwd", default=".")
    p.add_argument("--task")
    p.add_argument("--ui", action="store_true")
    p.add_argument("--app")
    p.add_argument("--detach", action="store_true")
    p.add_argument("--developer-dir", default="/Applications/Xcode.app/Contents/Developer")
    p.add_argument("argv", nargs=argparse.REMAINDER)
    p = sub.add_parser("attach")
    p.add_argument("job")
    p.add_argument("destination", type=Path)
    p = sub.add_parser("cancel")
    p.add_argument("job")
    args = parser.parse_args()
    if args.mode == "serve":
        serve(args.state.resolve(), args.port, args.token_file.read_text().strip())
    elif args.mode == "worker":
        worker(args.state, args.job)
    elif args.mode == "cancel":
        api("cancel", {"job": args.job})
    elif args.mode == "attach":
        return attach(args.job, args.destination)
    else:
        root = args.root.resolve()
        payload, snap = snapshot(root)
        job_id = str(uuid.uuid4())
        request = {"identity": [os.uname().nodename, repository_id(root), args.task or digest(str(root).encode())[:24]],
                   "root": str(root), "cwd": args.cwd, "snapshot_hash": snap["hash"],
                   "argv": args.argv[1:] if args.argv[0] == "--" else args.argv,
                   "developer_dir": args.developer_dir, "ui": args.ui, "app": args.app}
        api("submit", {"job": job_id, "request": request, "snapshot": base64.b64encode(payload).decode()})
        print("job=" + job_id, file=sys.stderr, flush=True)
        save(root / ".shim-last-job.json", {"job": job_id})
        if not args.detach:
            return attach(job_id, root / ".kanna/build" / job_id)


if __name__ == "__main__":
    try:
        sys.exit(main() or 0)
    except (OSError, ValueError, RuntimeError) as error:
        print("shim transport/model error: " + str(error), file=sys.stderr)
        sys.exit(125)  # CLI error; status.build_exit remains separate.
