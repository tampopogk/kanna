// The host owns this SDK session. Kanna owns attempts, receipts and event ack.
// No terminal handle, runtime spawn, mailbox ack or fallback input exists here.
export function createWakeBridge(session, identity, report) {
  const attempts = new Map();
  let connection;
  let disposed = false;
  const timers = new Set();
  const matches = (binding) => binding?.taskId === identity.taskId
    && binding.runId === identity.runId && binding.sessionId === session.sessionId;
  const receipt = async (item, value) => {
    if (disposed) return;
    await report({ ...item.attempt.binding, attemptId: item.attempt.id, ...value });
  };
  const inspect = async (item) => {
    if (item.messageId) return receipt(item, { kind: 'accepted', messageId: item.messageId });
    const events = await session.getEvents();
    const matches = events.filter(e => e.type === 'user.message' && e.data?.content === item.attempt.message);
    if (matches.length === 1 && typeof matches[0].id === 'string') {
      return receipt(item, { kind: 'observed', eventId: matches[0].id, content: item.attempt.message });
    }
    return receipt(item, { kind: 'uncertain', error: matches.length
      ? 'ambiguous native history; retained without resend'
      : 'not yet in native history; retained without resend' });
  };
  const uncertain = (item, error) => receipt(item, { kind: 'uncertain', error: String(error).slice(0, 512) });
  const unsubscribe = session.on(event => {
    if (event.type !== 'user.message') return;
    for (const item of attempts.values()) {
      if (event.data?.content === item.attempt.message) {
        void inspect(item).catch(error => uncertain(item, error).catch(() => {}));
      }
    }
  });
  return {
    async handle(frame) {
      if (disposed) return;
      if (frame.type === 'registered') {
        if (frame.protocol !== 1 || !matches(frame.binding)) throw Error('Copilot wake registration mismatch');
        connection = frame.binding.connectionId;
        return;
      }
      const attempt = frame.attempt;
      if (!connection || !matches(attempt?.binding) || attempt.binding.connectionId !== connection
        || typeof attempt.id !== 'string' || !attempt.message?.startsWith('[Kanna supervisor]')) {
        throw Error('Copilot wake attempt binding mismatch');
      }
      let item = attempts.get(attempt.id);
      if (item && item.attempt.message !== attempt.message) throw Error('Copilot wake attempt text changed');
      if (!item) { item = { attempt, sent: false }; attempts.set(attempt.id, item); }
      item.attempt = attempt;
      if (frame.type === 'inspect') {
        await inspect(item);
      } else if (frame.type === 'send') {
        if (item.sent) { await inspect(item); return; }
        item.sent = true; // reserve before entering the provider; never retry send
        // Queue acceptance can lag or be lost. Do not block stream/recovery on it.
        const timer = setTimeout(() => { void uncertain(item, 'native queue receipt timed out').catch(() => {}); }, 10_000);
        timers.add(timer);
        void Promise.resolve().then(() => session.send({ prompt: attempt.message, mode: 'enqueue' }))
          .then(async messageId => {
            if (typeof messageId !== 'string' || !messageId.trim()) throw Error('missing native queue id');
            item.messageId = messageId;
            await receipt(item, { kind: 'accepted', messageId });
          }).catch(error => uncertain(item, error).catch(() => {}))
          .finally(() => { clearTimeout(timer); timers.delete(timer); });
      } else throw Error('Unsupported Copilot wake operation');
    },
    dispose() { disposed = true; for (const timer of timers) clearTimeout(timer); timers.clear(); if (typeof unsubscribe === 'function') unsubscribe(); },
  };
}
