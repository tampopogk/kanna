import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { splitPublishedVersion } from "../src/runtime/release";
import { runStagingVersionGenrule } from "./staging-version-genrule";

// A macOS bundle carries the version twice. `make_plist.py` stamps
// CFBundleShortVersionString from the `version_file` attribute (//:VERSION),
// while Tauri's codegen compiles the *config's* `version` field into the binary
// as PackageInfo.version -- and that, not the plist, is what
// `tauri_plugin_updater` compares against the release feed, because
// apps/desktop/src-tauri/src/lib.rs registers the plugin with no
// `current_version` override.
//
// These cases pin both halves to one generated source so nothing accidental
// holds them together. There are two such sources, and which one a bundle uses
// is the whole version/RC design: a production bundle reads //:VERSION, while
// a staging bundle reads //:staging_version_file -- VERSION combined with the
// candidate counter in VERSION_RC. One commit therefore builds `X.Y.Z-staging.N`
// and `X.Y.Z`, which is why promotion needs no version commit of its own.
//
// What must never come back is a bundle taking its version from the committed
// tauri.conf.json, or hard-coding one: that is how the About menu and the
// updater came apart before.

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const desktopBuildPath = resolve(repoRoot, "apps", "desktop", "src-tauri", "BUILD.bazel");
const rootBuildPath = resolve(repoRoot, "BUILD.bazel");

const temporaries: string[] = [];
afterEach(() => {
  while (temporaries.length > 0) rmSync(temporaries.pop() as string, { recursive: true, force: true });
});

interface Genrule {
  name: string;
  srcs: string;
  out: string;
  cmd: string;
}

/**
 * Read the genrules out of a BUILD file. A line scanner rather than a regex:
 * the `cmd` bodies are heredocs holding both `)` lines and attribute-shaped
 * lines, so pattern matching across them silently swallows whole rules.
 */
function genrules(buildFile: string): Genrule[] {
  const lines = readFileSync(buildFile, "utf8").split("\n");
  const rules: Genrule[] = [];

  for (let index = 0; index < lines.length; index += 1) {
    if (lines[index] !== "genrule(") continue;

    const attributes = new Map<string, string[]>();
    let key: string | null = null;
    let inHeredoc = false;
    index += 1;

    for (; index < lines.length; index += 1) {
      const line = lines[index];
      if (inHeredoc) {
        if (line === '""",') inHeredoc = false;
        else attributes.get(key as string)?.push(line);
        continue;
      }
      if (line === ")") break;
      const attribute = line.match(/^ {4}([a-z_]+) = (.*)$/);
      if (attribute) {
        key = attribute[1];
        if (attribute[2] === '"""') {
          inHeredoc = true;
          attributes.set(key, []);
        } else {
          attributes.set(key, [attribute[2]]);
        }
        continue;
      }
      if (key) attributes.get(key)?.push(line);
    }

    const name = (attributes.get("name")?.join("\n") ?? "").match(/"([^"]+)"/)?.[1];
    const out = (attributes.get("outs")?.join("\n") ?? "").match(/"([^"]+)"/)?.[1];
    if (!name || !out) continue;
    rules.push({
      name,
      srcs: attributes.get("srcs")?.join("\n") ?? "",
      out,
      cmd: unescapeStarlark((attributes.get("cmd") ?? []).join("\n")),
    });
  }

  expect(rules.length, `${buildFile} must declare genrules`).toBeGreaterThan(0);
  return rules;
}

/** Resolve Starlark string escapes so the text matches what Bazel hands bash. */
function unescapeStarlark(text: string): string {
  return text.replace(/\\(.)/gs, (_, character: string) => {
    if (character === "n") return "\n";
    if (character === "t") return "\t";
    if (character === "r") return "\r";
    return character;
  });
}

