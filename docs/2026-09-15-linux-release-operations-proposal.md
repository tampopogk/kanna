# Linux archive setup proposed for later approval

Task `ba4b1503`; engineering follow-up to Ship `d3ce8dec` after PR1510.
This is a reviewable setup/runbook, **not executed infrastructure configuration
or authorization**. The existing relay compose/Caddy files remain unchanged.

## Recommended topology

Use the existing always-on relay VM's POSIX disk for a dedicated apt archive.
Keep apt signing and protected signing/passphrase files on Jeremy's trusted MBP.
Use `KANNA_LINUX_ARCHIVE_BACKEND=ssh` for one pinned SSH connection to the
existing storage helper, with no agent forwarding, SSHFS or copy-after-sign.
Caddy reads the archive through a read-only bind mount. The relay service and
its OTA private key remain separate; apt keys never go to that machine.

The owner decision is sharing the relay host's capacity/failure domain versus
funding a separate host, followed by authorization to provision the chosen host
and expose the apt endpoint. Technical settings below are the proposed implementation,
not a request for the owner to design storage or choose arbitrary paths.

### Host changes to authorize and prepare later

1. Create dedicated unprivileged account `kanna-apt`, home
   `/var/lib/kanna-apt`, with no sudo, cloud role or access to relay secrets.
   Its publication identity is separate from administrator access. Use an
   authorized public key with `restrict` (no PTY, forwarding or user rc).
   Permit only the command execution needed by the Python storage helper.
   The account needs no apt signing key or GitHub credential.
2. Use root-owned `/srv/kanna-apt` (`0755`), containing publisher-owned
   `/srv/kanna-apt/archive` (`0755`). Every ancestor is a real directory,
   no symlinks; storage is a local POSIX filesystem. Files are `0644`, the
   helper's persistent `.publication.lock` is `0600`. Do not unlink that lock.
   Keep `/usr/bin/python3` available on the VM; this is a release-tool dependency,
   not a packaged app dependency. Back up archive state/immutable history together.
3. Independently obtain the VM's SSH host public key/fingerprint through its
   authenticated administrative control plane. Put that exact host entry in
   the MBP's dedicated `0600` known-hosts file. Do not bootstrap trust by
   accepting an unauthenticated first SSH connection or blindly trusting
   `ssh-keyscan`. Select the independently provisioned publisher identity;
   this task creates/imports no owner credential.
