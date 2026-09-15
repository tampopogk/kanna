/** POSIX archive storage. The helper owns both the kernel lock and every write;
 * a dead client closes stdin and releases ownership, including after SIGKILL.
 * Requires a pre-existing, dedicated local filesystem root (not NFS/FUSE).
 * Never unlinks the lock inode or steals ownership based on elapsed time. */
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { isAbsolute } from "node:path";
import type { AptPublicationStorage } from "./linux-apt-publication";

const worker = String.raw`
import os, sys, json, base64, fcntl, stat, uuid
root_path = sys.argv[1]
flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
# Pin every component, refusing symlinks even above the archive.
root = os.open('/', flags)
for part in root_path.split('/'):
    if not part: continue
    nxt = os.open(part, flags, dir_fd=root)
    os.close(root)
    root = nxt
identity = os.fstat(root)
lock = os.open('.publication.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600, dir_fd=root)
if not stat.S_ISREG(os.fstat(lock).st_mode): raise RuntimeError('Invalid archive lock')
try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError: raise RuntimeError('Archive publication is already owned')
lock_identity = os.fstat(lock)
def same(a, b): return (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)
def owned():
    if not same(identity, os.stat(root_path, follow_symlinks=False)) or not same(lock_identity, os.stat('.publication.lock', dir_fd=root, follow_symlinks=False)):
        raise RuntimeError('Archive ownership lost')
def parent(path, create=False):
    parts = path.split('/')
    if any(p in ('', '.', '..') or p.startswith('.') for p in parts): raise RuntimeError('Invalid archive object path')
    fd = os.dup(root)
    try:
        for part in parts[:-1]:
            if create:
                try:
                    os.mkdir(part, 0o755, dir_fd=fd)
                    os.fsync(fd)
                except FileExistsError: pass
            nxt = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = nxt
        return fd, parts[-1]
    except:
        os.close(fd)
        raise
print(json.dumps({'ready': True}), flush=True)
for line in sys.stdin:
    try:
        request = json.loads(line)
        owned()
        operation = request['op']
        try: fd, name = parent(request['path'], operation != 'read')
        except FileNotFoundError:
            if operation != 'read': raise
            print(json.dumps({'value': None}), flush=True)
            continue
        try:
            if operation == 'read':
                try: obj = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
                except FileNotFoundError: value = None
                else:
                    with os.fdopen(obj, 'rb') as source:
                        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode): raise RuntimeError('Not a regular archive object')
                        value = base64.b64encode(source.read()).decode()
            else:
                data = base64.b64decode(request['bytes'], validate=True)
                temp = '.write-' + str(uuid.uuid4())
                obj = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o644, dir_fd=fd)
                try:
                    with os.fdopen(obj, 'wb') as output:
                        output.write(data)
                        output.flush()
                        os.fsync(output.fileno())
                    owned()
                    if operation == 'create':
                        try:
                            os.link(temp, name, src_dir_fd=fd, dst_dir_fd=fd, follow_symlinks=False)
                            value = True
                        except FileExistsError: value = False
                    elif operation == 'replace':
                        os.rename(temp, name, src_dir_fd=fd, dst_dir_fd=fd)
                        value = True
                    else: raise RuntimeError('Invalid archive operation')
                    os.fsync(fd)
                finally:
                    try: os.unlink(temp, dir_fd=fd)
                    except FileNotFoundError: pass
            owned()
            print(json.dumps({'value': value}), flush=True)
        finally: os.close(fd)
    except Exception as error:
        print(json.dumps({'error': str(error)}), flush=True)
        # Ownership/IO failure poisons this session; no later write is allowed.
        break
`;

export class FilesystemAptStorage implements AptPublicationStorage {
  private active = false;
  private rpc?: (op: string, path: string, bytes?: Uint8Array) => Promise<unknown>;
  constructor(readonly root: string, readonly python = "/usr/bin/python3") {
    if (!isAbsolute(root) || root.includes("/../") || root.endsWith("/..")) throw new Error("Archive root must be an absolute canonical path.");
  }
  async withExclusivePublication<T>(work: () => Promise<T>): Promise<T> {
    if (this.active) throw new Error("Archive publication is already owned by this adapter.");
    this.active = true;
    const child = spawn(this.python, ["-u", "-c", worker, this.root], { stdio: ["pipe", "pipe", "pipe"] });
    let failure: Error | undefined;
    let stderr = "";
    child.stderr.on("data", chunk => { stderr = (stderr + String(chunk)).slice(-2000); });
    let pending: { resolve: (value: unknown) => void; reject: (error: Error) => void } | undefined;
    const closed = new Promise<void>(resolve => child.once("close", () => {
      failure ??= new Error(`Archive storage helper stopped: ${stderr.trim()}`);
      pending?.reject(failure); pending = undefined; resolve();
    }));
    child.on("error", error => { failure = error; pending?.reject(error); pending = undefined; });
    child.stdin.on("error", error => { failure = error; pending?.reject(error); pending = undefined; });
    const lines = createInterface({ input: child.stdout });
    lines.on("line", line => {
      try {
        const result = JSON.parse(line);
        if (result.error) { failure = new Error(`Archive storage: ${result.error}`); pending?.reject(failure); }
        else pending?.resolve(result.ready ?? result.value);
      } catch { failure = new Error("Invalid archive helper response"); pending?.reject(failure); }
      pending = undefined;
    });
    try {
      await new Promise((resolve, reject) => { pending = { resolve, reject }; });
      this.rpc = async (op, path, bytes) => {
        if (failure) throw failure;
        if (pending) throw new Error("Concurrent archive operations are not supported.");
        return new Promise((resolve, reject) => {
          pending = { resolve, reject };
          child.stdin.write(JSON.stringify({ op, path, bytes: bytes && Buffer.from(bytes).toString("base64") }) + "\n");
        });
      };
      const value = await work();
      if (failure) throw failure;
      return value;
    } finally {
      this.rpc = undefined;
      child.stdin.end();
      await closed;
      this.active = false;
      lines.close();
    }
  }
  async read(path: string): Promise<Uint8Array | null> {
    if (!this.rpc) return this.withExclusivePublication(() => this.read(path));
    const result = await this.rpc("read", path);
    return result === null ? null : Buffer.from(result as string, "base64");
  }
  async create(path: string, bytes: Uint8Array): Promise<boolean> {
    if (!this.rpc) throw new Error("Archive write requires exclusive publication ownership.");
    return await this.rpc("create", path, bytes) as boolean;
  }
  async replace(path: string, bytes: Uint8Array): Promise<void> {
    if (!this.rpc) throw new Error("Archive write requires exclusive publication ownership.");
    await this.rpc("replace", path, bytes);
  }
}
