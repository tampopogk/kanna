/** Explicit same-candidate metadata transactions. Renewal JSON is authenticated
 * by its digest inside the clearsigned Release; the original receipt is immutable. */
import type { AptPublicationStorage, AptPublicationSigner } from "./linux-apt-publication";
import { verifyAptRelease, type AptVerificationKey } from "./linux-apt-signature";
import { inReleasePath, releaseIndexPath } from "./linux-apt";
import { sha256 } from "./linux-release-artifacts";
import { archiveState, candidateRelease, immutable, jsonBytes, readJson, releasePath, statePath, verifyLinuxClosure, verifyLinuxPublication, type LinuxCandidate, type LinuxPublicationReceipt } from "./linux-release-state";

export interface LinuxRenewal {
  schemaVersion: 1; tag: string; sequence: number; candidateSha256: string;
  previousInReleaseSha256: string; replacesInReleaseSha256: string | null; publicationReceiptSha256: string | null;
  date: string; validForHours: number;
}
interface RenewalEnvelope { record: LinuxRenewal; signedRelease: string }
export const renewalPath = (tag: string, sequence: number, name: string) => {
  if (!Number.isSafeInteger(sequence) || sequence < 1) throw new Error("Invalid Linux renewal sequence.");
  return releasePath(tag, `renewals/${sequence}/${name}`);
};
function renewalRelease(c: LinuxCandidate, r: LinuxRenewal): Buffer {
  return Buffer.concat([candidateRelease({ ...c, preparedAt: r.date, validForHours: r.validForHours }), Buffer.from(`X-Kanna-Renewal-SHA256: ${sha256(jsonBytes(r))}\n`)]);
}
/** Historical signature checks authenticate archived records at the end of
 * their validity interval. Only the live head must be valid NOW. This path is
 * internal to explicit renewal; ordinary publication still rejects expiry. */
