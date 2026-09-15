# Linux delivery: concrete next preparation

Task `d3ce8dec`; supersedes the old missing-product/lifecycle disposition.
PR1510 is merged at `188d118eea93cc99b8355f4f3ce5ecb1db34477d`, tree
`e17791378d422f4d008d69b1201977c7ba5cb7d9`. This task merged that source into
its own branch to run current tooling; its documentation/merge commits are
**not** the proposed release source. Prior handoffs/evidence are retained.

## Recommendation

Keep apt signing on Jeremy's trusted MBP; serve immutable public bytes from
an always-on host. Existing repository infrastructure is a GCE relay VM with
Caddy on 80/443 (`docs/relay-vm-operations.md`,
`services/relay/deploy/{docker-compose.yml,Caddyfile}`). Recommend a dedicated
archive directory and restricted publisher account on that VM, with a read-only
Caddy mount/vhost at proposed `https://apt.kanna.build`. Private apt keys never
leave the MBP. This is a recommendation based on declared infrastructure, not a
claim that this directory, account, DNS or serving configuration exists.

**No currently implemented topology is practical for this arrangement.** The
local-filesystem backend would require the traveling MBP itself to serve the
archive continuously; moving signing onto the relay violates key custody.
Implement remote POSIX storage over a pinned SSH connection: the remote helper
owns the same kernel lock and all atomic writes, while the MBP signs locally.
Hold ownership through public readback/projection; fail closed on connection or
ownership loss. Do not replace this with rsync, an SSHFS mount or copy-after-sign.
This preserves the tested POSIX transaction model and avoids introducing a new
object-store ownership protocol. Validate transport failure/recovery locally
before any deployment. The existing Caddy config only proxies the relay; it
does not already serve apt files.

The genuine owner choice is whether to share the existing relay host's failure
and capacity domain for this small initial archive (recommended), or fund a
separate archive host. Exact paths, restricted permissions, transport validation
and vhost configuration are engineering work, not questions for the owner.
Provisioning/DNS/production-service changes need a later concrete authorization.

Recommend seven-day metadata validity (168 hours), renewed every 48 hours from
the trusted MBP by the release operator, with expiry monitoring and a documented
backup operator/custody arrangement. **Renewal still needs implementation**:
same candidate/artifacts and original soak receipt, an explicit signed renewal
record, atomic InRelease replacement, public readback and interrupted/expired
transaction recovery. A timer or larger expiry value alone does not solve it.
Key generation/import, fingerprint bootstrap and protected passphrase storage
remain owner-host setup; no real key was inspected, created or changed here.

## Preparation blocker and selected source proposal

Executed current `./kd release status --platform linux`: Linux-only status,
`promotion.allowed=false`, missing archive configuration. Executed
`./kd release ship --platform linux --staging --dry-run --staging-iteration 2`:
exit 1, **“Linux release configuration missing/invalid”**, listing backend,
root, baseUrl, validForHours, publicKeyPath and fingerprint. It stopped before
any build. No placeholder backend/key was supplied.

`shipLinuxRelease` loads archive config/public key and takes the archive lock
before calling `collectLinuxRelease`; it also requires HEAD equal the selected
remote main/series tip. The standalone `kd build linux-package` registry does
not supply source stamps. Thus ordinary builds or this task's documentation
merge cannot produce the requested exact-source collector inputs.

**Next executable engineering action:** expose the existing collector through
a local-only canonical `kd release prepare --platform linux --ref <exact-sha>`
surface (proposed, not an available command). It must pin a clean isolated
source snapshot, verify revision/tree before and after collection, retain both
deb/report hashes and a preparation manifest, and require no archive/key/public
URL. Keep publication's current remote-tip/config/lineage gates unchanged.
Historical source support must truthfully carry compatible build-stamp support;
never stamp current binaries with an older revision or bypass report checks.

Proposed B: committed **VERSION 0.2.0**, iteration **2**, exact reviewed source
`188d118eea93cc99b8355f4f3ce5ecb1db34477d`; package versions
`0.2.0~staging.2-1`, both architectures. No unrelated macOS version bump.
Proposed A: product-capable source
`79f1c8225f0302d1e8d59dacedc5c148a9438272`, VERSION 0.2.0, iteration 1.
That source has real native evidence and predates actual transfer behavior
changes (`7c842b893`, `4b6e405b3`), so this is a genuine product delta, not a
documentation-only or renamed-package pair. A is an unpublished bootstrap
predecessor, not a historical release. Its old graph's stamp compatibility must
be resolved honestly by the preparation implementation; these identities are
proposals, not existing accepted artifacts. No unstamped CI bytes enter B's
release collection. If preparation code must land in B's source, root must pin
the resulting reviewed successor and rerun exact acceptance; do not silently
keep this older source identity. Recheck current remote tip at execution.

After collection: both real A→B installed upgrade lanes, exact installed
attestations and `641dbb6f` system/transfer evidence. Staging publication needs
explicit authorization and real configured infrastructure. Only verified public
receipt starts the full 24-hour B soak; production remains separately authorized.

## Current evidence and disposition

At two live checks during this preparation, final-head CI `34945960327` for
`344b1eaf08abb9c8471c6442811500e622b1b2f4` remained **in_progress**: both
apt-interop and prerequisite jobs passed, builds still running. No final-head
native PASS or installed result is claimed. Prior corrected CI `34940178572`
remains scoped to its earlier source. No duplicate CI/build/review was started.

No archive/host/key provisioning, public exposure, publication, promotion,
website link, version rewrite or other worktree mutation occurred. No owned
background processes remain. Public release URLs and accepted candidate
checksums remain unavailable. Immediate blocker is the engineering preparation
surface, not an owner needing to fill a selector table; no attention badge is
needed until host-sharing/provisioning approval is the actual next step.
