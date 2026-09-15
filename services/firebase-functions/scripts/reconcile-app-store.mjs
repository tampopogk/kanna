#!/usr/bin/env node
/** Bounded operator repair. Never a scheduled poller; no raw signed data logs. */
import { initializeApp, deleteApp } from "firebase-admin/app";
import { getFirestore } from "firebase-admin/firestore";
import { AppStoreServerAPIClient, Environment } from "@apple/app-store-server-library";
import { createAppleVerifier } from "../dist/src/billing/appStoreVerification.js";
import { createAppleGateway } from "../dist/src/billing/appStoreGateway.js";
import { applyAppleEvidence } from "../dist/src/billing/appStoreEvents.js";
import { readBillingState } from "../dist/src/billing/entitlement.js";

const args = process.argv.slice(2).filter(arg => arg !== "--");
const value = name => args[args.indexOf(name) + 1];
const allowed = new Set(["--project", "--uid", "--environment", "--original-id", "--days", "--apply"]);
for (let i = 0; i < args.length; i++) {
  if (!allowed.has(args[i])) throw new Error("Unknown argument");
  if (args[i] !== "--apply" && (!args[++i] || args[i].startsWith("--"))) throw new Error("Missing argument value");
}
const project = value("--project"), uid = value("--uid"), environment = value("--environment"), originalId = value("--original-id");
const days = args.includes("--days") ? Number(value("--days")) : 7;
if (!args.includes("--project") || !args.includes("--uid") || !args.includes("--original-id") || !args.includes("--environment")
  || !project || !uid || !/^[0-9]{1,80}$/.test(originalId) || !["production", "sandbox"].includes(environment)
  || !Number.isInteger(days) || days < 1 || days > 30) {
  throw new Error("Required: --project PROJECT --uid UID --environment production|sandbox --original-id ID [--days 1..30] [--apply]. Default: dry run.");
}
const apply = args.includes("--apply");
const app = initializeApp({ projectId: project });
try {
  const db = getFirestore(app);
  const record = (await db.doc(`appStoreSubscriptions/${environment}_${originalId}`).get()).data();
  if (record?.uid !== uid) throw new Error("Scoped subscription ownership does not match");
  const verifier = createAppleVerifier(process.env);
  const gateway = createAppleGateway(process.env, verifier);
  const client = new AppStoreServerAPIClient(process.env.APP_STORE_PRIVATE_KEY, process.env.APP_STORE_KEY_ID,
    process.env.APP_STORE_ISSUER_ID, "build.kanna.app", environment === "production" ? Environment.PRODUCTION : Environment.SANDBOX);
  const response = await client.getTransactionInfo(originalId);
  const tx = await verifier.transaction(response.signedTransactionInfo);
  if (tx.originalId !== originalId || tx.token !== record.token || tx.environment !== environment) throw new Error("Provider identity mismatch");
  let replayed = 0;
  for await (const signedPayload of gateway.notificationHistory(environment, originalId, Date.now() - days * 86_400_000, Date.now())) {
    const notification = await verifier.notification(signedPayload);
    if (!notification.evidence) continue;
    const transaction = notification.evidence.transaction;
    if (transaction.originalId !== originalId || transaction.token !== record.token || transaction.environment !== environment) throw new Error("History scope mismatch");
    if (apply) await applyAppleEvidence(db, uid, [notification.evidence], { notification, now: new Date().toISOString() });
    replayed++;
  }
  const before = await readBillingState(db, uid);
  const current = await gateway.current(tx);
  if (apply) await applyAppleEvidence(db, uid, current, { expectedRevision: before.sources.app_store?.revision ?? 0, now: new Date().toISOString() });
  console.log(JSON.stringify({ provider: "app_store", environment, mode: apply ? "applied" : "dry-run", historyEvents: replayed,
    statuses: current.map(item => item.status), subscriptionSuffix: originalId.slice(-4) }));
} catch {
  console.error("Apple reconciliation failed. No signed payloads or credentials logged. Check scoped configuration/ownership/provider availability; rerun to refetch after a concurrent write.");
  process.exitCode = 1;
} finally { await deleteApp(app); }
