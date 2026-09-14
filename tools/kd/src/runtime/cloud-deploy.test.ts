import { afterEach, describe, expect, it, vi } from "vitest";
import { buildRelayDeployPlan, deployFirebaseCloud } from "./cloud-deploy.js";
import * as environments from "./environment.js";
import type { CommandRunner } from "./process.js";

const SOURCE_COMMIT = "1f2e3d4c5b6a79880123456789abcdef01234567";
const identities = {
  staging: { ...environments.resolveKdEnvironment("staging") },
  prod: { ...environments.resolveKdEnvironment("prod") },
  dev: { ...environments.resolveKdEnvironment("dev") }
};

function mockPolicy(value: unknown): void {
  vi.spyOn(environments, "resolveKdEnvironment").mockImplementation((name) => ({
    ...identities[name],
    ...(name === "staging" ? { relayEntitlementEnforcement: value as never } : {})
  }));
}

function mockDeployment() {
  const calls: string[][] = [];
  const remoteEnvs: string[] = [];
  const runner: CommandRunner = {
    async run(command, args) {
      calls.push([command, ...args]);
      if (command === "git") {
        return { exitCode: 0, stdout: args[0] === "status" ? "" : SOURCE_COMMIT, stderr: "" };
      }
      // Capture each replacement .env, rather than treating redeploy as a merge.
      const remote = args[args.indexOf("--command") + 1];
      const env = remote?.match(/cat > \.env\.tmp <<'KANNA_RELAY_ENV'\n([\s\S]*?)\nKANNA_RELAY_ENV/);
      if (env) remoteEnvs.push(env[1]!);
      return { exitCode: 0, stdout: "", stderr: "" };
    }
  };
  return { runner, calls, remoteEnvs };
}

afterEach(() => vi.restoreAllMocks());

