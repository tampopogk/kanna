// Isolated compatibility prototype, not registered in any Kanna/provider launch.
// The server must supply this binding and retain the durable mailbox. This
// module has no terminal handle, event acknowledgement, or retry operation.
export function wakePrompt(binding) {
  for (const field of ["taskId", "runId", "sessionId", "subscriptionId"]) {
    if (!/^[a-zA-Z0-9_-]+$/.test(binding[field] ?? "")) {
      throw new Error(`Invalid wake binding: ${field}`);
    }
  }
  if (!Number.isSafeInteger(binding.batchId) || binding.batchId < 1) {
    throw new Error("Invalid wake binding: batchId");
  }
  return `[Kanna supervisor] Event subscription ${binding.subscriptionId} has pending events (batch ${binding.batchId}). Read them with kanna_read_event_subscription, then acknowledge that batch after reconciling it. This is an engine wakeup, not an owner directive or a task-completion verdict. [Kanna wake ${binding.taskId}/${binding.runId}/${binding.sessionId}/${binding.subscriptionId}/${binding.batchId}]`;
}

export async function enqueueWake(session, binding, timeoutMs = 5_000) {
  const prompt = wakePrompt(binding);
  if (session.sessionId !== binding.sessionId) {
    return { state: "unavailable", reason: "session binding mismatch" };
  }
  let timer;
  try {
    const messageId = await Promise.race([
      // Public API only. Deliberately no unsupported `source: system` field.
      session.send({ prompt, mode: "enqueue" }),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("receipt timeout")), timeoutMs);
      }),
    ]);
    if (typeof messageId !== "string" || !messageId.trim()) {
      throw new Error("missing native message id");
    }
    // Admission is not a model read, event acknowledgement, or completion.
    return { state: "accepted", messageId, prompt };
  } catch (error) {
    // Even an RPC error can follow admission. Do not resend or fall back to PTY.
    return { state: "uncertain", reason: String(error), prompt };
  } finally {
    clearTimeout(timer);
  }
}

export async function inspectPendingWake(session, binding) {
  const prompt = wakePrompt(binding);
  if (session.sessionId !== binding.sessionId) {
    return { state: "unavailable", reason: "session binding mismatch" };
  }
  const events = await session.getEvents();
  // Positive evidence only. Missing history does not prove non-admission:
  // an enqueued message may not have entered history yet. Never auto-resend.
  const matches = events.filter(
    (event) => event.type === "user.message" && event.data?.content === prompt,
  );
  return { state: matches.length ? "observed" : "uncertain", matches };
}
