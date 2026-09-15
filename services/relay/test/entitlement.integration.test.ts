/**
 * Relay entitlement enforcement against the real Firebase emulator
 * (`docs/specs/accounts-and-billing.md`, Decisions 5 and 8).
 *
 * Two relays share one emulator: one with
 * `KANNA_RELAY_ENTITLEMENT_ENFORCEMENT=on`, which is what every assertion about
 * refusal is made against, and one with the flag off, which is what proves the
 * shipped default is unchanged. Both are real processes speaking the real
 * protocol — the enforcement points sit in the handshake and the publication
 * paths, so an in-process test of the module alone would not show them wired.
 */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deleteApp, getApps, initializeApp, type App } from "firebase-admin/app";
import { getAuth } from "firebase-admin/auth";
import { getFirestore, type Firestore } from "firebase-admin/firestore";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import WebSocket from "ws";
import { ENTITLEMENT_REQUIRED_CODE } from "../src/entitlement.js";
import { appleFixture, billingFixture, startBillingHttpFixture } from "./support/billingHttpFixture.js";
import { signStripePayload } from "../../firebase-functions/src/billing/stripeSignature.js";
import type { StripeEventEnvelope } from "../../firebase-functions/src/billing/stripeEvents.js";

vi.mock("../../firebase-functions/src/billing/stripeGateway.js", async () => {
  const { billingGateways } = await import("./support/billingHttpFixture.js");
  return billingGateways;
});

import { appleEnv, appleJws, fixtureVerifier, notificationJws, renewalClaims, transactionClaims } from "../../firebase-functions/test/support/appleFixtures.js";
vi.mock("../../firebase-functions/src/billing/appStoreVerification.js", async importOriginal => {
  const original = await importOriginal<typeof import("../../firebase-functions/src/billing/appStoreVerification.js")>();
  return { ...original, createAppleVerifier: () => fixtureVerifier() };
});
vi.mock("../../firebase-functions/src/billing/appStoreGateway.js", async () => {
  const { appleGatewayFixture } = await import("./support/billingHttpFixture.js");
  return appleGatewayFixture;
});

const initialApps = new Set(getApps());
const PASSWORD = "password123";

/** Nonzero cache: live socket updates must not depend on TTL expiry. */
const ENTITLEMENT_CACHE_TTL_MS = 3_000;

interface TestAccount {
  uid: string;
  email: string;
  emailVerified: boolean;
  desktopId: string;
  desktopSecret: string;
  idToken: string;
}

const ACCOUNTS = {
  entitled: { uid: "ent-active", email: "ent-active@example.com", emailVerified: true },
  unentitled: { uid: "ent-none", email: "ent-none@example.com", emailVerified: true },
  unverified: { uid: "ent-unverified", email: "ent-unverified@example.com", emailVerified: false },
  comped: { uid: "ent-comped", email: "ent-comped@example.com", emailVerified: true },
  graceExpired: { uid: "ent-grace-gone", email: "ent-grace-gone@example.com", emailVerified: true },
} as const;

type AccountName = keyof typeof ACCOUNTS;

const accounts = {} as Record<AccountName, TestAccount>;

let firebaseProcess: ChildProcessWithoutNullStreams | null = null;
let firebaseConfigDir: string | null = null;
let enforcingRelay: ChildProcessWithoutNullStreams | null = null;
let permissiveRelay: ChildProcessWithoutNullStreams | null = null;
let adminApp: App | null = null;
let db: Firestore;
let authPort = 0;
let firestorePort = 0;
let enforcingPort = 0;
let permissivePort = 0;

function sha256Hex(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

async function findFreePort(): Promise<number> {
  return await new Promise<number>((resolvePort, reject) => {
    const server = createServer();
    server.unref();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") {
        server.close();
        reject(new Error("failed to resolve free port"));
        return;
      }
      server.close((error) => (error ? reject(error) : resolvePort(address.port)));
    });
  });
}

async function waitForRelay(port: number, timeoutMs = 60_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const response = await fetch(`http://127.0.0.1:${port}/health`).catch(() => null);
    if (response?.ok) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`relay on ${port} did not become ready`);
}

async function signIn(email: string): Promise<string> {
  const url = `http://127.0.0.1:${authPort}/identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key=kanna-local`;
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, password: PASSWORD, returnSecureToken: true }),
    }).catch(() => null);
    const body = (await response?.json().catch(() => null)) as { idToken?: string } | null;
    if (response?.ok && body?.idToken) return body.idToken;
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`auth emulator never signed in ${email}`);
}

function entitlementDoc(overrides: Record<string, unknown>): Record<string, unknown> {
  return {
    status: "active",
    source: "stripe",
    capabilities: ["cloud_relay", "cloud_task_index", "remote_task_control"],
    currentPeriodEndsAt: "2027-01-01T00:00:00.000Z",
    graceEndsAt: null,
    stripeCustomerId: "cus_relay_test",
    stripeSubscriptionId: "sub_relay_test",
    appStoreOriginalTransactionId: null,
    duplicateSources: false,
    environment: "staging",
    updatedAt: "2026-08-21T00:00:00.000Z",
    ...overrides,
  };
}

function entitlementRef(uid: string) {
  return db.doc(`users/${uid}/entitlements/cloud_access`);
}

/**
 * Run the real comp grant script against this emulator.
 *
 * This is the whole Slice-1 claim in one step: the operator script writes
 * `billing/comp`, calls `recomputeEntitlement` itself — nothing else would, no
 * Firestore trigger watches the source docs — and the relay then serves the
 * account. Asserting it here rather than only in the billing package's own
 * tests is what proves the two halves meet.
 */
async function grantCompAccess(email: string): Promise<void> {
  const repoRoot = fileURLToPath(new URL("../../..", import.meta.url));
  const child = spawn(
    "pnpm",
    ["--filter", "@kanna/firebase-functions", "comp:grant", "--", "--reason", "grandfathered", email],
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        GCLOUD_PROJECT: "kanna-local",
        FIRESTORE_EMULATOR_HOST: `127.0.0.1:${firestorePort}`,
        FIREBASE_AUTH_EMULATOR_HOST: `127.0.0.1:${authPort}`,
      },
      stdio: "pipe",
    },
  );
  let output = "";
  child.stdout.on("data", (chunk: Buffer) => (output += chunk.toString()));
  child.stderr.on("data", (chunk: Buffer) => (output += chunk.toString()));
  const code = await new Promise<number>((resolveCode) => {
    child.on("exit", (exitCode) => resolveCode(exitCode ?? 1));
  });
  if (code !== 0) {
    throw new Error(`comp:grant failed (${code}):\n${output}`);
  }
}

