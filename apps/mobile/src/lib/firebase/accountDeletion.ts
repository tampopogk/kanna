import { httpsCallable } from "firebase/functions";
import { getConfiguredFunctions } from "./configuredFunctions";

export async function requestMobileAccountDeletion(): Promise<void> {
  await httpsCallable<Record<string, never>, { deleted: true }>(getConfiguredFunctions(), "deleteAccount")({});
}
