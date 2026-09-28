import type { TaskInputResult } from "./types";

/** A successful HTTP request can mean queued, not delivered. */
export function taskInputResult(value: unknown): TaskInputResult {
  if (value === undefined || value === null) return { status: "delivered" };
  if (typeof value !== "object") throw new Error("Unrecognized input receipt; check delivery status before retrying.");
  const receipt = value as { state?: string; status?: string; id?: string; error?: string };
  if (receipt.status === "delivered" && receipt.state === undefined) return { status: "delivered" };
  if (receipt.state === "submitted") return { status: "delivered" };
  if ((receipt.state === "queued" || receipt.state === "submitting") && receipt.id) {
    return { status: "queued", deliveryId: receipt.id };
  }
  if (receipt.state === "failed") return { status: "failed", reason: "server_rejected", message: receipt.error ?? "Input was not submitted." };
  if (receipt.state === "uncertain") return { status: "uncertain", message: `${receipt.error ?? "Provider acceptance is uncertain."} Delivery ${receipt.id ?? "unknown"}; do not resend.` };
  throw new Error("Unrecognized input receipt; check delivery status before retrying.");
}
