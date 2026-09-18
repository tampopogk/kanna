/**
 * Removing a machine from the cloud desktop directory, against the real
 * Firestore emulator.
 *
 * The unit test covers authorization and argument handling with a fake store;
 * what only the emulator can prove is the part the whole design turns on -
 * that the entry's `tasks` subcollection goes with it. Firestore deletes do
 * not cascade, so a store that deleted the document alone would pass every
 * fake-backed assertion and still leave the account full of orphaned tasks.
 *
 * Skipped without `FIRESTORE_EMULATOR_HOST`; run with
 * `./kd emulators exec -- pnpm test`.
 */
import type { Firestore } from "firebase-admin/firestore";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import {
  firestoreAccountDesktopStore,
  removeAccountDesktop,
} from "../src/accountDesktops.js";
import { accountDeletionPath } from "../src/billing/types.js";
import {
  clearFirestoreEmulator,
  emulatorFirestore,
  hasFirestoreEmulator,
  shutdownEmulatorFirestore,
} from "./support/emulator.js";

describe.skipIf(!hasFirestoreEmulator)("account desktop directory removal", () => {
  let db: Firestore;

  beforeAll(() => {
    db = emulatorFirestore();
  });
  afterEach(clearFirestoreEmulator);
  afterAll(shutdownEmulatorFirestore);

  async function seedDesktop(
    uid: string,
    docId: string,
    taskIds: string[],
    desktopId: string | null = docId,
  ) {
    const desktopRef = db.doc(`users/${uid}/desktops/${docId}`);
    await desktopRef.set({
      ...(desktopId === null ? {} : { desktopId }),
      publicationSessionGeneration: 3,
    });
    for (const taskId of taskIds) {
      await desktopRef.collection("tasks").doc(taskId).set({ title: taskId, closedAt: null });
    }
  }

  const remove = (uid: string, desktopId: string) =>
    removeAccountDesktop(
      { desktopId },
      { uid },
      { store: firestoreAccountDesktopStore(db) },
    );

  it("deletes the entry and every task beneath it", async () => {
    await seedDesktop("user-1", "desktop-dead", ["task-a", "task-b"]);

    await expect(remove("user-1", "desktop-dead")).resolves.toEqual({ removed: 1 });

    expect((await db.doc("users/user-1/desktops/desktop-dead").get()).exists).toBe(false);
    const tasks = await db.collection("users/user-1/desktops/desktop-dead/tasks").get();
    expect(tasks.empty).toBe(true);
  });

  it("leaves the account's other machines and their tasks alone", async () => {
    await seedDesktop("user-1", "desktop-dead", ["task-a"]);
    await seedDesktop("user-1", "desktop-live", ["task-b"]);

    await remove("user-1", "desktop-dead");

    expect((await db.doc("users/user-1/desktops/desktop-live").get()).exists).toBe(true);
    const tasks = await db.collection("users/user-1/desktops/desktop-live/tasks").get();
    expect(tasks.docs.map((task) => task.id)).toEqual(["task-b"]);
  });

  it("never reaches another account's directory", async () => {
    await seedDesktop("user-2", "desktop-dead", ["task-a"]);

    await expect(remove("user-1", "desktop-dead")).resolves.toEqual({ removed: 0 });

    expect((await db.doc("users/user-2/desktops/desktop-dead").get()).exists).toBe(true);
  });

  it("sweeps tasks orphaned under an entry an older build already deleted", async () => {
    const desktopRef = db.doc("users/user-1/desktops/desktop-orphaned");
    await desktopRef.collection("tasks").doc("task-a").set({ title: "orphan" });

    await expect(remove("user-1", "desktop-orphaned")).resolves.toEqual({ removed: 0 });

    const tasks = await desktopRef.collection("tasks").get();
    expect(tasks.empty).toBe(true);
  });

  it("resolves a machine whose document id was sanitized away from its id", async () => {
    // `cloudDesktopDocumentId` maps "/" to "_", so the document id and the
    // machine's real desktop id are not the same string.
    await seedDesktop("user-1", "desktop_a_b", ["task-a"], "desktop/a/b");

    await expect(remove("user-1", "desktop/a/b")).resolves.toEqual({ removed: 1 });

    expect((await db.doc("users/user-1/desktops/desktop_a_b").get()).exists).toBe(false);
  });

  it("resolves a legacy entry that stored no desktop id at all", async () => {
    await seedDesktop("user-1", "desktop-legacy", ["task-a"], null);

    await expect(remove("user-1", "desktop-legacy")).resolves.toEqual({ removed: 1 });

    expect((await db.doc("users/user-1/desktops/desktop-legacy").get()).exists).toBe(false);
  });

  it("never removes an entry whose stored id names a different machine", async () => {
    // A document id is a sanitized desktop id, so one machine's document id
    // can equal another machine's raw desktop id. The stored field decides.
    await seedDesktop("user-1", "desktop_a_b", ["task-a"], "desktop/a/b");

    await expect(remove("user-1", "desktop_a_b")).resolves.toEqual({ removed: 0 });

    expect((await db.doc("users/user-1/desktops/desktop_a_b").get()).exists).toBe(true);
  });

  it("refuses while the account is being deleted, leaving the entry in place", async () => {
    await seedDesktop("user-1", "desktop-dead", ["task-a"]);
    await db.doc(accountDeletionPath("user-1")).set({ uid: "user-1", started: true });

    await expect(remove("user-1", "desktop-dead")).rejects.toMatchObject({
      reason: "account_deleted",
    });

    expect((await db.doc("users/user-1/desktops/desktop-dead").get()).exists).toBe(true);
  });
});
