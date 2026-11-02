#!/usr/bin/env python3
import base64
import contextlib
import hashlib
import http.server
import ipaddress
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import socketserver
import subprocess
import sys
import threading
import time
import uuid

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import intake
import wire

ROOT = Path(os.environ.get("ISOLATION_ROOT", Path.home() / "kanna-isolation"))
STATE = ROOT / "state"
VMS = STATE / "vms"
JOBS = STATE / "jobs"
GOLDEN = Path(os.environ.get("ISOLATION_GOLDEN", ROOT / "tart-home/vms/golden"))
VMRUN = ROOT / "bin/vmrun"
ADMIN_SOCKET = STATE / "admin.sock"
TENANT_PORT = 48800
MAX_STREAM = 64 << 20
MAX_SNAPSHOT = 512 << 20
MEMORY_BUDGET_GIB = 24
MAX_RUNNING_VMS = 2
MIN_FREE_DISK_GIB = 40
ALLOW = {("github.com", 443), ("codeload.github.com", 443), ("localtest.me", 443)}
FINAL = {"finished", "cancelled", "error", "lost", "retired"}
STATUS_FIELDS = {"state": str, "build_exit": (int, type(None)), "artifacts": str, "error": str}

lock = threading.RLock()
vm_procs = {}
active_caps = {}
egress_servers = {}


def save(path, value):
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    tmp.replace(path)


def load(path, default=None):
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        return default


def event(kind, **fields):
    with lock, (STATE / "events.jsonl").open("a") as f:
        f.write(json.dumps(dict(fields, t=time.time(), kind=kind), sort_keys=True) + "\n")


def tenants():
    return load(STATE / "tenants.json", {})


def tenant_for(token):
    if not token:
        return None
    return tenants().get(hashlib.sha256(token.encode()).hexdigest())


def vm_dir(vm):
    if not vm.replace("-", "").isalnum():
        raise ValueError("bad vm id")
    return VMS / vm


def vm_owner(vm):
    return load(vm_dir(vm) / "owner.json", {})


def agent_call(vm, header, payload=b"", max_payload=MAX_STREAM, frames=False):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(120)
    s.connect(str(vm_dir(vm) / "sock/agent.sock"))
    with s:
        wire.send(s, header, payload)
        if not frames:
            return wire.recv(s, max_payload)
        out = []
        while True:
            try:
                out.append(wire.recv(s, max_payload))
            except EOFError:
                return out
            if out[-1][0].get("op") in ("status", "error"):
                return out


def admit_vm(vm, memory):
    running = [v for v, p in vm_procs.items() if p.poll() is None]
    if len(running) >= MAX_RUNNING_VMS:
        raise ValueError("admission: %d VMs already running" % len(running))
    used = sum(vm_owner(v).get("memory_gib", 0) for v in running)
    if used + memory > MEMORY_BUDGET_GIB:
        raise ValueError("admission: memory budget %d GiB exceeded" % MEMORY_BUDGET_GIB)
    free = shutil.disk_usage(STATE).free / (1 << 30)
    if free < MIN_FREE_DISK_GIB:
        raise ValueError("admission: only %.1f GiB free on host" % free)


def vm_create(vm, tenant):
    d = vm_dir(vm)
    if d.exists():
        raise ValueError("vm exists")
    if tenant not in tenants().values():
        raise ValueError("unknown tenant")
    VMS.mkdir(parents=True, exist_ok=True)
    tmp = VMS / (vm + ".creating")
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir()
    for name in ("config.json", "nvram.bin", "disk.img"):
        subprocess.run(["cp", "-c", str(GOLDEN / name), str(tmp / name)], check=True)
    (tmp / "sock").mkdir(mode=0o700)
    save(tmp / "owner.json", {"vm": vm, "tenant": tenant, "created": time.time(), "state": "stopped",
                              "golden": str(GOLDEN)})
    tmp.rename(d)
    event("vm-create", vm=vm, tenant=tenant)
    return vm_owner(vm)


