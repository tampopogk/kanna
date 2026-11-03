#!/usr/bin/env python3
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, "/usr/local/kanna")
import model
import wire

HOME = Path.home()
STATE = HOME / ".kanna"
JOBS = STATE / "jobs"
MIRRORS = STATE / "mirrors"
SOCKET = STATE / "agent.sock"
MAX_SNAPSHOT = 1 << 30
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")


def out(argv):
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=30).stdout.strip()
    except (OSError, subprocess.SubprocessError) as error:
        return "error: %s" % error


def identity():
    ioreg = out(["ioreg", "-rd1", "-c", "IOPlatformExpertDevice"])
    def field(key):
        m = re.search(r'"%s" = "([^"]*)"' % key, ioreg)
        return m.group(1) if m else None
    return {"user": out(["id", "-un"]), "uid": os.getuid(), "groups": out(["id", "-Gn"]),
            "console_uid": os.stat("/dev/console").st_uid,
            "console_user": out(["stat", "-f", "%Su", "/dev/console"]),
            "launchd_domain": out(["launchctl", "managername"]),
            "hostname": socket.gethostname(), "serial": field("IOPlatformSerialNumber"),
            "platform_uuid": field("IOPlatformUUID"),
            "vsock_cid": out(["/usr/local/kanna/vsockrelay", "cid"]),
            "macos": out(["sw_vers", "-productVersion"]), "build": out(["sw_vers", "-buildVersion"]),
            "xcode": out(["/usr/bin/xcodebuild", "-version"]), "arch": out(["uname", "-m"]),
            "interfaces": out(["ifconfig", "-l"]), "admin": "admin" in out(["id", "-Gn"]).split()}


def save(path, value):
    model.save(path, value)


def worker(job_id):
    job = JOBS / job_id
    request = json.loads((job / "request.json").read_text())
    status = {"job": job_id, "state": "waiting", "build_exit": None, "artifacts": "pending", "started": time.time()}
    save(job / "status.json", status)
    results = job / "results"
    results.mkdir(exist_ok=True)
    mirror = MIRRORS / model.digest(json.dumps(request["identity"]).encode())[:24]
    try:
        with model.lock(mirror / "mirror.lock"), model.lock(STATE / "heavy-slot.lock"):
            if (job / "cancel").exists():
                raise RuntimeError("cancelled before execution")
            model.unpack((job / "upload.tgz").read_bytes(), job / "input")
            snap = json.loads((job / "input/snapshot.json").read_text())
            src = mirror / "src"
            if not src.exists():
                model.run(["git", "clone", "-q", str(job / "input/source.bundle"), str(src)], stderr=subprocess.STDOUT)
            else:
                model.run(["git", "-C", str(src), "fetch", "-q", str(job / "input/source.bundle"), "HEAD"], stderr=subprocess.STDOUT)
            model.run(["git", "-C", str(src), "reset", "-q", "--hard", snap["commit"]])
            model.run(["rsync", "-a", "--delete", "--exclude=.git", str(job / "input/src") + "/", str(src) + "/"])
            env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(HOME),
                   "DEVELOPER_DIR": request["developer_dir"], "TMPDIR": tempfile.gettempdir(),
                   "KANNA_JOB": job_id}
            proxy = "http://job:%s@127.0.0.1:3128" % request["capability"]
            env.update(HTTPS_PROXY=proxy, https_proxy=proxy, HTTP_PROXY=proxy, http_proxy=proxy)
            version = model.run(["/usr/bin/xcodebuild", "-version"], env=env)
            cache = mirror / "derived" / model.digest(version.encode())[:12]
            cache.mkdir(parents=True, exist_ok=True)
            argv, paths = model.translate(request["argv"], request["root"], src, results, cache, request["cwd"])
            who = identity()
            if request["ui"] and (who["uid"] != who["console_uid"] or who["launchd_domain"] != "Aqua"):
                raise RuntimeError("UI job requires its user to own the Aqua console session")
            save(results / "execution.json", {"argv": argv, "paths": paths, "snapshot": snap, "identity": who,
                 "xcode": version, "request": {k: v for k, v in request.items() if k != "capability"}})
            with (results / "stdout.raw").open("wb") as o, (results / "stderr.raw").open("wb") as e:
                process = subprocess.Popen(argv, cwd=src / request["cwd"], env=env, stdout=o, stderr=e,
                                           start_new_session=True)
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
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(process.pid, signal.SIGKILL)
                status.update(build_exit=process.returncode, state="cancelled" if cancelled else "finished")
            raw = (results / "stdout.raw").read_bytes() + (results / "stderr.raw").read_bytes()
            (results / "diagnostics.txt").write_text(model.diagnostics(raw, paths))
            for bundle in list(results.rglob("*.xcresult")):
                summary = subprocess.run(["xcrun", "xcresulttool", "get", "test-results", "summary", "--path", str(bundle)],
                                         env=env, capture_output=True)
                (results / "tests.json").write_bytes(summary.stdout)
                subprocess.run(["xcrun", "xcresulttool", "export", "attachments", "--path", str(bundle),
                                "--output-path", str(results / "attachments")], env=env, capture_output=True)
            if request.get("app"):
                products = list(cache.rglob(request["app"]))
                if not products:
                    raise RuntimeError("required app not produced: " + request["app"])
                subprocess.run(["ditto", "-c", "-k", "--keepParent", str(products[0]), str(results / "app.zip")], check=True)
            status["artifacts"] = "ready"
    except Exception as exc:
        status.update(state="error", error=str(exc), artifacts="failed")
    status["ended"] = time.time()
    save(job / "status.json", status)


