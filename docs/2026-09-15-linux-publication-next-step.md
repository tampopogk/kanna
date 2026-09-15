# Linux delivery: next publication step

Current disposition after PR1513 merged as
`0a74508b1a4b9b304c28b5b97fdcfecfc8d33f51`: controller and both Ubuntu 24.04
installed/upgrade lanes are complete. Do not repeat them unchanged.
**Product B remains `a9df2f1d46fb08bcf53200b738e0fdc3c127b636`, tree
`3415e82f8a273b6ff94494d367139038eb026a62`, VERSION 0.2.0, staging iteration 2.**
Main/controller is not the product source. No public candidate or soak exists.

## Recommended setup and exact owner decision

**Approve sharing the existing always-on relay VM for a dedicated apt archive,
plus scoped host/DNS/Caddy setup and apt-key setup on Jeremy's trusted MBP.**
This is the recommended host choice; a separate paid host is the alternative.
The owner must explicitly identify the approved relay environment before any
live change (especially if it is production). Nothing in this report grants
production service authorization. Engineering owns the paths/account/config;
the owner need not design those details.

Use the implemented SSH POSIX backend: archive account `kanna-apt` without
sudo/cloud roles/relay-secret access, local disk `/srv/kanna-apt/archive`,
read-only Caddy mount, and a dedicated HTTPS archive origin. The earlier
`apt.kanna.build` proposal is only a proposed DNS name, **not an existing URL**.
Signing and the private apt key stay on the MBP. No NFS/SSHFS/rsync backend,
mobile storage reuse or signing on the relay.

The detailed reviewable host/account/Caddy changes are already in
[the setup proposal](2026-09-15-linux-release-operations-proposal.md).
The SSH and metadata-renewal implementation gaps in older Ship notes are closed.
No new architecture or implementation audit is needed for those features.

### Configuration to install only after setup authorization

This is an inert review template, not a configured environment file. Replace
all bracketed values with independently verified setup results before writing
selectors into the MBP's owner-only `~/.kanna/.env.release.local`:

```dotenv
KANNA_LINUX_ARCHIVE_BACKEND=ssh
KANNA_LINUX_ARCHIVE_ROOT=/srv/kanna-apt/archive
KANNA_LINUX_ARCHIVE_BASE_URL=<approved-public-https-origin>
KANNA_LINUX_ARCHIVE_VALID_HOURS=168
KANNA_LINUX_SSH_HOST=<approved-hostname-or-IPv4>
KANNA_LINUX_SSH_USER=kanna-apt
KANNA_LINUX_SSH_PORT=22
KANNA_LINUX_SSH_KNOWN_HOSTS_PATH=/Users/jeremyhale/.kanna/linux-apt/known_hosts
KANNA_LINUX_SSH_IDENTITY_PATH=/Users/jeremyhale/.kanna/linux-apt/publisher_identity
KANNA_LINUX_APT_PUBLIC_KEY_PATH=/Users/jeremyhale/.kanna/linux-apt/public.asc
KANNA_LINUX_APT_FINGERPRINT=<independently-pinned-40-hex-primary-fingerprint>
KANNA_LINUX_APT_PRIVATE_KEY_PATH=/Users/jeremyhale/.kanna/linux-apt/private.asc
KANNA_LINUX_APT_PASSPHRASE_PATH=/Users/jeremyhale/.kanna/linux-apt/passphrase
```

These proposed credential paths are not assertions that files exist. An already
suitable owner key may be selected; otherwise apt key generation/import and
protected storage need approved owner-host setup. The adapter requires v4 RSA
at least 3072 bits with SHA512; independently pin the primary fingerprint and
SSH host key. The passphrase selector is optional; never put its value in config.
The SSH identity must work with batch authentication and no agent forwarding.
No real MBP key availability was newly inspected or inferred from this Studio.

Recommend 168-hour metadata validity and operator renewal every 48 hours on the
trusted MBP, with expiry monitoring and a backup operator/custody plan agreed
before exposure. Renewal is now implemented; it preserves the original receipt
and soak timestamp. It is not a standing authorization or an installed schedule.
No current canonical apt-key setup command is declared: do not substitute an
invented `kd` command or perform raw signing/key provisioning in this task.

## Prepared acceptance: usable now, system check deliberately absent