4. Add a read-only mount to the **Caddy** service in the reviewed relay compose
   configuration: `/srv/kanna-apt/archive:/srv/kanna-apt:ro`. Add the following
   vhost after host-sharing, DNS and public-service changes are approved:

   ```caddyfile
   apt.kanna.build {
       root * /srv/kanna-apt
       file_server {
           hide .*
       }
       @metadata path /dists/* /linux/state.json
       header @metadata Cache-Control "no-cache"
   }
   ```

   The `hide` syntax follows [Caddy’s file_server documentation](https://caddyserver.com/docs/caddyfile/directives/file_server).
   No directory listing is enabled. Dotfiles/temporary writes stay hidden;
   public objects contain no private keys. Apt by-hash indexes and pool packages
   are immutable. Do not set an immutable cache policy on `InRelease` or state.
   The existing relay vhost continues proxying `relay:8080` unchanged.
   These snippets have not been deployed or validated against a live Caddy host.
5. Prepare DNS for `apt.kanna.build` at the approved host and verify HTTPS with
   its normal certificate flow. Deploy reviewed relay configuration only through
   `kd cloud deploy --relay` for the explicitly approved environment/ref. Do not
   replace live compose/Caddy files by hand. There is no archive URL yet.

## MBP selectors to fill from that approved setup

Use the owner-only `~/.kanna/.env.release.local`. These are proposed paths and
non-secret selectors, not an installed file:

```dotenv
KANNA_LINUX_ARCHIVE_BACKEND=ssh
KANNA_LINUX_ARCHIVE_ROOT=/srv/kanna-apt/archive
KANNA_LINUX_ARCHIVE_BASE_URL=https://apt.kanna.build
KANNA_LINUX_SSH_HOST=<approved-storage-hostname-or-IPv4>
KANNA_LINUX_SSH_USER=kanna-apt
KANNA_LINUX_SSH_PORT=22
KANNA_LINUX_SSH_KNOWN_HOSTS_PATH=/Users/jeremyhale/.kanna/linux-apt/known_hosts
KANNA_LINUX_SSH_IDENTITY_PATH=/Users/jeremyhale/.kanna/linux-apt/publisher_identity
KANNA_LINUX_APT_PUBLIC_KEY_PATH=/Users/jeremyhale/.kanna/linux-apt/public.asc
KANNA_LINUX_APT_FINGERPRINT=<independently-pinned-40-hex-primary-fingerprint>
KANNA_LINUX_APT_PRIVATE_KEY_PATH=/Users/jeremyhale/.kanna/linux-apt/private.asc
KANNA_LINUX_APT_PASSPHRASE_PATH=/Users/jeremyhale/.kanna/linux-apt/passphrase
KANNA_LINUX_ARCHIVE_VALID_HOURS=<explicitly-approved-validity-hours>
```

The passphrase selector is optional. Private key, passphrase, SSH identity and
known-hosts files must be owner-only regular files. The apt key profile remains
v4 RSA >=3072 / SHA512. Provisioning/importing real keys, fingerprint bootstrap
and protected custody are later owner-host operations, outside this task.
The SSH client is batch-only and uses the selected identity with no agent;
select a credential usable under that constraint and protect it with the
account's restricted authority and filesystem permissions.

The local `filesystem` backend remains supported; omit SSH selectors and point
`KANNA_LINUX_ARCHIVE_ROOT` at its actual local POSIX archive. A traveling MBP
is not the proposed always-on serving host.

## Preparation and release sequence

1. Read the final reviewed source and retain the earlier evidence's real SHAs.
   Local preparation needs none of the selectors above:

   ```sh
   ./kd release prepare --platform linux --ref <exact-A-support-commit> --staging-iteration 1
   ./kd release prepare --platform linux --ref <exact-reviewed-B-commit> --staging-iteration 2
   ```

   `79f1c8225f0302d1e8d59dacedc5c148a9438272` itself is unsupported because
   its graph/report lacks stamps. The precise stamp-only backport is the diff
   to `188d118eea93cc99b8355f4f3ce5ecb1db34477d` limited to
   `packaging/linux/products.bzl` and
   `tools/kd/src/runtime/linux-bazel-package.ts`: two manifest fields,
   two interface fields and the report's revision/tree fields. Review and commit
   that support on the historical product baseline; record the resulting **new
   SHA/tree**, never A's old SHA. No such backport or final artifact pair was
   created here. This preparation command uses the current controller and the
   chosen source's unchanged graph; do not copy current product code into A.
2. Ship selects the genuine product delta and both package versions, prepares
   both architectures, and retains each manifest/deb/report/hash. It owns real
   installed A→B upgrade testing. Task `641dbb6f` supplies exact system/transfer
   acceptance. No synthetic fixture or earlier-head native CI is final acceptance.
3. After infrastructure, protected keys and exact acceptance are available,
   run Linux status/rehearsal, then the separately authorized staging publication.
   `ship` still requires the clean exact source equal to its selected remote tip.
   A preparation manifest is not a way around any publication gate. Only the
   first verified public publication receipt starts the full 24-hour B soak.
4. Production remains a separate named-human request after acceptance and soak.
   Deliver actual verified URLs/hashes to the website only after publication.

## Renewal proposal and operations

Ship recommends 168-hour validity and manual renewal every 48 hours from the
trusted MBP. This is **a proposal**, not a configured default or standing
production authorization. Before exposing the archive, approve an operator,
backup operator/key custody, expiration monitoring and an explicit validity and
renewal plan. No scheduler or monitoring service is installed by this task.

For an authorized renewal, read the exact candidate and last sequence from
`./kd release status --platform linux`. Execute with an explicit next sequence
(starting at 1), actual candidate tag and approved hours:

```sh
./kd release renew --platform linux --candidate <exact-linux-tag> --renewal <N> --valid-for-hours <approved-hours>
./kd release status --platform linux
```

On interruption, retry that same sequence/hours. If it expired, deliberately
choose N+1; the signed history is retained. The original publication receipt and
soak timestamp do not move. A pending publication with complete closure and a
cached signed commit can be renewed even when public receipt creation failed;
without a receipt its first proven public observation starts soak conservatively.
Missing/unverified initial inputs are refused rather than signed by a metadata
operation. Never delete journal/receipt/lock files, rewrite immutable records,
set the clock back or bypass host/signature verification to recover.

## Evidence from this implementation

- Real disposable Git repositories exercise exact-ref isolation, dirty-controller
  exclusion, source mutation/stamp mismatch refusal, both deb/report hashes and
  the completion manifest without archive/key/remote configuration. Native
  compilation is replaced with explicit synthetic executable payloads; actual
  Debian serialization and the unweakened collector/verifier run.
- Actual `./kd release prepare` on the proposed historical A exits with the
  unsupported graph/report stamp diagnostic before Bazel/native compilation.
  Its temporary source is removed. No final A/B native build is claimed.
- A disposable loopback listener passes accepted sockets to `/usr/sbin/sshd -i`
  with fixture-only host/client keys and configuration. The actual SSH client
  runs the shared Python helper. Tests prove pinned-host refusal, archive-wide
  contention, immutable/readback behavior, traversal refusal, actual SSH and
  helper SIGKILL failure, EOF lock release and new-session recovery. No system
  SSH service is enabled or modified. The listener, children and fixtures are
  cleaned up. No owner keyring/import, archive or host is used.
- Renewal integration uses real local POSIX storage and disposable OpenPGP
  test keys with synthetic packages. It covers expiry, commit/readback failure,
  lost envelope/journal acknowledgement, explicit supersession of expired
  pending metadata, original/renewal timestamp preservation, pending publication
  recovery, changed bytes and Linux/macOS isolation. HTTP/GitHub boundaries in
  command tests are fixtures; no public archive or GitHub release is mutated.

Focused test counts and final verification are recorded in this task's durable
result. Earlier native CI remains evidence for its own recorded heads only.

### Executed verification

- `pnpm --dir tools/kd typecheck`: PASS.
- Nine focused suites (`linux-release-prepare`, `linux-release-lifecycle`,
  `linux-release-commands`, `linux-ssh-storage`, `release-tasks`,
  `linux-apt-publication`, `linux-release-build`, `release-lineage`,
  `release-platform`): **175 tests PASS**. After final ownership-probe and
  literal known-hosts-path hardening, the two affected storage/lifecycle suites
  were rerun: **25 tests PASS**; typecheck passed again.
- `./kd release prepare --help`: PASS using kd's installed bundle.
- `./kd release prepare --platform linux --ref
  79f1c8225f0302d1e8d59dacedc5c148a9438272 --staging-iteration 1
  --out-dir .tmp/historical-a-preparation`: expected exit 1,
  `Unsupported historical Linux source ...: its package graph/report lacks
  revision/tree stamps.` No native build ran; no output manifest was fabricated.
- `git diff --check`: PASS. Task `.tmp/` is empty and no fixture SSH/helper/test
  processes remain. No owner's SSH service, public endpoint, archive, private
  key/keyring, release channel, relay/Caddy/DNS, website or billing resource was
  changed. Test-only keys existed only in disposable fixtures and were removed.
