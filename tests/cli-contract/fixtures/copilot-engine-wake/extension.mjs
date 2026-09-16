// Test driver. Only the fake parent in copilot-engine-wake.test.ts launches it.
// No Copilot executable, network endpoint, credentials or model are used.
import { pathToFileURL } from "node:url";
import { enqueueWake, inspectPendingWake } from "./adapter.mjs";

try {
  const { joinSession } = await import(pathToFileURL(process.env.KANNA_TEST_SDK));
  const session = await joinSession();
  const binding = JSON.parse(process.env.KANNA_TEST_BINDING);
  const result = process.env.KANNA_TEST_OPERATION === "inspect"
    ? await inspectPendingWake(session, binding)
    : await enqueueWake(session, binding, 150);
  process.send(result, () => process.exit(0));
} catch (error) {
  process.send({ state: "error", reason: String(error) }, () => process.exit(1));
}
