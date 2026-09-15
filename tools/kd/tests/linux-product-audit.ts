/** Independent native-lane verification of the package action's ELF reader. */
import { deepStrictEqual, equal } from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseReadelf, type ElfFacts } from "../src/runtime/linux-elf-audit";
import { INSTALLED_EXECUTABLES, packageLayout, type LinuxChannel } from "../src/runtime/linux-package";

const [deb] = process.argv.slice(2);
const report = JSON.parse(readFileSync(`${deb}.json`, "utf8")) as {
  builder: string;
  channel: LinuxChannel;
  sha256: string;
  executables: Array<ElfFacts & { sha256: string }>;
};
const hash = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");
equal(report.builder, "bazel");
equal(hash(deb), report.sha256, "package digest");
deepStrictEqual(report.executables.map(f => f.path), [...INSTALLED_EXECUTABLES]);
const normalize = (facts: ElfFacts) => ({
  machine: facts.machine,
  interpreter: facts.interpreter,
  needed: [...facts.needed].sort(),
  runpaths: [...facts.runpaths].sort(),
  versionRequirements: Object.fromEntries(Object.entries(facts.versionRequirements).map(([k, v]) => [k, [...v].sort()])),
});
for (const expected of report.executables) {
  const path = join(packageLayout({ channel: report.channel }).libDir, expected.path);
  equal(hash(path), expected.sha256, `${expected.path}: installed bytes`);
  const measured = parseReadelf(path, execFileSync("readelf", ["--wide", "-h", "-l", "-d", "-V", path], { encoding: "utf8" }));
  deepStrictEqual(normalize(measured), normalize(expected), `${expected.path}: native readelf disagrees with the Bazel audit`);
}
process.stdout.write("All eight installed executable hashes and native readelf facts match the Bazel package report.\n");
