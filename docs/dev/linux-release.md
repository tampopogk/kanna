# Linux release lifecycle

`kd release status|ship|promote --platform linux` owns Linux only:
`desktop-linux-staging` / `desktop-linux`, `linux-vX.Y.Z[-staging.N]`,
`release/linux/X.Y`, and apt suites `staging` / `stable`. Omitting the platform
still selects macOS. Unsupported platforms and Linux cut, reset, rollback,
bump, single-architecture and soak-override selectors are refused.

This is the lifecycle/storage implementation, not release authorization or a
claim that a public Linux package exists. The repository Ship runbook owns
execution and human authorization. No public archive was provisioned by this
implementation.

## Configuration

Use the existing owner-only `~/.kanna/.env.release.local` loader. These are
**selectors**, not secret values. No default backend, location, key, URL or
metadata lifetime is chosen for the owner:

| Selector | Required value |
| --- | --- |
| `KANNA_LINUX_ARCHIVE_BACKEND` | `filesystem` |
| `KANNA_LINUX_ARCHIVE_ROOT` | Absolute path to an existing dedicated local POSIX archive directory, without symlink components |
| `KANNA_LINUX_ARCHIVE_BASE_URL` | Public HTTPS URL serving that directory, without credentials, query or fragment |
| `KANNA_LINUX_ARCHIVE_VALID_HOURS` | Explicit positive validity interval for apt metadata |
| `KANNA_LINUX_APT_PUBLIC_KEY_PATH` | Absolute path to the armored public key |
| `KANNA_LINUX_APT_FINGERPRINT` | Full 40-hex primary fingerprint, pinned independently |
| `KANNA_LINUX_APT_PRIVATE_KEY_PATH` | Absolute path to the owner-only private key on the trusted release host; required to publish |
| `KANNA_LINUX_APT_PASSPHRASE_PATH` | Optional absolute owner-only file for an encrypted key's passphrase; one terminal newline is removed |

The adapter needs `/usr/bin/python3` on the release host for `fcntl.flock` and
fd-relative filesystem operations. It holds one archive-wide kernel lock,
including across suites, signing, public readback and GitHub projection. The
helper owns both the lock and writes. EOF releases the lock when the publisher
dies; the lock file itself is never removed or timed out. Contention is an
explicit refusal, not a polling/retry loop. Writes use fsynced temporary files,
hard-link create-if-absent, atomic rename and directory fsync. Reads use actual
stored bytes. Symlinks, traversal and lost ownership are refused.

Only a dedicated **local POSIX filesystem** is supported: not NFS, FUSE, an
object-store mount, or an rsync mirror. The owner must choose a serving topology
where that same archive is available from the trusted release host and public
URL. The implementation creates no directory, server, bucket, credentials or
billing resource. A different storage backend must implement the existing
`AptPublicationStorage` contract and demonstrate its atomic ownership semantics.
There is no implicit reuse of mobile Firebase/GCS storage.

The existing pinned OpenPGP adapter requires v4 RSA >=3072 bits and SHA512,
verifies the selected public/private fingerprint pair, and refuses expired,
revoked, mismatched or malformed signatures/metadata. Real keys remain on the
trusted release host; builders receive no private key material.

## Preparation and publication

```sh
./kd release status --platform linux
./kd release ship --platform linux --staging --dry-run --staging-iteration 1
# After exact installed evidence and explicit staging publication authorization:
./kd release ship --platform linux --staging --release --staging-iteration 1 --skip-build --acceptance /absolute/acceptance.json
./kd release status --platform linux --acceptance /absolute/acceptance.json
# After named-human production authorization, full soak, upgrade/system evidence:
./kd release promote X.Y.Z-staging.N --platform linux --acceptance /absolute/acceptance.json
```

A clean committed `VERSION` owns X.Y.Z; there is no release-time version-file
rewrite or renamed package. `--branch` defaults to `main`; either main or the
matching `release/linux/X.Y` must have exactly the local HEAD as its remote tip.
The source is checked before/after collection and again before publication.