`docs/evidence/2026-09-15-linux-bootstrap/publication-preparation/B-acceptance.json`
contains four checks: installed and upgrade for each architecture, sourced from
actual run34959684410. Each references retained JSON evidence by SHA256; upgrade
checks name accepted bootstrap A's actual new source and older Debian version.
Paths are relative to the repo root, so the file remains portable with its
committed evidence. Invoke from that root. The existing canonical schema/hash
reader accepted it: no staging **acceptance** blocker; production acceptance
reports exactly `Missing or duplicate system acceptance for both.` This is not
a claim that configured staging publication is allowed.

Task641dbb6f owns authenticated cross-machine acceptance. It is parked for normal
staging login; neither endpoint is open. No system pass or live route is invented.

## Exact remaining publication constraint

Fresh read-only `./kd release status --platform linux` on the connected Studio
reports promotion.allowed=false: missing backend/root/baseUrl/validForHours/
publicKeyPath/fingerprint. No dry-run/build was started.

Separately, configured `ship` currently requires its clean HEAD to equal
`origin/main` or the selected `release/linux/X.Y` tip, and always calls the
collector. Fresh `ls-remote` returned main at the PR1513 merge above and **no
release/linux/0.2 branch**. Thus B is not the supported current publication tip.
The prepared manifest is not a ship input: `--skip-build` checks Bazel outputs
under matching stamps; it does not import `.build/linux-prepared/...`. B's
isolated build checkout/output cache was cleaned after collection, with the
verified debs/reports/manifests preserved.

**Consequently there is no currently executable canonical command that publishes
these exact retained B bytes from the current controller.** Provisioning alone
will not fix that. Do not run main's ship with B's acceptance, create a release
branch with raw Git, recut/reset, rebuild B, or relabel the tested artifacts.

Smallest engineering follow-up to route: add canonical consumption of the
verified preparation manifest for existing `ship`, reusing artifact/report,
source/tree, evidence and ownership checks, and establish an explicitly
approved immutable promotion-base pin for B through supported release tooling.
The current Linux cut operation is refused, so that pin cannot be silently
created as a workaround. Keep remote lineage/promotion fences intact; publish
and promotion must retain B's exact product identity. This is a bounded missing
publication-input/pin path, not another missing product or testing dependency.
No implementation change or release-branch action is performed here.

## Command sequence once prerequisites are satisfied

The next existing read-only command on the configured trusted MBP is:

```sh
./kd release status --platform linux --acceptance "$PWD/docs/evidence/2026-09-15-linux-bootstrap/publication-preparation/B-acceptance.json"
```

After the exact prepared-input/promotion-base route exists, rehearsal must name
B/iteration2 and report its actual artifacts and blockers. Do not use today's
build-oriented dry-run to rebuild unchanged packages. The existing publication
command shape is below for review only; it is **not runnable for B yet**, because
that branch/input route does not exist and publishing remains held:

```sh
./kd release ship --platform linux --staging --branch release/linux/0.2 --staging-iteration 2 --release --skip-build --acceptance "$PWD/docs/evidence/2026-09-15-linux-bootstrap/publication-preparation/B-acceptance.json"
./kd release status --platform linux --acceptance "$PWD/docs/evidence/2026-09-15-linux-bootstrap/publication-preparation/B-acceptance.json"
```

Explicit publication authorization is still required. Only verified public
InRelease/closure readback and the canonical publication receipt start the full
24-hour soak. The expected tag spelling would be `linux-v0.2.0-staging.2`, not an
existing release. Production additionally requires system acceptance and an
explicit named-human request; it rebuilds production identity rather than
relabeling these staging debs.

## Website handoff

Portable, reviewable data:
`docs/evidence/2026-09-15-linux-bootstrap/publication-preparation/kanna-web-handoff.json`.
It includes actual B version/source/tree, both filenames/architectures/byte sizes/
full hashes and the floor evidence URL. `downloadLinksEnabled=false`; archive,
public-key and artifact URLs/fingerprint/receipt are null. No temporary GitHub
validation draft or expired Actions artifact is a website download target.

After real publication, fill URLs only from the verified candidate/receipt and
independently pinned public-key distribution. For this staging package, future
instructions must say `kanna-staging`, version `0.2.0~staging.2-1`, Ubuntu24.04,
architecture amd64 or arm64. The repo's install pattern is apt installation of
the downloaded local deb (including declared dependencies); do not advertise
production `kanna`, a stable channel, or unattended archive upgrades until those
actual signed public resources exist. Direct-deb setup is not automatic apt
archive configuration. No website write or public link has been made.

Ship stays open for this setup/publication work. No native build, VM mutation,
public infrastructure change, key creation, publication or soak occurred here.