export async function linuxMetadataHistory(storage: AptPublicationStorage, c: LinuxCandidate, key: AptVerificationKey, now: Date, sequence: number, historical = false) {
  if (key.fingerprint.toLowerCase() !== c.fingerprint.toLowerCase()) throw new Error("Linux candidate key pin differs from configuration.");
  const receipt = await readJson<LinuxPublicationReceipt>(storage, releasePath(c.tag, "publication.json"));
  const hashes: string[] = [];
  let signed: Uint8Array | null = null;
  let release = candidateRelease(c);
  let date = c.preparedAt;
  let hours = c.validForHours;
  for (let n = 0; n <= sequence; n++) {
    if (n > 0) {
      const envelope = await readJson<RenewalEnvelope>(storage, renewalPath(c.tag, n, "renewal.json"));
      const r = envelope?.record;
      if (!r || r.schemaVersion !== 1 || r.tag !== c.tag || r.sequence !== n || r.candidateSha256 !== sha256(jsonBytes(c)) || r.previousInReleaseSha256 !== hashes[n - 1] || !Number.isFinite(Date.parse(r.date)) || Date.parse(r.date) < Date.parse(date) || !Number.isFinite(r.validForHours) || r.validForHours <= 0 || (r.publicationReceiptSha256 !== null && (!receipt || r.publicationReceiptSha256 !== sha256(jsonBytes(receipt))))) throw new Error("Invalid signed Linux renewal history.");
      date = r.date; hours = r.validForHours; release = renewalRelease(c, r);
    }
    signed = n === 0 ? await storage.read(releasePath(c.tag, "InRelease")) : Buffer.from((await readJson<RenewalEnvelope>(storage, renewalPath(c.tag, n, "renewal.json")))!.signedRelease, "base64");
    if (!signed) throw new Error("Missing archived Linux metadata signature; finish uploading the pending publication before renewal.");
    const expiry = new Date(date).getTime() + hours * 3_600_000;
    const at = n < sequence || historical ? new Date(Math.min(now.getTime(), expiry - 1000)) : now;
    await verifyAptRelease({ ...key, now: at, signedRelease: signed, expectedRelease: release });
    hashes.push(sha256(signed));
  }
  return { signed: signed!, release, hashes };
}
export async function verifyLinuxRenewalReceipt(storage: AptPublicationStorage, c: LinuxCandidate, sequence: number, now: Date, signature: Uint8Array): Promise<void> {
  if (!sequence) return;
  const envelope = await readJson<RenewalEnvelope>(storage, renewalPath(c.tag, sequence, "renewal.json"));
  const receipt = await readJson<{ tag: string; sequence: number; recordSha256: string; inReleaseSha256: string; verifiedAt: string }>(storage, renewalPath(c.tag, sequence, "publication.json"));
  if (!envelope || !receipt || receipt.tag !== c.tag || receipt.sequence !== sequence || receipt.recordSha256 !== sha256(jsonBytes(envelope.record)) || receipt.inReleaseSha256 !== sha256(signature) || !Number.isFinite(Date.parse(receipt.verifiedAt)) || Date.parse(receipt.verifiedAt) > now.getTime()) throw new Error("Missing or inconsistent Linux renewal publication receipt.");
}
export async function renewLinuxCandidate(input: {
  storage: AptPublicationStorage; candidate: LinuxCandidate; sequence: number; validForHours: number;
  signer: AptPublicationSigner; key: AptVerificationKey; now: () => Date;
  observeCommit: (c: LinuxCandidate, paths: string[]) => Promise<void>;
  project: (c: LinuxCandidate, r: LinuxPublicationReceipt) => Promise<void>;
}) {
  const { storage, candidate: c, sequence, validForHours } = input;
  renewalPath(c.tag, sequence, "renewal.json");
  if (!Number.isFinite(validForHours) || validForHours <= 0) throw new Error("Renewal requires explicit positive validity hours.");
  let state = await archiveState(storage);
  const slot = c.channel === "desktop-linux" ? "production" : "staging";
  if ((state[slot] !== c.tag && state.pending !== c.tag) || (state.pending && state.pending !== c.tag) || (state.pendingRenewal && state.pendingRenewal.tag !== c.tag)) throw new Error("Renewal must name the active or pending candidate; another transaction is pending.");
  const current = state.renewals?.[c.tag] ?? 0;
  let pending = state.pendingRenewal?.sequence ?? 0;
  // A lost acknowledgement immediately after the immutable envelope write must
  // be recovered too: adopt that exact envelope into the pending journal.
  const orphan = Math.max(current, pending) + 1;
  if (await storage.read(renewalPath(c.tag, orphan, "renewal.json"))) {
    pending = orphan;
    state = { ...state, pendingRenewal: { tag: c.tag, sequence: pending } };
    await storage.replace(statePath, jsonBytes(state));
  }
  const head = Math.max(current, pending);
  if (sequence !== head + 1 && sequence !== head) throw new Error(`Renewal must retry ${head || 1} or explicitly request sequence ${head + 1}.`);
  const receipt = await readJson<LinuxPublicationReceipt>(storage, releasePath(c.tag, "publication.json"));
  // Verify all stored dependencies before permitting even an expired signature
  // to be replaced. Renewal never manufactures a missing package or report.
  await verifyLinuxClosure(storage, c);
  const history = await linuxMetadataHistory(storage, c, input.key, input.now(), head, true);
  if (receipt && (receipt.tag !== c.tag || receipt.candidateSha256 !== sha256(jsonBytes(c)) || !history.hashes.includes(receipt.inReleaseSha256) || !Number.isFinite(Date.parse(receipt.verifiedAt)) || Date.parse(receipt.verifiedAt) > input.now().getTime())) throw new Error("Invalid original Linux publication receipt.");
  const live = await storage.read(inReleasePath(c.channel));
  const liveHash = live ? sha256(live) : null;
  const pendingRecord = pending ? (await readJson<RenewalEnvelope>(storage, renewalPath(c.tag, pending, "renewal.json")))!.record : null;
  const allowed = [history.hashes[head], ...(pendingRecord ? [pendingRecord.replacesInReleaseSha256] : []), ...(!receipt && head === 0 ? [c.previousInReleaseSha256] : [])];
  if (!allowed.includes(liveHash!)) throw new Error("Linux InRelease moved outside the renewal transaction.");
  const path = renewalPath(c.tag, sequence, "renewal.json");
  let envelope = await readJson<RenewalEnvelope>(storage, path);
  if (!envelope) {
    const record: LinuxRenewal = { schemaVersion: 1, tag: c.tag, sequence, candidateSha256: sha256(jsonBytes(c)), previousInReleaseSha256: history.hashes[head], replacesInReleaseSha256: liveHash, publicationReceiptSha256: receipt ? sha256(jsonBytes(receipt)) : null, date: input.now().toISOString(), validForHours };
    const signed = await input.signer.sign(renewalRelease(c, record));
    await verifyAptRelease({ ...input.key, now: input.now(), signedRelease: signed, expectedRelease: renewalRelease(c, record) });
    // Record and signature commit together, so interrupted signing cannot leave
    // an unsigned, expired intent that no future command can recover.
    envelope = { record, signedRelease: Buffer.from(signed).toString("base64") };
    await immutable(storage, path, jsonBytes(envelope));
  }
  const { record } = envelope;
  if (record.validForHours !== validForHours) throw new Error("Renewal retry validity differs from its immutable record.");
  const release = renewalRelease(c, record);
  const signature = Buffer.from(envelope.signedRelease, "base64");
  state = { ...state, pendingRenewal: { tag: c.tag, sequence } };
  await storage.replace(statePath, jsonBytes(state));
  // An expired pending renewal requires an explicit next sequence. Never give
  // a retry a fresh date/signature under the same record.
  await linuxMetadataHistory(storage, c, input.key, input.now(), sequence);
  await storage.replace(releaseIndexPath(c.channel), release);
  await storage.replace(inReleasePath(c.channel), signature);
  const committed = await storage.read(inReleasePath(c.channel));
  if (!committed || sha256(committed) !== sha256(signature)) throw new Error("Renewed InRelease readback differs.");
  await input.observeCommit(c, Array.from({ length: sequence }, (_, n) => renewalPath(c.tag, n + 1, "renewal.json")));
  const renewalReceiptPath = renewalPath(c.tag, sequence, "publication.json");
  const renewalReceipt = await readJson(storage, renewalReceiptPath) ?? { tag: c.tag, sequence, recordSha256: sha256(jsonBytes(record)), inReleaseSha256: sha256(signature), verifiedAt: input.now().toISOString() };
  const observed = renewalReceipt as { tag: string; sequence: number; recordSha256: string; inReleaseSha256: string; verifiedAt: string };
  if (observed.tag !== c.tag || observed.sequence !== sequence || observed.recordSha256 !== sha256(jsonBytes(record)) || observed.inReleaseSha256 !== sha256(signature) || !Number.isFinite(Date.parse(observed.verifiedAt)) || Date.parse(observed.verifiedAt) > input.now().getTime()) throw new Error("Invalid Linux renewal publication receipt.");
  await immutable(storage, renewalReceiptPath, jsonBytes(renewalReceipt));
  await verifyLinuxRenewalReceipt(storage, c, sequence, input.now(), signature);
  // Without an earlier verified public observation there was no soak. The
  // initial receipt names the signature first actually observed, even if renewed.
  const original = receipt ?? { tag: c.tag, candidateSha256: sha256(jsonBytes(c)), inReleaseSha256: sha256(signature), verifiedAt: observed.verifiedAt };
  await immutable(storage, releasePath(c.tag, "publication.json"), jsonBytes(original));
  state = { ...state, [slot]: c.tag, renewals: { ...state.renewals, [c.tag]: sequence } };
  await storage.replace(statePath, jsonBytes(state));
  await verifyLinuxPublication(storage, c, input.key, input.now());
  await input.project(c, original);
  await storage.replace(statePath, jsonBytes({ ...state, pendingRenewal: null, pending: null }));
  return { candidate: c.tag, renewal: record, receipt: original, renewalReceipt };
}
