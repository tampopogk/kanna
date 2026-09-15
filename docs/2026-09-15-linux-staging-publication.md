# Linux staging publication — 2026-09-15

**Published:** [linux-v0.2.0-staging.2](https://github.com/tampopogk/kanna/releases/tag/linux-v0.2.0-staging.2),
channel `desktop-linux-staging`, apt suite `staging`, package
`kanna-staging` version `0.2.0~staging.2-1`.

Product source: `a9df2f1d46fb08bcf53200b738e0fdc3c127b636`, tree
`3415e82f8a273b6ff94494d367139038eb026a62`. The original prepared packages
were retained and published without rebuilding. Execution controller:
`5c1ca1b3b904d364bb231d591daed63911200a68`.

## Downloads

| Architecture | Verified package | SHA256 |
|---|---|---|
| amd64 / x86_64 | [44,493,092 bytes](https://apt.kanna.build/pool/main/k/kanna-staging/kanna-staging_0.2.0~staging.2-1_amd64.deb) | `2dca964a748a2058c7b52fc453f6907788ba03e8e02f26836dfd8ee830aacee4` |
| arm64 | [43,811,392 bytes](https://apt.kanna.build/pool/main/k/kanna-staging/kanna-staging_0.2.0~staging.2-1_arm64.deb) | `ab5dec3dc5b4d6902449dad490a8d38bd3a1fa6d759682b7963c7dc4ea077c3d` |

On Ubuntu24.04, download the matching package and install it with apt so its
runtime dependencies are resolved. For example, from the download directory:

```sh
# amd64 / x86_64
sudo apt install ./kanna-staging_0.2.0~staging.2-1_amd64.deb
# arm64 (use this instead on an arm64 machine)
sudo apt install ./kanna-staging_0.2.0~staging.2-1_arm64.deb
```

Launch **Kanna Staging**. These are staging downloads; do not label them a
production release. Both Ubuntu24.04 architecture lanes passed the exact A→B
installed/upgrade pair, 17/17 each. Live cross-machine system acceptance for
this exact product candidate is still missing. C dev fixtures are separate.

## Public archive identity and verification

- [Public signing key](https://apt.kanna.build/keys/kanna-archive.asc), fingerprint
  `4e76a6afcdb83a5050daebb364bbc361a9d7ec7a`.
- Key SHA256 `0b11597589d7cc2b8523991f554384226bfa3948e629527838464cbd29fd1880`.
- [Candidate provenance](https://apt.kanna.build/linux/releases/linux-v0.2.0-staging.2/candidate.json),
  SHA256 `53d885b28afaf42dc223ea0946c5b1e3f14e5ecb6891b781824d25aeeafbdb46`.
- [Publication receipt](https://apt.kanna.build/linux/releases/linux-v0.2.0-staging.2/publication.json),
  SHA256 `9160e01c4408ca81b00723d1672f96891ffdad65346c1fa10439ab01fe9d8ae9`.
- [Signed apt metadata](https://apt.kanna.build/dists/staging/InRelease),
  SHA256 `1cd85f57feacb12a225978f72c4308d9c132919cc63b05706b46081525f3d29d`.

Canonical publish verified the signed public artifact/index/report closure.
Post-status confirmed staging points to the new tag with no pending transaction.
Ship independently retrieved and hash-checked candidate, receipt, key and
InRelease over HTTPS. Public evidence is retained under
[evidence](evidence/2026-09-15-linux-bootstrap/publication-preparation/published/).
No private signing or SSH key left the trusted MBP.

## Soak and remaining work

Verified publication: **2026-09-15T15:28:43.942Z**. The full24h soak completes
no earlier than **2026-09-16T15:28:43.942Z**, independently of desktop staging.21.
Production remains blocked by missing system acceptance and the incomplete
soak; no promotion or override was requested or performed.

Current apt metadata is valid until **2026-09-22T15:28:14Z**. If this candidate
remains served, use the documented explicit canonical metadata-renewal procedure
before expiry; no renewal scheduler was provisioned or assumed.

Website76e93c62 completed the Linux staging deployment: PR9, commit89dd4e6,
Pages run34989482384, with live HTTP/layout checks reported complete by root.
Structured
[website handoff](evidence/2026-09-15-linux-bootstrap/publication-preparation/kanna-web-handoff.json)
contains URLs, sizes, all hashes, receipt and limitations. Ship remains open.
One publication mobile notification was accepted (1 accepted, 0 failed).

## Execution and preserved limitation

The MBP ran canonical `./kd release ship --platform linux --staging --branch main`
with the retained B manifest, `--source-ref` and `--promotion-base` both pinned
to the product SHA above, `--staging-iteration 2`, committed B acceptance and
`--release`, then `./kd release status --platform linux --acceptance ...`.

Authorization: **“go ahead with your recommendations for staging release,
linux apt hosting etc.”**

The first setup apply changed only the approved archive/Caddy scope, then failed
with `fetch failed`. Its exact fetch phase/cause and raw first-drain socket
observations were not returned. These limitations remain recorded. Read-only
reconciliation verified original relay identity/image/start/env, restored public
health and matching apt key. A fresh canonical no-proxy-change recovery reused
keys and passed HTTPS/RPC before installing Linux selectors. No second network
interruption occurred. See the [setup execution record](2026-09-15-linux-staging-setup-execution.md).

## Subsequent acceptance reconciliation

Root reports final acceptance641dbb6f for **C / desktop staging.21 FAILED**:
same-account sign-in passed, but zero transfers completed. Forced cloud push
returned `source-peer-not-found`, pull returned404, and cross-host transfer mDNS
was absent. Both isolated allocations were cleaned. Root assigned targeted
transfer repair440c7635 and recovery-prompt repair110cdc20; dev-restart task
a031920d is separate.

These C findings are not Linux B system acceptance. B remains pinned to the
published a9df2f1d source and retains only its recorded installed/upgrade evidence.
Elapsed24h alone cannot satisfy missing system acceptance or authorize production.
No production promotion, repeat publication or additional mobile notification
was performed. Ship remains open for subsequent explicitly coordinated work.
