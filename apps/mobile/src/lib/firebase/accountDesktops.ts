import { httpsCallable } from "firebase/functions";
import { getConfiguredFunctions } from "./configuredFunctions";

/**
 * Forget one machine from the signed-in account's cloud desktop directory.
 *
 * `users/{uid}/desktops/{docId}` is backend-authored - `firestore.rules` keeps
 * the collection `allow write: if false` so a client cannot forge an entry -
 * and the entry owns a `tasks` subcollection that a client delete would
 * orphan. Removal therefore goes through the callable, which deletes the
 * entry and everything beneath it with the admin SDK.
 */
export async function requestAccountDesktopRemoval(desktopId: string): Promise<void> {
  await httpsCallable<{ desktopId: string }, { removed: number }>(
    getConfiguredFunctions(),
    "removeAccountDesktop"
  )({ desktopId });
}
