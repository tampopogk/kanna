import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { cleanupUnrecordedDaemon, type DaemonObservation } from "../src/runtime/daemon-ownership";
import { executeDevDownWithContext } from "../src/tasks/registry";

const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
function fixture() {
  const scratch = resolve("../../.tmp/kd-cleanup-tests");
  mkdirSync(scratch, { recursive: true });
  const root = realpathSync(mkdtempSync(join(scratch, "owned-"))); roots.push(root);
  const daemonDir = join(root, ".kanna-daemon");
  const bin = join(root, ".build/debug");
  mkdirSync(daemonDir); mkdirSync(bin, { recursive: true });
  for (const name of ["kanna-daemon", "kanna-terminal-recovery"]) writeFileSync(join(bin, name), "fixture");
  const processes = new Map<number, DaemonObservation>([
    [111, { identity: "daemon-start", parent: 1, executable: join(bin, "kanna-daemon"), cwd: bin,
      files: [join(daemonDir, "kanna-daemon_111_rCURRENT.log")] }],
    [112, { identity: "recovery-start", parent: 111, executable: join(bin, "kanna-terminal-recovery"), cwd: bin, files: [] }]
  ]);
  const signals: number[] = [];
  return { root, daemonDir, processes, signals, input: {
    repoRoot: root, daemonDir, pid: 111,
    observe: (pid: number) => processes.get(pid),
    children: () => [112],
    operations: { identity: (pid: number) => processes.get(pid)?.identity,
      signal: (pid: number) => { signals.push(pid); processes.delete(pid); }, graceMs: 1, pollMs: 1 }
  } };
}

describe("missing-inventory daemon ownership", () => {
  it("cleans an independently owned daemon and pinned recovery child without creating inventory", async () => {
    const f = fixture();
    expect(await cleanupUnrecordedDaemon(f.input)).toEqual({ pidFileKilled: 111 });
    expect(f.signals).toEqual([111, 112]);
  });
  it.each(["executable", "cwd", "log", "directory", "child"])("refuses wrong ownership: %s", async kind => {
    const f = fixture(); const other = fixture();
    const daemon = f.processes.get(111)!;
    if (kind === "executable") daemon.executable = other.processes.get(111)!.executable;
    if (kind === "cwd") daemon.cwd = other.root;
    if (kind === "log") daemon.files = other.processes.get(111)!.files;
    if (kind === "directory") f.input.daemonDir = other.daemonDir;
    if (kind === "child") f.processes.get(112)!.executable = other.processes.get(112)!.executable;
    expect((await cleanupUnrecordedDaemon(f.input)).failure).toContain("cleanup incomplete");
    expect(f.signals).toEqual([]);
  });
  it("refuses stale PID reuse between observation and termination", async () => {
    const f = fixture();
    f.input.operations.identity = () => "reused";
    expect((await cleanupUnrecordedDaemon(f.input)).failure).toContain("identity-mismatch");
    expect(f.signals).toEqual([]);
  });
  it("rechecks ownership immediately before signalling even with unchanged start identity", async () => {
    const f = fixture(); let calls = 0;
    f.input.observe = pid => {
      const o = f.processes.get(pid);
      return pid === 111 && ++calls > 1 && o ? { ...o, files: [] } : o;
    };
    expect((await cleanupUnrecordedDaemon(f.input)).failure).toContain("failed");
    expect(f.signals).toEqual([]);
  });
  it("reports an incomplete canonical dev down rather than Stopped", async () => {
    const f = fixture();
    writeFileSync(join(f.daemonDir, "daemon.pid"), "111");
    const result = await executeDevDownWithContext({ killDaemon: true }, {
      runner: { run: async () => ({ exitCode: 1, stdout: "", stderr: "no server running" }) },
      context: { repoRoot: f.root, tmux: { server: "fixture", session: "fixture" }, ports: {}, env: { KANNA_DAEMON_DIR: f.daemonDir } }
    }, { cleanupOperations: { identity: () => "live", signal: () => { throw new Error("must not signal"); } } });
    expect(result.ok).toBe(false);
    expect(result.message).toContain("cleanup incomplete");
  });
});
