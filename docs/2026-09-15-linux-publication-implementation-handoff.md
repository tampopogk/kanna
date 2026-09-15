# Next Linux slice: canonical candidate lifecycle and upgrade proof

Prepared by ship task `d3ce8dec` for root to route alongside product review.
Implementation only: no publication, provisioning, new tasks or builds requested
by this handoff.

## Starting point

Product candidate `f59c9138c99c6991848f7a555c73401cad95faae` is awaiting its
single review; implementation `79f1c8225f0302d1e8d59dacedc5c148a9438272`
passed CI `34930914452` on both architectures, including 11 install-only tests
each. Final delta is documentation/evidence only. Read its committed
`docs/2026-09-14-linux-product-package-boundary.md`; do not repeat that audit.
Integrate its reviewed/merged result before dependent implementation validation.
The older shipping handoff's missing-product/Cargo-path findings are superseded
by this candidate, not its remaining publication and upgrade gaps.

## Existing contracts — implement, do not reopen

- Route `--platform linux` through CLI/schema/registry to Linux status, ship and
  promote. Reuse lineage rules with the existing platform descriptor:
  `desktop-linux-staging` / `desktop-linux`, `linux-v…`, `release/linux/X.Y`;
  apt suites `staging` / `stable`. Linux never reads/writes macOS candidate state
  or enters notarization/updater signing. Reject unsupported selectors early.
- Require both x86_64/amd64 and arm64 from one candidate source/version. Consume
  declared Bazel deb/report outputs, validate actual bytes, package identity,
  audit and hashes; refuse prototype/overridden/stale or mixed artifacts.
  `VERSION` owns the build version; staging iteration is a separate Bazel
  setting. Never rename one package to claim another version.
- Record immutable candidate provenance: source and stamped build revision/tree,
  version/iteration/channel, per-architecture hashes and reports, acceptance
  evidence, promotion base and verified staging publication time. Retain exact
  inputs/timestamp on retry. Reconcile tag/provenance/channel interruptions
  without silently replacing a candidate or restarting its clock.
- Linux production requires its own tested candidate and full **24-hour soak**;
  expose `promotion.allowed` and all blockers. Promotion rebuilds the exact
  soaked source with production identity (`kanna`, distinct from
  `kanna-staging`) and verifies resulting artifacts. MacOS `.20` and changed
  source cannot supply Linux acceptance/soak. No override/reset is authorized.
- Reuse `linux-apt-publication.ts` and the pinned OpenPGP signer. Implement real
  storage's archive-wide exclusive ownership, atomic create-if-absent and
  replace, readback verification and ownership-loss refusal. Immutable pool and
  by-hash objects precede **InRelease last**. It is the apt commit point;
  GitHub channel/provenance projections must recover consistently with it.
  Real apt keys stay on the owner's trusted MacBook Pro, never builders/CI.

## Bounded implementation and proof

1. Wire the lifecycle above with explicit configuration and fail-closed missing
   configuration. Add focused tests for Linux/macOS isolation, artifact/source
   mismatch, soak and stale acceptance, interrupted publication and retry.
   Add platform-specific preflight and update Ship EXTEND to the actual path.
   Prove the concrete storage adapter with disposable storage/test keys before
   requesting real provisioning; synthetic in-memory tests alone are insufficient.
2. Produce a genuine predecessor/candidate pair from **distinct product-capable
   source revisions and ordered package versions**, each for both architectures.
   Preserve their source/build/hash manifests. Neither the old Cargo prototype,
   documentation-only revision, nor relabelling the current single package
   establishes a real cross-version predecessor. No historical release claim
   is needed for an unpublished bootstrap pair; label that limit explicitly.
3. Run `./kd test linux-installed --old-artifact <A.deb> --new-artifact <B.deb>
   --channel staging` on each matching isolated host. Assert live daemon/agent
   pid **and start time**, task/run/workspace continuity, and exactly-once
   post-upgrade input with durable records. Reuse the existing lane and fail on
   missing prerequisites. Preserve reports tied to the exact installed bytes.
4. Coordinate exact candidate identity with `641dbb6f` for vigorous system and
   cross-machine transfer tests. Install-only CI is not that acceptance.
   Deliver status/dry-run evidence and the remaining operational blockers to
   root; do not publish or claim release readiness from this implementation.

