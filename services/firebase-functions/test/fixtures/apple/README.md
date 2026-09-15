# Synthetic Apple-verifier fixtures

These certificates and `leaf.key` are public, **test-only** ES256 signing
material generated for this repository on 2026-09-15, valid for ten years. They
are not Apple certificates, accounts or credentials. The intermediate and leaf
include the Apple verifier's expected certificate-purpose OIDs so tests exercise
the real certificate-chain and JWS verification code.

`test/support/appleFixtures.ts` supplies the synthetic root explicitly with
online checks disabled. The production factory has no configuration switch for
this trust and rejects the signer. `firebase.json` excludes `test` from upload;
the source build excludes tests. Only the real public Apple roots in `resources`
are copied into the deployed artifact.

If these expire, generate a new P-256 root/intermediate/leaf under the worktree's
`.tmp`, preserving CA constraints and the purpose OIDs in the existing
certificates, and replace this complete fixture chain together. Never substitute
real In-App Purchase private keys here.
