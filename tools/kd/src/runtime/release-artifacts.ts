import { cpSync, existsSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { mustRun, type ReleaseEnvironment } from "./release-command";
import { resolveUpdaterSigningKey, updaterSignerEnvironment } from "./updater-key";
import type { ReleaseShipInput } from "./release-ship";

export type ReleaseArchLabel = "arm64" | "x86_64";

export function releaseAssetName(version: string, label: ReleaseArchLabel, environment: ReleaseEnvironment = "production"): string {
  if (environment === "staging") return `Kanna_Staging_${version}_${label}.dmg`;
  return `Kanna_${version}_${label}.dmg`;
}

export function updaterAssetName(version: string, label: ReleaseArchLabel, environment: ReleaseEnvironment = "production"): string {
  if (environment === "staging") return `Kanna_Staging_${version}_${label}.app.tar.gz`;
  return `Kanna_${version}_${label}.app.tar.gz`;
}

export function updaterSignatureName(version: string, label: ReleaseArchLabel, environment: ReleaseEnvironment = "production"): string {
  return `${updaterAssetName(version, label, environment)}.sig`;
}

export function updaterPlatformKey(label: ReleaseArchLabel): string {
  return label === "arm64" ? "darwin-aarch64" : "darwin-x86_64";
}

export function bazelTargetForLabel(label: ReleaseArchLabel, dryRun: boolean, environment: ReleaseEnvironment = "production"): string {
  if (environment === "staging") {
    return dryRun ? `//:kanna_signed_dmg_staging_${label}` : `//:kanna_notarized_dmg_staging_${label}`;
  }
  return dryRun ? `//:kanna_signed_dmg_release_${label}` : `//:kanna_notarized_dmg_release_${label}`;
}

export function signedAppTargetForLabel(label: ReleaseArchLabel, environment: ReleaseEnvironment = "production"): string {
  if (environment === "staging") {
    return label === "arm64" ? "//:kanna_signed_app_staging_arm64" : "//:kanna_signed_app_staging_x86_64";
  }
  return label === "arm64" ? "//:kanna_signed_app_release_arm64" : "//:kanna_signed_app_release_x86_64";
}

export function updaterBundleTargetForLabel(label: ReleaseArchLabel, environment: ReleaseEnvironment = "production"): string {
  if (environment === "staging") {
    return label === "arm64" ? "//:kanna_updater_bundle_staging_arm64" : "//:kanna_updater_bundle_staging_x86_64";
  }
  return label === "arm64" ? "//:kanna_updater_bundle_release_arm64" : "//:kanna_updater_bundle_release_x86_64";
}

export async function resolveBazelOutput(input: ReleaseShipInput, target: string): Promise<string> {
  const output = await mustRun(input.runner, "bazel", ["cquery", "-c", "opt", target, "--output=files"], input.repoRoot, input.env);
  const path = output.split("\n").filter(Boolean).at(-1);
  if (!path) throw new Error(`Bazel did not report an output file for ${target}`);
  return join(input.repoRoot, path);
}

export async function validateDmgImageResources(input: ReleaseShipInput, dmgPath: string): Promise<void> {
  const script = `
set -eu
mount_dir="$(mktemp -d "\${TMPDIR:-/tmp}/kanna-dmg-validate.XXXXXX")"
assets_file="$(mktemp "\${TMPDIR:-/tmp}/kanna-dmg-assets.XXXXXX")"
cleanup() {
  hdiutil detach -quiet "$mount_dir" >/dev/null 2>&1 || true
  rm -f "$assets_file"
  rmdir "$mount_dir" >/dev/null 2>&1 || true
}
trap cleanup EXIT
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mount_dir" "$1"
find "$mount_dir" -maxdepth 6 -type f \\( -name '*.icns' -o -name '*.png' \\) > "$assets_file"
if [ ! -s "$assets_file" ]; then
  echo "No PNG or ICNS image resources found in $1" >&2
  exit 1
fi
while IFS= read -r asset; do
  sips -g pixelWidth -g pixelHeight "$asset" >/dev/null
done < "$assets_file"
`.trim();

  await mustRun(input.runner, "sh", ["-c", script, "validate-dmg-images", dmgPath], input.repoRoot, input.env);
}

export function writeLatestJson(path: string, version: string, notes: string, pubDate: string, platforms: Record<string, { signature: string; url: string }>): void {
  writeFileSync(path, JSON.stringify({ version, notes, pub_date: pubDate, platforms }, null, 2) + "\n");
}

export async function createUpdaterBundle(
  input: ReleaseShipInput,
  bundleSource: string,
  bundlePath: string,
  signaturePath: string
): Promise<void> {
  const signingKey = await resolveUpdaterSigningKey({
    env: input.env
  });
  await createUpdaterBundleWithSigningKey(
    input,
    bundleSource,
    bundlePath,
    signaturePath,
    signingKey
  );
}

export async function createUpdaterBundleWithSigningKey(
  input: ReleaseShipInput,
  bundleSource: string,
  bundlePath: string,
  signaturePath: string,
  signingKey: string
): Promise<void> {
  rmSync(bundlePath, { force: true });
  cpSync(bundleSource, bundlePath);
  // The validated file content goes through the signer environment, never argv.
  // An explicit empty password prevents `tauri signer` from prompting during a
  // non-interactive ship; updater signing keys used by kd must be unencrypted.
  const signerEnv = updaterSignerEnvironment(input.env, signingKey);
  const signerArgs = ["--dir", join(input.repoRoot, "apps", "desktop"), "exec", "tauri", "signer", "sign", bundlePath];
  const signer = await input.runner.run("pnpm", signerArgs, {
    cwd: input.repoRoot,
    env: signerEnv
  });
  if (signer.exitCode !== 0) {
    // Never surface signer output: a broken signer could echo its secret env.
    throw new Error(`Tauri updater signing failed for ${bundlePath}.`);
  }
  const generatedSig = `${bundlePath}.sig`;
  if (!existsSync(generatedSig)) throw new Error(`Expected updater signature not found: ${generatedSig}`);
  if (generatedSig !== signaturePath) {
    rmSync(signaturePath, { force: true });
    renameSync(generatedSig, signaturePath);
  }
}