interface AuthResult {
  ws: WebSocket;
  userId: string;
  capabilities: Record<string, unknown>;
  entitlement: Record<string, unknown> | undefined;
}

function connectAndAuth(port: number, payload: Record<string, unknown>): Promise<AuthResult> {
  return new Promise((resolveAuth, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}`);
    const timeout = setTimeout(() => {
      ws.close();
      reject(new Error("auth timed out"));
    }, 10_000);
    ws.on("open", () => ws.send(JSON.stringify({ type: "auth", access_updates: true, ...payload })));
    const handler = (raw: Buffer) => {
      const message = JSON.parse(raw.toString()) as Record<string, unknown>;
      if (message.type !== "auth_ok") return;
      clearTimeout(timeout);
      ws.off("message", handler);
      resolveAuth({
        ws,
        userId: message.userId as string,
        capabilities: message.capabilities as Record<string, unknown>,
        entitlement: message.entitlement as Record<string, unknown> | undefined,
      });
    };
    ws.on("message", handler);
    ws.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
  });
}

/**
 * Connect, send one auth frame, and resolve with the close code the relay
 * answers with. A tunnel socket is never told `auth_ok`, so the close code is
 * the whole reply.
 */
function connectAndAwaitClose(
  port: number,
  payload: Record<string, unknown>,
): Promise<number> {
  return new Promise((resolveCode, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}`);
    const timeout = setTimeout(() => {
      ws.close();
      reject(new Error("close timed out"));
    }, 10_000);
    ws.on("open", () => ws.send(JSON.stringify({ type: "auth", access_updates: true, ...payload })));
    ws.on("close", (code: number) => {
      clearTimeout(timeout);
      resolveCode(code);
    });
    ws.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
  });
}

function waitForMessage(
  ws: WebSocket,
  predicate: (message: Record<string, unknown>) => boolean,
  timeoutMs = 10_000,
): Promise<Record<string, unknown>> {
  return new Promise((resolveMessage, reject) => {
    const timeout = setTimeout(() => reject(new Error("waitForMessage timed out")), timeoutMs);
    const handler = (raw: Buffer) => {
      let message: Record<string, unknown>;
      try {
        message = JSON.parse(raw.toString()) as Record<string, unknown>;
      } catch {
        return;
      }
      if (!predicate(message)) return;
      clearTimeout(timeout);
      ws.off("message", handler);
      resolveMessage(message);
    };
    ws.on("message", handler);
  });
}

function closeAndWait(ws: WebSocket): Promise<void> {
  return new Promise((resolveClose) => {
    if (ws.readyState >= WebSocket.CLOSING) {
      resolveClose();
      return;
    }
    ws.on("close", () => resolveClose());
    ws.close();
  });
}

function snapshot(desktopId: string): Record<string, unknown> {
  return {
    schemaVersion: 1,
    desktop: { displayName: "Entitlement Test Mac" },
    tasks: [
      {
        localRepoId: "repo-entitlement",
        ownerDesktopId: desktopId,
        ownerLocalTaskId: "task-entitlement",
        title: "Entitlement publication",
        promptSnippet: "Entitlement publication",
        waitingPromptSnippet: null,
        displayName: null,
        stage: "in progress",
        activity: "idle",
        status: "active",
        repo: {
          cloudRepoId: "repo-entitlement",
          name: "Kanna",
          remoteUrl: "git@github.com:kanna/kanna.git",
          remoteUrlHash: "remote-hash",
          defaultBranch: "main",
        },
        branch: "task-entitlement",
        baseRef: "origin/main",
        prNumber: null,
        prUrl: null,
        agent: { provider: "codex", type: "pty" },
        transfer: {
          state: "none",
          transferId: null,
          sourceDesktopId: null,
          destinationDesktopId: null,
        },
        blockedByTaskIds: [],
        createdAt: "2026-08-21 00:00:00",
        updatedAt: "2026-08-21 00:01:00",
        closedAt: null,
      },
    ],
  };
}

/** Authenticate as a desktop and publish one snapshot, returning the ack. */
async function publishAs(
  port: number,
  account: TestAccount,
  id: string,
): Promise<{ ack: Record<string, unknown>; auth: AuthResult }> {
  const auth = await connectAndAuth(port, {
    desktop_id: account.desktopId,
    desktop_secret: account.desktopSecret,
  });
  const ack = waitForMessage(auth.ws, (message) =>
    message.type === "task_snapshot_ack" && message.id === id);
  auth.ws.send(JSON.stringify({
    type: "task_snapshot_publish",
    id,
    snapshot: snapshot(account.desktopId),
  }));
  return { ack: await ack, auth };
}

async function spawnRelay(port: number, enforcement: "on" | "off"): Promise<ChildProcessWithoutNullStreams> {
  const child = spawn("pnpm", ["exec", "tsx", "src/index.ts"], {
    cwd: fileURLToPath(new URL("..", import.meta.url)),
    env: {
      ...process.env,
      FIREBASE_PROJECT_ID: "kanna-local",
      FIREBASE_AUTH_EMULATOR_HOST: `127.0.0.1:${authPort}`,
      FIRESTORE_EMULATOR_HOST: `127.0.0.1:${firestorePort}`,
      PORT: String(port),
      KANNA_RELAY_ENTITLEMENT_ENFORCEMENT: enforcement,
      KANNA_RELAY_ENTITLEMENT_CACHE_TTL_MS: String(ENTITLEMENT_CACHE_TTL_MS),
    },
    detached: true,
    stdio: "pipe",
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    process.stderr.write(`[relay:${enforcement}] ${chunk.toString()}`);
  });
  child.stdout?.resume();
  return child;
}

async function terminateProcessTree(child: ChildProcessWithoutNullStreams | null): Promise<void> {
  if (!child?.pid || child.exitCode !== null || child.signalCode !== null) return;
  await new Promise<void>((resolveExit) => {
    const timeout = setTimeout(() => {
      try {
        process.kill(-child.pid!, "SIGKILL");
      } catch {
        // Already exited.
      }
      resolveExit();
    }, 2_000);
    child.once("exit", () => {
      clearTimeout(timeout);
      resolveExit();
    });
    try {
      process.kill(-child.pid!, "SIGTERM");
    } catch {
      clearTimeout(timeout);
      resolveExit();
    }
  });
}

