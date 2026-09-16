import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

interface PackageManifest {
  dependencies: Record<string, string>;
  devDependencies: Record<string, string>;
}

interface LockfilePackage {
  dependencies?: Record<string, string>;
  devDependencies?: Record<string, string>;
}

interface PackageLockfile {
  packages: Record<string, LockfilePackage>;
}

const manifest = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8"),
) as PackageManifest;
const lockfile = JSON.parse(
  readFileSync(new URL("../package-lock.json", import.meta.url), "utf8"),
) as PackageLockfile;

describe("Firebase Functions package lockfile", () => {
  it("records every declared dependency for npm ci", () => {
    const rootPackage = lockfile.packages[""];

    expect(rootPackage).toBeDefined();
    if (!rootPackage) return;

    expect(rootPackage.dependencies).toEqual(manifest.dependencies);
    expect(rootPackage.devDependencies).toEqual(manifest.devDependencies);

    for (const dependency of Object.keys(manifest.dependencies)) {
      expect(lockfile.packages[`node_modules/${dependency}`]).toBeDefined();
    }
  });
});
