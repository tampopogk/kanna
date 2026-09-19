import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// A macOS bundle carries the version twice. `make_plist.py` stamps
// CFBundleShortVersionString from the `version_file` attribute (//:VERSION),
// while Tauri's codegen compiles the *config's* `version` field into the binary
// as PackageInfo.version -- and that, not the plist, is what
// `tauri_plugin_updater` compares against the release feed, because
// apps/desktop/src-tauri/src/lib.rs registers the plugin with no
// `current_version` override.
//
// `syncVersionFiles()` in tools/kd/src/runtime/release.ts rewrites VERSION,
// tauri.conf.json and Cargo.toml together, so during `kd release ship` the two
// halves agree by accident. Any direct Bazel build picks up the committed
// tauri.conf.json version instead, and the app's About menu then reports one
// version while its updater believes another. These cases pin both halves to
// //:VERSION so the accident is not what holds them together.

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
function runConfigGenrule(rule: Genrule): Record<string, unknown> {
  const dir = mkdtempSync(join(tmpdir(), "kd-desktop-version-wiring-"));
  temporaries.push(dir);

  const staleConfig = {
    version: "0.0.68",
    productName: "Kanna",
    identifier: "build.kanna",
    plugins: { updater: { endpoints: ["https://example.invalid/latest.json"] } },
  };
  const versionPath = join(dir, "VERSION");
  writeFileSync(versionPath, "9.9.9\n");
  const outputPath = join(dir, rule.out.split("/").pop() as string);

  const command = rule.cmd.replace(/\$\(location ([^)]+)\)/g, (_, label: string) => {
    if (label === "//:VERSION") return versionPath;
    // Every other input is a Tauri config: either the committed source or the
    // output of a sibling config genrule.
    const path = join(dir, `${label.replace(/[^A-Za-z0-9.]/g, "_")}.json`);
    writeFileSync(path, `${JSON.stringify(staleConfig, null, 2)}\n`);
    return path;
  });

  execFileSync("bash", ["-c", command.replaceAll("$@", outputPath)], { cwd: dir, encoding: "utf8" });
  return JSON.parse(readFileSync(outputPath, "utf8")) as Record<string, unknown>;
}

describe("desktop bundle version wiring", () => {
  it("stamps every generated Tauri config from //:VERSION", () => {
    for (const rule of tauriConfigGenrules()) {
      expect(rule.srcs, `${rule.name} must depend on //:VERSION`).toContain("//:VERSION");
      expect(runConfigGenrule(rule).version, `${rule.name} must stamp version from //:VERSION`).toBe("9.9.9");
    }
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

  it("stamps every macOS bundle plist from the same //:VERSION file", () => {
    const source = readFileSync(rootBuildPath, "utf8");
    const bundles = [...source.matchAll(/\ntauri_bundle_inputs\(\n {4}name = "([^"]+)",\n([\s\S]*?)\n\)\n/g)];

    expect(bundles.length, "BUILD.bazel must declare tauri_bundle_inputs targets").toBeGreaterThan(0);
    for (const [, name, body] of bundles) {
      expect(body, `${name} must take its plist version from the VERSION file`).toContain('version_file = "VERSION"');
      expect(body, `${name} must not hard-code a plist version`).not.toMatch(/\n {4}version = "/);
    }
  });
});