describe("Relay entitlement enforcement", () => {
  beforeAll(async () => {
    authPort = Number(process.env.KANNA_FIREBASE_AUTH_PORT) || await findFreePort();
    firestorePort = Number(process.env.KANNA_FIREBASE_FIRESTORE_PORT) || await findFreePort();
    enforcingPort = Number(process.env.KANNA_RELAY_PORT) || await findFreePort();
    permissivePort = await findFreePort();
    const hubPort = await findFreePort();
    const loggingPort = await findFreePort();

    const taskTmp = fileURLToPath(new URL("../../../.tmp/", import.meta.url));
    await mkdir(taskTmp, { recursive: true });
    firebaseConfigDir = await mkdtemp(join(taskTmp, "kanna-entitlement-firebase-"));
    const configPath = join(firebaseConfigDir, "firebase.json");
    await writeFile(
      configPath,
      JSON.stringify({
        firestore: {
          rules: resolve(fileURLToPath(new URL("../../../firestore.rules", import.meta.url))),
        },
        emulators: {
          auth: { host: "127.0.0.1", port: authPort },
          firestore: { host: "127.0.0.1", port: firestorePort },
          hub: { host: "127.0.0.1", port: hubPort },
          logging: { host: "127.0.0.1", port: loggingPort },
          ui: { enabled: false },
        },
      }),
    );
    firebaseProcess = spawn(
      "pnpm",
      ["exec", "firebase", "emulators:start", "--project", "kanna-local", "--config", configPath],
      {
        cwd: fileURLToPath(new URL("../../..", import.meta.url)),
        env: { ...process.env },
        detached: true,
        stdio: "pipe",
      },
    );
    firebaseProcess.stderr?.on("data", (chunk: Buffer) => {
      process.stderr.write(`[firebase] ${chunk.toString()}`);
    });
    firebaseProcess.stdout?.resume();

    process.env.FIRESTORE_EMULATOR_HOST = `127.0.0.1:${firestorePort}`;
    process.env.FIREBASE_AUTH_EMULATOR_HOST = `127.0.0.1:${authPort}`;
    adminApp = initializeApp({ projectId: "kanna-local" }, `entitlement-suite-${firestorePort}`);
    db = getFirestore(adminApp);
    const auth = getAuth(adminApp);

    // Wait for the auth emulator, then build every account this suite needs.
    const deadline = Date.now() + 60_000;
    for (;;) {
      try {
        await auth.listUsers(1);
        break;
      } catch (error) {
        if (Date.now() > deadline) throw error;
        await new Promise((r) => setTimeout(r, 250));
      }
    }

    for (const [name, spec] of Object.entries(ACCOUNTS) as [AccountName, typeof ACCOUNTS[AccountName]][]) {
      await auth.createUser({
        uid: spec.uid,
        email: spec.email,
        emailVerified: spec.emailVerified,
        password: PASSWORD,
      });
      const desktopId = `desktop-${spec.uid}`;
      const desktopSecret = `secret-${spec.uid}`;
      await db.doc(`desktopCredentials/${desktopId}`).set({
        desktopId,
        desktopSecretHash: sha256Hex(desktopSecret),
        displayName: "Entitlement Test Mac",
        revokedAt: null,
        uid: spec.uid,
        updatedAt: new Date(0).toISOString(),
      });
      accounts[name] = {
        uid: spec.uid,
        email: spec.email,
        emailVerified: spec.emailVerified,
        desktopId,
        desktopSecret,
        idToken: await signIn(spec.email),
      };
    }

    // The entitlement records, written as the reducer writes them.
    await entitlementRef(accounts.entitled.uid).set(entitlementDoc({}));
    // An unverified account whose subscription is otherwise in good standing:
    // Decision 1 says the relay refuses it anyway.
    await entitlementRef(accounts.unverified.uid).set(entitlementDoc({}));
    // Dunning that ran out: `grace` is still the stored status because nothing
    // sweeps it, so the enforcement-side read is what has to expire it.
    await entitlementRef(accounts.graceExpired.uid).set(entitlementDoc({
      status: "grace",
      graceEndsAt: "2026-08-01T00:00:00.000Z",
    }));
    // …and `accounts.unentitled` deliberately gets no document at all.
    await grantCompAccess(accounts.comped.email);

    enforcingRelay = await spawnRelay(enforcingPort, "on");
    permissiveRelay = await spawnRelay(permissivePort, "off");
    await waitForRelay(enforcingPort);
    await waitForRelay(permissivePort);
  }, 180_000);

  afterAll(async () => {
    await terminateProcessTree(enforcingRelay);
    await terminateProcessTree(permissiveRelay);
    await terminateProcessTree(firebaseProcess);
    await Promise.all(getApps().filter(app => !initialApps.has(app)).map(deleteApp));
    delete process.env.FIRESTORE_EMULATOR_HOST;
    delete process.env.FIREBASE_AUTH_EMULATOR_HOST;
    if (firebaseConfigDir) await rm(firebaseConfigDir, { recursive: true, force: true });
  });

  it("joins fresh verified Auth, actual HTTP callables and signed billing events to live relay access", async () => {
    // This is NOT the Firebase CLI Functions emulator or hosted Stripe. The
    // exported onCall/onRequest handlers run on local HTTP with only Stripe
    // gateway factories injected; their auth/protocol/core logic is unchanged.
    vi.stubEnv("GCLOUD_PROJECT", "kanna-local");
    vi.stubEnv("FIREBASE_CONFIG", JSON.stringify({ projectId: "kanna-local" }));
    vi.stubEnv("STRIPE_SECRET_KEY", "sk_test_injected_fixture_only");
    vi.stubEnv("STRIPE_WEBHOOK_SECRET", "whsec_launch_fixture_only");
    vi.stubEnv("STRIPE_PORTAL_CONFIGURATION_ID", "bpc_fixture_only");
    vi.stubEnv("KANNA_PORTAL_BASE_URL", "https://portal.example.test");
    const billing = await startBillingHttpFixture(Number(process.env.KANNA_FIREBASE_FUNCTIONS_PORT) || await findFreePort());
    const sockets: WebSocket[] = [];
    const email = `launch-${Date.now()}@example.test`;
    const authRequest = async (method: string, body: Record<string, unknown>) => {
      const response = await fetch(`http://127.0.0.1:${authPort}/identitytoolkit.googleapis.com/v1/accounts:${method}?key=kanna-local`, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
      });
      expect(response.status).toBe(200);
      return await response.json() as { localId: string; idToken: string };
    };
    const call = async (name: string, token?: string, data: Record<string, unknown> = {}) => {
      const response = await fetch(`${billing.url}/${name}`, {
        method: "POST", headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
        body: JSON.stringify({ data }),
      });
      return { status: response.status, body: await response.json() as Record<string, unknown> };
    };
    try {
      const registered = await authRequest("signUp", { email, password: PASSWORD, returnSecureToken: true });
      const uid = registered.localId;
      expect((await call("createCheckoutSession", undefined, { plan: "monthly" })).body)
        .toMatchObject({ error: { status: "UNAUTHENTICATED" } });
      expect((await call("createCheckoutSession", registered.idToken, { plan: "monthly" })).body)
        .toMatchObject({ error: { details: { reason: "email_verification_required" } } });
      expect(billingFixture.checkoutCalls).toBe(0);
      await authRequest("sendOobCode", { requestType: "VERIFY_EMAIL", idToken: registered.idToken });
      const codesResponse = await fetch(`http://127.0.0.1:${authPort}/emulator/v1/projects/kanna-local/oobCodes`);
      const codes = await codesResponse.json() as { oobCodes: { email: string; requestType: string; oobCode: string }[] };
      const verification = codes.oobCodes.find((code) => code.email === email && code.requestType === "VERIFY_EMAIL");
      if (!verification) throw new Error("No verification email fixture");
      await authRequest("update", { oobCode: verification.oobCode });
      const token = await signIn(email);
      const checkout = await call("createCheckoutSession", token, { plan: "monthly" });
      expect(checkout).toMatchObject({ status: 200, body: { result: { sessionId: "cs_launch_fixture", plan: "monthly" } } });
      expect(billingFixture.checkoutInput).toMatchObject({ uid, customerId: "cus_launch_fixture", priceId: "price_launch_fixture" });
      expect(await call("createCheckoutSession", token, { plan: "monthly" })).toEqual(checkout);
      expect(billingFixture.checkoutCalls).toBe(1);
      expect((await entitlementRef(uid).get()).exists).toBe(false);

      const desktopId = `desktop-${uid}`;
      const desktopSecret = `fixture-${uid}`;
      // Desktop registration is an injected credential fixture, not native sign-in.
      await db.doc(`desktopCredentials/${desktopId}`).set({ uid, desktopId, desktopSecretHash: sha256Hex(desktopSecret), revokedAt: null });
      const desktop = await connectAndAuth(enforcingPort, { desktop_id: desktopId, desktop_secret: desktopSecret });
      sockets.push(desktop.ws);
      const phone = await connectAndAuth(enforcingPort, { id_token: token });
      sockets.push(phone.ws);
      const access = (active: boolean, status: string) => [desktop.ws, phone.ws].map((ws) => waitForMessage(ws, (message) => {
        const entitlement = message.entitlement as { active?: boolean; status?: string } | undefined;
        return message.type === "auth_ok" && entitlement?.active === active && entitlement.status === status;
      }));
      const checkAccess = async (id: string, active: boolean) => {
        const publication = waitForMessage(desktop.ws, (m) => m.type === "task_snapshot_ack" && m.id === id);
        desktop.ws.send(JSON.stringify({ type: "task_snapshot_publish", id, snapshot: snapshot(desktopId) }));
        expect(await publication).toMatchObject(active ? { ok: true } : { ok: false, code: 4402 });
        const invoke = waitForMessage(phone.ws, (m) => m.type === "response" && m.id === id);
        phone.ws.send(JSON.stringify({ type: "invoke", id, command: "list_active_desktops", args: {} }));
        const result = await invoke;
        if (active) expect(result).toMatchObject({ data: { desktopIds: [desktopId] } });
        else expect(result).toMatchObject({ code: 4402 });
      };
      let eventNumber = 0;
      const deliver = async (file: string, patch: Record<string, unknown> = {}, signatureSecret = "whsec_launch_fixture_only") => {
        const source = await readFile(new URL(`../../firebase-functions/test/fixtures/stripe/${file}`, import.meta.url), "utf8");
        const event = JSON.parse(source.replaceAll("fixture-checkout-user", uid).replaceAll("cus_TestSlice1", "cus_launch_fixture")) as StripeEventEnvelope;
        event.id = `evt_launch_${uid}_${++eventNumber}`;
        event.created = Math.floor(Date.now() / 1000) + eventNumber;
        Object.assign(event.data.object, patch);
        const body = JSON.stringify(event);
        const response = await fetch(`${billing.url}/stripeWebhook`, {
          method: "POST", headers: { "content-type": "application/json", "stripe-signature": signStripePayload(body, signatureSecret) }, body,
        });
        return { status: response.status, body: await response.json() };
      };
      const transition = async (file: string, active: boolean, status: string, patch: Record<string, unknown> = {}) => {
        const observed = access(active, status);
        expect(await deliver(file, patch)).toMatchObject({ status: 200, body: { code: "applied" } });
        await Promise.all(observed);
        expect((await entitlementRef(uid).get()).data()).toMatchObject({ source: "stripe", status });
      };
      await checkAccess("pending", false);
      expect(await deliver("checkout.session.completed.json", {}, "wrong_fixture_secret"))
        .toMatchObject({ status: 400, body: { code: "invalid_signature" } });
      await checkAccess("bad-signature", false);
      await transition("checkout.session.completed.json", true, "active");
      await checkAccess("purchased", true);
      await transition("invoice.paid.json", true, "active");
      await checkAccess("renewed", true);
      await transition("invoice.payment_failed.json", true, "grace", { next_payment_attempt: Math.floor(Date.now() / 1000) + 3 });
      const graceExpired = access(false, "grace");
      await checkAccess("grace", true);
      await Promise.all(graceExpired);
      await checkAccess("grace-expired", false);
      await transition("invoice.paid.json", true, "active");
      await checkAccess("recovered", true);
      expect(await call("createPortalSession", token)).toMatchObject({ status: 200, body: { result: { url: "https://billing.stripe.test/launch-fixture" } } });
      expect(billingFixture.portalCustomer).toBe("cus_launch_fixture");
      await transition("customer.subscription.updated.cancel_at_period_end.json", true, "active");
      expect((await db.doc(`users/${uid}/billing/stripe`).get()).data()).toMatchObject({ cancelAtPeriodEnd: true });
      await checkAccess("cancel-pending", true);
      await transition("customer.subscription.deleted.json", false, "expired");
      await checkAccess("canceled", false);
      expect((await db.doc(`users/${uid}`).get()).exists).toBe(true);

      const deletedAccess = access(false, "none");
      expect(await call("deleteAccount", token)).toMatchObject({ status: 200, body: { result: { deleted: true } } });
      await Promise.all(deletedAccess);
      // Control sessions can remain open; deletion fences value-bearing work
      // and removes credentials, rather than promising immediate socket closure.
      const denied = waitForMessage(phone.ws, (m) => m.type === "response" && m.id === "deleted-invoke");
      phone.ws.send(JSON.stringify({ type: "invoke", id: "deleted-invoke", command: "list_active_desktops", args: {} }));
      expect(await denied).toMatchObject({ code: 4402 });
      expect(billingFixture.canceledSubscriptions).toContain("sub_TestSlice1");
      expect(billingFixture.closedCustomers).toContain("cus_launch_fixture");
      expect((await db.doc(`accountDeletions/${uid}`).get()).exists).toBe(true);
      expect((await db.doc(`users/${uid}`).get()).exists).toBe(false);
      expect((await db.doc(`desktopCredentials/${desktopId}`).get()).exists).toBe(false);
      expect(await deliver("invoice.paid.json", { metadata: { firebase_uid: uid } }))
        .toMatchObject({ status: 200, body: { code: "deleted_account" } });
      expect((await entitlementRef(uid).get()).exists).toBe(false);
      expect(await deliver("invoice.paid.json", { metadata: {}, customer: "cus_unmapped_fixture" }))
        .toMatchObject({ status: 200, body: { code: "unresolved_account" } });
      if (!adminApp) throw new Error("Missing emulator app");
      await expect(getAuth(adminApp).getUser(uid)).rejects.toMatchObject({ code: "auth/user-not-found" });
    } finally {
      await Promise.all(sockets.map(closeAndWait));
      await billing.close();
      vi.unstubAllEnvs();
    }
  }, 60_000);

  it("joins native registration and signed Apple notifications to already-connected paid relay controls", async () => {
    vi.stubEnv("GCLOUD_PROJECT", "kanna-local");
    vi.stubEnv("FIREBASE_CONFIG", JSON.stringify({ projectId: "kanna-local" }));
    vi.stubEnv("STRIPE_SECRET_KEY", "sk_fixture_only");
    for (const [key, value] of Object.entries(appleEnv)) vi.stubEnv(key, value);
    const billing = await startBillingHttpFixture(Number(process.env.KANNA_FIREBASE_FUNCTIONS_PORT) || await findFreePort());
    const sockets: WebSocket[] = [];
    const uid = `apple-${Date.now()}`, email = `${uid}@example.test`, desktopId = `desktop-${uid}`, desktopSecret = `test-${uid}`;
    const call = async (name: string, token: string, data: Record<string, unknown> = {}) => {
      const response = await fetch(`${billing.url}/${name}`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify({ data }) });
      return { status: response.status, body: await response.json() as { result?: { appAccountToken: string; outcome: string } } };
    };
    try {
      await getAuth(adminApp!).createUser({ uid, email, password: PASSWORD, emailVerified: true });
      const token = await signIn(email);
      const admission = await call("beginAppStorePurchase", token);
      expect(admission.status).toBe(200);
      const appAccountToken = admission.body.result!.appAccountToken;
      await db.doc(`desktopCredentials/${desktopId}`).set({ uid, desktopId, desktopSecretHash: sha256Hex(desktopSecret), revokedAt: null });
      const desktop = await connectAndAuth(enforcingPort, { desktop_id: desktopId, desktop_secret: desktopSecret }); sockets.push(desktop.ws);
      const phone = await connectAndAuth(enforcingPort, { id_token: token }); sockets.push(phone.ws);
      const access = (active: boolean, status: string) => [desktop.ws, phone.ws].map(ws => waitForMessage(ws, message => {
        const entitlement = message.entitlement as { active?: boolean; status?: string } | undefined;
        return message.type === "auth_ok" && entitlement?.active === active && entitlement.status === status;
      }));
      const check = async (id: string, active: boolean) => {
        const ack = waitForMessage(desktop.ws, m => m.type === "task_snapshot_ack" && m.id === id);
        desktop.ws.send(JSON.stringify({ type: "task_snapshot_publish", id, snapshot: snapshot(desktopId) }));
        expect(await ack).toMatchObject(active ? { ok: true } : { ok: false, code: 4402 });
        const response = waitForMessage(phone.ws, m => m.type === "response" && m.id === id);
        phone.ws.send(JSON.stringify({ type: "invoke", id, command: "list_active_desktops", args: {} }));
        expect(await response).toMatchObject(active ? { data: { desktopIds: [desktopId] } } : { code: 4402 });
      };
      let signedDate = Date.now();
      const purchaseDate = signedDate - 60_000;
      let tx = transactionClaims(appAccountToken, { purchaseDate, signedDate });
      let renewal = renewalClaims({ signedDate });
      const verifier = fixtureVerifier();
      appleFixture.current = [{ ...await verifier.pair(appleJws(tx), appleJws(renewal), "sandbox"), status: 1, signedDate }];
      await check("apple-before", false);
      const purchased = access(true, "active");
      expect(await call("registerAppStoreTransaction", token, { signedTransaction: appleJws(tx) })).toMatchObject({ status: 200, body: { result: { outcome: "accepted" } } });
      await Promise.all(purchased); await check("apple-purchased", true);
      const send = async (signedPayload: string) => {
        const response = await fetch(`${billing.url}/appStoreNotifications`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ signedPayload }) });
        expect(response.status).toBe(200); return await response.json();
      };
      let lastPayload = "";
      const transition = async (notificationType: string, status: number, active: boolean, normalized: string,
        txPatch: Record<string, unknown> = {}, renewalPatch: Record<string, unknown> = {}) => {
        signedDate += 1000;
        tx = transactionClaims(appAccountToken, { purchaseDate, signedDate, ...txPatch });
        renewal = renewalClaims({ signedDate, ...renewalPatch });
        lastPayload = notificationJws(tx, renewal, { notificationType, notificationUUID: randomUUID(), signedDate,
          data: { bundleId: "build.kanna.app", appAppleId: 123456789, environment: "Sandbox", status,
            signedTransactionInfo: appleJws(tx), signedRenewalInfo: appleJws(renewal) } });
        const changed = access(active, normalized);
        await send(lastPayload); await Promise.all(changed);
        expect((await db.doc(`users/${uid}/billing/app_store`).get()).data()).toMatchObject({ source: "app_store", environment: "sandbox", status: normalized });
      };
      await transition("DID_RENEW", 1, true, "active"); await check("apple-renewed", true);
      await transition("DID_CHANGE_RENEWAL_STATUS", 1, true, "active", {}, { autoRenewStatus: 0 });
      await check("apple-renewal-off", true);
      await transition("DID_FAIL_TO_RENEW", 4, true, "grace", {}, { isInBillingRetryPeriod: true, gracePeriodExpiresDate: Date.now() + 3000 });
      const expired = access(false, "grace"); await check("apple-grace", true); await Promise.all(expired); await check("apple-grace-ended", false);
      await transition("DID_RENEW", 1, true, "active");
      await transition("REFUND", 5, false, "revoked", { revocationDate: Date.now() }); await check("apple-revoked", false);
      await transition("REFUND_REVERSED", 1, true, "active"); await check("apple-recovered", true);
      expect(await send(lastPayload)).toMatchObject({ code: "duplicate" });
      const deleted = access(false, "none");
      expect((await call("deleteAccount", token)).status).toBe(200); await Promise.all(deleted);
      expect(await send(lastPayload)).toMatchObject({ code: "unresolved_account" });
      expect((await entitlementRef(uid).get()).exists).toBe(false);
      expect((await db.doc(`appAccountTokens/${appAccountToken}`).get()).exists).toBe(false);
    } finally {
      await Promise.all(sockets.map(closeAndWait)); await billing.close();
      vi.unstubAllEnvs();
    }
  });

  it("advertises the full capability set to an entitled desktop and publishes", async () => {
    const { ack, auth } = await publishAs(enforcingPort, accounts.entitled, "entitled-publish");

    expect(auth.capabilities.tunnelServices).toEqual(["ksp", "task-transfer"]);
    expect(auth.capabilities.taskSnapshotPublication).toBeDefined();
    expect(auth.capabilities.mobileNotifications).toBeDefined();
    expect(auth.entitlement).toEqual({
      active: true,
      status: "active",
      currentPeriodEndsAt: "2027-01-01T00:00:00.000Z",
      graceEndsAt: null,
    });
    expect(ack).toMatchObject({ ok: true });
    await closeAndWait(auth.ws);
  });

  it("keeps notifications free for an unentitled desktop while paid publication stays gated", async () => {
    const { ack, auth } = await publishAs(enforcingPort, accounts.unentitled, "unentitled-publish");

    // Decision 5: the session still completes auth. Closing it would be exactly
    // the generic connection error the neutral inactive state exists to avoid.
    expect(auth.userId).toBe(accounts.unentitled.uid);
    expect(auth.capabilities.tunnelServices).toEqual([]);
    expect(auth.capabilities.taskSnapshotPublication).toBeUndefined();
    expect(auth.capabilities.mobileNotifications).toEqual({ version: 2 });
    // Desktop-to-desktop routing crosses the relay, so the 2026-08-21 owner
    // ruling makes it paid too — it is no longer advertised either.
    expect(auth.capabilities.desktopRouting).toBeUndefined();
    expect(auth.entitlement).toEqual({
      active: false,
      status: "none",
      currentPeriodEndsAt: null,
      graceEndsAt: null,
    });
    expect(ack).toMatchObject({ ok: false, code: ENTITLEMENT_REQUIRED_CODE });

    const pushAck = waitForMessage(auth.ws, (message) =>
      message.type === "mobile_notification_ack" && message.id === "unentitled-push");
    auth.ws.send(JSON.stringify({
      type: "mobile_notification_publish",
      id: "unentitled-push",
      notification: {
        kind: "task_awaiting_input",
        taskId: "task-entitlement",
        title: "Waiting",
        body: "Ready for review",
      },
    }));
    await expect(pushAck).resolves.toMatchObject({
      ok: true,
      delivery: {
        acceptedCount: 0,
        failedCount: 0,
      },
    });
    await closeAndWait(auth.ws);
  });

  it("refuses an unentitled tunnel request with the entitlement code", async () => {
    const auth = await connectAndAuth(enforcingPort, { id_token: accounts.unentitled.idToken });
    expect(auth.capabilities.tunnelServices).toEqual([]);

    const response = waitForMessage(auth.ws, (message) =>
      message.type === "response" && message.id === "unentitled-tunnel");
    auth.ws.send(JSON.stringify({
      type: "tunnel_request",
      id: "unentitled-tunnel",
      desktopId: accounts.unentitled.desktopId,
      service: "task-transfer",
    }));
    await expect(response).resolves.toMatchObject({
      error: "entitlement required",
      code: ENTITLEMENT_REQUIRED_CODE,
    });
    await closeAndWait(auth.ws);
  });

  it("refuses an unentitled phone invoke with the entitlement code", async () => {
    // Remote task control is the path the owner's 2026-08-21 ruling added to
    // the enforced set: everything through the relay is paid, LAN is free.
    const auth = await connectAndAuth(enforcingPort, { id_token: accounts.unentitled.idToken });

    const response = waitForMessage(auth.ws, (message) =>
      message.type === "response" && message.id === "unentitled-invoke");
    auth.ws.send(JSON.stringify({
      type: "invoke",
      id: "unentitled-invoke",
      desktopId: accounts.unentitled.desktopId,
      command: "list_sessions",
      args: {},
    }));
    await expect(response).resolves.toMatchObject({
      error: "entitlement required",
      code: ENTITLEMENT_REQUIRED_CODE,
    });
    await closeAndWait(auth.ws);
  });

  it("lets an entitled phone invoke through to the router", async () => {
    const auth = await connectAndAuth(enforcingPort, { id_token: accounts.entitled.idToken });

    const response = waitForMessage(auth.ws, (message) =>
      message.type === "response" && message.id === "entitled-invoke");
    auth.ws.send(JSON.stringify({
      type: "invoke",
      id: "entitled-invoke",
      desktopId: accounts.entitled.desktopId,
      command: "list_sessions",
      args: {},
    }));
    // No desktop is connected, so the router's own answer is the proof that the
    // invoke passed the entitlement gate rather than being refused by it.
    const message = await response;
    expect(message.error).toBe("Desktop offline");
    expect(message.code).toBeUndefined();
    await closeAndWait(auth.ws);
  });

  it("refuses an unentitled desktop-to-desktop invoke with the entitlement code", async () => {
    const auth = await connectAndAuth(enforcingPort, {
      desktop_id: accounts.unentitled.desktopId,
      desktop_secret: accounts.unentitled.desktopSecret,
    });
    expect(auth.capabilities.desktopRouting).toBeUndefined();

    const response = waitForMessage(auth.ws, (message) =>
      message.type === "response" && message.id === "unentitled-sibling-invoke");
    auth.ws.send(JSON.stringify({
      type: "invoke",
      id: "unentitled-sibling-invoke",
      desktopId: "some-other-desktop",
      command: "list_sessions",
      args: {},
    }));
    await expect(response).resolves.toMatchObject({
      error: "entitlement required",
      code: ENTITLEMENT_REQUIRED_CODE,
    });
    // Decision 5: an entitlement never closes an ordinary session, so the
    // desktop is still connected and can read its own state.
    expect(auth.ws.readyState).toBe(WebSocket.OPEN);
    await closeAndWait(auth.ws);
  });

  it("lets an entitled desktop-to-desktop invoke through to the router", async () => {
    const auth = await connectAndAuth(enforcingPort, {
      desktop_id: accounts.entitled.desktopId,
      desktop_secret: accounts.entitled.desktopSecret,
    });
    expect(auth.capabilities.desktopRouting).toBeDefined();

    const response = waitForMessage(auth.ws, (message) =>
      message.type === "response" && message.id === "entitled-sibling-invoke");
    auth.ws.send(JSON.stringify({
      type: "invoke",
      id: "entitled-sibling-invoke",
      desktopId: "some-other-desktop",
      command: "list_sessions",
      args: {},
    }));
    const message = await response;
    expect(message.error).toBe("Desktop offline");
    expect(message.code).toBeUndefined();
    await closeAndWait(auth.ws);
  });

  it("routes the same unentitled invokes untouched with the flag off", async () => {
    // The frames the enforcing relay refuses, byte for byte, against the
    // permissive one: the router's own answer, no `code`, nothing read from
    // Firestore on the way.
    const phone = await connectAndAuth(permissivePort, { id_token: accounts.unentitled.idToken });
    const phoneResponse = waitForMessage(phone.ws, (message) =>
      message.type === "response" && message.id === "flag-off-invoke");
    phone.ws.send(JSON.stringify({
      type: "invoke",
      id: "flag-off-invoke",
      desktopId: accounts.unentitled.desktopId,
      command: "list_sessions",
      args: {},
    }));
    const phoneMessage = await phoneResponse;
    expect(phoneMessage.error).toBe("Desktop offline");
    expect(phoneMessage.code).toBeUndefined();
    await closeAndWait(phone.ws);

    const desktop = await connectAndAuth(permissivePort, {
      desktop_id: accounts.unentitled.desktopId,
      desktop_secret: accounts.unentitled.desktopSecret,
    });
    expect(desktop.capabilities.desktopRouting).toBeDefined();
    const desktopResponse = waitForMessage(desktop.ws, (message) =>
      message.type === "response" && message.id === "flag-off-sibling-invoke");
    desktop.ws.send(JSON.stringify({
      type: "invoke",
      id: "flag-off-sibling-invoke",
      desktopId: "some-other-desktop",
      command: "list_sessions",
      args: {},
    }));
    const desktopMessage = await desktopResponse;
    expect(desktopMessage.error).toBe("Desktop offline");
    expect(desktopMessage.code).toBeUndefined();
    await closeAndWait(desktop.ws);
  });

  it("lets an entitled tunnel request through to the router", async () => {
    const auth = await connectAndAuth(enforcingPort, { id_token: accounts.entitled.idToken });

    const response = waitForMessage(auth.ws, (message) =>
      message.type === "response" && message.id === "entitled-tunnel");
    auth.ws.send(JSON.stringify({
      type: "tunnel_request",
      id: "entitled-tunnel",
      desktopId: accounts.entitled.desktopId,
      service: "task-transfer",
    }));
    // No desktop is connected, so the router's own answer is the proof that the
    // request passed the entitlement gate rather than being refused by it.
    const message = await response;
    expect(message.error).toBe("Desktop offline");
    expect(message.code).toBeUndefined();
    await closeAndWait(auth.ws);
  });

  it("refuses an unentitled tunnel socket at the handshake", async () => {
    // Reachable without a phone: a desktop can open a tunnel socket directly.
    // The pending-tunnel lookup would refuse this one anyway, so the assertion
    // that matters is *which* refusal answers — 4402 means the entitlement gate
    // turned it away before the router ever saw it.
    const code = await connectAndAwaitClose(enforcingPort, {
      desktop_id: accounts.unentitled.desktopId,
      desktop_secret: accounts.unentitled.desktopSecret,
      tunnel_id: "tunnel-unentitled",
    });

    expect(code).toBe(ENTITLEMENT_REQUIRED_CODE);
  });

  it("lets an unentitled tunnel socket past the handshake with the flag off", async () => {
    // The same connection, byte for byte, against the permissive relay: it
    // reaches the router, which answers 4404 because no phone requested this
    // tunnel. Nothing entitlement-shaped happens on the way.
    const code = await connectAndAwaitClose(permissivePort, {
      desktop_id: accounts.unentitled.desktopId,
      desktop_secret: accounts.unentitled.desktopSecret,
      tunnel_id: "tunnel-unentitled",
    });

    expect(code).toBe(4404);
    expect(code).not.toBe(ENTITLEMENT_REQUIRED_CODE);
  });

  it("refuses an unverified phone token holding an active subscription", async () => {
    const auth = await connectAndAuth(enforcingPort, { id_token: accounts.unverified.idToken });

    expect(auth.capabilities.tunnelServices).toEqual([]);
    expect(auth.entitlement).toMatchObject({
      active: false,
      status: "unknown",
      reason: "unverified_email",
    });
    await closeAndWait(auth.ws);
  });

  it("serves an account the comp seeding script granted", async () => {
    // The entitlement here was derived by the reducer from `billing/comp`, and
    // enforcement never branches on source: it reads status and capabilities.
    const comp = (await db.doc(`users/${accounts.comped.uid}/billing/comp`).get()).data();
    expect(comp).toMatchObject({ source: "comp", active: true, reason: "grandfathered" });

    const { ack, auth } = await publishAs(enforcingPort, accounts.comped, "comped-publish");
    expect(auth.capabilities.tunnelServices).toEqual(["ksp", "task-transfer"]);
    expect(auth.entitlement).toMatchObject({
      active: true,
      status: "active",
      // Comp never expires; only an explicit revocation ends it.
      currentPeriodEndsAt: null,
    });
    expect(ack).toMatchObject({ ok: true });
    await closeAndWait(auth.ws);
  });

  it("refuses a grace record whose grace period has already ended", async () => {
    const { ack, auth } = await publishAs(enforcingPort, accounts.graceExpired, "grace-publish");

    expect(auth.capabilities.tunnelServices).toEqual([]);
    expect(auth.entitlement).toMatchObject({ active: false, status: "grace" });
    expect(ack).toMatchObject({ ok: false, code: ENTITLEMENT_REQUIRED_CODE });
    await closeAndWait(auth.ws);
  });

  it("activates, revokes, expires grace and recovers publication and invokes on the same connections", async () => {
    const account = accounts.unentitled;
    const desktop = await connectAndAuth(enforcingPort, {
      desktop_id: account.desktopId, desktop_secret: account.desktopSecret,
    });
    const phone = await connectAndAuth(enforcingPort, { id_token: account.idToken });
    const publish = async (id: string) => {
      const ack = waitForMessage(desktop.ws, (m) => m.type === "task_snapshot_ack" && m.id === id);
      desktop.ws.send(JSON.stringify({ type: "task_snapshot_publish", id, snapshot: snapshot(account.desktopId) }));
      return ack;
    };
    const invoke = async (id: string) => {
      const response = waitForMessage(phone.ws, (m) => m.type === "response" && m.id === id);
      phone.ws.send(JSON.stringify({ type: "invoke", id, command: "list_active_desktops", args: {} }));
      return response;
    };
    const accessUpdate = (ws: WebSocket, active: boolean, status: string) => waitForMessage(ws, (m) => {
      const entitlement = m.entitlement as Record<string, unknown> | undefined;
      return m.type === "auth_ok" && entitlement?.active === active && entitlement.status === status;
    });
    const update = async (record: Record<string, unknown>, active: boolean, status: string) => {
      const observed = [accessUpdate(desktop.ws, active, status), accessUpdate(phone.ws, active, status)];
      await entitlementRef(account.uid).set(entitlementDoc(record));
      return Promise.all(observed);
    };
    try {
      expect(await publish("before-purchase")).toMatchObject({ ok: false, code: 4402 });
      expect(await invoke("before-purchase-invoke")).toMatchObject({ code: 4402 });
      const activated = await update({}, true, "active");
      expect(activated[0].capabilities).toMatchObject({ taskSnapshotPublication: { version: 2 }, desktopRouting: { version: 2 } });
      expect(await publish("purchased")).toMatchObject({ ok: true });
      expect(await invoke("purchased-invoke")).toMatchObject({ data: { desktopIds: [account.desktopId] } });
      await update({ status: "revoked", capabilities: [] }, false, "revoked");
      expect(await publish("revoked")).toMatchObject({ ok: false, code: 4402 });
      expect(await invoke("revoked-invoke")).toMatchObject({ code: 4402 });
      await update({ status: "grace", graceEndsAt: new Date(Date.now() + 1500).toISOString() }, true, "grace");
      const expired = [accessUpdate(desktop.ws, false, "grace"), accessUpdate(phone.ws, false, "grace")];
      expect(await publish("in-grace")).toMatchObject({ ok: true });
      await Promise.all(expired);
      expect(await publish("grace-ended")).toMatchObject({ ok: false, code: 4402 });
      await update({ source: "comp" }, true, "active");
      expect(await publish("recovered")).toMatchObject({ ok: true });
      expect(await invoke("recovered-invoke")).toMatchObject({ data: { desktopIds: [account.desktopId] } });
      expect(desktop.ws.readyState).toBe(WebSocket.OPEN);
      expect(phone.ws.readyState).toBe(WebSocket.OPEN);
    } finally {
      await closeAndWait(phone.ws);
      await closeAndWait(desktop.ws);
      await entitlementRef(account.uid).delete();
    }
  });

  it("does not send unsolicited capability frames to older control peers", async () => {
    const account = accounts.unentitled;
    const old = await connectAndAuth(enforcingPort, { id_token: account.idToken, access_updates: false });
    const current = await connectAndAuth(enforcingPort, { desktop_id: account.desktopId, desktop_secret: account.desktopSecret });
    const frames: string[] = [];
    old.ws.on("message", (frame) => frames.push(frame.toString()));
    try {
      const changed = waitForMessage(current.ws, (m) => m.type === "auth_ok" && (m.entitlement as { active?: boolean })?.active === true);
      await entitlementRef(account.uid).set(entitlementDoc({}));
      await changed;
      expect(frames).toEqual([]);
    } finally {
      await closeAndWait(old.ws);
      await closeAndWait(current.ws);
      await entitlementRef(account.uid).delete();
    }
  });

  it("revokes an established tunnel without closing the desktop control session", async () => {
    const account = accounts.entitled;
    const desktop = await connectAndAuth(enforcingPort, {
      desktop_id: account.desktopId, desktop_secret: account.desktopSecret,
    });
    const phone = await connectAndAuth(enforcingPort, { id_token: account.idToken });
    let tunnel: WebSocket | null = null;
    try {
      const establish = waitForMessage(desktop.ws, (m) => m.type === "tunnel_establish");
      const ready = waitForMessage(phone.ws, (m) => m.type === "tunnel_ready");
      phone.ws.send(JSON.stringify({ type: "tunnel_request", id: "open", desktopId: account.desktopId }));
      const request = await establish;
      tunnel = new WebSocket(`ws://127.0.0.1:${enforcingPort}`);
      const desktopTunnel = tunnel;
      desktopTunnel.on("open", () => desktopTunnel.send(JSON.stringify({
        type: "auth", desktop_id: account.desktopId, desktop_secret: account.desktopSecret, tunnel_id: request.tunnelId,
      })));
      const desktopReady = waitForMessage(desktopTunnel, (m) => m.type === "tunnel_ready");
      await Promise.all([ready, desktopReady]);
      const forwarded = new Promise<string>((resolveFrame) => desktopTunnel.once("message", (data) => resolveFrame(data.toString())));
      phone.ws.send("opaque payload");
      expect(await forwarded).toBe("opaque payload");
      const closed = new Promise<number>((resolveClose) => phone.ws.once("close", resolveClose));
      await entitlementRef(account.uid).set(entitlementDoc({ status: "revoked", capabilities: [] }));
      expect(await closed).toBe(4402);
      expect(desktop.ws.readyState).toBe(WebSocket.OPEN);
    } finally {
      if (tunnel) await closeAndWait(tunnel);
      await closeAndWait(phone.ws);
      await closeAndWait(desktop.ws);
      await entitlementRef(account.uid).set(entitlementDoc({}));
    }
  });

  it("changes nothing at all with the flag off", async () => {
    // The same account that is refused everything by the enforcing relay.
    const { ack, auth } = await publishAs(permissivePort, accounts.unentitled, "flag-off-publish");

    expect(auth.capabilities.tunnelServices).toEqual(["ksp", "task-transfer"]);
    expect(auth.capabilities.taskSnapshotPublication).toBeDefined();
    expect(auth.capabilities.mobileNotifications).toBeDefined();
    // Absent, not `false`: with enforcement off `auth_ok` keeps its old shape.
    expect(auth.entitlement).toBeUndefined();
    expect(ack).toMatchObject({ ok: true });
    await closeAndWait(auth.ws);

    const phone = await connectAndAuth(permissivePort, { id_token: accounts.unverified.idToken });
    expect(phone.capabilities.tunnelServices).toEqual(["ksp", "task-transfer"]);
    expect(phone.entitlement).toBeUndefined();
    await closeAndWait(phone.ws);
  });
});
