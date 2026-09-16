import http from 'node:http';
import fs from 'node:fs';
import { joinSession } from '@github/copilot-sdk/extension';
import { createWakeBridge } from './bridge.mjs';

const identity = { taskId: process.env.KANNA_TASK_ID, runId: process.env.KANNA_STAGE_RUN_ID };
const base = new URL(process.env.KANNA_SERVER_BASE_URL);
if (base.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname)
  || !identity.taskId || !identity.runId) throw Error('Kanna wake requires a local server and launch identity');
const session = await joinSession();
if (!process.env.SESSION_ID || session.sessionId !== process.env.SESSION_ID) throw Error('Kanna wake native session mismatch');
const endpoint = new URL(`/v1/tasks/${encodeURIComponent(identity.taskId)}/copilot-wake`, base);
endpoint.search = new URLSearchParams({ runId: identity.runId, sessionId: session.sessionId });
const headers = {};
if (process.env.KANNA_TASK_EVENTS_TOKEN_PATH) {
  headers.Authorization = `Bearer ${fs.readFileSync(process.env.KANNA_TASK_EVENTS_TOKEN_PATH, 'utf8').trim()}`;
}
let stream;
let stopping = false;
const bridge = createWakeBridge(session, identity, body => new Promise((resolve, reject) => {
  // The task id is in the endpoint, not caller-controlled receipt provenance.
  const { taskId: _taskId, ...receipt } = body;
  const data = JSON.stringify(receipt);
  const senderStream = stream;
  const url = new URL(endpoint); url.pathname += '/receipt'; url.search = '';
  const request = http.request(url, { method:'POST', headers:{ ...headers, 'Content-Type':'application/json', 'Content-Length':Buffer.byteLength(data) } }, response => {
    response.resume();
    response.on('end', () => {
      if (response.statusCode === 200) resolve();
      else { senderStream?.destroy(); reject(Error(`Kanna wake receipt HTTP ${response.statusCode}`)); }
    });
  });
  request.setTimeout(10_000, () => request.destroy(Error('Kanna wake receipt timeout')));
  request.on('error', error => { senderStream?.destroy(); reject(error); });
  request.end(data);
}));

// The private SDK stdin pipe belongs to the provider parent. Its end must not
// leave an orphan extension reconnecting to Kanna after the TUI has died.
process.stdin.once('end', () => { stopping = true; stream?.destroy(); bridge.dispose(); });

async function connect() {
  return new Promise((resolve, reject) => {
    stream = http.get(endpoint, { headers }, response => {
      if (response.statusCode !== 200) {
        response.resume();
        // Old server or stale run cannot become valid through retrying this identity.
        if ([400, 401, 403, 404, 409].includes(response.statusCode)) stopping = true;
        reject(Error(`Kanna wake registration HTTP ${response.statusCode}`)); return;
      }
      let buffer = '';
      let chain = Promise.resolve();
      response.setEncoding('utf8');
      response.on('data', chunk => {
        buffer += chunk;
        if (buffer.length > 1_048_576) { stream.destroy(Error('Kanna wake frame too large')); return; }
        let end;
        while ((end = buffer.indexOf('\n\n')) !== -1) {
          const block = buffer.slice(0, end); buffer = buffer.slice(end + 2);
          const data = block.split('\n').filter(l => l.startsWith('data:')).map(l => l.slice(5).trimStart()).join('\n');
          if (data) chain = chain.then(() => bridge.handle(JSON.parse(data))).catch(error => { stream.destroy(error); });
        }
      });
      response.on('end', resolve);
      response.on('error', reject);
    });
    stream.on('error', reject);
    // SSE keepalives normally arrive every 15s; detect a broken transport only.
    stream.setTimeout(45_000, () => stream.destroy(Error('Kanna wake stream timed out')));
  });
}
let delay = 1_000;
while (!stopping) {
  const started = Date.now();
  try { await connect(); } catch (error) { console.error(`[Kanna wake] ${error.message}; mailbox retained`); }
  if (stopping) break;
  if (Date.now() - started > 30_000) delay = 1_000;
  await new Promise(resolve => { const timer = setTimeout(resolve, delay); timer.unref(); });
  delay = Math.min(delay * 2, 15_000); // connection recovery, never resend/poll events
}
bridge.dispose();