The collector runs the established Bazel package lane for **both** x86_64 and
arm64. It supplies validated revision/tree build stamps to the declared package
report; checks the report's source/version/iteration/channel/architecture and
hash; inspects the real Debian ar/control/data bytes and all eight packaged
executable hashes; and applies the runtime policy. A missing stamp, Cargo
prototype, audit override, changed bytes, mixed source, stale output or renamed
package fails. `--skip-build` is Bazel's up-to-date check under the same stamps
and iteration, not permission to consume an arbitrary output directory.
The stamp records build provenance, not a claim of cross-host reproducibility.

New staging iterations advance Linux tags and archive state only. Existing
candidate inputs are immutable. Staging cannot regress or diverge from its own
candidate; an unpromoted release/linux branch freezes main. Production floors
come only from Linux tags/provenance. Release-series abandonment is checked in
Linux's namespace. This implementation provides no operation to waive those
rules or rewrite a Linux series.

Dry-run/build-only collects the packages and reports `publication.allowed` and
blockers. It writes no candidate/signature/channel objects. It needs explicit
archive configuration, but missing acceptance or private-key selection is
reported without pretending a build grants publication authority.

## Acceptance contract

`--acceptance` reads a strict JSON attestation plus the named evidence files.
Every evidence file's actual SHA-256 must match. These are operator/acceptance
runner attestations, not an automated claim that a generic text file proves a
test passed. Ship must populate them from the real installed/system reports.
Evidence is copied into immutable `linux/evidence/<sha256>` objects. The original
attestation is retained in candidate provenance. Paths are local input locations;
archived evidence is addressed by digest.

```json
{
  "schemaVersion": 1,
  "sourceRevision": "<40 hex git commit>",
  "sourceTree": "<40 hex git tree>",
  "version": "X.Y.Z",
  "iteration": 1,
  "artifacts": { "x86_64": "<64 hex deb digest>", "arm64": "<64 hex deb digest>" },
  "checks": [
    {
      "kind": "installed",
      "architecture": "arm64",
      "status": "pass",
      "testedAt": "2026-09-15T00:00:00.000Z",
      "evidencePath": "/absolute/installed-report.json",
      "evidenceSha256": "<64 hex report digest>"
    }
  ]
}
```

The example is intentionally incomplete, not passing evidence. Staging requires
exactly one `installed` check per architecture. Production additionally requires
one `upgrade` per architecture and one `system` check with architecture `both`.
An upgrade check adds `predecessor: {sourceRevision, version, sha256}` with a
*different* product source, different package hash and older canonical Debian
staging version. Ship owns constructing and actually testing that genuine pair;
metadata checks cannot establish that a predecessor was product-capable.
`system` evidence must cover task `641dbb6f`'s cross-machine/transfer acceptance.

The envelope's source/tree/version/iteration and both hashes must match the
active staging candidate exactly. Future-dated, missing, duplicate or stale
checks block promotion. New upgrade/system evidence can be supplied to status
and promotion after staging; it does not modify the staging candidate or reset
its clock. Promotion retains that complete attestation alongside production
artifacts, linked through `promotedFrom` to the exact tested staging candidate.
No install-only CI result or macOS candidate satisfies these checks.

## Commit point and interruption recovery

Archive files:

- `pool/.../*.deb`: immutable package bytes.
- `dists/{staging,stable}/main/binary-*/by-hash/SHA256/*`: immutable indexes.
- `linux/releases/<linux-tag>/candidate.json`: immutable source/build/artifact,
  acceptance, promotion-base, previous-candidate and configuration provenance.
- Adjacent `<architecture>.report.json` and `InRelease`: immutable measured
  reports and the exact signed commit bytes for retries.
- Adjacent `publication.json`: immutable candidate/signature digests plus the
  **first verified public publication time**.
- `linux/state.json`: atomically replaced active staging/production and pending
  transaction tags.

