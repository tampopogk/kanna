/**
 * `./kd build linux-package` — build one architecture's `.deb`.
 *
 * The order is the point. Build, then *audit*, then derive `Depends` from what
 * the audit found, then package. A conventional Debian build writes `Depends`
 * by hand and hopes; here the declared dependencies are a consequence of the
 * measured artifact closure, so a library that appears because the build host
 * happened to have it cannot become a silent runtime requirement — it fails
 * the audit instead.
 *
 * The canonical path consumes audited Bazel outputs on supported build hosts.
 * The historical direct assembler remains an explicitly marked prototype.
 */

import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import type { CommandRunner } from "./process";
import {
  INSTALLED_EXECUTABLES,
  buildDebCommand,
  channelIdentity,
  debFileName,
  debianVersion,
  debianArchitecture,
  packageLayout,
  stageLinuxPackageTree,
  type LinuxArchitecture,
  type LinuxChannel,
} from "./linux-package";
import {
  auditArtifacts,
  dependsFromAudit,
  formatAuditReport,
  parseReadelf,
  readRuntimePolicy,
  readelfCommand,
  type AuditResult,
} from "./linux-elf-audit";

export interface LinuxPackageBuildInput {
  repoRoot: string;
  channel: LinuxChannel;
  architecture: LinuxArchitecture;
  version: string;
  stagingIteration?: number;
  /** Historical prototype input directory; unused by the Bazel path. */
  binariesDir?: string;
  outputDir: string;
  env: NodeJS.ProcessEnv;
  runner: CommandRunner;
  /** Skip the audit's veto. Only for iterating locally: a release build must
   *  never set it, and `assembleLinuxPackage` records that it was set. */
  allowAuditFindings?: boolean;
}

export interface LinuxPackageBuildResult {
  builder: "bazel" | "prototype";
  debPath: string;
  sha256: string;
  depends: string[];
  audit: AuditResult;
  auditReport: string;
  auditOverridden: boolean;
}

/** Where `kd` stages the eight executables a package ships. Inside `.build/`,
 *  so it follows the repo's artifact rule and is cleaned with everything else. */
export function linuxPackageStagingDir(repoRoot: string, architecture: LinuxArchitecture): string {
  return join(repoRoot, ".build", "linux-package", architecture);
}

/** Build and collect only declared Bazel package outputs. A skip is a Bazel
 * up-to-date check, so an old Cargo staging directory can never satisfy it. */
export async function buildLinuxPackageFromBazel(input: Omit<LinuxPackageBuildInput, "binariesDir"> & {
  skipBuild?: boolean;
  stagingIteration?: number;
}): Promise<LinuxPackageBuildResult> {
  if (input.allowAuditFindings) throw new Error("Bazel Linux packages require a clean audit; overrides are not supported.");
  const sourceVersion = readFileSync(join(input.repoRoot, "VERSION"), "utf8").trim();
  if (input.version !== sourceVersion) throw new Error(`Package version must match VERSION (${sourceVersion}); stamp the source before building.`);
  const iteration = input.channel === "staging" ? (input.stagingIteration ?? 1) : input.stagingIteration;
  const expectedVersion = debianVersion(input.version, input.channel, iteration);
  const label = `//packaging/linux:deb_${input.channel}_${input.architecture}`;
  const options = ["-c", "opt", `--//packaging/linux:staging_iteration=${iteration ?? 1}`];
  const run = async (args: string[], streamOutput = false) => {
    const result = await input.runner.run("bazel", args, { cwd: input.repoRoot, env: input.env, streamOutput });
    if (result.exitCode !== 0) throw new Error(`Bazel Linux package failed: ${result.stderr || result.stdout}`);
    return result.stdout;
  };
  await run(["build", ...options, ...(input.skipBuild ? ["--check_up_to_date"] : []), label], true);
  const outputs = (await run(["cquery", ...options, "--output=files", label])).trim().split(/\r?\n/);
  const one = (suffix: string) => {
    const matches = outputs.filter(p => p.endsWith(suffix));
    if (matches.length !== 1) throw new Error(`Expected one declared ${suffix} output for ${label}.`);
    return join(input.repoRoot, matches[0]);
  };
  const builtDeb = one(".deb");
  const reportPath = one(".json");
  const report = JSON.parse(readFileSync(reportPath, "utf8")) as {
    builder: string; version: string; channel: string; architecture: string; debianVersion: string;
    sha256: string; depends: string[]; audit: AuditResult;
  };
  const sha256 = createHash("sha256").update(readFileSync(builtDeb)).digest("hex");
  if (report.builder !== "bazel" || report.version !== sourceVersion || report.channel !== input.channel ||
      report.architecture !== input.architecture ||
      report.debianVersion !== expectedVersion ||
      report.sha256 !== sha256 || report.audit.findings.length) {
    throw new Error("Declared Linux package output does not match its audit report.");
  }
  mkdirSync(input.outputDir, { recursive: true });
  const debPath = join(input.outputDir, debFileName({ ...input, stagingIteration: iteration }));
  // Bazel outputs are read-only. Replace collected files atomically rather
  // than overwriting a previous copy that inherited that mode.
  const collection = mkdtempSync(join(input.outputDir, ".collect-"));
  try {
    for (const [source, destination] of [[builtDeb, debPath], [reportPath, `${debPath}.json`]]) {
      const staged = join(collection, "output");
      copyFileSync(source, staged);
      chmodSync(staged, 0o644);
      renameSync(staged, destination);
    }
  } finally {
    rmSync(collection, { recursive: true, force: true });
  }
  return { builder: "bazel", debPath, sha256, depends: report.depends, audit: report.audit,
    auditReport: formatAuditReport(input.architecture, report.audit), auditOverridden: false };
}

