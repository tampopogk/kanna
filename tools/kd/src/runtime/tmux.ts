import { execFileSync } from "node:child_process";
import type { DevWindow } from "./dev-plan";
import type { CommandRunner } from "./process";
import {
  processIdentity,
  recordInventoryResource,
  terminateInventoryProcess,
  type InventoryProcess
} from "./process-inventory";

export interface TmuxTarget {
  server: string;
  session: string;
  inventoryPath?: string;
}

export interface StartTmuxSessionOptions {
  reconcileKey?: string;
}

const RECONCILE_OPTION = "@kanna_reconcile_key";

export interface TmuxWindowState {
  exists: boolean;
  dead: boolean;
  exitCode?: number;
}

export interface WaitForTmuxWindowReadyOptions {
  attempts?: number;
  delayMs?: number;
}

const ALWAYS_FORWARDED_ENV_KEYS = [
  "KANNA_DESKTOP_AUTO_SIGN_IN_EMAIL",
  "KANNA_DESKTOP_AUTO_SIGN_IN_PASSWORD",
];

// The Linux desktop window has to reach the user's display server, and tmux
// will not carry it there on its own: a window only keeps what `-e` named for
// it, while the tmux *server* keeps whichever environment first created it —
// which on a machine that has run `kd` from more than one login is not
// necessarily this session's. Naming the session variables explicitly is also
// what makes a respawn land on the same display the window started on.
// macOS has no such variables, so it keeps the two-key list it always had.
const LINUX_SESSION_ENV_KEYS = [
  "WAYLAND_DISPLAY",
  "DISPLAY",
  "XAUTHORITY",
  "XDG_RUNTIME_DIR",
  "XDG_SESSION_TYPE",
  "XDG_CURRENT_DESKTOP",
  "DBUS_SESSION_BUS_ADDRESS",
  "GDK_BACKEND",
  // Set by `linuxDesktopWebkitEnv` when this machine has no usable DRM render
  // node. A respawn that lost it would start an app with no window.
  "WEBKIT_DISABLE_DMABUF_RENDERER",
];

export function tmuxWindowEnvKeys(platform: NodeJS.Platform = process.platform): string[] {
  return platform === "linux"
    ? [...ALWAYS_FORWARDED_ENV_KEYS, ...LINUX_SESSION_ENV_KEYS]
    : [...ALWAYS_FORWARDED_ENV_KEYS];
}

export function tmuxWindowEnvArgs(
  env: NodeJS.ProcessEnv,
  platform: NodeJS.Platform = process.platform
): string[] {
  return tmuxWindowEnvKeys(platform).flatMap((key) => {
    const value = env[key];
    return typeof value === "string" && value.length > 0
      ? [`${key}=${value}`]
      : [];
  });
}

function hasTmuxWindowEnv(env: NodeJS.ProcessEnv): boolean {
  return tmuxWindowEnvArgs(env).length > 0;
}

function tmuxQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

async function runTmuxSourceCommand(
  runner: CommandRunner,
  target: TmuxTarget,
  command: string,
  env: NodeJS.ProcessEnv
) {
  return runner.run(
    "tmux",
    ["-L", target.server, "source-file", "-"],
    { env, stdin: `${command}\n` },
  );
}

function tmuxCommandLine(command: string, args: string[]): string {
  return [command, ...args.map(tmuxQuote)].join(" ");
}

async function setRemainOnExit(runner: CommandRunner, target: TmuxTarget): Promise<void> {
  await runner.run("tmux", ["-L", target.server, "set-option", "-t", target.session, "remain-on-exit", "on"]);
}

async function newTmuxWindowWithEnv(
  runner: CommandRunner,
  target: TmuxTarget,
  window: DevWindow
) {
  return runTmuxSourceCommand(
    runner,
    target,
    tmuxCommandLine("new-window", [
      "-t",
      target.session,
      "-n",
      window.name,
      "-c",
      window.cwd,
      ...tmuxWindowEnvArgs(window.env).flatMap((entry) => ["-e", entry]),
      window.command,
    ]),
    window.env,
  );
}

async function respawnTmuxWindowWithEnv(
  runner: CommandRunner,
  target: TmuxTarget,
  window: DevWindow
) {
  return runTmuxSourceCommand(
    runner,
    target,
    tmuxCommandLine("respawn-window", [
      "-k",
      "-t",
      `${target.session}:${window.name}`,
      "-c",
      window.cwd,
      ...tmuxWindowEnvArgs(window.env).flatMap((entry) => ["-e", entry]),
      window.command,
    ]),
    window.env,
  );
}

