/** Release collection accepts only the canonical Bazel lane, on a clean pinned
 * source. Inspect the actual ar/control/data bytes as well as its declared audit. */
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { basename, join } from "node:path";
import { gunzipSync } from "node:zlib";
import { buildLinuxPackageFromBazel } from "./linux-release-build";
import { channelIdentity, debFileName, debianArchitecture, debianVersion, INSTALLED_EXECUTABLES, packageLayout, type LinuxChannel } from "./linux-package";
import { auditArtifacts, readRuntimePolicy, type ElfFacts } from "./linux-elf-audit";
import type { CommandRunner } from "./process";
import type { AptPublicationArtifact } from "./linux-apt-publication";

export const sha256 = (bytes: Uint8Array | string): string => createHash("sha256").update(bytes).digest("hex");
export interface LinuxSource { revision: string; tree: string }
export interface LinuxArtifactIdentity {
  architecture: "x86_64" | "arm64";
  sourceRevision: string;
  buildRevision: string;
  buildTree: string;
  version: string;
  channel: LinuxChannel;
  iteration: number | null;
  label: string;
  fileName: string;
  sha256: string;
  sizeBytes: number;
  reportSha256: string;
}
export interface CollectedLinuxArtifact {
  identity: LinuxArtifactIdentity;
  publication: AptPublicationArtifact;
  report: Uint8Array;
}
export async function cleanLinuxSource(repoRoot: string, env: NodeJS.ProcessEnv, runner: CommandRunner): Promise<LinuxSource> {
  const git = async (args: string[]) => {
    const result = await runner.run("git", args, { cwd: repoRoot, env });
    if (result.exitCode) throw new Error(`Cannot pin Linux source: ${result.stderr}`);
    return result.stdout.trim();
  };
  if (await git(["status", "--porcelain", "--untracked-files=normal"])) throw new Error("Linux release requires a clean committed worktree.");
  const revision = await git(["rev-parse", "HEAD"]);
  const tree = await git(["rev-parse", "HEAD^{tree}"]);
  if (![revision, tree].every(v => /^[a-f0-9]{40}$/.test(v))) throw new Error("Invalid Linux source identity.");
  return { revision, tree };
}
function tarFiles(bytes: Buffer): Map<string, Buffer> {
  const files = new Map<string, Buffer>();
  for (let offset = 0; offset + 512 <= bytes.length;) {
    const header = bytes.subarray(offset, offset + 512);
    if (header.every(value => value === 0)) break;
    const str = (start: number, end: number) => header.subarray(start, end).toString().replace(/\0.*$/s, "");
    const checksum = Number.parseInt(str(148, 156).trim(), 8);
    const measured = header.reduce((sum, byte, index) => sum + (index >= 148 && index < 156 ? 32 : byte), 0);
    if (checksum !== measured) throw new Error("Invalid Debian tar header checksum.");
    const size = Number.parseInt(str(124, 136).trim(), 8);
    const name = [str(345, 500), str(0, 100)].filter(Boolean).join("/").replace(/^\.\//, "");
    if (!Number.isSafeInteger(size) || size < 0 || offset + 512 + size > bytes.length || name.split("/").includes("..")) throw new Error("Malformed Debian tar member.");
    if (header[156] === 0 || header[156] === 48) {
      if (files.has(name)) throw new Error("Duplicate Debian tar member.");
      files.set(name, bytes.subarray(offset + 512, offset + 512 + size));
    }
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  return files;
}
export function inspectLinuxDeb(bytes: Uint8Array): { control: Record<string, string>; files: Map<string, Buffer> } {
  const deb = Buffer.from(bytes);
  if (deb.subarray(0, 8).toString() !== "!<arch>\n") throw new Error("Not a declared Debian ar package.");
  const members = new Map<string, Buffer>();
  for (let offset = 8; offset < deb.length;) {
    const header = deb.subarray(offset, offset + 60);
    const size = Number(header.subarray(48, 58).toString().trim());
    const name = header.subarray(0, 16).toString().trim().replace(/\/$/, "");
    if (header.length !== 60 || header.subarray(58).toString() !== "`\n" || !Number.isSafeInteger(size) || size < 0 || offset + 60 + size > deb.length || members.has(name)) throw new Error("Malformed Debian ar package.");
    members.set(name, deb.subarray(offset + 60, offset + 60 + size));
    offset += 60 + size + size % 2;
  }
  if (members.size !== 3 || members.get("debian-binary")?.toString() !== "2.0\n" || !members.has("control.tar.gz") || !members.has("data.tar.gz")) throw new Error("Expected canonical Bazel Debian members.");
  const controlBytes = tarFiles(gunzipSync(members.get("control.tar.gz")!)).get("control");
  if (!controlBytes) throw new Error("Missing Debian control.");
  const control: Record<string, string> = {};
  let previous = "";
  for (const line of controlBytes.toString().trimEnd().split("\n")) {
    if (/^ /.test(line) && previous) { control[previous] += `\n${line}`; continue; }
    const match = /^([A-Za-z0-9-]+): (.+)$/.exec(line);
    if (!match || control[match[1]] !== undefined) throw new Error("Malformed Debian control.");
    previous = match[1]; control[previous] = match[2];
  }
  return { control, files: tarFiles(gunzipSync(members.get("data.tar.gz")!)) };
}
export function verifyLinuxArtifact(input: {
  repoRoot: string; source: LinuxSource; version: string; channel: LinuxChannel; iteration?: number;
  architecture: "x86_64" | "arm64"; debPath: string; report: Uint8Array; bytes: Uint8Array;
}): CollectedLinuxArtifact {
  const report = JSON.parse(Buffer.from(input.report).toString());
  const hash = sha256(input.bytes);
  if (report.buildRevision !== input.source.revision || report.buildTree !== input.source.tree || report.builder !== "bazel" || report.auditOverridden || report.version !== input.version || report.channel !== input.channel || report.architecture !== input.architecture || report.sha256 !== hash || report.debianVersion !== debianVersion(input.version, input.channel, input.iteration) || report.audit?.findings?.length !== 0) throw new Error("Linux artifact/report identity mismatch or overridden audit.");
  const { control, files } = inspectLinuxDeb(input.bytes);
  if (control.Package !== channelIdentity(input.channel).packageName || control.Version !== report.debianVersion || control.Architecture !== debianArchitecture(input.architecture) || control.Depends !== report.depends.join(", ")) throw new Error("Actual Debian control identity differs from the candidate.");
  const facts = report.executables as Array<ElfFacts & { sha256: string }>;
  if (!Array.isArray(facts) || JSON.stringify(facts.map(f => f.path).sort()) !== JSON.stringify([...INSTALLED_EXECUTABLES].sort())) throw new Error("Missing/duplicate audited Linux executable.");
  const libDir = packageLayout({ channel: input.channel }).libDir.replace(/^\//, "");
  for (const fact of facts) {
    const installed = files.get(`${libDir}/${fact.path}`);
    if (!installed || sha256(installed) !== fact.sha256) throw new Error(`Packaged executable differs from audit: ${fact.path}.`);
  }
  const policy = readRuntimePolicy(input.repoRoot);
  if (auditArtifacts(policy, input.architecture, facts).findings.length || facts.some(f => f.interpreter !== policy.architectures[input.architecture].interpreter || f.needed.some(n => /^(libc\+\+|libc\+\+abi|libunwind)\.so/.test(n)))) throw new Error("Linux release runtime audit failed.");
  const fileName = basename(input.debPath);
  if (fileName !== debFileName({ channel: input.channel, version: input.version, architecture: input.architecture, stagingIteration: input.iteration })) throw new Error("Renamed Linux package is not a canonical release artifact.");
  return {
    identity: { architecture: input.architecture, sourceRevision: input.source.revision, buildRevision: input.source.revision, buildTree: input.source.tree, version: input.version, channel: input.channel, iteration: input.iteration ?? null, label: `//packaging/linux:deb_${input.channel}_${input.architecture}`, fileName, sha256: hash, sizeBytes: input.bytes.byteLength, reportSha256: sha256(input.report) },
    report: Buffer.from(input.report),
    publication: { bytes: Buffer.from(input.bytes), artifact: { architecture: control.Architecture, fileName, sha256: hash, sizeBytes: input.bytes.byteLength, controlFields: control } },
  };
}
export async function collectLinuxRelease(input: {
  repoRoot: string; env: NodeJS.ProcessEnv; runner: CommandRunner; source: LinuxSource;
  version: string; channel: LinuxChannel; iteration?: number; skipBuild?: boolean;
}): Promise<CollectedLinuxArtifact[]> {
  const artifacts: CollectedLinuxArtifact[] = [];
  for (const architecture of ["x86_64", "arm64"] as const) {
    const built = await buildLinuxPackageFromBazel({ ...input, architecture, stagingIteration: input.iteration, outputDir: join(input.repoRoot, ".build/linux-package/out") });
    artifacts.push(verifyLinuxArtifact({ ...input, architecture, debPath: built.debPath, bytes: readFileSync(built.debPath), report: readFileSync(`${built.debPath}.json`) }));
  }
  const after = await cleanLinuxSource(input.repoRoot, input.env, input.runner);
  if (JSON.stringify(after) !== JSON.stringify(input.source)) throw new Error("Linux source changed while Bazel artifacts were built.");
  return artifacts;
}