/** Read one artifact's ELF facts through `readelf` on the build host. */
export async function readElfFacts(runner: CommandRunner, repoRoot: string, env: NodeJS.ProcessEnv, path: string) {
  const [command, args] = readelfCommand(path);
  const result = await runner.run(command, args, { cwd: repoRoot, env });
  if (result.exitCode !== 0) {
    throw new Error(`readelf failed on ${path}: ${result.stderr || result.stdout}`);
  }
  return parseReadelf(path, result.stdout);
}

/**
 * Historical prototype assembler; never a publication input.
 * Stage, audit, package.
 *
 * The tree is staged twice on purpose: once to have real installed paths for
 * the audit to read, and again with the derived `Depends` written into
 * `control`. Auditing the *staged* files rather than the build outputs is what
 * makes the finding describe the artifact a user will actually run.
 */
export async function assembleLinuxPackage(input: LinuxPackageBuildInput): Promise<LinuxPackageBuildResult> {
  const policy = readRuntimePolicy(input.repoRoot);
  const binariesDir = input.binariesDir ?? linuxPackageStagingDir(input.repoRoot, input.architecture);
  for (const name of INSTALLED_EXECUTABLES) {
    if (!existsSync(join(binariesDir, name))) {
      throw new Error(`${name} is not staged in ${binariesDir}. Build it before packaging.`);
    }
  }

  const treeRoot = join(input.outputDir, `${channelIdentity(input.channel).packageName}-tree`);
  rmSync(treeRoot, { recursive: true, force: true });
  mkdirSync(input.outputDir, { recursive: true });

  const stageInput = {
    channel: input.channel,
    root: treeRoot,
    binariesDir,
    builtinResourcesDir: join(input.repoRoot, ".kanna"),
    iconsDir: join(input.repoRoot, "apps", "desktop", "src-tauri", "icons"),
    control: {
      version: input.version,
      stagingIteration: input.stagingIteration,
      architecture: input.architecture,
      // Placeholder: replaced below by the audited closure. Never shipped —
      // the tree is rebuilt with the derived list before `dpkg-deb` runs.
      depends: ["libc6"] as string[],
    },
  };
  stageLinuxPackageTree(stageInput);

  const layout = packageLayout({ channel: input.channel, prefix: join(treeRoot, "usr") });
  const facts = [];
  for (const name of INSTALLED_EXECUTABLES) {
    facts.push(await readElfFacts(input.runner, input.repoRoot, input.env, join(layout.libDir, name)));
  }
  const audit = auditArtifacts(policy, input.architecture, facts);
  const auditReport = formatAuditReport(input.architecture, audit);
  if (audit.findings.length > 0 && input.allowAuditFindings !== true) {
    throw new Error(
      `Linux runtime audit failed; the package would not run on ${policy.baseline.distribution}.\n${auditReport}`
    );
  }

  const depends = dependsFromAudit(policy, audit);
  rmSync(treeRoot, { recursive: true, force: true });
  stageLinuxPackageTree({ ...stageInput, control: { ...stageInput.control, depends } });

  const debPath = join(
    input.outputDir,
    debFileName({
      channel: input.channel,
      version: input.version,
      stagingIteration: input.stagingIteration,
      architecture: input.architecture,
    })
  );
  const [command, args] = buildDebCommand(treeRoot, debPath);
  const built = await input.runner.run(command, args, { cwd: input.repoRoot, env: input.env });
  if (built.exitCode !== 0) {
    throw new Error(`dpkg-deb failed: ${built.stderr || built.stdout}`);
  }

  return {
    builder: "prototype",
    debPath,
    sha256: createHash("sha256").update(readFileSync(debPath)).digest("hex"),
    depends,
    audit,
    auditReport,
    auditOverridden: input.allowAuditFindings === true && audit.findings.length > 0,
  };
}

/** One line an operator or a CI log can read without opening the JSON. */
export function formatLinuxPackageResult(input: {
  channel: LinuxChannel;
  architecture: LinuxArchitecture;
  result: LinuxPackageBuildResult;
}): string {
  const lines = [
    `${input.result.debPath} (${debianArchitecture(input.architecture)})`,
    `  sha256: ${input.result.sha256}`,
    `  Depends: ${input.result.depends.join(", ")}`,
    input.result.auditReport,
  ];
  if (input.result.builder === "prototype") {
    lines.push("  PROTOTYPE: these inputs have no Bazel product provenance and must not be published.");
  }
  if (input.result.auditOverridden) {
    lines.push("  WARNING: audit findings were overridden. This artifact must not be published.");
  }
  return lines.join("\n");
}
