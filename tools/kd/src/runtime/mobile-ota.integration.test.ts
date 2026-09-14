import { execFile, spawn, type ChildProcess } from "node:child_process";
import { createHash, generateKeyPairSync, verify } from "node:crypto";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { expect, it } from "vitest";
import { buildMobileOtaPublishPlan, stageOtaUpdate } from "./mobile-ota";

const run = promisify(execFile);
const repoRoot = fileURLToPath(new URL("../../../../", import.meta.url));

// Opt in: performs a real Hermes Android export and exercises the real relay
// process against the exact directory the publisher would upload. No cloud/device writes.
it.skipIf(process.env.KANNA_RUN_OTA_EXPORT_INTEGRATION !== "1")("serves a signed real Android Expo export and every exported asset", async () => {
  const scratchRoot = join(repoRoot, ".tmp");
  await mkdir(scratchRoot, { recursive: true });
  const scratch = await mkdtemp(join(scratchRoot, "ota-real-export-"));
  let relay: ChildProcess | undefined;
  let relayExit: Promise<unknown> | undefined;
  try {
    const distDir = join(scratch, "export");
    const plan = await buildMobileOtaPublishPlan({
      repoRoot, environment: "staging", platform: "android", distDir, updateId: "export-pending",
    });
    const [exportCommand, configCommand] = plan.commands;
    const exported = await run(exportCommand.command, exportCommand.args, {
      cwd: exportCommand.cwd, env: { ...process.env, ...exportCommand.env }, maxBuffer: 16 * 1024 * 1024,
    });
    await writeFile(join(scratchRoot, "ota-android-export.log"), exported.stdout + exported.stderr);
    const config = await run(configCommand.command, configCommand.args, {
      cwd: configCommand.cwd, env: { ...process.env, ...configCommand.env }, maxBuffer: 16 * 1024 * 1024,
    });
    const sourceCommit = (await run("git", ["rev-parse", "HEAD"], { cwd: repoRoot })).stdout.trim();
    const staged = await stageOtaUpdate({
      scratch, distDir, expoConfigBytes: Buffer.from(config.stdout), platform: "android",
      runtimeVersion: plan.runtimeVersion, releaseVersion: plan.releaseVersion,
      source: { ref: "HEAD", commit: sourceCommit, shortCommit: sourceCommit.slice(0, 12) },
    });
    const storage = join(scratch, "storage");
    const runtimeRoot = join(storage, "ota/android", plan.runtimeVersion);
    await cp(staged.path, join(runtimeRoot, "updates", staged.updateId), { recursive: true });
    await mkdir(join(runtimeRoot, "channels"), { recursive: true });
    await writeFile(join(runtimeRoot, "channels/staging.json"), JSON.stringify({ currentUpdateId: staged.updateId, createdAt: new Date().toISOString() }));
    const keys = generateKeyPairSync("rsa", { modulusLength: 2048 });
    const privateKeyPath = join(scratch, "signing.pem");
    await writeFile(privateKeyPath, keys.privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 });
    const port = await new Promise<number>((resolve, reject) => {
      const server = createServer();
      server.once("error", reject);
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        if (!address || typeof address === "string") throw new Error("missing port");
        server.close(error => error ? reject(error) : resolve(address.port));
      });
    });
    let relayOutput = "";
    relay = spawn("pnpm", ["exec", "tsx", "src/index.ts"], {
      cwd: join(repoRoot, "services/relay"), detached: true, stdio: "pipe",
      env: { ...process.env, PORT: String(port), KANNA_OTA_STORAGE_DIR: storage,
        KANNA_OTA_PRIVATE_KEY_PEM: "", KANNA_OTA_PRIVATE_KEY_PATH: privateKeyPath },
    });
    relay.stdout?.on("data", chunk => { relayOutput += chunk; });
    relay.stderr?.on("data", chunk => { relayOutput += chunk; });
    relayExit = new Promise(resolve => relay!.once("exit", resolve));
    await expect.poll(async () => {
      if (relay!.exitCode !== null) throw new Error(relayOutput);
      return (await fetch(`http://127.0.0.1:${port}/health`).catch(() => null))?.status;
    }, { timeout: 60_000 }).toBe(200);
    const response = await fetch(`http://127.0.0.1:${port}/ota/manifest`, { headers: {
      "expo-protocol-version": "1", "expo-platform": "android",
      "expo-runtime-version": plan.runtimeVersion, "expo-channel-name": "staging",
    } });
    expect(response.status).toBe(200);
    const multipart = await response.text();
    const json = multipart.split("\r\n\r\n")[1].split("\r\n--")[0];
    const signature = /expo-signature: sig="([^"]+)"/.exec(multipart)![1];
    expect(verify("RSA-SHA256", Buffer.from(json), keys.publicKey, Buffer.from(signature, "base64"))).toBe(true);
    const manifest = JSON.parse(json);
    expect(manifest.id).toBe(staged.updateId);
    expect(manifest.runtimeVersion).toBe(plan.runtimeVersion);
    const metadata = JSON.parse(await readFile(join(distDir, "metadata.json"), "utf8")).fileMetadata.android;
    const originals = [metadata.bundle, ...metadata.assets.map((asset: { path: string }) => asset.path)];
    const assets = [manifest.launchAsset, ...manifest.assets];
    expect(assets.length).toBe(originals.length);
    expect(assets.length).toBeGreaterThan(1);
    for (const [index, asset] of assets.entries()) {
      const original = await readFile(join(distDir, originals[index]));
      expect(asset.hash).toBe(createHash("sha256").update(original).digest("base64url"));
      expect(new URL(asset.url).searchParams.get("platform")).toBe("android");
      const served = await fetch(asset.url);
      expect(served.status).toBe(200);
      expect(Buffer.from(await served.arrayBuffer())).toEqual(original);
    }
    await writeFile(join(scratchRoot, "ota-android-integration.json"), JSON.stringify({
      sourceCommit, runtimeVersion: plan.runtimeVersion, updateId: staged.updateId,
      verifiedAssets: assets.length, signedManifest: true, platform: "android", deviceDelivery: "not tested",
    }, null, 2));
  } finally {
    if (relay?.pid) {
      try { process.kill(-relay.pid, "SIGTERM"); } catch { /* already exited */ }
      await relayExit;
    }
    await rm(scratch, { recursive: true, force: true });
  }
}, 300_000);