/** Every genrule that emits a Tauri config consumed by a bundle or by codegen. */
function tauriConfigGenrules(): Genrule[] {
  const rules = genrules(desktopBuildPath).filter((rule) => rule.out.endsWith(".conf.json"));
  expect(rules.length, "expected Tauri config genrules in the desktop BUILD file").toBeGreaterThan(0);
  return rules;
}

/**
 * Run a genrule's `cmd` the way Bazel does -- textually substituting
 * `$(location ...)` and `$@`, then handing the result to bash -- against a
 * fixture whose committed config version is deliberately stale.
 */
function runConfigGenrule(
  rule: Genrule,
  versions: { version?: string; stagingVersion?: string } = {}
): Record<string, unknown> {
  const dir = mkdtempSync(join(tmpdir(), "kd-desktop-version-wiring-"));
  temporaries.push(dir);

  const staleConfig = {
    version: "0.0.68",
    productName: "Kanna",
    identifier: "build.kanna",
    plugins: { updater: { endpoints: ["https://example.invalid/latest.json"] } },
  };
  const versionPath = join(dir, "VERSION");
  writeFileSync(versionPath, `${versions.version ?? "9.9.9"}\n`);
  // The staging config stamps from the generated combined file, so the fixture
  // supplies what that genrule would have produced for the same VERSION.
  const stagingVersionPath = join(dir, "VERSION_staging");
  writeFileSync(stagingVersionPath, `${versions.stagingVersion ?? "9.9.9-staging.4"}\n`);
  const outputPath = join(dir, rule.out.split("/").pop() as string);

  const command = rule.cmd.replace(/\$\(location ([^)]+)\)/g, (_, label: string) => {
    if (label === "//:VERSION") return versionPath;
    if (label === "//:staging_version_file") return stagingVersionPath;
    // Every other input is a Tauri config: either the committed source or the
    // output of a sibling config genrule.
    const path = join(dir, `${label.replace(/[^A-Za-z0-9.]/g, "_")}.json`);
    writeFileSync(path, `${JSON.stringify(staleConfig, null, 2)}\n`);
    return path;
  });

  execFileSync("bash", ["-c", command.replaceAll("$@", outputPath)], { cwd: dir, encoding: "utf8" });
  return JSON.parse(readFileSync(outputPath, "utf8")) as Record<string, unknown>;
}

/**
 * The version kd writes into the worktree for a build that must publish
 * `published`. This is kd's own split, not a restatement of it, so the two
 * halves cannot drift apart in the one direction that matters.
 */
function versionFilesFor(published: string): { version: string; candidate: string } {
  const { base, candidate } = splitPublishedVersion(published);
  return { version: base, candidate: String(candidate) };
}

describe("the version kd writes is the version Bazel stamps", () => {
  // The fault this pins is an interaction, so it drives the real genrule with
  // the real file contents rather than a hand-written fixture: kd wrote the
  // fully-suffixed string into VERSION on the bare-main path, the genrule
  // appended the counter again, and a bundle published as 0.5.0-staging.3 was
  // built as 0.5.0-staging.3-staging.1 -- semver-greater than its own feed, so
  // no installed staging client would ever update to any candidate.
  it.each([
    { published: "0.5.0-staging.3", path: "a bare-main candidate" },
    { published: "0.4.1-staging.2", path: "a release-branch candidate" },
    { published: "1.10.0-staging.11", path: "multi-digit components" }
  ])("round-trips $path through the real genrule", ({ published }) => {
    const files = versionFilesFor(published);
    expect(runStagingVersionGenrule(files.version, files.candidate)).toBe(published);
  });

  it("carries that same string into the staging config the updater compares", () => {
    const published = "0.5.0-staging.3";
    const files = versionFilesFor(published);
    const stagingVersion = runStagingVersionGenrule(files.version, files.candidate);
    const staging = tauriConfigGenrules().find((rule) => rule.srcs.includes("//:staging_version_file"));
    expect(staging, "expected a staging Tauri config genrule").toBeDefined();

    const config = runConfigGenrule(staging as Genrule, { version: files.version, stagingVersion });
    expect(config.version).toBe(published);
  });

  it("leaves a production version untouched, counter and all", () => {
    // A production build reads VERSION directly, so the published version is
    // written whole and the counter beside it is never consulted.
    expect(versionFilesFor("0.4.1")).toEqual({ version: "0.4.1", candidate: "0" });
  });
});