def egress_handler(vm):
    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            head = b""
            while b"\r\n\r\n" not in head and len(head) < 16384:
                chunk = self.request.recv(4096)
                if not chunk:
                    return
                head += chunk
            lines = head.split(b"\r\n\r\n")[0].decode("latin-1").split("\r\n")
            decision = {"vm": vm, "request": lines[0][:200]}
            try:
                method, target, _ = lines[0].split(" ", 2)
                headers = dict(l.split(":", 1) for l in lines[1:] if ":" in l)
                auth = {k.strip().lower(): v.strip() for k, v in headers.items()}.get("proxy-authorization", "")
                cap = ""
                if auth.lower().startswith("basic "):
                    cap = base64.b64decode(auth[6:]).decode(errors="replace").partition(":")[2]
                with lock:
                    job = active_caps.get(cap)
                if not job or job["vm"] != vm:
                    raise PermissionError("no active job capability for this VM")
                decision["job"] = job["job"]
                if method != "CONNECT":
                    raise PermissionError("only CONNECT is allowed")
                host, _, port = target.rpartition(":")
                port = int(port)
                if (host.lower(), port) not in ALLOW:
                    raise PermissionError("destination not on allowlist")
                addrs = sorted({a[4][0] for a in socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)})
                decision["resolved"] = addrs
                bad = [a for a in addrs if not ipaddress.ip_address(a.split("%")[0]).is_global]
                if bad:
                    raise PermissionError("resolved to non-global address %s" % bad)
                upstream = socket.create_connection((addrs[0], port), timeout=15)
            except Exception as error:
                decision.update(allowed=False, reason=str(error))
                event("egress", **decision)
                self.request.sendall(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                return
            decision["allowed"] = True
            event("egress", **decision)
            self.request.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
            rest = head.split(b"\r\n\r\n", 1)[1]
            if rest:
                upstream.sendall(rest)
            pump(self.request, upstream)
    return Handler


def pump(a, b):
    def one(x, y):
        with contextlib.suppress(OSError):
            while True:
                data = x.recv(65536)
                if not data:
                    break
                y.sendall(data)
        with contextlib.suppress(OSError):
            y.shutdown(socket.SHUT_WR)
    t = threading.Thread(target=one, args=(b, a), daemon=True)
    t.start()
    one(a, b)
    t.join()
    b.close()


class UnixServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True


def vm_start(vm, cpus=6, memory=8):
    with lock:
        owner = vm_owner(vm)
        if not owner:
            raise ValueError("no such vm")
        if vm in vm_procs and vm_procs[vm].poll() is None:
            raise ValueError("already running")
        admit_vm(vm, memory)
        d = vm_dir(vm)
        sock = d / "sock"
        for p in sock.glob("*.sock"):
            p.unlink()
        server = UnixServer(str(sock / "egress.sock"), egress_handler(vm))
        os.chmod(sock / "egress.sock", 0o600)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        egress_servers[vm] = server
        log = (d / "vmrun.log").open("ab")
        proc = subprocess.Popen([str(VMRUN), str(d), str(sock), str(cpus), str(memory)],
                                stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
        vm_procs[vm] = proc
        owner.update(state="running", cpus=cpus, memory_gib=memory, pid=proc.pid, started=time.time())
        save(d / "owner.json", owner)
        event("vm-start", vm=vm, tenant=owner["tenant"], pid=proc.pid, cpus=cpus, memory_gib=memory)
        return owner


def vm_wait(vm, timeout=600):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        proc = vm_procs.get(vm)
        if proc and proc.poll() is not None:
            raise RuntimeError("vmrun exited %s" % proc.returncode)
        try:
            header, _ = agent_call(vm, {"op": "hello"}, max_payload=1 << 20)
            owner = vm_owner(vm)
            owner["identity"] = header.get("identity")
            save(vm_dir(vm) / "owner.json", owner)
            return header
        except (OSError, EOFError, ValueError) as error:
            last = str(error)
        time.sleep(3)
    raise RuntimeError("guest agent not ready: %s" % last)


def vm_stop(vm, crash=False):
    with lock:
        proc = vm_procs.get(vm)
        owner = vm_owner(vm)
        if proc and proc.poll() is None:
            proc.send_signal(signal.SIGKILL if crash else signal.SIGTERM)
            with contextlib.suppress(subprocess.TimeoutExpired):
                proc.wait(timeout=90)
            if proc.poll() is None:
                proc.kill()
                proc.wait()
        server = egress_servers.pop(vm, None)
        if server:
            server.shutdown()
            server.server_close()
        for cap, job in list(active_caps.items()):
            if job["vm"] == vm:
                del active_caps[cap]
        if owner:
            owner["state"] = "crashed" if crash else "stopped"
            save(vm_dir(vm) / "owner.json", owner)
        event("vm-stop", vm=vm, crash=crash)
        return owner


def vm_retire(vm):
    owner = vm_owner(vm)
    vm_stop(vm)
    with lock:
        for job in JOBS.iterdir() if JOBS.exists() else []:
            req = load(job / "host.json", {})
            if req.get("vm") == vm:
                st = load(job / "status.json", {})
                if st.get("state") not in FINAL:
                    st.update(state="retired", error="guest retired")
                    save(job / "status.json", st)
        shutil.rmtree(vm_dir(vm))
    event("vm-retire", vm=vm, tenant=owner.get("tenant"))
    return {"vm": vm, "retired": True, "exists": vm_dir(vm).exists()}


def credential(vm, name, data):
    header, _ = agent_call(vm, {"op": "credential", "name": name}, data, max_payload=1 << 20)
    event("credential", vm=vm, name=name, ok=header.get("op") == "ok")
    return header


def job_paths(job):
    return JOBS / str(uuid.UUID(job))


def run_job(job, vm, cap, request, snapshot):
    d = job_paths(job)
    status = load(d / "status.json")
    rejected = []
    try:
        header, _ = agent_call(vm, {"op": "run", "job": job, "capability": cap, "request": request}, snapshot,
                               max_payload=1 << 20)
        if header.get("op") != "ok" or header.get("job") != job:
            raise RuntimeError("guest refused run: %r" % header)
        offsets = {"stdout.raw": 0, "stderr.raw": 0}
        failures = 0
        while True:
            time.sleep(0.5)
            try:
                frames = agent_call(vm, {"op": "attach", "job": job, "offsets": offsets, "capability": cap},
                                    frames=True)
                failures = 0
            except (OSError, EOFError, ValueError, socket.timeout) as error:
                failures += 1
                if failures > 20 or vm_procs.get(vm) is None or vm_procs[vm].poll() is not None:
                    raise RuntimeError("guest lost: %s" % error)
                continue
            files = []
            guest = None
            for h, payload in frames:
                op = h.get("op")
                if h.get("job") != job:
                    rejected.append({"op": op, "reason": "frame for another job", "claimed": str(h.get("job"))[:80]})
                    continue
                if op == "stream" and h.get("name") in offsets:
                    if offsets[h["name"]] + len(payload) > MAX_STREAM:
                        rejected.append({"op": op, "reason": "stream limit"})
                        continue
                    with (d / h["name"]).open("ab") as f:
                        f.write(payload)
                    offsets[h["name"]] += len(payload)
                elif op == "file":
                    files.append((h, payload))
                elif op == "status" and isinstance(h.get("status"), dict):
                    guest = h["status"]
                else:
                    rejected.append({"op": str(op)[:40], "reason": "unexpected frame"})
            if guest is None:
                continue
            clean = {k: guest[k] for k, t in STATUS_FIELDS.items() if isinstance(guest.get(k), t)}
            ignored = sorted(set(guest) - set(STATUS_FIELDS) - {"job", "started", "ended", "pid"})
            if clean.get("state") not in FINAL:
                with lock:
                    status.update({k: v for k, v in clean.items() if k in ("state",)})
                    save(d / "status.json", status)
                continue
            store = intake.Store(d / "artifacts")
            try:
                for h, payload in files:
                    if h.get("capability") != cap:
                        rejected.append({"op": "file", "path": str(h.get("path"))[:200], "reason": "capability mismatch"})
                        continue
                    try:
                        store.put(h.get("path"), payload, h.get("sha256"))
                    except intake.Rejected as error:
                        rejected.append({"op": "file", "path": str(h.get("path"))[:200], "reason": str(error)})
            finally:
                store.close()
            save(d / "manifest.json", store.manifest)
            status.update(clean, guest_fields_ignored=ignored)
            break
    except Exception as error:
        status.update(state="lost" if "lost" in str(error) else "error", error=str(error))
    with lock:
        active_caps.pop(cap, None)
    status["rejected"] = rejected
    status["ended"] = time.time()
    save(d / "status.json", status)
    event("job-end", job=job, vm=vm, state=status["state"], rejected=len(rejected))


def submit(tenant, body):
    request = body.get("request") or {}
    ignored = {k: request.pop(k) for k in ("tenant", "vm", "capability", "uid", "user") if k in request}
    snapshot = base64.b64decode(body.get("snapshot", ""))
    if len(snapshot) > MAX_SNAPSHOT:
        raise ValueError("snapshot too large")
    clean = {k: request.get(k) for k in ("identity", "root", "cwd", "argv", "developer_dir", "ui", "app", "snapshot_hash")}
    if not isinstance(clean["argv"], list) or not all(isinstance(a, str) for a in clean["argv"]):
        raise ValueError("argv must be a list of strings")
    with lock:
        candidates = [p.name for p in VMS.iterdir() if (p / "owner.json").exists()
                      and vm_owner(p.name).get("tenant") == tenant
                      and p.name in vm_procs and vm_procs[p.name].poll() is None]
        if not candidates:
            raise LookupError("no running guest for tenant")
        vm = sorted(candidates)[0]
        job = str(uuid.uuid4())
        cap = secrets.token_hex(24)
        d = job_paths(job)
        d.mkdir(parents=True)
        save(d / "host.json", {"job": job, "tenant": tenant, "vm": vm, "submitted": time.time(),
                               "ignored_request_fields": sorted(ignored), "request": clean})
        save(d / "status.json", {"job": job, "state": "queued"})
        active_caps[cap] = {"job": job, "vm": vm, "tenant": tenant}
    event("submit", job=job, tenant=tenant, vm=vm, ignored=sorted(ignored))
    threading.Thread(target=run_job, args=(job, vm, cap, clean, snapshot), daemon=True).start()
    return {"job": job}


def owned_job(tenant, job):
    try:
        d = job_paths(job)
    except (ValueError, TypeError, AttributeError):
        raise LookupError("no such job")
    host = load(d / "host.json")
    if not host or host["tenant"] != tenant:
        raise LookupError("no such job")
    return d, host


def tenant_api(tenant, action, body):
    if action == "submit":
        return submit(tenant, body)
    d, host = owned_job(tenant, body.get("job"))
    if action == "status":
        status = load(d / "status.json")
        streams = {}
        for name in ("stdout.raw", "stderr.raw"):
            p = d / name
            data = p.read_bytes() if p.exists() else b""
            streams[name] = base64.b64encode(data[body.get("offsets", {}).get(name, 0):]).decode()
        return dict(status, streams=streams, vm=host["vm"], tenant=tenant)
    if action == "artifacts":
        return {"manifest": load(d / "manifest.json", [])}
    if action == "artifact":
        rel = body.get("path")
        if rel not in {m["path"] for m in load(d / "manifest.json", [])}:
            raise LookupError("no such artifact")
        fd = os.open(d / "artifacts", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            for part in intake.check_path(rel)[:-1]:
                nfd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                os.close(fd)
                fd = nfd
            f = os.open(rel.split("/")[-1], os.O_RDONLY | os.O_NOFOLLOW, dir_fd=fd)
            with os.fdopen(f, "rb") as fh:
                return {"data": base64.b64encode(fh.read()).decode()}
        finally:
            os.close(fd)
    if action == "cancel":
        st = load(d / "status.json")
        if st.get("state") in FINAL:
            return {"job": body["job"], "state": st["state"]}
        header, _ = agent_call(host["vm"], {"op": "cancel", "job": body["job"]}, max_payload=1 << 20)
        event("cancel", job=body["job"], tenant=tenant)
        return {"job": body["job"], "requested": header.get("op") == "ok"}
    raise ValueError("unknown action")


class TenantHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        auth = self.headers.get("Authorization", "")
        tenant = tenant_for(auth[7:] if auth.startswith("Bearer ") else "")
        action = self.path.strip("/")
        code, result = 200, None
        try:
            if not tenant:
                code, result = 401, {"error": "unauthenticated"}
            else:
                length = int(self.headers.get("Content-Length", "0"))
                if length > MAX_SNAPSHOT * 2:
                    raise ValueError("body too large")
                result = tenant_api(tenant, action, json.loads(self.rfile.read(length) or b"{}"))
        except LookupError as error:
            code, result = 404, {"error": str(error)}
        except Exception as error:
            code, result = 400, {"error": str(error)}
        if code != 200:
            event("tenant-refused", tenant=tenant, action=action, code=code, error=result["error"])
        data = json.dumps(result).encode()
        self.send_response(code)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class ThreadingHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


ADMIN = {
    "vm-create": lambda b: vm_create(b["vm"], b["tenant"]),
    "vm-start": lambda b: vm_start(b["vm"], b.get("cpus", 6), b.get("memory", 8)),
    "vm-wait": lambda b: vm_wait(b["vm"], b.get("timeout", 600))[0],
    "vm-stop": lambda b: vm_stop(b["vm"], b.get("crash", False)),
    "vm-retire": lambda b: vm_retire(b["vm"]),
    "credential": lambda b: credential(b["vm"], b["name"], base64.b64decode(b["data"]))[0],
    "list": lambda b: {p.name: dict(vm_owner(p.name), running=p.name in vm_procs and vm_procs[p.name].poll() is None)
                       for p in VMS.iterdir() if (p / "owner.json").exists()},
    "job": lambda b: {"host": load(job_paths(b["job"]) / "host.json"), "status": load(job_paths(b["job"]) / "status.json")},
}


class AdminHandler(socketserver.StreamRequestHandler):
    def handle(self):
        body = json.loads(self.rfile.readline())
        try:
            result = {"ok": True, "result": ADMIN[body["op"]](body)}
        except Exception as error:
            result = {"ok": False, "error": "%s: %s" % (type(error).__name__, error)}
        self.wfile.write((json.dumps(result, default=str) + "\n").encode())


def init():
    STATE.mkdir(parents=True, exist_ok=True)
    os.chmod(STATE, 0o700)
    tokens = STATE / "tokens"
    tokens.mkdir(mode=0o700, exist_ok=True)
    table = {}
    for tenant in ("tenant-a", "tenant-b", "tenant-c"):
        p = tokens / (tenant + ".token")
        if not p.exists():
            p.write_text("DUMMY-" + tenant + "-" + secrets.token_hex(16))
            p.chmod(0o600)
        table[hashlib.sha256(p.read_text().encode()).hexdigest()] = tenant
    save(STATE / "tenants.json", table)
    key = STATE / "device.key"
    if not key.exists():
        key.write_text("DUMMY-HOST-DEVICE-KEY-CANARY-" + secrets.token_hex(16) + "\n")
        key.chmod(0o600)
    canary = ROOT / "canary"
    canary.mkdir(exist_ok=True)
    host = canary / "host-secret.txt"
    if not host.exists():
        host.write_text("DUMMY-HOST-CANARY-" + secrets.token_hex(16) + "\n")
    print(json.dumps({"state": str(STATE), "tenants": sorted(table.values())}))


def serve():
    for d in VMS.glob("*") if VMS.exists() else []:
        owner = load(d / "owner.json", {})
        if owner.get("state") == "running":
            owner["state"] = "quarantined"
            save(d / "owner.json", owner)
            event("vm-quarantine", vm=d.name, reason="broker restarted without its vmrun")
    with contextlib.suppress(FileNotFoundError):
        ADMIN_SOCKET.unlink()
    admin = UnixServer(str(ADMIN_SOCKET), AdminHandler)
    os.chmod(ADMIN_SOCKET, 0o600)
    threading.Thread(target=admin.serve_forever, daemon=True).start()
    event("broker-start", pid=os.getpid())
    ThreadingHTTPServer(("127.0.0.1", TENANT_PORT), TenantHandler).serve_forever()


def admin_call(body):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(str(ADMIN_SOCKET))
    s.sendall((json.dumps(body) + "\n").encode())
    data = b""
    while not data.endswith(b"\n"):
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
    print(data.decode().strip())


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "init":
        init()
    elif mode == "serve":
        serve()
    elif mode == "admin":
        admin_call(json.loads(sys.argv[2]))
