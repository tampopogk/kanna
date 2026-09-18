/**
 * `removeAccountDesktop` end to end: a real token, the deployed callable
 * running inside the Functions emulator, and the Firestore emulator behind it.
 *
 * The other two suites stop short of the thing a person actually depends on.
 * The unit test hands the core a caller object it made up; the Firestore suite
 * calls the core directly with the admin SDK. Neither runs the `onCall`
 * wrapper, and neither can tell you whether the uid this function trusts comes
 * from a verified Auth token or from a field in the request body - which is
 * the whole authorization story, since "remove a machine" is destructive and
 * every account's directory sits one uid apart in the same collection.
 *
 * Here the function is invoked over HTTP the way a phone invokes it, so the
 * token is verified by the emulator rather than asserted by the test.
 *
 * Skipped unless the Firestore, Auth and Functions emulators are all up; run
 * with `./kd emulators exec -- pnpm test`.
 */
import type { Firestore } from "firebase-admin/firestore";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import {
  clearFirestoreEmulator,
  emulatorFirestore,
  shutdownEmulatorFirestore,
} from "./support/emulator.js";
import {
  callFunction,
  hasFunctionsEmulator,
  signUpEmulatorUser,
} from "./support/functionsEmulator.js";

interface RemovalResult {
  removed: number;
}

describe.skipIf(!hasFunctionsEmulator)("removeAccountDesktop over the callable wire", () => {
  let db: Firestore;

  beforeAll(() => {
    db = emulatorFirestore();
  });
  afterEach(clearFirestoreEmulator);
  afterAll(shutdownEmulatorFirestore);

  async function seedDesktop(uid: string, desktopId: string, taskIds: string[]) {
    const desktopRef = db.doc(`users/${uid}/desktops/${desktopId}`);
    await desktopRef.set({ desktopId, publicationSessionGeneration: 1 });
    for (const taskId of taskIds) {
      await desktopRef.collection("tasks").doc(taskId).set({ title: taskId });
    }
  }

  const remove = (desktopId: string, idToken?: string, extra: Record<string, unknown> = {}) =>
    callFunction<RemovalResult>("removeAccountDesktop", {
      data: { desktopId, ...extra },
      idToken,
    });

  it("removes the caller's own machine and its tasks", async () => {
    const owner = await signUpEmulatorUser();
    await seedDesktop(owner.uid, "desktop-dead", ["task-a", "task-b"]);

    const response = await remove("desktop-dead", owner.idToken);

    expect(response.status).toBe(200);
    expect(response.result).toEqual({ removed: 1 });
    expect((await db.doc(`users/${owner.uid}/desktops/desktop-dead`).get()).exists).toBe(false);
    const tasks = await db.collection(`users/${owner.uid}/desktops/desktop-dead/tasks`).get();
    expect(tasks.empty).toBe(true);
  });

  it("takes the account from the verified token, not from the request body", async () => {
    // The payload names the victim's uid the way a hand-rolled client would.
    // Only the deployed wrapper can prove it is ignored.
    const victim = await signUpEmulatorUser();
    const caller = await signUpEmulatorUser();
    await seedDesktop(victim.uid, "desktop-dead", ["task-a"]);

    const response = await remove("desktop-dead", caller.idToken, { uid: victim.uid });

    expect(response.status).toBe(200);
    expect(response.result).toEqual({ removed: 0 });
    expect((await db.doc(`users/${victim.uid}/desktops/desktop-dead`).get()).exists).toBe(true);
    const tasks = await db.collection(`users/${victim.uid}/desktops/desktop-dead/tasks`).get();
    expect(tasks.docs.map((task) => task.id)).toEqual(["task-a"]);
  });

  it("leaves another account's identically named machine alone", async () => {
    const owner = await signUpEmulatorUser();
    const stranger = await signUpEmulatorUser();
    await seedDesktop(owner.uid, "desktop-shared-name", ["task-a"]);
    await seedDesktop(stranger.uid, "desktop-shared-name", ["task-b"]);

    await remove("desktop-shared-name", owner.idToken);

    expect((await db.doc(`users/${owner.uid}/desktops/desktop-shared-name`).get()).exists).toBe(
      false,
    );
    expect(
      (await db.doc(`users/${stranger.uid}/desktops/desktop-shared-name`).get()).exists,
    ).toBe(true);
  });

  it("refuses an unauthenticated call with the reason the client renders", async () => {
    const owner = await signUpEmulatorUser();
    await seedDesktop(owner.uid, "desktop-dead", ["task-a"]);

    const response = await remove("desktop-dead");

    expect(response.status).toBe(401);
    expect(response.error?.status).toBe("UNAUTHENTICATED");
    expect(response.error?.details?.reason).toBe("sign_in_required");
    expect((await db.doc(`users/${owner.uid}/desktops/desktop-dead`).get()).exists).toBe(true);
  });

  it("refuses a call naming no machine", async () => {
    const owner = await signUpEmulatorUser();

    const response = await callFunction<RemovalResult>("removeAccountDesktop", {
      data: {},
      idToken: owner.idToken,
    });

    expect(response.status).toBe(400);
    expect(response.error?.status).toBe("INVALID_ARGUMENT");
    expect(response.error?.details?.reason).toBe("invalid_desktop_request");
  });

  it("reports a machine that is already gone as a completed removal", async () => {
    const owner = await signUpEmulatorUser();

    const response = await remove("desktop-never-existed", owner.idToken);

    expect(response.status).toBe(200);
    expect(response.result).toEqual({ removed: 0 });
  });
});