The journal claims a candidate before upload. Pool objects and by-hash indexes
precede canonical indexes; signed `dists/<suite>/InRelease` is the apt commit
point and is replaced last. Readback checks the signature and entire stored
closure. The configured public HTTPS endpoint must then serve the exact
InRelease, candidate, reports, by-hash indexes and deb bytes before a receipt is
recorded. A local upload alone starts no soak.

GitHub immutable version releases and Linux pointer release bodies project the
archive provenance, receipt and actual archive URLs. They are always
`--latest=false`, leaving macOS latest-release behavior alone. The immutable
tag's source and both release bodies are read back. A projection failure leaves
a durable pending transaction; rerun the same command to finish it. No other
candidate may overtake pending work. Different source/artifacts/evidence/config,
immutable object collisions or an out-of-band InRelease move are refused.

An existing receipt's timestamp survives every retry. If interruption happened
after apt commit but before any durable verified-public receipt, earlier public
availability is unproven: recovery conservatively records its first verified
observation. It does not backdate soak to package creation or signature time.
Status uses unrounded elapsed time and requires at least **24 hours** for Linux,
even if the macOS policy is zero. It reports every known blocker, including
pending state, projection/readback errors, moved base, lineage, abandoned series,
keys, acceptance and soak. Promotion builds production identity at the exact
soaked commit/tree; staging package bytes are never relabelled as `kanna`.

`Valid-Until` expiry fails closed. This slice implements no renewal command and
chooses no renewal schedule/owner. A pending transaction whose signed metadata
expired cannot silently acquire a new date or signature. Before real publication,
Ship must obtain an explicit validity/renewal operational plan; implementing a
renewal procedure must preserve the candidate's receipt/soak identity.

## Evidence and remaining needs (task e26eaa13)

Focused tests exercise real disposable filesystem storage and disposable RSA
keys, crash/ownership loss, immutable collision, InRelease interruption before
and after replacement, projection retry, public-readback failure, exact soak,
stale/missing acceptance, Linux/macOS isolation, source stamps and actual Debian
payload verification. The apt interoperability runner now uses this concrete
adapter; local `--prepare-only` validates stored/signature bytes, not Ubuntu apt
or installed behavior. No new native app build or two-version pair is claimed.

Verification executed in this worktree:

- `pnpm --dir tools/kd typecheck`: pass.
- Seven focused release/storage/lineage/registry suites: **149 tests pass**.
- `pnpm --dir tools/kd build --out-dir ../../.tmp/apt-adapter-bundle`: pass;
  `pnpm --dir tools/kd exec tsx tests/linux-apt-interop.ts --prepare-only`: pass
  on Darwin ARM64 with the emitted signer and concrete filesystem adapter.
- `./kd release status --platform linux`: reports only Linux channels,
  `promotion.allowed: false`, and missing Linux configuration; no macOS preflight.
- Existing broad CLI test `tests/cli.test.ts:1725` still fails on the unrelated
  cloud deploy expectation omitting `dryRun: false`, already recorded in the
  product/package handoff. The dedicated Linux CLI/schema/registry tests pass.

PR-head native CI [34935441089](https://github.com/tampopogk/kanna/actions/runs/34935441089)
finished **failure**, exact head `5b3d5027096db9be3947877794da57fba4ef7b97`:
both architectures fail compiling `kanna_task_transfer` at
`crates/task-transfer/BUILD.bazel:29` with E0433, unresolved
`kanna_runtime_defaults`. Installed checks are skipped. Apt interop and
prerequisite jobs passed. This is an independent product graph defect, left
outside this lifecycle implementation. Earlier native run `34930914452` remains
PASS only for source `79f1c8225f0302d1e8d59dacedc5c148a9438272`.

Ship `d3ce8dec` still needs the corrected final product build, real archive
configuration/public serving topology, protected keys and public fingerprint /
bootstrap URL, explicit metadata validity and renewal procedure, the chosen
initial version/series, genuine two-version installed upgrade reports, exact
system acceptance from `641dbb6f`, staging publication authorization, and the
resulting full soak plus named-human production request. These gaps are not
closed by the implementation tests. No archive provisioning, real publication,
key changes, release reset/promotion, website link or billing change was made.
