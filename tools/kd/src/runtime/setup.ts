import { existsSync } from "node:fs";
import { join } from "node:path";
import type { CommandRunner } from "./process";

export interface SetupCheck {
  name: string;
  ok: boolean;
  message: string;
}

export interface SetupResult {
  ok: boolean;
  checks: SetupCheck[];
}

// The pinned Ghostty commit behind libghostty-vt-sys refuses to build on older Zig.
export const MINIMUM_ZIG_VERSION = "0.16.0";

function versionAtLeast(version: string, minimum: string): boolean {
  const parse = (value: string) => value.match(/\d+(?:\.\d+)+/)?.[0].split(".").map(Number) ?? null;
  const actual = parse(version);
  const required = parse(minimum);
  if (!actual || !required) return false;
  for (let i = 0; i < Math.max(actual.length, required.length); i += 1) {
    const difference = (actual[i] ?? 0) - (required[i] ?? 0);
    if (difference !== 0) return difference > 0;
  }
  return true;
}

async function commandVersion(runner: CommandRunner, command: string, args: string[]): Promise<string | null> {
  const result = await runner.run(command, args);
  if (result.exitCode !== 0) return null;
  return result.stdout.trim() || result.stderr.trim() || command;
}

async function checkNativeToolchain(runner: CommandRunner, platform: NodeJS.Platform): Promise<SetupCheck[]> {
  if (platform === "darwin") {
    const xcode = await runner.run("xcode-select", ["-p"]);
    return [{
      name: "xcode",
      ok: xcode.exitCode === 0,
      message: xcode.exitCode === 0 ? xcode.stdout.trim() : "install with: xcode-select --install"
    }];
  }

  // Linux has no single developer-tools bundle: check the C/C++ compiler and
  // the GTK/WebKitGTK development packages Tauri links against.
  const compiler = await commandVersion(runner, "cc", ["--version"]);
  const guiLibs = await runner.run("pkg-config", ["--exists", "gtk+-3.0", "webkit2gtk-4.1"]);
  return [
    {
      name: "cc",
      ok: compiler !== null,
      message: compiler?.split("\n")[0] ?? "missing command: cc (install build-essential)"
    },
    {
      name: "webkitgtk",
      ok: guiLibs.exitCode === 0,
      message: guiLibs.exitCode === 0
        ? "gtk+-3.0 and webkit2gtk-4.1 present"
        : "install with: apt install libgtk-3-dev libwebkit2gtk-4.1-dev"
    }
  ];
}

export async function checkSetupPrerequisites(
  runner: CommandRunner,
  repoRoot: string,
  platform: NodeJS.Platform = process.platform
): Promise<SetupResult> {
  const checks: SetupCheck[] = await checkNativeToolchain(runner, platform);

  for (const [name, command, args] of [
    ["rust", "rustc", ["--version"]],
    ["cargo", "cargo", ["--version"]],
    ["node", "node", ["--version"]],
    ["pnpm", "pnpm", ["--version"]],
    ["bazel", "bazel", ["version"]],
    ["git", "git", ["--version"]],
    ["tmux", "tmux", ["-V"]]
  ] as const) {
    const version = await commandVersion(runner, command, [...args]);
    checks.push({ name, ok: version !== null, message: version ?? `missing command: ${command}` });
  }

  const zig = await commandVersion(runner, "zig", ["version"]);
  checks.push({
    name: "zig",
    ok: zig !== null && versionAtLeast(zig, MINIMUM_ZIG_VERSION),
    message: zig === null
      ? "missing command: zig"
      : versionAtLeast(zig, MINIMUM_ZIG_VERSION)
        ? zig
        : `zig ${zig} is older than the required ${MINIMUM_ZIG_VERSION} (libghostty-vt build); upgrade with: brew upgrade zig`
  });

  checks.push({
    name: "node_modules",
    ok: existsSync(join(repoRoot, "node_modules")),
    message: existsSync(join(repoRoot, "node_modules")) ? "present" : "run ./kd setup"
  });

  return { ok: checks.every((check) => check.ok), checks };
}

export async function installSetupDependencies(runner: CommandRunner, repoRoot: string): Promise<void> {
  const result = await runner.run("pnpm", ["install"], { cwd: repoRoot });
  if (result.exitCode !== 0) {
    throw new Error(result.stderr || result.stdout || "pnpm install failed");
  }
}
