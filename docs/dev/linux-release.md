# Linux release lifecycle

`kd release prepare|status|ship|renew|promote --platform linux` owns Linux only:
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
| `KANNA_LINUX_ARCHIVE_BACKEND` | `filesystem` or opt-in `ssh` |
| `KANNA_LINUX_ARCHIVE_ROOT` | Absolute path to an existing dedicated POSIX archive directory on the selected storage host, without symlink components |
| `KANNA_LINUX_ARCHIVE_BASE_URL` | Public HTTPS URL serving that directory, without credentials, query or fragment |
| `KANNA_LINUX_ARCHIVE_VALID_HOURS` | Explicit positive validity interval for apt metadata |
| `KANNA_LINUX_APT_PUBLIC_KEY_PATH` | Absolute path to the armored public key |
| `KANNA_LINUX_APT_FINGERPRINT` | Full 40-hex primary fingerprint, pinned independently |
| `KANNA_LINUX_APT_PRIVATE_KEY_PATH` | Absolute path to the owner-only private key on the trusted release host; required to publish |
| `KANNA_LINUX_APT_PASSPHRASE_PATH` | Optional absolute owner-only file for an encrypted key's passphrase; one terminal newline is removed |

The adapter needs `/usr/bin/python3` on the storage host for `fcntl.flock` and
fd-relative filesystem operations. It holds one archive-wide kernel lock,
including across suites, signing, public readback and GitHub projection. The
helper owns both the lock and writes. EOF releases the lock when the publisher
dies; the lock file itself is never removed or timed out. Contention is an
explicit refusal, not a polling/retry loop. Writes use fsynced temporary files,
hard-link create-if-absent, atomic rename and directory fsync. Reads use actual
stored bytes. Symlinks, traversal and lost ownership are refused.

Both backends require a dedicated **local POSIX filesystem on the storage
host**: not NFS, FUSE, an object-store mount, SSHFS or an rsync mirror. The local
backend runs the helper directly. The SSH backend sends that **same helper**
over one SSH exec channel; the remote process owns the lock and every read/write
through signing, public readback and GitHub projection. Signing remains local.
Transport failure poisons the scope, including its final ownership fence; a new
explicit invocation acquires fresh ownership and resumes durable state. No
connection pooling, secondary upload session or host-key discovery is used.

SSH additionally requires these explicit selectors (no defaults):

| Selector | Value |
| --- | --- |
| `KANNA_LINUX_SSH_HOST` | Direct hostname or IPv4 address of the storage host |
| `KANNA_LINUX_SSH_USER` | Dedicated publisher account |
| `KANNA_LINUX_SSH_PORT` | SSH port, 1–65535 |
| `KANNA_LINUX_SSH_KNOWN_HOSTS_PATH` | Absolute owner-only regular file containing the independently pinned host key |
| `KANNA_LINUX_SSH_IDENTITY_PATH` | Absolute owner-only regular file for the explicitly selected SSH identity |

The client uses `/usr/bin/ssh`, no user SSH config, strict host-key checking,
only the selected known-hosts file/identity, batch authentication, no agent or
port forwarding, and no multiplexed connection. Connection establishment and
unresponsive transports have bounded SSH timeouts. Host pin failure is a refusal,
never a prompt or a reason to disable checking. SSH transport credentials are
separate from apt signing keys. Only signed metadata, artifact bytes and storage
operations cross the SSH channel.

The implementation creates no archive directory, server, account, credentials,
DNS or billing resource. There is no implicit reuse of mobile Firebase/GCS
storage. See [the proposed deployment runbook](../2026-09-15-linux-release-operations-proposal.md)
for the concrete topology awaiting later approval.

The existing pinned OpenPGP adapter requires v4 RSA >=3072 bits and SHA512,
verifies the selected public/private fingerprint pair, and refuses expired,
revoked, mismatched or malformed signatures/metadata. Real keys remain on the
trusted release host; builders receive no private key material.

## Preparation and publication

### Local exact-ref preparation (no archive or key configuration)

```sh
./kd release prepare --platform linux --ref <40-hex-commit> --staging-iteration N
# Optional: --out-dir <new-directory>
```

Current kd fetches only the selected local commit into a private detached Git
repository under the calling worktree's `.tmp/`. It never changes the calling
branch, index or files; dirty controller files cannot enter the build source.
It pins and checks revision/tree/cleanliness before and after the existing
collector. Both architecture builds use batch Bazel, leaving no build daemon
attached to the removed checkout. The source's own graph, package action and
runtime policy own all build inputs. Preparation loads no release environment
file, archive configuration, keys, public URL or remote-tip/lineage gates.

