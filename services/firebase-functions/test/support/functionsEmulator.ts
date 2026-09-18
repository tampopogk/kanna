/**
 * Calling a deployed callable through the Functions emulator.
 *
 * `emulator.ts` drives the core function with the admin SDK in this process,
 * which leaves the deployed shape untested: the `onCall` wrapper, the callable
 * wire envelope, and - the part worth proving - that the uid comes from a
 * verified Auth token rather than from anything in the request body. That last
 * one is only a real assertion when the function runs inside the emulator with
 * a genuine token on the request; in-process, the test hands the core whatever
 * caller it likes and proves nothing about token handling.
 *
 * Gated like the Firestore tests, so `pnpm test` stays green with no emulators
 * running; `./kd emulators exec -- pnpm test` runs the real thing.
 *
 * One trap worth knowing before you debug a surprising result: the Functions
 * emulator loads `dist/` once, at startup, and does not pick up a later build.
 * Editing a function and rebuilding changes nothing until the emulators are
 * restarted - so a run that looks like it exonerates your change may simply be
 * the previous build answering.
 */
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { EMULATOR_PROJECT_ID, firestoreEmulatorHost } from "./emulator.js";

export const authEmulatorHost = process.env.FIREBASE_AUTH_EMULATOR_HOST;

const repoRoot = join(import.meta.dirname, "..", "..", "..", "..");

function readJson(path: string): unknown {
  try {
    return JSON.parse(readFileSync(path, "utf8")) as unknown;
  } catch {
    return null;
  }
}

function functionsPortIn(config: unknown): number | null {
  if (typeof config !== "object" || config === null) return null;
  const { emulators } = config as { emulators?: unknown };
  if (typeof emulators !== "object" || emulators === null) return null;
  const { functions } = emulators as { functions?: unknown };
  if (typeof functions !== "object" || functions === null) return null;
  const { port } = functions as { port?: unknown };
  return typeof port === "number" ? port : null;
}

/**
 * The Functions port of the emulator this run is already talking to.
 *
 * `kd` writes one generated config per emulator instance, named for that
 * instance's Firestore port, so deriving the Functions port from the Firestore
 * host keeps both clients pointed at the same emulator on a machine running
 * several worktrees side by side. `firebase.json` is the fallback for a plain
 * `firebase emulators:exec`.
 */
function resolveFunctionsEmulatorPort(): number | null {
  const fromEnv = Number(process.env.KANNA_FIREBASE_FUNCTIONS_PORT);
  if (Number.isInteger(fromEnv) && fromEnv > 0 && fromEnv <= 65_535) return fromEnv;

  const firestorePort = firestoreEmulatorHost?.split(":").pop();
  const candidates = [
    ...(firestorePort ? [join(repoRoot, `.firebase-${firestorePort}.kanna.json`)] : []),
    join(repoRoot, "firebase.json"),
  ];
  for (const candidate of candidates) {
    const port = functionsPortIn(readJson(candidate));
    if (port !== null) return port;
  }
  return null;
}

export const functionsEmulatorPort = resolveFunctionsEmulatorPort();

/** True when a callable can be invoked over the wire for this run. */
export const hasFunctionsEmulator = Boolean(
  firestoreEmulatorHost && authEmulatorHost && functionsEmulatorPort,
);

export interface CallableError {
  status?: string;
  message?: string;
  details?: { reason?: string };
}

export interface CallableResponse<T> {
  status: number;
  result?: T;
  error?: CallableError;
}

/**
 * Invoke a callable exactly as a client does: the `{ data }` envelope, the
 * bearer token in the `Authorization` header, and the raw response left intact
 * so a test can assert the refusal envelope a client would actually receive.
 */
export async function callFunction<T>(
  name: string,
  input: { data: unknown; idToken?: string },
): Promise<CallableResponse<T>> {
  const response = await fetch(
    `http://127.0.0.1:${functionsEmulatorPort}/${EMULATOR_PROJECT_ID}/us-central1/${name}`,
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(input.idToken ? { Authorization: `Bearer ${input.idToken}` } : {}),
      },
      body: JSON.stringify({ data: input.data }),
    },
  );
  if (response.status === 404) {
    throw new Error(
      `The Functions emulator is not serving ${name}. It loads dist/, so a ` +
        "callable added since the last build is invisible to it: run " +
        "`pnpm --filter @kanna/firebase-functions build` and restart the emulators.",
    );
  }
  const body = (await response.json().catch(() => null)) as {
    result?: T;
    error?: CallableError;
  } | null;
  return { status: response.status, result: body?.result, error: body?.error };
}

/** A fresh signed-in account, and the token the emulator minted for it. */
export async function signUpEmulatorUser(): Promise<{ uid: string; idToken: string }> {
  const response = await fetch(
    `http://${authEmulatorHost}/identitytoolkit.googleapis.com/v1/accounts:signUp?key=${EMULATOR_PROJECT_ID}`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        email: `desktop-removal-${randomUUID()}@kanna.test`,
        password: "emulator-password",
        returnSecureToken: true,
      }),
    },
  );
  const body = (await response.json().catch(() => null)) as {
    idToken?: string;
    localId?: string;
  } | null;
  if (!response.ok || !body?.idToken || !body.localId) {
    throw new Error(
      `Failed to create an Auth emulator user: ${response.status} ${JSON.stringify(body)}`,
    );
  }
  return { uid: body.localId, idToken: body.idToken };
}
