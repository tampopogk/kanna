/**
 * Forget one machine from the caller's own cloud desktop directory.
 *
 * `users/{uid}/desktops/{docId}` is written by the relay when a desktop begins
 * a cloud publication session (`services/relay/src/cloudTaskPublication.ts`),
 * and nothing ever removed it: `endSession` only clears the doc's `transfer`
 * field, so a machine that is gone for good - a dev instance from a worktree
 * that no longer exists, a reinstalled Mac - stays in the account directory
 * forever and every signed-in client keeps listing it.
 *
 * Why this is a callable and not a Firestore rules relaxation:
 *
 * - The desktop doc owns a `tasks` subcollection. Firestore deletes do not
 *   cascade, so a client `deleteDoc` would orphan every task document under a
 *   directory entry nobody can see any more - a worse leak than the one being
 *   fixed. `recursiveDelete` is admin-SDK only.
 * - `firestore.rules` keeps `allow write: if false` on this collection, which
 *   is what stops a client *forging* a desktop entry. Routing removal through
 *   a function grants "forget" without ever granting "forge".
 *
 * Removing a machine that is in fact still alive is safe and deliberately not
 * guarded against: its next `beginSession`/`reconcile` re-creates the document.
 * The operation is idempotent - a directory entry that is already gone is the
 * outcome the caller asked for, not an error.
 */
import type { DocumentReference, Firestore } from "firebase-admin/firestore";
import { BillingRequestError } from "./billing/errors.js";
import { accountDeletionPath } from "./billing/types.js";

export interface RemoveAccountDesktopCaller {
  uid: string;
}

export interface RemoveAccountDesktopRequest {
  /**
   * The machine's desktop id, as every client already knows it. Document ids
   * are *sanitized* desktop ids (`cloudDesktopDocumentId` in the relay), so
   * the store resolves the entry the same way a reading client derives its
   * identity - by the stored `desktopId` field, falling back to the document
   * id - instead of re-deriving that mapping in a third place.
   */
  desktopId?: unknown;
}

export interface RemoveAccountDesktopResult {
  /** How many directory entries were removed; 0 is a completed no-op. */
  removed: number;
}

export interface AccountDesktopStore {
  isAccountDeleting(uid: string): Promise<boolean>;
  removeDesktop(uid: string, desktopId: string): Promise<number>;
}

export interface RemoveAccountDesktopDependencies {
  store: AccountDesktopStore;
}

export async function removeAccountDesktop(
  request: RemoveAccountDesktopRequest | null | undefined,
  caller: RemoveAccountDesktopCaller | null,
  dependencies: RemoveAccountDesktopDependencies,
): Promise<RemoveAccountDesktopResult> {
  if (!caller) {
    throw new BillingRequestError(
      "unauthenticated",
      "sign_in_required",
      "Sign in before removing a machine from your account.",
    );
  }

  // The caller names only a machine. The uid comes from the verified Auth
  // token, so the only directory this can address is the caller's own -
  // "remove someone else's machine" is not expressible here.
  const desktopId = parseDesktopId(request?.desktopId);

  if (await dependencies.store.isAccountDeleting(caller.uid)) {
    throw new BillingRequestError(
      "failed-precondition",
      "account_deleted",
      "This account is being deleted.",
    );
  }

  return { removed: await dependencies.store.removeDesktop(caller.uid, desktopId) };
}

export function parseDesktopId(value: unknown): string {
  const desktopId = typeof value === "string" ? value.trim() : "";
  if (desktopId.length === 0 || desktopId.length > 1500) {
    throw new BillingRequestError(
      "invalid-argument",
      "invalid_desktop_request",
      "A machine id is required.",
    );
  }
  return desktopId;
}

/**
 * Whether a desktop id may also be used verbatim as a document id. A stored
 * `desktopId` field is the primary key; this fallback exists only for legacy
 * entries written without one, which a reading client identifies by document
 * id. A value Firestore cannot address as a single path segment simply has no
 * such fallback - it is never interpolated into a path.
 */
function addressableAsDocumentId(desktopId: string): boolean {
  return (
    !desktopId.includes("/") &&
    desktopId !== "." &&
    desktopId !== ".." &&
    !/^__.*__$/.test(desktopId)
  );
}

export function firestoreAccountDesktopStore(db: Firestore): AccountDesktopStore {
  return {
    async isAccountDeleting(uid) {
      return (await db.doc(accountDeletionPath(uid)).get()).exists;
    },
    async removeDesktop(uid, desktopId) {
      const desktops = db.collection(`users/${uid}/desktops`);
      const matches = await desktops.where("desktopId", "==", desktopId).get();
      const refs = new Map<string, DocumentReference>(
        matches.docs.map((document) => [document.id, document.ref]),
      );

      if (addressableAsDocumentId(desktopId) && !refs.has(desktopId)) {
        const byDocumentId = await desktops.doc(desktopId).get();
        const storedDesktopId = byDocumentId.data()?.desktopId;
        // Never delete an entry whose own `desktopId` names a different
        // machine: a document id is a *sanitized* desktop id, so ids from two
        // machines can collide, and the stored field is the authority.
        if (typeof storedDesktopId !== "string" || storedDesktopId === desktopId) {
          refs.set(byDocumentId.id, byDocumentId.ref);
        }
      }

      let removed = 0;
      for (const ref of refs.values()) {
        // Read before deleting so the count reports entries that existed,
        // while the recursive delete still runs for an entry whose parent
        // document is already gone but whose tasks were left behind.
        if ((await ref.get()).exists) removed += 1;
        await db.recursiveDelete(ref);
      }
      return removed;
    },
  };
}

export function accountDesktopDependencies(
  db: Firestore,
): RemoveAccountDesktopDependencies {
  return { store: firestoreAccountDesktopStore(db) };
}
