/** Relay-observed access. Missing/unknown is not evidence of inactivity. */
export interface CloudAccessSnapshot {
  active: boolean;
  status: "active" | "grace" | "expired" | "revoked" | "none" | "unknown";
  currentPeriodEndsAt: string | null;
  graceEndsAt: string | null;
  reason?: "unverified_email";
}

export function readCloudAccess(value: unknown): CloudAccessSnapshot | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (typeof record.active !== "boolean" || ![
    "active", "grace", "expired", "revoked", "none", "unknown"
  ].includes(String(record.status))) return null;
  return {
    active: record.active,
    status: record.status as CloudAccessSnapshot["status"],
    currentPeriodEndsAt: typeof record.currentPeriodEndsAt === "string" ? record.currentPeriodEndsAt : null,
    graceEndsAt: typeof record.graceEndsAt === "string" ? record.graceEndsAt : null,
    ...(record.reason === "unverified_email" ? { reason: "unverified_email" as const } : {})
  };
}

export function cloudAccessAction(access: CloudAccessSnapshot | null | undefined): string | null {
  if (!access) return null;
  if (access.reason === "unverified_email") return "Verify your email to use Kanna Cloud.";
  if (access.status === "unknown") return null;
  if (access.status === "grace" && access.graceEndsAt && Date.parse(access.graceEndsAt) <= Date.now()) {
    return "Your payment grace period has ended. Manage your subscription to restore cloud access.";
  }
  if (!access.active) return "An active subscription is required for Kanna Cloud. Manage your account to restore access.";
  return null;
}