describe("desktop bundle version wiring", () => {
  it("stamps every generated Tauri config from a generated version file", () => {
    for (const rule of tauriConfigGenrules()) {
      const staging = rule.srcs.includes("//:staging_version_file");
      expect(
        staging || rule.srcs.includes("//:VERSION"),
        `${rule.name} must depend on //:VERSION or //:staging_version_file`
      ).toBe(true);
      // A staging config carries the candidate counter; a production config is
      // the bare release version. Both are stamped, neither is the committed one.
      expect(runConfigGenrule(rule).version, `${rule.name} must stamp its version`).toBe(
        staging ? "9.9.9-staging.4" : "9.9.9"
      );
    }
  });

  it("gives the staging config the candidate counter and production the bare version", () => {
    // The pairing that makes one commit buildable as both. If a staging config
    // ever stamps the bare version, every candidate of a series reports the
    // same version and installed staging clients stop updating between them.
    const byStagingSource = tauriConfigGenrules().map((rule) => ({
      name: rule.name,
      staging: rule.srcs.includes("//:staging_version_file")
    }));
    expect(byStagingSource.filter((rule) => rule.staging).map((rule) => rule.name)).toEqual([
      "tauri_staging_bazel_config"
    ]);
    expect(byStagingSource.filter((rule) => !rule.staging).length).toBeGreaterThan(0);
  });

  it("feeds the Tauri context codegen only version-stamped configs", () => {
    // The plist is right because `version_file` points at //:VERSION. The
    // binary is only right if the config reaching Tauri's codegen is one of the
    // genrules above -- pointing it back at the committed tauri.conf.json is
    // exactly how the two halves came apart.
    const stamped = new Set(tauriConfigGenrules().map((rule) => rule.name));
    const source = readFileSync(desktopBuildPath, "utf8");
    const configAttributes = [...source.matchAll(/\n {4}config = (select\(\{[\s\S]*?\n {4}\}\)|"[^"]+"),/g)];

    expect(configAttributes.length, "expected config attributes on the Tauri codegen rules").toBeGreaterThan(0);
    for (const [, attribute] of configAttributes) {
      const labels = [...attribute.matchAll(/":([A-Za-z0-9_]+)"/g)].map((match) => match[1]);
      expect(labels.length, `unparsed config attribute: ${attribute}`).toBeGreaterThan(0);
      for (const label of labels) {
        expect(stamped, `config ${label} does not stamp version from //:VERSION`).toContain(label);
      }
    }
  });

  it("stamps every macOS bundle plist from a version file, matching its own config", () => {
    const source = readFileSync(rootBuildPath, "utf8");
    const bundles = [...source.matchAll(/\ntauri_bundle_inputs\(\n {4}name = "([^"]+)",\n([\s\S]*?)\n\)\n/g)];

    expect(bundles.length, "BUILD.bazel must declare tauri_bundle_inputs targets").toBeGreaterThan(0);
    for (const [, name, body] of bundles) {
      // A staging bundle must read the same combined file its Tauri config
      // does, or the plist and the compiled PackageInfo disagree again -- the
      // exact split this file exists to prevent, just one layer along.
      const stagingConfig = body.includes("tauri_staging_bazel_config");
      expect(body, `${name} must take its plist version from a version file`).toContain(
        stagingConfig ? 'version_file = ":staging_version_file"' : 'version_file = "VERSION"'
      );
      expect(body, `${name} must not hard-code a plist version`).not.toMatch(/\n {4}version = "/);
    }
  });
});
