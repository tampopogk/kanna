import { execFileSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { basename, dirname, join, relative, isAbsolute } from "node:path";
import { processIdentity, terminateInventoryProcess, type ProcessCleanupOperations } from "./process-inventory";

export interface DaemonObservation {
  identity: string;
  parent: number;
  executable: string;
  cwd: string;
  files: string[];
}

/** Kernel observations only: never read an inherited environment or trust argv as an executable. */
export function observeDaemonProcess(pid: number): DaemonObservation | undefined {
  if (process.platform !== "darwin") return undefined;
  try {
    const identity = processIdentity(pid);
    if (!identity) return undefined;
    const parent = Number(execFileSync("ps", ["-p", String(pid), "-o", "ppid="], { encoding: "utf8" }).trim());
    const output = execFileSync("lsof", ["-nP", "-a", "-p", String(pid), "-Fn"], { encoding: "utf8" });
    let fd = "";
    let executable = "";
    let cwd = "";
    const files: string[] = [];
    for (const line of output.split("\n")) {
      if (line.startsWith("f")) fd = line.slice(1);
      if (!line.startsWith("n")) continue;
      const path = line.slice(1);
      if (fd === "txt" && !executable) executable = path;
      if (fd === "cwd") cwd = path;
      if (/^\d/.test(fd)) files.push(path);
    }
    if (processIdentity(pid) !== identity) return undefined;
    return { identity, parent, executable, cwd, files };
  } catch {
    return undefined;
  }
}

function inside(root: string, path: string): boolean {
  const part = relative(root, path);
  return part !== "" && part !== ".." && !part.startsWith("../") && !isAbsolute(part);
}

/** Conservative fallback for a task-owned macOS build whose inventory was lost.
 * The open per-PID daemon log binds the live process to the exact daemon directory;
 * kernel executable and cwd bind it to this checkout independently of the pidfile.
 */
export async function cleanupUnrecordedDaemon(input: {
  repoRoot: string;
  daemonDir: string;
  pid: number;
  observe?: (pid: number) => DaemonObservation | undefined;
  children?: (pid: number) => number[];
  operations?: ProcessCleanupOperations;
}): Promise<{ pidFileKilled?: number; failure?: string }> {
  const observe = input.observe ?? observeDaemonProcess;
  try {
    const root = realpathSync(input.repoRoot);
    const dir = realpathSync(input.daemonDir);
    if (!inside(root, dir) || dir !== join(root, ".kanna-daemon")) {
      throw new Error("daemon directory is not this checkout's .kanna-daemon");
    }
    const original = observe(input.pid);
    const owns = (o: DaemonObservation | undefined): o is DaemonObservation => !!o &&
      !!original && o.identity === original.identity &&
      inside(join(root, ".build"), realpathSync(o.executable)) &&
      basename(o.executable) === "kanna-daemon" && inside(root, realpathSync(o.cwd)) &&
      o.files.some(path => dirname(path) === dir && basename(path).startsWith(`kanna-daemon_${input.pid}_`) && path.endsWith(".log"));
    if (!owns(original)) throw new Error("live daemon executable/cwd/open-log ownership could not be verified");
    const children = input.children ?? ((pid: number) => {
      const rows = execFileSync("ps", ["-axo", "pid=,ppid="], { encoding: "utf8" });
      return rows.trim().split("\n").map(row => row.trim().split(/\s+/).map(Number))
        .filter(([, parent]) => parent === pid).map(([child]) => child);
    });
    // Pin the recovery child before stopping its parent. Any other live child is
    // an active/unknown session: do not turn cleanup into an implicit task stop.
    const recovery = children(input.pid).map(pid => ({ pid, observed: observe(pid) }));
    for (const child of recovery) {
      const o = child.observed;
      if (!o || o.parent !== input.pid || o.executable !== join(dirname(original.executable), "kanna-terminal-recovery")) {
        throw new Error("daemon has an unverified child; stop its sessions before cleanup");
      }
    }
    const operations = input.operations ?? {};
    const terminate = async (pid: number, pinned: DaemonObservation, verify: (o: DaemonObservation | undefined) => boolean) => {
      const outcome = await terminateInventoryProcess({ kind: "process", pid, label: "unrecorded-owned-daemon", identity: pinned.identity }, {
        ...operations,
        identity: operations.identity ?? processIdentity,
        signal: (target, signal) => {
          if (!verify(observe(target))) throw new Error("live ownership changed before signal");
          (operations.signal ?? process.kill)(target, signal);
        }
      });
      if (outcome !== "cleaned") throw new Error(`PID ${pid} cleanup ${outcome}`);
    };
    await terminate(input.pid, original, owns);
    for (const child of recovery) {
      const pinned = child.observed!;
      const live = observe(child.pid);
      const identity = (operations.identity ?? processIdentity)(child.pid);
      if (!identity || identity !== pinned.identity) continue;
      if (!live) throw new Error(`PID ${child.pid} recovery ownership could not be rechecked`);
      await terminate(child.pid, pinned, o => !!o && o.identity === pinned.identity &&
        o.executable === pinned.executable && (o.parent === input.pid || o.parent === 1));
    }
    return { pidFileKilled: input.pid };
  } catch (error) {
    return { failure: `Daemon ${input.pid} cleanup incomplete: ${error instanceof Error ? error.message : String(error)}` };
  }
}
