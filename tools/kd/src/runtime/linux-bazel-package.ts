/** Bazel action entry point: reuse the installed layout and runtime policy. */
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { auditArtifacts, dependsFromAudit, type ElfFacts, type RuntimePolicy } from "./linux-elf-audit";
import { INSTALLED_EXECUTABLES, stageLinuxPackageTree, debianVersion, type LinuxArchitecture, type LinuxChannel } from "./linux-package";

const [manifestPath] = process.argv.slice(2);
interface PackageManifest {
  buildRevision: string;
  buildTree: string;
  architecture: LinuxArchitecture;
  channel: LinuxChannel;
  iteration: number;
  versionFile: string;
  products: Record<string, string>;
  resources: string[];
  icons: string[];
  policy: string;
  tool: string;
  output: string;
  report: string;
}
const input = JSON.parse(readFileSync(manifestPath, "utf8")) as PackageManifest;
const version = readFileSync(input.versionFile, "utf8").trim();
// Each Bazel executable owns its runfiles; the child must locate its own tree.
const toolEnv = { ...process.env };
delete toolEnv.RUNFILES_DIR;
delete toolEnv.RUNFILES_MANIFEST_FILE;
const runTool = (args: string[]) => execFileSync(resolve(input.tool), args, { encoding: "utf8", env: toolEnv });
// A malformed version must fail before writing an archive.
debianVersion(version, input.channel, input.channel === "staging" ? input.iteration : undefined);
const work = `${input.output}.work`;
mkdirSync(work, { recursive: true });
try {
  const binariesDir = join(work, "bin");
  const resourcesDir = join(work, ".kanna");
  const iconsDir = join(work, "icons");
  mkdirSync(binariesDir, { recursive: true });
  mkdirSync(iconsDir, { recursive: true });
  for (const name of INSTALLED_EXECUTABLES) copyFileSync(input.products[name], join(binariesDir, name));
  for (const source of input.resources) {
    const marker = source.indexOf(".kanna/");
    if (marker < 0) throw new Error(`Not a declared Kanna resource: ${source}`);
    const relative = source.slice(marker + ".kanna/".length);
    const dest = join(resourcesDir, relative);
    mkdirSync(dirname(dest), { recursive: true });
    copyFileSync(source, dest);
  }
  for (const size of [32, 64, 128]) {
    const source = input.icons.find(p => p.endsWith(`/icons/${size}x${size}.png`));
    if (!source) throw new Error(`Missing ${size}px icon`);
    copyFileSync(source, join(iconsDir, `${size}x${size}.png`));
  }
  const policy = JSON.parse(readFileSync(input.policy, "utf8")) as RuntimePolicy;
  const facts = JSON.parse(runTool(["elf", ...INSTALLED_EXECUTABLES.map(n => input.products[n])])) as ElfFacts[];
  facts.forEach((fact, index) => { fact.path = INSTALLED_EXECUTABLES[index]; });
  const audit = auditArtifacts(policy, input.architecture, facts);
  for (const fact of facts) {
    if (fact.interpreter !== policy.architectures[input.architecture].interpreter) throw new Error(`Wrong ELF interpreter: ${fact.path}`);
    if (fact.needed.some(n => /^(libc\+\+|libc\+\+abi|libunwind)\.so/.test(n))) throw new Error(`C++ runtime must be static: ${fact.path}`);
  }
  if (audit.findings.length) throw new Error(JSON.stringify(audit.findings, null, 2));
  const depends = dependsFromAudit(policy, audit);
  const root = join(work, "tree");
  stageLinuxPackageTree({ channel: input.channel, root, binariesDir, builtinResourcesDir: resourcesDir, iconsDir,
    control: { version, architecture: input.architecture, stagingIteration: input.channel === "staging" ? input.iteration : undefined, depends } });
  runTool(["deb", root, input.output]);
  const hash = (p: string) => createHash("sha256").update(readFileSync(p)).digest("hex");
  writeFileSync(input.report, JSON.stringify({ builder: "bazel", buildRevision: input.buildRevision, buildTree: input.buildTree, version, channel: input.channel,
    architecture: input.architecture, debianVersion: debianVersion(version, input.channel, input.channel === "staging" ? input.iteration : undefined),
    sha256: hash(input.output), depends, audit,
    executables: facts.map((fact, i) => ({ ...fact, path: INSTALLED_EXECUTABLES[i], sha256: hash(input.products[INSTALLED_EXECUTABLES[i]]) })),
  }, null, 2) + "\n");
} finally {
  rmSync(work, { recursive: true, force: true });
}