## Decisions/configuration still needed before real publication

- Owner-selected archive backend/location and public base URL, plus credentials
  and an atomic ownership mechanism meeting the existing storage contract.
  Do not implicitly reuse or change billing/production cloud resources.
- Protected apt key/passphrase source, public fingerprint pins and bootstrap
  URL; owner-run backup/rotation/revocation procedure. No secret material in chat.
- Archive `Valid-Until` duration and explicit renewal ownership/procedure;
  renewal must not reset candidate soak. Existing code takes a duration but
  does not establish this operational policy.
- Exact initial Linux series/version and channel publication authorization.
  Public production requires Jeremy's explicit named production request after
  candidate tests and soak. An implementation assignment grants none of these
  external mutations. Root selects the real predecessor/candidate revisions
  when routing acceptance; do not manufacture a version pair just to pass.

Final ship handoff to `d3ce8dec`/kanna-web: immutable release and deb URLs,
version/source, both SHA-256 values, audit/upgrade/system evidence, verified
channel state and soak, public key fingerprint and tested Signed-By
install/update instructions. Validation CI archives are not public downloads.

## Operational follow-up at corrected head `7f3dd4f35`

The preceding implementation brief is now historical: lifecycle/storage work
is implemented and in single review in `e26eaa13`. Per the supplied confirmed
result, CI `34940178572` passed all eight jobs at
`7f3dd4f35ce3633144fb74aaf9b3302a96560092`, including both native builds and
11 install-only tests per architecture. PR1509 CI `34935441089` failed; it is
not current acceptance. The new CI packages have blank source stamps and cannot
be reused as publishable collector inputs. No upgrade/system/soak is implied.
This follow-up reads that commit's `docs/dev/linux-release.md`; it performs no
new audit or build. Await reviewed merge before selecting the release source.

Concrete operational choices remaining:

| Choice | Implemented constraint / required next input |
| --- | --- |
| Archive location and public serving | Only `filesystem` backend is implemented. Choose an existing dedicated absolute local POSIX directory on the trusted release host, with no symlink components, and the public HTTPS URL serving **that same directory**. NFS/FUSE/object mounts and rsync mirrors do not qualify. If this topology is unsuitable for the owner's MacBook Pro, root must route a different storage adapter before provisioning; choosing a bucket URL alone cannot configure this backend. |
| Host prerequisite | The owner-host procedure must verify `/usr/bin/python3`, required for the kernel lock and fd-relative writes; no alternate locking scheme is implied. |
| Key selectors and public bootstrap | Choose armored public/private key paths on the trusted host, independently pinned full primary fingerprint, optional protected passphrase-file path, and public key bootstrap URL. Existing signer requires v4 RSA >=3072 bits. Keep secret material out of the handoff; creation/import is a separate owner operation. |
| Expiry and renewal | Choose `KANNA_LINUX_ARCHIVE_VALID_HOURS`, renewal owner and a concrete renewal procedure. There is **no renewal command** yet; a schedule alone is insufficient. Route that missing procedure before relying on a persistent public archive. It must preserve receipt/soak identity and handle expired pending transactions explicitly. |
| Initial candidate | After reviewed merge, choose committed `VERSION`, first staging iteration and exact remote-tip main or matching `release/linux/X.Y`. Linux bump/cut/reset/rollback/soak-override selectors are unsupported. Do not invent a version or branch operation in this task. |
| Real execution authorization | Obtain explicit Linux staging publication authorization for the concrete candidate/configuration. Production remains a later named-human production request after exact acceptance and the full 24-hour soak. |

All release selectors belong in the existing owner-only
`~/.kanna/.env.release.local`; none have been written here. The runbook lists
their exact names. Public HTTPS readback must verify all published bytes before
the durable publication receipt starts soak; local preparation starts no clock.

After those choices and merge, Ship collects both **stamped** candidate packages,
obtains exact installed evidence, and retains genuine predecessor/candidate
packages for two-version testing. The strict acceptance envelope binds
source/tree/version/iteration and both deb hashes; staging needs installed checks
for each architecture, production additionally needs upgrade checks for each
and a `both` system check from `641dbb6f`. Later upgrade/system evidence may be
added without resetting the same candidate's soak. No public artifact URL is
available from the CI validation run.