export async function hasTmuxSession(runner: CommandRunner, target: TmuxTarget): Promise<boolean> {
  const result = await runner.run("tmux", ["-L", target.server, "has-session", "-t", target.session]);
  return result.exitCode === 0;
}

export async function startTmuxSession(
  runner: CommandRunner,
  target: TmuxTarget,
  windows: DevWindow[],
  options: StartTmuxSessionOptions = {}
): Promise<void> {
  const [first, ...rest] = windows;
  if (!first) {
    throw new Error("Cannot start tmux session without windows");
  }

  const firstCommand = hasTmuxWindowEnv(first.env) ? "sleep 2147483647" : first.command;
  const firstResult = await runner.run(
    "tmux",
    [
      "-L", target.server,
      "new-session", "-d",
      "-s", target.session,
      "-n", first.name,
      "-c", first.cwd,
      firstCommand
    ],
    { env: first.env }
  );
  if (firstResult.exitCode !== 0) {
    if (isDuplicateSessionError(firstResult.stderr)) {
      if (target.inventoryPath) {
        recordInventoryResource(target.inventoryPath, { kind: "tmux-server", socket: target.server });
      }
      if (options.reconcileKey && !(await tmuxSessionMatchesReconcileKey(runner, target, options.reconcileKey))) {
        const stopped = await stopTmuxSession(runner, target);
        if (!stopped) {
          throw new Error(`tmux failed to reconcile existing session ${target.session}`);
        }
        await startTmuxSession(runner, target, windows, options);
        return;
      }
      await addMissingTmuxWindows(runner, target, windows);
      return;
    }
    throw new Error(`tmux failed to start ${target.session}:${first.name}: ${firstResult.stderr}`);
  }
  if (target.inventoryPath) {
    recordInventoryResource(target.inventoryPath, { kind: "tmux-server", socket: target.server });
  }
  await recordTmuxServerSocketPath(runner, target);

  await setRemainOnExit(runner, target);
  if (options.reconcileKey) {
    await setTmuxSessionReconcileKey(runner, target, options.reconcileKey);
  }

  if (hasTmuxWindowEnv(first.env)) {
    const respawned = await respawnTmuxWindow(runner, target, first);
    if (!respawned) {
      throw new Error(`tmux failed to start ${target.session}:${first.name}: window was not created`);
    }
  }
  await recordTmuxPane(runner, target, first.name);

  for (const window of rest) {
    const result = hasTmuxWindowEnv(window.env)
      ? await newTmuxWindowWithEnv(runner, target, window)
      : await runner.run(
          "tmux",
          [
            "-L",
            target.server,
            "new-window",
            "-t",
            target.session,
            "-n",
            window.name,
            "-c",
            window.cwd,
            window.command
          ],
          { env: window.env }
        );
    if (result.exitCode !== 0) {
      throw new Error(`tmux failed to start ${target.session}:${window.name}: ${result.stderr}`);
    }
    await recordTmuxPane(runner, target, window.name);
  }
}

async function tmuxSessionMatchesReconcileKey(
  runner: CommandRunner,
  target: TmuxTarget,
  reconcileKey: string
): Promise<boolean> {
  const result = await runner.run("tmux", [
    "-L", target.server,
    "show-options", "-v",
    "-t", target.session,
    RECONCILE_OPTION
  ]);
  return result.exitCode === 0 && result.stdout.trim() === reconcileKey;
}

async function setTmuxSessionReconcileKey(
  runner: CommandRunner,
  target: TmuxTarget,
  reconcileKey: string
): Promise<void> {
  const result = await runner.run("tmux", [
    "-L", target.server,
    "set-option", "-t", target.session,
    RECONCILE_OPTION, reconcileKey
  ]);
  if (result.exitCode !== 0) {
    throw new Error(`tmux failed to record launch profile for ${target.session}: ${result.stderr}`);
  }
}

async function recordTmuxServerSocketPath(runner: CommandRunner, target: TmuxTarget): Promise<void> {
  if (!target.inventoryPath) return;
  const result = await runner.run("tmux", ["-L", target.server, "display-message", "-p", "#{socket_path}"]);
  const socketPath = result.stdout.trim();
  if (result.exitCode === 0 && socketPath) {
    recordInventoryResource(target.inventoryPath, { kind: "tmux-server", socket: target.server, socketPath });
  }
}

async function recordTmuxPane(runner: CommandRunner, target: TmuxTarget, window: string): Promise<void> {
  if (!target.inventoryPath) return;
  const result = await runner.run("tmux", [
    "-L", target.server, "display-message", "-p", "-t", `${target.session}:${window}`, "#{pane_pid}"
  ]);
  const pid = Number(result.stdout.trim());
  if (result.exitCode === 0 && Number.isInteger(pid) && pid > 1) {
    recordInventoryResource(target.inventoryPath, { kind: "process", pid, label: `tmux:${target.session}:${window}` });
  }
}

