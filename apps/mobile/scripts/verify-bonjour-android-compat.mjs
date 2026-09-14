import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { __internal } = require("../plugins/withKannaBonjour.js");
const mobileRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const worktreeRoot = path.resolve(mobileRoot, "../..");
const tempRoot = path.join(worktreeRoot, ".tmp");
fs.mkdirSync(tempRoot, { recursive: true });
const fixtureRoot = fs.mkdtempSync(path.join(tempRoot, "bonjour-android-compat-"));
const packageName = "build.kanna.compat";
const packagePath = path.join(...packageName.split("."));

function write(root, relativePath, contents) {
  const target = path.join(root, relativePath);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, contents);
}

function compile(sourceRoot, outputRoot, classpath) {
  const sources = [];
  function collect(directory) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const target = path.join(directory, entry.name);
      if (entry.isDirectory()) collect(target);
      else if (entry.name.endsWith(".java")) sources.push(target);
    }
  }
  collect(sourceRoot);
  fs.mkdirSync(outputRoot, { recursive: true });
  const args = ["-d", outputRoot];
  if (classpath) args.push("-classpath", classpath);
  execFileSync("javac", [...args, ...sources], { stdio: "inherit" });
}

function platformSources({ hostnameMethod }) {
  return {
    "android/os/Build.java": `package android.os;
public final class Build {
  public static final class VERSION { public static int SDK_INT; }
  public static final class VERSION_CODES { public static final int TIRAMISU = 33; }
}`,
    "android/os/ext/SdkExtensions.java": `package android.os.ext;
public final class SdkExtensions {
  public static int extensionVersion;
  public static int getExtensionVersion(int sdk) { return extensionVersion; }
}`,
    "android/net/nsd/NsdServiceInfo.java": `package android.net.nsd;
public class NsdServiceInfo {
  ${hostnameMethod ? "public String getHostname() { return \"studio.local\"; }" : ""}
}`
  };
}

function writePlatform(root, options) {
  for (const [relativePath, contents] of Object.entries(platformSources(options))) {
    write(root, relativePath, contents);
  }
}

const compileSources = path.join(fixtureRoot, "compile-src");
const compiled = path.join(fixtureRoot, "compiled");
writePlatform(compileSources, { hostnameMethod: true });
write(
  compileSources,
  path.join(packagePath, "KannaNsdCompat.java"),
  __internal.androidNsdCompatSource(packageName)
);
compile(compileSources, compiled);

function verifyRuntime(name, { hostnameMethod, cases }) {
  const sourceRoot = path.join(fixtureRoot, `${name}-src`);
  const outputRoot = path.join(fixtureRoot, `${name}-classes`);
  writePlatform(sourceRoot, { hostnameMethod });
  write(
    sourceRoot,
    path.join(packagePath, "CompatibilityRunner.java"),
    `package ${packageName};
import android.net.nsd.NsdServiceInfo;
import android.os.Build;
import android.os.ext.SdkExtensions;
public final class CompatibilityRunner {
  public static void main(String[] args) {
    ${cases.map(({ sdk, extension, expected }) => `
    Build.VERSION.SDK_INT = ${sdk};
    SdkExtensions.extensionVersion = ${extension};
    String actual${sdk} = KannaNsdCompat.hostname(new NsdServiceInfo());
    if (${expected === null ? `actual${sdk} != null` : `!\"${expected}\".equals(actual${sdk})`}) {
      throw new AssertionError("SDK ${sdk}, extension ${extension}: " + actual${sdk});
    }`).join("\n")}
  }
}`
  );
  compile(sourceRoot, outputRoot, compiled);
  execFileSync("java", ["-cp", `${outputRoot}${path.delimiter}${compiled}`, `${packageName}.CompatibilityRunner`], {
    stdio: "inherit"
  });
}

try {
  // These runtime classpaths intentionally omit getHostname(), reproducing the
  // binary shape that crashed when the generated module ran on API 34/35.
  verifyRuntime("legacy", {
    hostnameMethod: false,
    cases: [
      { sdk: 34, extension: 0, expected: null },
      { sdk: 35, extension: 16, expected: null }
    ]
  });
  verifyRuntime("modern", {
    hostnameMethod: true,
    cases: [
      { sdk: 36, extension: 0, expected: "studio.local" },
      { sdk: 34, extension: 17, expected: "studio.local" }
    ]
  });
  console.log("Bonjour Android compatibility: API 34/35 fallback and API 36/T-extension hostname paths passed.");
} finally {
  fs.rmSync(fixtureRoot, { recursive: true, force: true });
}