The default result directory is `.build/linux-prepared/<sha>-staging.N/`:
both canonical debs, their measured `.deb.json` reports, and `manifest.json`
with source revision/tree, committed version, iteration, both artifact/report
hashes and sizes. Outputs are flushed; the manifest is the atomic completion
marker. An existing output directory is refused, never overwritten. An interrupted
collection without a manifest is incomplete; keep it for diagnosis and use a new
output directory. The isolated source is removed on success or failure.
Preparation is evidence collection, not acceptance, publication or a soak start.
Ship's publication path retains its configuration, key, lock and remote-tip gates.

Historical compatibility is explicit and conservative: sources missing the
known graph/report stamp support fail **before building**. In particular,
`79f1c8225f0302d1e8d59dacedc5c148a9438272` is product-capable but unsupported
as an exact stamped release source. The stamp-only difference from that commit
to `188d118eea93cc99b8355f4f3ce5ecb1db34477d` is confined to
`packaging/linux/products.bzl` (two manifest fields) and
`tools/kd/src/runtime/linux-bazel-package.ts` (interface/report fields).
A defensible predecessor path is a separately reviewed commit adding exactly
that support to the historical product baseline, then preparing its **new SHA**.
No checkout patching, historical-SHA stamping of modified source, renamed package
or import of unstamped CI packages is supported. Ship selects the final genuine
A/B pair and runs both installed-upgrade lanes. Earlier CI evidence retains its
original source identity; no exact-ref native result is claimed by these tests.

### Configured publication

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

#### Retained prepared packages and an immutable product base

When the controller or main advances after collection, explicitly select the
retained product instead of rebuilding it:

```sh
./kd release ship --platform linux --staging --branch main \
  --prepared-manifest /absolute/prepared/manifest.json \
  --source-ref <40-hex-product-commit> --promotion-base <same-40-hex-commit> \
  --staging-iteration N --acceptance /absolute/acceptance.json --dry-run
```

All three selectors and the iteration are required together; `--skip-build`
and promotion selectors are refused in this mode. The controller must be clean.
The manifest must name both architectures with the same source/tree, committed
product VERSION and iteration. Canonical verification reads the actual retained
debs and reports, verifies their hashes and source stamps, inspects control/data
and executable bytes, and applies the **product commit's** runtime policy from
an isolated source snapshot. No Bazel invocation or cache is needed. Files must
be regular files; manifest filenames cannot escape the preparation directory.
Acceptance still names the exact measured package hashes and retained evidence.
A dry-run still needs archive configuration but writes no candidate/channel.

The plan reports `controller` separately from `source`. Immutable candidate
provenance records `promotionBase: {kind: "commit", branch, revision}`, with
revision equal to the product source. This is a recorded commit selection, not
a new Git branch/tag operation. The selected remote branch must contain that
commit at rehearsal, publication, status and promotion; unreadable ancestry,
rewinds excluding the source, and divergence fail closed. Ordinary candidates
without this discriminator retain the exact-tip rule above. Existing Linux
lineage, branch freeze, production floor, ownership and soak gates still apply.
Retries must retain the complete promotion-base selection as well as artifacts.

After separately authorized publication, promotion automatically checks out the
recorded product commit with the current clean controller. It requires the full
tested-candidate soak and upgrade/system evidence, then rebuilds **production**
identity at that commit. Retained staging debs are never relabeled. Pinned
promotion refuses `--skip-build`; it has a fresh isolated build directory.
Changing the controller does not change or restart candidate soak; changing the
product requires a new candidate with new evidence and soak.

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

## Explicit metadata renewal

```sh
# Only after authorization to update this candidate's public apt metadata:
./kd release renew --platform linux --candidate linux-vX.Y.Z-staging.N --renewal 1 --valid-for-hours <hours>
# Stable metadata uses the exact production tag: --candidate linux-vX.Y.Z
```

`renew` signs locally and publishes new Date/Valid-Until metadata for the **same
candidate and packages**. The tag, source/tree, artifacts/reports, acceptance,
original candidate and original `publication.json` never change. GitHub still
projects that original receipt; an existing soak timestamp survives renewal and
every retry. No remote branch movement, native build, new version, channel switch
or production promotion is part of renewal. Public metadata updates require the
authorization for their target suite; a production renewal is a named-human
production operation.