describe("relay deploy plan", () => {
  it.each(["staging", "production"] as const)("keeps %s explicitly off in the shipped registry", (environment) => {
    const plan = buildRelayDeployPlan({ repoRoot: "/repo", environment, commit: SOURCE_COMMIT });
    expect(plan.entitlementEnforcement).toEqual({
      value: "off",
      source: `tools/kd/src/runtime/environment.ts#${environment === "production" ? "prod" : "staging"}.relayEntitlementEnforcement`
    });
    expect(plan.commands.at(-1)?.args.at(-1)).toContain("KANNA_RELAY_ENTITLEMENT_ENFORCEMENT=off\n");
  });

  it.each(["on", "off", undefined] as const)("retains registry policy %s across two mocked redeploys", async (value) => {
    mockPolicy(value);
    const { runner, remoteEnvs } = mockDeployment();
    const info: string[] = [];
    const input = {
      repoRoot: "/repo", runner, environment: "staging" as const, relay: true,
      env: { KANNA_RELAY_ENTITLEMENT_ENFORCEMENT: value === "on" ? "off" : "on" },
      writeInfo: (line: string) => info.push(line), writeWarning: () => {}
    };
    const first = await deployFirebaseCloud(input);
    const second = await deployFirebaseCloud(input);
    expect(second).toEqual(first);
    expect(remoteEnvs).toHaveLength(2);
    expect(remoteEnvs[1]).toBe(remoteEnvs[0]);
    expect(remoteEnvs[1]).toContain(`KANNA_RELAY_ENTITLEMENT_ENFORCEMENT=${value ?? "off"}\n`);
    expect(first.relay?.entitlementEnforcement.value).toBe(value ?? "off");
    expect(first.relay?.entitlementEnforcement.source.includes("default off")).toBe(value === undefined);
    expect(info).toHaveLength(2);
    expect(JSON.parse(info[0]!.slice("Relay deploy plan: ".length))).toEqual(first.relay);
  });

  it("keeps a staging on choice out of production, even with an inherited shell flag", async () => {
    mockPolicy("on");
    const { runner, remoteEnvs } = mockDeployment();
    const input = {
      repoRoot: "/repo", runner, relay: true, ref: "release/0.2",
      env: { KANNA_RELAY_ENTITLEMENT_ENFORCEMENT: "on", KANNA_CLOUD_ENV: "staging" },
      writeInfo: () => {}, writeWarning: () => {}
    };
    await deployFirebaseCloud({ ...input, environment: "staging" });
    const production = await deployFirebaseCloud({ ...input, environment: "production" });
    expect(remoteEnvs[0]).toContain("KANNA_RELAY_ENTITLEMENT_ENFORCEMENT=on\n");
    expect(remoteEnvs[1]).toContain("FIREBASE_PROJECT_ID=kanna-build\n");
    expect(remoteEnvs[1]).toContain("KANNA_RELAY_ENTITLEMENT_ENFORCEMENT=off\n");
    expect(production.relay?.entitlementEnforcement.source).toContain("#prod.");
  });

  it("replaces an enabled fixture with off when the environment policy is rolled back", async () => {
    mockPolicy("on");
    const { runner, remoteEnvs } = mockDeployment();
    const input = {
      repoRoot: "/repo", runner, environment: "staging" as const, relay: true, env: {},
      writeInfo: () => {}, writeWarning: () => {}
    };
    await deployFirebaseCloud(input);
    vi.restoreAllMocks();
    mockPolicy("off");
    await deployFirebaseCloud(input);
    expect(remoteEnvs[0]).toContain("KANNA_RELAY_ENTITLEMENT_ENFORCEMENT=on\n");
    expect(remoteEnvs[1]).toContain("KANNA_RELAY_ENTITLEMENT_ENFORCEMENT=off\n");
    expect(remoteEnvs[1]).not.toContain("KANNA_RELAY_ENTITLEMENT_ENFORCEMENT=on");
  });

  it.each(["", "true", "1", "ON", " on ", "staging", null, false, "on\nINJECTED=yes"])(
    "rejects invalid registry policy %j before any remote/build command", async (value) => {
      mockPolicy(value);
      const { runner, calls } = mockDeployment();
      await expect(deployFirebaseCloud({
        repoRoot: "/repo", runner, env: {}, environment: "staging", relay: true, functions: true
      })).rejects.toThrow("staging.relayEntitlementEnforcement must be off or on");
      expect(calls.every(([command]) => command === "git")).toBe(true);
    }
  );

  it("refuses a Firebase override selecting the other environment before any remote work", async () => {
    const { runner, calls } = mockDeployment();
    await expect(deployFirebaseCloud({
      repoRoot: "/repo", runner, environment: "production", relay: true, functions: true,
      ref: "release/0.2", env: { KANNA_FIREBASE_PRODUCTION_PROJECT: "kanna-staging" }
    })).rejects.toThrow("Refusing a cross-project relay deploy");
    expect(calls.every(([command]) => command === "git")).toBe(true);
  });

  it("refuses a mismatched registry identity", () => {
    vi.spyOn(environments, "resolveKdEnvironment").mockReturnValue(identities.staging);
    expect(() => buildRelayDeployPlan({
      repoRoot: "/repo", environment: "production", commit: SOURCE_COMMIT
    })).toThrow("Relay environment identity does not match production");
  });

  it("reports a safe local plan with the same policy evidence as deployment", async () => {
    mockPolicy("on");
    const { runner, calls } = mockDeployment();
    const input = {
      repoRoot: "/repo", runner, environment: "staging" as const, relay: true,
      env: { KANNA_RELAY_STATS_TOKEN: "secret-must-not-appear" },
      writeInfo: () => {}, writeWarning: () => {}
    };
    const plan = await deployFirebaseCloud({ ...input, dryRun: true });
    expect(calls.every(([command]) => command === "git")).toBe(true);
    expect(plan).toMatchObject({
      dryRun: true, deployed: false, targets: [],
      source: { ref: "HEAD", commit: SOURCE_COMMIT },
      relay: { projectId: "kanna-staging", environment: "staging", entitlementEnforcement: { value: "on" } }
    });
    expect(JSON.stringify(plan)).not.toContain("secret-must-not-appear");
    expect((await deployFirebaseCloud(input)).relay).toEqual(plan.relay);
  });

  it.each([{}, { relay: true, functions: true }, { relay: true, portal: true }])(
    "refuses unsupported dry-run target selection %j", async (targets) => {
      const { runner, calls } = mockDeployment();
      await expect(deployFirebaseCloud({
        repoRoot: "/repo", runner, env: {}, environment: "staging", dryRun: true, ...targets
      })).rejects.toThrow("--dry-run requires --relay as its only target");
      expect(calls).toEqual([]);
    }
  );

  it("passes OTA bucket and private key secret wiring to the relay VM", () => {
    const plan = buildRelayDeployPlan({
      repoRoot: "/repo",
      environment: "staging",
      commit: "1f2e3d4c5b6a",
    });

    const remote = plan.commands.at(-1);
    expect(remote?.args.join("\n")).toContain("kanna-mobile-ota-private-key-pem");
    expect(remote?.args.join("\n")).toContain("KANNA_OTA_BUCKET=kanna-staging.firebasestorage.app");
    expect(remote?.args.join("\n")).toContain("KANNA_OTA_KEY_ID=kanna-mobile-ota-v1");
    expect(remote?.args.join("\n")).toContain("KANNA_OTA_PRIVATE_KEY_PATH=/run/secrets/kanna_ota_private_key.pem");
  });
});
