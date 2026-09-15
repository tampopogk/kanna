import { describe, expect, it, vi } from "vitest";
import { SignedDataVerifier, VerificationException, VerificationStatus } from "@apple/app-store-server-library";
import { appleEnv, appleJws, fixtureVerifier, notificationJws, renewalClaims, transactionClaims } from "./support/appleFixtures.js";
import { appleRoots, createAppleVerifier } from "../src/billing/appStoreVerification.js";
import { appleSource, shouldApplyApple } from "../src/billing/appStoreEvents.js";

describe("Apple trust and signed state", () => {
  it("verifies an actual certificate chain and both nested signatures", async () => {
    const result = await fixtureVerifier().notification(notificationJws(transactionClaims(), renewalClaims()));
    expect(result.evidence?.transaction).toMatchObject({ environment: "sandbox", originalId: "100000001" });
    expect(result.evidence?.renewal.autoRenew).toBe(true);
  });
  it("bundles three Apple roots and never trusts the fixture signer in production", async () => {
    expect(appleRoots()).toHaveLength(3);
    await expect(createAppleVerifier(appleEnv).transaction(appleJws(transactionClaims()))).rejects.toMatchObject({ reason: "invalid_apple_transaction" });
  });
  it("keeps certificate-status network failures retryable instead of rejecting the purchase permanently", async () => {
    const failure = vi.spyOn(SignedDataVerifier.prototype, "verifyAndDecodeTransaction")
      .mockRejectedValueOnce(new VerificationException(VerificationStatus.RETRYABLE_VERIFICATION_FAILURE));
    try {
      await expect(fixtureVerifier().transaction(appleJws(transactionClaims()))).rejects.toMatchObject({ reason: "apple_retry_required" });
    } finally { failure.mockRestore(); }
  });
  it.each([
    { bundleId: "other.app" }, { productId: "annual" }, { subscriptionGroupIdentifier: "foreign" },
    { environment: "Xcode" }, { appAccountToken: undefined }, { appAccountToken: "foreign" },
    { inAppOwnershipType: "FAMILY_SHARED" }, { originalTransactionId: "../../path" },
  ])("rejects wrong claims %j", async overrides => {
    await expect(fixtureVerifier().transaction(appleJws(transactionClaims(undefined, overrides)))).rejects.toMatchObject({ reason: "invalid_apple_transaction" });
  });
  it("rejects tampering, foreign renewal identity and mixed environments", async () => {
    const verifier = fixtureVerifier();
    const tx = appleJws(transactionClaims());
    await expect(verifier.transaction(tx.slice(0, -5) + "aaaaa")).rejects.toThrow();
    for (const changes of [{ originalTransactionId: "99" }, { environment: "Production" }, { productId: "other" }]) {
      await expect(verifier.notification(notificationJws(transactionClaims(), renewalClaims(changes)))).rejects.toThrow();
    }
  });
  it("does not grant on TEST or unknown notifications", async () => {
    for (const notificationType of ["TEST", "NEW_UNKNOWN_TYPE"]) {
      expect((await fixtureVerifier().notification(notificationJws(transactionClaims(), renewalClaims(), { notificationType }))).evidence).toBeNull();
    }
  });
  it("accepts a verified sandbox notification without a production numeric app id", async () => {
    const result = await fixtureVerifier().notification(appleJws({ notificationType: "TEST",
      notificationUUID: "22222222-2222-4222-8222-222222222222", signedDate: Date.now(),
      data: { bundleId: "build.kanna.app", environment: "Sandbox" } }));
    expect(result).toMatchObject({ environment: "sandbox", evidence: null });
  });
  it("renewal intent cannot activate an expired period; equal-date replay cannot undo revocation", async () => {
    const verifier = fixtureVerifier();
    const pair = await verifier.pair(appleJws(transactionClaims()), appleJws(renewalClaims()), "sandbox");
    const evidence = { ...pair, status: 1, signedDate: Date.now() };
    const active = appleSource(evidence, "uid", new Date().toISOString(), null);
    const revoked = { ...active, status: "revoked" as const };
    expect(shouldApplyApple(revoked, active)).toBe(false);
    expect(shouldApplyApple(active, revoked)).toBe(true);
    expect(appleSource({ ...evidence, transaction: { ...evidence.transaction, expiresDate: Date.now() - 1000 } }, "uid", new Date().toISOString(), null).status).toBe("expired");
    const ended = { ...evidence, status: 2, transaction: { ...evidence.transaction, expiresDate: Date.now() - 1000 } };
    expect(appleSource(ended, "uid", new Date().toISOString(), null).paymentOutstanding).toBe(false);
    expect(appleSource({ ...ended, status: 3, renewal: { ...ended.renewal, autoRenew: false, billingRetry: false } }, "uid", new Date().toISOString(), null).paymentOutstanding).toBe(true);
    expect(shouldApplyApple(revoked, { ...active, transactionSignedDate: active.transactionSignedDate + 100,
      renewalSignedDate: active.renewalSignedDate + 100, evidenceSignedDate: active.evidenceSignedDate + 100 })).toBe(true);
  });
});