Each explicit sequence stores immutable
`linux/releases/<tag>/renewals/<sequence>/renewal.json`, containing the record
and base64 clearsigned metadata together. The record binds the candidate digest,
previous signature, exact metadata being replaced, original receipt digest (or
null before any verified publication), date and selected lifetime. The signed
Release includes `X-Kanna-Renewal-SHA256`, authenticating that record. The journal
claims a `pendingRenewal` before changing canonical metadata. The helper atomically
replaces Release and then InRelease; local closure/signature verification and
public byte readback precede the immutable renewal `publication.json` receipt.
That receipt has its own first-observation timestamp; it does not replace soak.

Retry the **same sequence and validity** after interruption. An envelope whose
write succeeded before its acknowledgement/journal is recovered intact. A lost
public readback leaves pending state and blocks ship/promote or another candidate.
The same sequence never gets a new date or signature. If its metadata expired,
explicitly request the next sequence; it retains the earlier signed history and
can supersede pending metadata. Unchanged artifact closure and exact live metadata
are checked first; corruption or an out-of-band change is refused. The latest
metadata must verify at the current time. Expired historical signatures are
verified against their original authenticated contents within their own validity
interval solely to establish history, never to make expired live metadata usable.

A pending initial publication with a cached signed commit and complete uploaded
closure can be renewed, including after expiration. If no original verified-public
receipt exists, its first receipt names the renewed signature and the durable first
verified public observation; it never backdates availability to the original
preparation/signature date. If the original transaction has not even produced its
cached signed commit or complete closure, renewal refuses: it cannot substitute
for the original artifact collection/publication gates. Finish that original
publication while valid; an expired unsigned/incomplete intent requires explicit
repair through the original verified publication inputs, not hand-edited archive
state or renewal of unverified inputs.

No validity interval or renewal schedule is a production default. The command
requires explicit hours and sequence. Ship's proposal of **168 hours validity /
48 hours renewal** remains an operational proposal, with operator availability,
expiry monitoring and backup custody still needing approval. No public scheduler
is installed or invoked. Status reports expired metadata and pending renewal as
promotion blockers; renewal never waives the full tested-candidate soak.

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
prerequisite jobs passed. The owner-authorized follow-up in this task corrects
that graph defect: both `kanna_task_transfer` and
`kanna_task_transfer_x86_64` now declare `runtime-defaults` directly, matching
the direct use in `src/main.rs`. The library already had its dependency. No
product behavior changes. The existing path-dependency guard now covers the
case where both binary variants omit a dependency that only their library had.

Local correction verification: `bazel --batch build -c opt
//crates/task-transfer:kanna_task_transfer
//crates/task-transfer:kanna_task_transfer_x86_64` completed successfully on
Darwin ARM64, covering both executable variants; the batch process exited.
The four focused `Bazel workspace path dependencies` tests also pass.

The first correction verification run, `34940014222` at `a6e3e11d0`, exposed
an apt bundle smoke-test assumption before native builds finished: the signer
now shares OpenPGP code with kd, so its license/source map live in a reachable
chunk rather than next to the entry module. The smoke check now follows its
already-verified emitted dependency closure for those artifacts. Local isolated
bundle sign/verify, tamper/expiry, closure/license/source checks and disposable
filesystem fixture preparation pass. That run was cancelled and superseded by
the final full native verification, not counted as native acceptance.

The task's durable completion result records the exact corrected commit and its
actual **Linux Release Check** run outcome, including both native builds and
install-only jobs. Require that corrected-source result; neither the failed
PR-head run nor older native run `34930914452` (PASS only for
`79f1c8225f0302d1e8d59dacedc5c148a9438272`) establishes current-head success.
This follow-up preserves the earlier 149 lifecycle tests/typecheck evidence;
only the affected graph/parity checks and required native workflow are rerun.

Ship `d3ce8dec` still needs verified exact release-candidate artifacts, real archive
configuration/public serving topology, protected keys and public fingerprint /
bootstrap URL, explicit metadata validity and renewal procedure, the chosen
initial version/series, genuine two-version installed upgrade reports, exact
system acceptance from `641dbb6f`, staging publication authorization, and the
resulting full soak plus named-human production request. These gaps are not
closed by the implementation tests. No archive provisioning, real publication,
key changes, release reset/promotion, website link or billing change was made.