function isDuplicateSessionError(stderr: string): boolean {
  const normalized = stderr.toLowerCase();
  return normalized.includes("duplicate session") || normalized.includes("session already exists");
}

async function addMissingTmuxWindows(
  runner: CommandRunner,
  target: TmuxTarget,
  windows: DevWindow[]
): Promise<void> {
  const list = await runner.run("tmux", ["-L", target.server, "list-windows", "-t", target.session, "-F", "#{window_name}"]);
  if (list.exitCode !== 0) {
    throw new Error(`tmux failed to inspect existing session ${target.session}: ${list.stderr}`);
  }
  await setRemainOnExit(runner, target);

  const existing = new Set(
    list.stdout
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
  );

  for (const window of windows) {
    if (existing.has(window.name)) {
      continue;
    }
    const result = hasTmuxWindowEnv(window.env)
      ? await newTmuxWindowWithEnv(runner, target, window)
      : await runner.run(
          "tmux",
          [
            "-L",
            target.server,
            "new-window",
            "-t",
            target.session,
            "-n",
            window.name,
            "-c",
            window.cwd,
            window.command
          ],
          { env: window.env }
        );
    if (result.exitCode !== 0) {
      throw new Error(`tmux failed to start ${target.session}:${window.name}: ${result.stderr}`);
    }
  }
}

export async function stopTmuxSession(runner: CommandRunner, target: TmuxTarget): Promise<boolean> {
  if (!(await hasTmuxSession(runner, target))) {
    return false;
  }

  const list = await runner.run("tmux", ["-L", target.server, "list-windows", "-t", target.session, "-F", "#{window_name}"]);
  for (const name of list.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)) {
    await runner.run("tmux", ["-L", target.server, "send-keys", "-t", `${target.session}:${name}`, "C-c"]);
  }
  const killed = await runner.run("tmux", ["-L", target.server, "kill-session", "-t", target.session]);
  return killed.exitCode === 0;
}

export async function stopTmuxWindow(runner: CommandRunner, target: TmuxTarget, window: string): Promise<boolean> {
  const list = await runner.run("tmux", ["-L", target.server, "list-windows", "-t", target.session, "-F", "#{window_name}"]);
  if (list.exitCode !== 0) {
    return false;
  }
  const exists = list.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .includes(window);
  if (!exists) {
    return false;
  }

  await runner.run("tmux", ["-L", target.server, "send-keys", "-t", `${target.session}:${window}`, "C-c"]);
  await runner.run("tmux", ["-L", target.server, "kill-window", "-t", `${target.session}:${window}`]);
  return true;
}

function paneProcessGroup(panePid: number): InventoryProcess[] {
  const group = Number(execFileSync("ps", ["-p", String(panePid), "-o", "pgid="], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"]
  }).trim());
  // tmux creates an isolated session/process group for every pane. Refuse to
  // signal anything unless that ownership boundary is still exactly the pane
  // we inspected; this must never become a broad kill by inherited group id.
  if (!Number.isInteger(group) || group !== panePid) return [];
  const rows = execFileSync("ps", ["-axo", "pid=,pgid="], { encoding: "utf8" });
  return rows
    .split("\n")
    .map((row) => row.trim().split(/\s+/).map(Number))
    .filter(([pid, pgid]) => Number.isInteger(pid) && pid > 1 && pgid === group)
    .flatMap(([pid]): InventoryProcess[] => {
      const identity = processIdentity(pid);
      return identity
        ? [{ kind: "process", pid, label: `tmux-pane-group:${panePid}`, identity }]
        : [];
    });
}

