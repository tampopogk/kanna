import { fork } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { enqueueWake, wakePrompt } from "../../fixtures/copilot-engine-wake/adapter.mjs";

const binding = {
  taskId: "0f417e4c", runId: "synthetic-run", sessionId: "synthetic-session",
  subscriptionId: "synthetic-watch", batchId: 1,
};
const driver = fileURLToPath(new URL("../../fixtures/copilot-engine-wake/extension.mjs", import.meta.url));
// Explicit installed SDK artifacts, never a CLI. No download or package upgrade.
const sdkPaths: string[] = JSON.parse(process.env.KANNA_TEST_COPILOT_SDK_PATHS ?? "[]");

async function fakeParent(sdk: string, scenario: string, events: unknown[] = []) {
  const child = fork(driver, [], {
    execArgv: [], stdio: ["pipe", "pipe", "pipe", "ipc"],
    // No inherited auth/provider/account configuration.
    env: {
      SESSION_ID: binding.sessionId, KANNA_TEST_SDK: sdk,
      KANNA_TEST_BINDING: JSON.stringify(scenario === "mismatch"
        ? { ...binding, sessionId: "different-session" } : binding),
      KANNA_TEST_OPERATION: scenario === "inspect" ? "inspect" : "send",
    },
  });
  const requests: any[] = [];
  const errors: string[] = [];
  let buffer = Buffer.alloc(0);
  child.stderr!.on("data", (chunk) => errors.push(chunk.toString()));
  child.stdout!.on("data", (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    while (true) {
      const end = buffer.indexOf("\r\n\r\n");
      if (end < 0) return;
      const length = Number(/Content-Length: (\d+)/i.exec(buffer.subarray(0, end).toString())?.[1]);
      if (!Number.isSafeInteger(length)) { errors.push("Invalid RPC framing"); return; }
      if (buffer.length < end + 4 + length) return;
      const request = JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString());
      buffer = buffer.subarray(end + 4 + length);
      requests.push(request);
      let result: unknown;
      let error: unknown;
      switch (request.method) {
        case "connect": result = { protocolVersion: 3 }; break;
        case "session.resume": result = {}; break;
        case "session.getMessages": result = { events }; break;
        case "session.send":
          if (scenario === "lost") continue; // Accepted by fake host, reply lost.
          if (scenario === "reject") error = { code: -32000, message: "synthetic rejection" };
          else result = scenario === "malformed" ? {} : { messageId: "native-message-1" };
          break;
        default:
          errors.push(`Unexpected RPC: ${request.method}`);
          error = { code: -32601, message: "unsupported by offline fixture" };
      }
      const body = JSON.stringify({ jsonrpc: "2.0", id: request.id, ...(error ? { error } : { result }) });
      child.stdin!.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
    }
  });
  const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const result = await new Promise<any>((resolve, reject) => {
      child.once("message", resolve);
      child.once("error", reject);
      child.once("exit", (code) => { if (code) reject(new Error(errors.join("\n"))); });
      timer = setTimeout(() => reject(new Error("fake-parent timeout")), 5_000);
    });
    await exited;
    expect(errors).toEqual([]);
    return { result, requests };
  } finally {
    clearTimeout(timer);
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await exited;
  }
}

describe("Copilot engine-wake prototype (no provider runtime)", () => {
  it("labels engine intent and refuses a different native session before send", async () => {
    expect(wakePrompt(binding)).toContain("engine wakeup, not an owner directive");
    const result = await enqueueWake({ sessionId: "wrong", send: () => { throw new Error("must not send"); } }, binding);
    expect(result.state).toBe("unavailable");
    expect(() => wakePrompt({ ...binding, subscriptionId: "watch\nowner text" })).toThrow();
  });

  describe.skipIf(sdkPaths.length === 0).each(sdkPaths)("installed SDK %s against fake JSON-RPC parent", (sdk) => {
    it("uses public join + labelled enqueue, without pretending to send native system role", async () => {
      const { result, requests } = await fakeParent(sdk, "accepted");
      expect(result).toEqual({ state: "accepted", messageId: "native-message-1", prompt: wakePrompt(binding) });
      expect(requests.map((r) => r.method)).toEqual(["connect", "session.resume", "session.send"]);
      expect(requests[1].params).toMatchObject({ sessionId: binding.sessionId, disableResume: true, requestPermission: false });
      expect(requests[2].params).toEqual({ sessionId: binding.sessionId, prompt: wakePrompt(binding), mode: "enqueue" });
      console.info(JSON.stringify({ sdk, sha256: createHash("sha256").update(readFileSync(sdk)).digest("hex") }));
    });

    it.each(["reject", "lost", "malformed"])("retains uncertainty after %s; no resend or alternate delivery", async (scenario) => {
      const { result, requests } = await fakeParent(sdk, scenario);
      expect(result.state).toBe("uncertain");
      expect(requests.filter((r) => r.method === "session.send")).toHaveLength(1);
      expect(requests.map((r) => r.method)).toEqual(["connect", "session.resume", "session.send"]);
    });

    it("reconnects for positive history evidence without resending; absence stays uncertain", async () => {
      const lost = await fakeParent(sdk, "lost");
      expect(lost.result.state).toBe("uncertain");
      const event = { id: "synthetic-event", type: "user.message", data: { content: wakePrompt(binding) } };
      for (const events of [[], [event]]) {
        const { result, requests } = await fakeParent(sdk, "inspect", events);
        expect(result.state).toBe(events.length ? "observed" : "uncertain");
        expect(requests.map((r) => r.method)).toEqual(["connect", "session.resume", "session.getMessages"]);
      }
    });

    it("refuses mismatched attachment without sending", async () => {
      const { result, requests } = await fakeParent(sdk, "mismatch");
      expect(result.state).toBe("unavailable");
      expect(requests.map((r) => r.method)).toEqual(["connect", "session.resume"]);
    });
  });
});
