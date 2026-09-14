export type EntitlementStatus = "active" | "grace" | "expired" | "revoked";

export interface CloudEntitlement {
  status: EntitlementStatus;
  source: "stripe" | "app_store" | "comp" | "free_beta" | "grandfathered" | "promo";
  capabilities: string[];
  currentPeriodEndsAt: unknown | null;
  graceEndsAt: unknown | null;
  environment: string;
  stripeCustomerId?: string | null;
}

export type CloudAccessState = "pending" | "active" | "grace" | "expired" | "revoked" | "read-failure";

export function graceDeadline(entitlement: CloudEntitlement | null): number | null {
  const value = entitlement?.graceEndsAt;
  const millis = typeof value === "string" ? Date.parse(value) : NaN;
  return Number.isFinite(millis) ? millis : null;
}

/** Matches relay authority: active survives renewal latency; grace ends at its deadline. */
export function cloudAccessState(entitlement: CloudEntitlement | null, now: number): CloudAccessState {
  if (!entitlement) return "pending";
  if (entitlement.status === "grace") {
    const deadline = graceDeadline(entitlement);
    return deadline !== null && deadline <= now ? "expired" : "grace";
  }
  return entitlement.status;
}
