import {
  DesignRequestError,
  type DesignCandidate,
  type DesignApproval,
  type DesignTransport,
} from "@kanna/design-editor";
import { invoke } from "../invoke";
import { DesktopServerRequestError, requestDesktopServerJson } from "./desktopServerClient";

/**
 * The desktop's client for one task's App Design session
 * (docs/specs/app-design.md). The document, threads and delivery live on
 * kanna-server; the desktop speaks as the person, over its authenticated
 * local client. Approval is the one exception: preparing a candidate is an
 * HTTP call, but confirming it goes over the native control socket (a Tauri
 * command), which only this desktop process can use.
 */

function designPath(taskId: string, suffix = ""): string {
  return `/v1/tasks/${encodeURIComponent(taskId)}/design${suffix}`;
}

async function request<T>(path: string, options: { method?: string; body?: unknown } = {}): Promise<T> {
  try {
    return await requestDesktopServerJson<T>(path, options);
  } catch (error) {
    if (error instanceof DesktopServerRequestError) {
      const body = error.parsedBody();
      const message = typeof body?.message === "string" ? body.message : error.message;
      throw new DesignRequestError(message, error.status, typeof body?.reason === "string" ? body.reason : null);
    }
    throw error;
  }
}

export function desktopDesignTransport(taskId: string): DesignTransport {
  return {
    view: () => request(designPath(taskId)),
    changes: ({ doc, feed, timeoutMs }) => {
      const query = new URLSearchParams();
      if (doc !== undefined && doc >= 0) query.set("doc", String(doc));
      if (feed !== undefined && feed >= 0) query.set("feed", String(feed));
      if (timeoutMs !== undefined) query.set("timeoutMs", String(timeoutMs));
      return request(designPath(taskId, `/changes?${query.toString()}`));
    },
    sync: (body) => request(designPath(taskId, "/document/sync"), { method: "POST", body }),
    createThread: (body) => request(designPath(taskId, "/threads"), { method: "POST", body }),
    reply: (threadId, body) =>
      request(designPath(taskId, `/threads/${encodeURIComponent(threadId)}/comments`), { method: "POST", body }),
    resolve: (threadId, resolved) =>
      request(designPath(taskId, `/threads/${encodeURIComponent(threadId)}/resolve`), {
        method: "POST",
        body: { resolved },
      }),
    retryDelivery: (deliveryId) =>
      request(designPath(taskId, `/deliveries/${encodeURIComponent(deliveryId)}/retry`), { method: "POST" }),
  };
}

export function setDesignPosition(taskId: string, position: string): Promise<{ position: string }> {
  return request(designPath(taskId, "/position"), { method: "POST", body: { position } });
}

/** Commit the design's disposable repository and publish the snapshot to approve. */
export function prepareDesignCandidate(taskId: string): Promise<DesignCandidate> {
  return request(designPath(taskId, "/approval/candidate"), { method: "POST" });
}

/** The person's confirmation, over the native control socket. */
export async function confirmDesignApproval(
  taskId: string,
  approvalId: string,
  token: string,
): Promise<DesignApproval> {
  try {
    return await invoke<DesignApproval>("confirm_design_approval", { taskId, approvalId, token });
  } catch (error) {
    throw new DesignRequestError(error instanceof Error ? error.message : String(error), 409, "confirmation_refused");
  }
}

export function reopenDesign(taskId: string): Promise<{ reopened: boolean; status: string }> {
  return request(designPath(taskId, "/approval/reopen"), { method: "POST" });
}

export function retryDesignHandoff(taskId: string): Promise<{ phase: string }> {
  return request(designPath(taskId, "/approval/retry"), { method: "POST" });
}
