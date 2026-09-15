/** Actual loopback OpenSSH transport + unchanged POSIX helper. No system sshd
 * configuration, owner credentials, public listener or archive is used. */
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:net";
import { join, resolve } from "node:path";
import { userInfo } from "node:os";
import { afterAll, beforeAll, expect, it } from "vitest";
import { FilesystemAptStorage, SshAptStorage, type SshAptTransport } from "../src/runtime/linux-apt-storage";

const repo = resolve(import.meta.dirname, "../../..");
mkdirSync(join(repo, ".tmp"), { recursive: true });
const root = mkdtempSync(join(repo, ".tmp/ssh-storage-test-"));
let server: Server;
const children = new Map<ChildProcess, Promise<void>>();
let transport: SshAptTransport;
let log = "";
beforeAll(async () => {
  // Disposable test-only authentication/host keys, never imported into an
  // agent/keyring or reused for owner access. Removed with the fixture.
  for (const name of ["host", "client"]) execFileSync("/usr/bin/ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-f", join(root, name)]);
  const config = join(root, "sshd_config");
  // inetd mode inherits the accepted socket. Keep port 0 bound for the whole
  // fixture, rather than releasing a reservation before sshd can bind it.
  server = createServer(socket => {
    const child = spawn("/usr/sbin/sshd", ["-i", "-e", "-f", config], { stdio: [socket, socket, "pipe"] });
    children.set(child, new Promise<void>(r => child.once("close", () => r())));
    child.stderr!.on("data", b => { log += String(b); });
    socket.destroy();
  });
  await new Promise<void>(r => server.listen(0, "127.0.0.1", r));
  const port = (server.address() as { port: number }).port;
  const knownHostsPath = join(root, "known hosts");
  writeFileSync(knownHostsPath, `[127.0.0.1]:${port} ${readFileSync(join(root, "host.pub"), "utf8")}`, { mode: 0o600 });
  writeFileSync(config, `ListenAddress 127.0.0.1\nPort ${port}\nHostKey ${root}/host\nPidFile ${root}/sshd.pid\nAuthorizedKeysFile ${root}/client.pub\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nAllowUsers ${userInfo().username}\nAllowTcpForwarding no\nAllowAgentForwarding no\nPermitTTY no\nLogLevel VERBOSE\n`);
  transport = { host: "127.0.0.1", port, user: userInfo().username, knownHostsPath, identityPath: join(root, "client") };
});
afterAll(async () => {
  if (server) await new Promise<void>((r, j) => server.close(e => e ? j(e) : r()));
  for (const child of children.keys()) child.kill();
  await Promise.all(children.values());
  rmSync(root, { recursive: true, force: true });
});
function fixture() {
  const archive = mkdtempSync(join(root, "archive-"));
  return { archive, storage: new SshAptStorage(archive, transport) };
}
it("uses a pinned SSH session and the shared helper lock/write/readback contract", async () => {
  const { storage, archive } = fixture();
  await storage.withExclusivePublication(async () => {
    await expect(new FilesystemAptStorage(archive).read("object")).rejects.toThrow(/already owned/);
    expect(await storage.create("pool/object", Buffer.from("old"))).toBe(true);
    expect(await storage.create("pool/object", Buffer.from("new"))).toBe(false);
    await storage.replace("state", Buffer.from("complete"));
    expect(await storage.read("state")).toEqual(Buffer.from("complete"));
  });
  expect(await new FilesystemAptStorage(archive).read("pool/object")).toEqual(Buffer.from("old"));
});
it("refuses an unpinned host and confines remote object paths", async () => {
  const { storage, archive } = fixture();
  const wrong = join(root, "wrong_hosts");
  writeFileSync(wrong, `[127.0.0.1]:${transport.port} ${readFileSync(join(root, "client.pub"), "utf8")}`, { mode: 0o600 });
  await expect(new SshAptStorage(archive, { ...transport, knownHostsPath: wrong }).read("absent")).rejects.toThrow(/host key|HOST IDENTIFICATION/i);
  await expect(storage.read("../escape")).rejects.toThrow(/Invalid archive/);
});
it.each(["ssh", "helper"])("fails closed on real %s death and recovers with a new ownership session", async kind => {
  const { storage, archive } = fixture();
  await expect(storage.withExclusivePublication(async () => {
    await storage.create("pool/object", Buffer.from("retained"));
    // Identify only fixture children: helper argv includes this unique root;
    // SSH argv includes this fixture's identity path and Python/root command.
    const rows = execFileSync("ps", ["-axo", "pid=,ppid=,command="], { encoding: "utf8" }).split("\n");
    const pattern = kind === "ssh" ? "/usr/bin/ssh -F" : " -u -c";
    const matches = rows.filter(line => line.includes(archive) && line.includes(pattern) && (kind !== "helper" || !line.includes("/usr/bin/ssh -F")));
    expect(matches).toHaveLength(1);
    const pid = Number(matches[0].trim().split(/\s+/)[0]);
    process.kill(pid, "SIGKILL");
    if (kind === "ssh") await storage.replace("state", Buffer.from("must fail"));
    // Helper death after external readback must also fail the scope's final fence.
  })).rejects.toThrow(/helper stopped|EPIPE|closed/);
  // Kernel lock wait is a happens-before assertion for EOF-driven helper exit,
  // with a generous liveness bound rather than a timing performance assertion.
  execFileSync("/usr/bin/python3", ["-c", "import sys,fcntl; f=open(sys.argv[1]); fcntl.flock(f,fcntl.LOCK_EX)", join(archive, ".publication.lock")], { timeout: 30000 });
  expect(await new SshAptStorage(archive, transport).read("pool/object")).toEqual(Buffer.from("retained"));
});