async function stopTmuxPaneProcessGroup(
  runner: CommandRunner,
  target: TmuxTarget,
  window: string
): Promise<void> {
  const pane = await runner.run("tmux", [
    "-L", target.server, "display-message", "-p", "-t", `${target.session}:${window}`, "#{pane_pid} #{pane_dead}"
  ]);
  const [pidText, dead] = pane.stdout.trim().split(/\s+/, 2);
  const panePid = Number(pidText);
  if (pane.exitCode !== 0 || dead === "1" || !Number.isInteger(panePid) || panePid <= 1) return;

  const owned = paneProcessGroup(panePid);
  const paneResource = owned.find((resource) => resource.pid === panePid);
  if (!paneResource) {
    throw new Error(`tmux could not establish an isolated process group for ${target.session}:${window}`);
  }
  const recheck = await runner.run("tmux", [
    "-L", target.server, "display-message", "-p", "-t", `${target.session}:${window}`, "#{pane_pid} #{pane_dead}"
  ]);
  const [recheckedPid, recheckedDead] = recheck.stdout.trim().split(/\s+/, 2);
  if (recheck.exitCode !== 0 || recheckedDead === "1" || Number(recheckedPid) !== panePid ||
      processIdentity(panePid) !== paneResource.identity) {
    throw new Error(`tmux pane ownership changed while stopping ${target.session}:${window}`);
  }

  // Give the foreground command its normal terminal shutdown first. Any
  // wrapper/listener left behind is then reaped by pinned PID/start identity.
  // A Kanna daemon calls setsid() at spawn, so its live task sessions are not
  // members of this pane-owned group and are deliberately untouched.
  await runner.run("tmux", ["-L", target.server, "send-keys", "-t", `${target.session}:${window}`, "C-c"]);
  await new Promise<void>((resolve) => setTimeout(resolve, 100));
  for (const resource of owned) {
    const outcome = await terminateInventoryProcess(resource, { graceMs: 500, pollMs: 25 });
    if (outcome === "failed") {
      throw new Error(`tmux could not stop owned process ${resource.pid} for ${target.session}:${window}`);
    }
  }
}

export async function tmuxWindowState(
  runner: CommandRunner,
  target: TmuxTarget,
  window: string
): Promise<TmuxWindowState> {
  const result = await runner.run("tmux", [
    "-L", target.server,
    "display-message", "-p",
    "-t", `${target.session}:${window}`,
    "#{pane_dead} #{pane_dead_status}"
  ]);
  if (result.exitCode !== 0) return { exists: false, dead: false };
  const [dead, status] = result.stdout.trim().split(/\s+/, 2);
  return {
    exists: true,
    dead: dead === "1",
    exitCode: /^\d+$/.test(status ?? "") ? Number(status) : undefined
  };
}

export async function waitForTmuxWindowReady(
  runner: CommandRunner,
  target: TmuxTarget,
  window: string,
  ready: () => Promise<boolean>,
  options: WaitForTmuxWindowReadyOptions = {}
): Promise<{ ready: boolean; failure?: string }> {
  const attempts = options.attempts ?? 240;
  const delayMs = options.delayMs ?? 250;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    const state = await tmuxWindowState(runner, target, window);
    if (!state.exists) return { ready: false, failure: "tmux window disappeared during startup" };
    if (state.dead) {
      const suffix = state.exitCode === undefined ? "" : ` with exit code ${state.exitCode}`;
      return { ready: false, failure: `tmux pane exited during startup${suffix}` };
    }
    if (await ready()) return { ready: true };
    if (attempt + 1 < attempts) {
      await new Promise<void>((resolve) => setTimeout(resolve, delayMs));
    }
  }
  return { ready: false, failure: "timed out waiting for startup readiness" };
}

export async function respawnTmuxWindow(runner: CommandRunner, target: TmuxTarget, window: DevWindow): Promise<boolean> {
  const list = await runner.run("tmux", ["-L", target.server, "list-windows", "-t", target.session, "-F", "#{window_name}"]);
  if (list.exitCode !== 0) {
    return false;
  }
  const exists = list.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .includes(window.name);
  if (!exists) {
    return false;
  }

  await setRemainOnExit(runner, target);
  await stopTmuxPaneProcessGroup(runner, target, window.name);

  const result = hasTmuxWindowEnv(window.env)
    ? await respawnTmuxWindowWithEnv(runner, target, window)
    : await runner.run(
        "tmux",
        [
          "-L",
          target.server,
          "respawn-window",
          "-k",
          "-t",
          `${target.session}:${window.name}`,
          "-c",
          window.cwd,
          window.command
        ],
        { env: window.env }
      );
  if (result.exitCode !== 0) {
    throw new Error(`tmux failed to respawn ${target.session}:${window.name}: ${result.stderr}`);
  }
  const after = await tmuxWindowState(runner, target, window.name);
  return after.exists && !after.dead;
}

export async function captureTmuxLog(runner: CommandRunner, target: TmuxTarget, window: string): Promise<string> {
  const result = await runner.run("tmux", [
    "-L",
    target.server,
    "capture-pane",
    "-t",
    `${target.session}:${window}`,
    "-p",
    "-S",
    "-50"
  ]);
  if (result.exitCode !== 0) {
    throw new Error(`tmux log failed for ${target.session}:${window}: ${result.stderr}`);
  }
  return result.stdout;
}