def artifacts(results):
    for root, dirs, files in os.walk(results):
        dirs.sort()
        for name in sorted(files):
            p = Path(root) / name
            st = p.lstat()
            if not (st.st_mode & 0o170000 == 0o100000):
                continue
            yield str(p.relative_to(results)), p


def job_dir(header):
    job_id = header.get("job", "")
    if not NAME.match(job_id):
        raise ValueError("bad job id")
    return job_id, JOBS / job_id


def handle(conn):
    with conn:
        header, payload = wire.recv(conn, MAX_SNAPSHOT)
        op = header.get("op")
        if op == "hello":
            wire.send(conn, {"op": "hello", "identity": identity()})
        elif op == "credential":
            if not NAME.match(header.get("name", "")):
                raise ValueError("bad credential name")
            d = STATE / "credentials"
            d.mkdir(parents=True, exist_ok=True)
            d.chmod(0o700)
            p = d / header["name"]
            p.write_bytes(payload)
            p.chmod(0o600)
            wire.send(conn, {"op": "ok"})
        elif op == "run":
            job_id, job = job_dir(header)
            try:
                job.mkdir(parents=True)
            except FileExistsError:
                wire.send(conn, {"op": "ok", "job": job_id, "duplicate": True})
                return
            request = dict(header["request"], capability=header["capability"])
            save(job / "request.json", request)
            (job / "upload.tgz").write_bytes(payload)
            with (job / "worker.log").open("wb") as log:
                subprocess.Popen([sys.executable, __file__, "worker", job_id], start_new_session=True,
                                 stdin=subprocess.DEVNULL, stdout=log, stderr=log)
            wire.send(conn, {"op": "ok", "job": job_id})
        elif op == "cancel":
            job_id, job = job_dir(header)
            (job / "cancel").touch()
            wire.send(conn, {"op": "ok", "job": job_id})
        elif op == "attach":
            job_id, job = job_dir(header)
            status_path = job / "status.json"
            status = json.loads(status_path.read_text()) if status_path.exists() else {"state": "queued"}
            offsets = header.get("offsets", {})
            for name in ("stdout.raw", "stderr.raw"):
                p = job / "results" / name
                data = p.read_bytes() if p.exists() else b""
                wire.send(conn, {"op": "stream", "job": job_id, "name": name}, data[offsets.get(name, 0):])
            if status.get("state") in ("finished", "cancelled", "error"):
                for rel, p in artifacts(job / "results"):
                    data = p.read_bytes()
                    wire.send(conn, {"op": "file", "job": job_id, "capability": header.get("capability"),
                                     "path": rel, "sha256": hashlib.sha256(data).hexdigest()}, data)
            wire.send(conn, {"op": "status", "job": job_id, "status": status})
        else:
            wire.send(conn, {"op": "error", "error": "unknown op"})


def serve():
    if out(["id", "-un"]) != "kanna-build":
        sys.exit(0)
    STATE.mkdir(mode=0o700, exist_ok=True)
    JOBS.mkdir(exist_ok=True)
    with contextlib.suppress(FileNotFoundError):
        SOCKET.unlink()
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(SOCKET))
    SOCKET.chmod(0o600)
    server.listen(16)
    while True:
        conn, _ = server.accept()
        def run(c=conn):
            try:
                handle(c)
            except Exception as error:
                with contextlib.suppress(Exception):
                    wire.send(c, {"op": "error", "error": str(error)})
        threading.Thread(target=run, daemon=True).start()


if __name__ == "__main__":
    if sys.argv[1:2] == ["worker"]:
        worker(sys.argv[2])
    else:
        serve()
