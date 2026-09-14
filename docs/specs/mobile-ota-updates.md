# Self-Hosted OTA Updates for Kanna Mobile

Kanna mobile uses self-hosted Expo Updates for staging and production JS/asset
updates. Development builds remain Metro/dev-client only. OTA updates are served
by the relay and stored in the per-environment Firebase/GCS bucket.

## Client Scope

- `expo-updates` is configured only when `KANNA_APP_ENV` is `staging` or `prod`.
- The manifest URL is derived from the environment relay URL by converting
  `wss://` to `https://` and appending `/ota/manifest`.
- The app checks after initial model initialization and on foreground, throttled
  to once every five minutes.
- Downloaded updates do not reload active sessions immediately. The app shows
  an in-app prompt and also reloads on the next foreground after backgrounding.

## Shared Contract

- Protocol: Expo Updates protocol version `1`, platform `ios` or `android`.
- Manifest endpoint: `GET /ota/manifest`.
- Asset endpoint: `GET /ota/assets?key=<hash>&runtimeVersion=<rv>&platform=<ios|android>`.
- Origins: `https://relay-staging.kanna.build` for staging and `https://relay.kanna.build` for production.
- Dev: disabled.
- Channels: `staging` and `production`, passed by `expo-channel-name`.
- Runtime version: sourced from the selected environment in `apps/mobile/src/mobileEnvironments.json` as `runtimeVersion`.
- Code signing: RSA SHA-256, Expo alg `rsa-v1_5-sha256`.
- Code signing key id: `kanna-mobile-ota-v1`.
- Public cert: `apps/mobile/certs/ota-codesign.pem`.
- Public cert profile: `apps/mobile/certs/ota-codesign.cnf`.
- Public cert key usage: critical `digitalSignature`.
- Public cert extended key usage: critical Code Signing (`1.3.6.1.5.5.7.3.3`).
- Public cert SHA-256 fingerprint: `18:5A:94:97:1B:8C:07:A4:CA:8E:22:51:85:FA:64:31:EE:6C:9B:B8:E5:AD:06:17:93:CD:AD:90:CF:D9:6B:22`.
- Private key secret: Google Secret Manager secret `kanna-mobile-ota-private-key-pem` in each environment project.

## Native Versioning and Archive Provenance

### Motivation: reconstructing the App Store 1.0 binary

The first App Store binary predates the durable archive ledger below. App
Store Connect identifies it as uploaded August 17, 2026 at 21:04, with original
file `727ee03c-3740-44ef-bc17-0d1ff8a925eb.ipa`. Repository forensics make its
runtime knowable only circumstantially: the last `origin/main` tip before the
upload was `64824093a` (August 17 at 19:39 +09:00), that commit used production
runtime `2.1.4`, and `2.1.4` remained current until August 19. The binary
therefore embeds runtime `2.1.4` regardless of which contemporaneous ref
produced it. The owner later confirmed App Store Connect identifies the
released binary as build 3, but no historical ledger binds that build to an
exact source ref.

Current production OTA configuration uses runtime `2.2.2`, first introduced by
`6f303e3dd` on August 23. Those updates cannot reach the 1.0 store binary, so a
new native binary is mandatory. This reconstruction—and the inability to name
the exact source/build—is why archive provenance must be written and pushed at
archive time rather than deferred until publication.

### Policy

The mobile release series is independent of the desktop release
series. A mobile `1.0`, `1.1`, or `1.0.1` release may accompany any desktop
`0.3.x` release; neither number is derived from the other. The checked-in
mobile release version uses Apple's three-component form in
`apps/mobile/VERSION` (for example `1.0.0`, which the App Store may display as
`1.0`). It is advanced with `kd mobile version bump --patch|--minor|--major`
and is shared by native and OTA deliveries of the same mobile release.

The four identity values answer different questions:

- **Mobile release version** identifies the running customer-facing mobile
  release, whether it arrived in a binary or by OTA. Advance it for changed
  release content. Staging and production delivery of the same release may
  share it, but another changed OTA on either channel advances it again.
- **Marketing version (`CFBundleShortVersionString`)** is the release version
  embedded in a native App Store binary. An OTA never changes the installed
  value; the client reports the running release and native version separately.
- **Build number (`CFBundleVersion`)** identifies one App Store binary. Use a
  numeric value higher than every prior production archive/upload, including
  across marketing versions. Any rebuilt binary from changed source takes a
  new build number; an OTA does not.
- **Expo `runtimeVersion`** is a native compatibility key, not a customer-
  facing version. Increment it in every entry of
  `apps/mobile/src/mobileEnvironments.json` whenever native code, native
  configuration, the Expo SDK, a native dependency, the native-identity
  plugin, or the embedded OTA signing certificate changes. JS/assets-only
  changes keep it unchanged. An OTA is reachable only by installed binaries
  with the same runtime.

Every successful `kd mobile archive --production` records provenance before
any optional upload by creating and pushing the immutable annotated tag
`mobile-archive-v<marketing-version>-<build-number>` at the archived commit.
Its JSON message records the requested ref, full and short commit, marketing
version, build number, runtime version, bundle id, and archive timestamp. This
git tag is the durable archive-time ledger; `.build/` output is disposable.
Inspect it with `git show mobile-archive-v<version>-<build>`. Reusing the same
version/build at another commit is refused: both the annotated tag's peeled
Git target and the commit in its JSON message must equal the requested archive
commit. `kd mobile publish` later adds its separate
`mobile-v<version>-<build>` tag with upload, verification, and App Store
Connect identifiers.

## GCS Layout

Objects live in the environment bucket under `ota/`:

```text
ota/<platform>/<runtimeVersion>/updates/<updateId>/metadata.json
ota/<platform>/<runtimeVersion>/updates/<updateId>/expoConfig.json
ota/<platform>/<runtimeVersion>/updates/<updateId>/kanna-source.json
ota/<platform>/<runtimeVersion>/updates/<updateId>/bundles/<sha256-base64url>.hbc
ota/<platform>/<runtimeVersion>/updates/<updateId>/assets/<sha256-base64url>
ota/<platform>/<runtimeVersion>/channels/<channel>.json
```

Each platform/runtime/channel has its own pointer. Existing iOS objects need no
migration. Android requests its embedded runtime and only receives objects from
that exact Android runtime path; missing pointers return 404, never an iOS or
other-runtime fallback. An iOS binary at runtime R does not authorize Android
runtime S to load R. Publishing Android/R before an Android/R binary ships is
safe but reaches no Android installation until a compatible binary exists.
Keep older pointers; never relabel new JS with an older runtime to force delivery.

The channel pointer is the commit point:

```json
{
  "currentUpdateId": "<updateId>",
  "createdAt": "...",
  "runtimeVersion": "2.1.1",
  "releaseVersion": "1.0.1",
  "sourceRef": "release/0.2",
  "sourceCommit": "<40-hex sha>"
}
```

`updateId` is deterministic: SHA-256 of `metadata.json`, converted to the Expo
UUID shape using the first 32 hex characters.

`kanna-source.json` records the git source and mobile release the update was exported from:

```json
{ "updateId": "<updateId>", "ref": "release/0.2", "commit": "<40-hex sha>", "shortCommit": "<12-hex sha>", "releaseVersion": "1.0.1", "sourceBranch": "release/0.2" }
```

The pointer's `sourceRef`/`sourceCommit` answer "what is this channel serving
right now"; `kanna-source.json` stays with the update, so an update a later
rollback re-points to is still traceable after the pointer has been rewritten.
The release version is also written to `metadata.json`; because `updateId` is
that file's hash, two otherwise identical releases with different release
numbers remain distinct immutable updates. The relay copies it into signed Expo
manifest metadata, which lets the running client display the release without
confusing it with the installed native binary. Older metadata without the field
continues to serve. A staging rollback pointer recovers source fields from the target record and
preserves its release version when metadata provides one. New staging source
records also carry the verified canonical `sourceBranch`.

## Relay

The relay handles:

- `GET /ota/manifest`: validates Expo headers, reads the channel pointer, builds a multipart manifest, and signs the manifest or `noUpdateAvailable` directive with `expo-signature`.
- `GET /ota/assets`: serves content-addressed bundle/asset objects with immutable cache headers.

Deploy wiring fetches `kanna-mobile-ota-private-key-pem` from Secret Manager
onto the VM and mounts it read-only into the relay container at
`/run/secrets/kanna_ota_private_key.pem`.

## Operations

Provision the environment bucket, required API, and relay bucket-read IAM before the first deploy. This command is idempotent and requires an explicit environment:

```bash
./kd mobile ota provision --staging
./kd mobile ota provision --production
```

Provision the private key secret after key generation or rotation. This command idempotently enables Secret Manager, creates the secret when absent, adds a version, and grants the relay service account access:

```bash
./kd mobile ota provision-secret --staging --key-path "$HOME/.kanna/secrets/kanna-mobile-ota-v1-private-key.pem"
./kd mobile ota provision-secret --production --key-path "$HOME/.kanna/secrets/kanna-mobile-ota-v1-private-key.pem"
```

Before its first cloud command, `provision-secret` parses the committed
certificate and rejects an invalid private key or a key whose derived public
key does not match the certificate. It never prints PEM contents or derived
key bytes.

Reissue the public certificate only from the existing private key and committed
profile. Generate into a temporary directory, inspect the public certificate,
then replace only `apps/mobile/certs/ota-codesign.pem`:

```bash
OTA_CERT_DIR=$(mktemp -d /tmp/kanna-ota-cert.XXXXXX)
openssl req -new -x509 -sha256 -days 3650 \
  -key "$HOME/.kanna/secrets/kanna-mobile-ota-v1-private-key.pem" \
  -config apps/mobile/certs/ota-codesign.cnf \
  -out "$OTA_CERT_DIR/ota-codesign.pem"
openssl x509 -in "$OTA_CERT_DIR/ota-codesign.pem" -noout -purpose
```

The public output must report `Code signing : Yes`. Replacing the certificate
changes embedded native update configuration, so increment every environment's
`runtimeVersion` before installing or publishing a build containing it. Never
print, copy into the repository, or commit the private key.

Deploy relay support through the normal cloud deploy flow:

```bash
./kd cloud deploy --staging --relay
./kd cloud deploy --production --relay --ref release/0.2
```

Publish one platform's JS/asset update (`--platform` defaults to `ios`).
`kd mobile publish` remains the iOS App Store binary pipeline; OTA uses
`kd mobile ota publish`:

```bash
./kd mobile ota publish --staging --platform ios
./kd mobile ota publish --staging --platform android
./kd mobile ota publish --production --platform android --ref release/0.2
```

`publish` exports whatever the working tree contains, so the source is a guard
rather than a parameter — the same treatment `kd cloud deploy` and `kd mobile
archive` apply:

- `--ref <branch|tag|sha>` is **required** with `--production`. An OTA publish
  pushes JS straight to installed apps, so the source commit must be named
  rather than inferred from whatever happens to be checked out.
- A dirty worktree is refused; so is a `--ref` that is not the checked-out
  commit (`git checkout` it first).
- Without `--ref`, staging resolves `HEAD` and reports it, so the output still
  records what shipped.
- `--rollback-to` re-points the channel at an already-published update and
  exports nothing, so it needs no `--ref`. It still refuses a dirty worktree.

The resolved commit appears in the command output (`Source: <ref> (<short
sha>)`), in the result data as `source`, and in the two GCS records described
under [GCS Layout](#gcs-layout). `kd mobile ota status` prints the raw pointer,
so it shows the source of the update the channel currently serves.

`--ref` narrows what a publish can ship; it does not change the approval
policy. Production publishes and rollbacks still require an explicit human
request (see [release.md](../dev/release.md#mobile-ota)).

`publish` validates the committed certificate and its validity window before
Expo export or cloud upload. The same RSA key, certificate and key ID cover both
platforms. Expo's Android plugin embeds the global URL, request headers,
certificate and signing metadata, and the Android downloader verifies signed
manifests/directives. Android support here changes no native configuration and
requires no runtime bump.

Staging OTA now invokes the existing desktop staging-lineage policy **before
export, including dry-run**. This closes a previous guard gap: some publishes
that previously succeeded are now refused. A source must be verifiably on
`origin/main` or `origin/release/X.Y`, then satisfy the same active-candidate
ancestry, freeze and recorded lineage rules as desktop staging. An arbitrary
task branch cannot bypass an active release-branch freeze. Name a canonical
`--ref` at the checked-out commit or publish from that canonical branch;
unverifiable source/candidate data refuses publication. Rollback checks the
target's durable `kanna-source.json` commit/branch, never the current checkout.
Legacy targets with missing or unidentifiable provenance cannot authorize a
staging rollback. No reset, abandon, or soak bypass is added.

Export, config/runtime validation, hashing and staging finish before any upload.
The complete selected update directory is reconciled (including checksums)
before writing its pointer, even if metadata exists from an interrupted upload.
Export/upload failure leaves the pointer unchanged; a pointer-write failure is
reported as failure. Each invocation selects one platform: a later Android
failure leaves an earlier successful iOS publication intact. There is no
cross-platform transaction or automatic rollback. The same mobile release
number can be published separately to both platforms; changed content advances
`apps/mobile/VERSION` under the existing per-pointer version guard.

There is no OTA promotion command and `kd release` does not copy or require OTA
objects. Production delivery uses its own explicitly authorized platform publish
and verification.

Check the current pointer:

```bash
./kd mobile ota status --staging --platform android
./kd mobile ota status --production --platform android
```

Run the read-only cloud and relay preflight before asking a human to verify an
OTA on a physical device:

```bash
./kd mobile ota doctor --staging --platform android
./kd mobile ota doctor --production --platform android
```

`status`, `doctor`, and its alias `preflight` accept `--platform ios|android`
(default `ios`). They inspect only that platform; missing Android objects do not
fail an iOS check. The iOS native `mobile qa --production --ota` caller stays
explicitly iOS; use Android OTA doctor for Android. `preflight` is an alias for `doctor`. The command does not publish, roll back,
write GCS objects, modify Secret Manager, or install/launch a device app. It
requires Google Cloud credentials for the target project and verifies:

- environment resolution, OTA bucket, channel, and `runtimeVersion`
- committed certificate validity and Code Signing extended key usage
- current GCS channel pointer and referenced update metadata/config readability
- relay `/health` and `/ota/manifest` behavior for the current channel
- Secret Manager private-key secret existence
- relay VM service account resolution
- relay service account IAM for Secret Manager and OTA GCS reads
- every selected-platform runtime channel pointer, with older pointer publication dates marked stale relative to the newest channel pointer
- paired-device build observations from the running desktop, including compatibility and confirmed application when available

Publish success means the artifacts and pointer were published. It does not mean
an installation received them. Publish (including rollback) now warns when no
recently observed paired device runs the target runtime, names the reported
runtimes, and reports unknown inventory explicitly. It remains allowed.
`status` includes these observations alongside the pointer and recent updates;
its exit status still describes pointer readability. `doctor` returns a nonzero
result for WARN as well as FAIL, so unknown device data cannot produce an
all-PASS preflight. The current device-report contract has no platform field,
so reports are labelled platform-unidentified. A matching runtime alone proves
neither Android compatibility nor application. Confirmation requires a fresh OTA
report naming the exact selected-platform update ID; identifying a particular
phone still requires device-specific evidence.

The inventory source defaults to `http://127.0.0.1:48121` for staging and
`http://127.0.0.1:48120` for production, regardless of the publishing worktree's
development ports. Before reading `/v1/mobile/builds`, tooling checks
`/v1/status`: its desktop-server environment must be `staging` or `production`,
respectively. Mobile build reports use `staging` or `prod` instead. An
unreadable status or environment mismatch produces WARN naming the queried URL
and reported environment (UNKNOWN when unavailable), and counts no devices.
Set `KANNA_OTA_DEVICE_SERVER_URL=http://127.0.0.1:<port>` to inspect a different
local desktop instance. Output names the source and desktop id. This is a
census of that desktop's paired devices, not every installation or every
machine in the account. Environment and channel must both match. Reports over
24 hours old (or with invalid/future timestamps) are shown as stale and never
prove current reachability. Historical mismatches remain visible.

The mobile app reports build identity when establishing a trusted LAN route,
alongside the existing once-per-route pairing-material refresh. It does not
require notification permission. Offline/remote-only devices retain their last
LAN observation; older clients have no observation. Publishing this JS to a new
runtime cannot teach an already-stranded older binary to report: until it runs
a reporting-capable build, it remains UNKNOWN. Reinstall the compatible native
build to receive that runtime's publications. No OTA compatibility rule,
runtime value, or signing/provisioning behavior changes.

An in-app “this build can no longer receive updates” banner is deferred. Neither
a publisher's checkout runtime nor a newer historical bucket entry establishes
that an older runtime is retired: publishing ahead of a native rollout is
legitimate, runtime identifiers are opaque, and older pointers may still be
maintained. A reliable banner needs an explicit channel support/retirement
signal, plus bootstrap of its reader onto older installations. This change
warns the operator about the specific publication a known device cannot receive
without inventing that policy.

The canonical staging setup and verification sequence is:

```bash
./kd mobile ota provision --staging
./kd mobile ota provision-secret --staging --key-path "$HOME/.kanna/secrets/kanna-mobile-ota-v1-private-key.pem"
./kd cloud deploy --staging --relay
./kd mobile ota doctor --staging
./kd mobile ota status --staging
```

The last two commands are read-only. Publishing is a separate operation and is
not implied by provisioning or deployment.

Rollback by repointing the channel to a prior update id:

```bash
./kd mobile ota publish --staging --platform android --rollback-to <updateId>
./kd mobile ota publish --production --rollback-to <updateId>
```

Dry run a publish or rollback with `--dry-run`.

## Release Verification

Automated preflight with real staging or production cloud resources is not a CI
claim because it needs Google Cloud credentials for the target project. Run it
explicitly before human device verification:

```bash
gcloud auth application-default login
./kd mobile ota doctor --staging
```

For production, use credentials authorized for `kanna-build` and run:

```bash
./kd mobile ota doctor --production
```

Human-only post-merge verification remains: publish to staging, run the staging
app on a physical iPhone, confirm the update is fetched and applied, change a
visible JS string, republish, and confirm the replacement update applies on
foreground or restart. Agent automation must not install, launch, or run
physical-device Appium for this check.


### Android implementation and Ship acceptance

Run the opt-in real-export integration locally (no cloud writes or device access):

```sh
KANNA_RUN_OTA_EXPORT_INTEGRATION=1 pnpm --dir tools/kd exec vitest run src/runtime/mobile-ota.integration.test.ts
```

It exports Android through the publisher's command plan, stages with production
code, serves via the real relay process/local storage, verifies the signed
manifest, and compares the launch bundle and every declared asset to the export.
Evidence is written under the worktree's `.tmp/`.

For task c1516501, deployment/publication/device acceptance belongs to Ship after
implementation/review. Coordinate exclusive phone access with task 69234f81;
resolve the staging environment with `kanna_info`. The authorized Samsung serial
is `R5CX42N3NLK`, package `build.kanna.app.staging`; every adb invocation must use
`adb -s R5CX42N3NLK`. Preserve its pairing/data and send no agent input from it.
Inspect the actual installed embedded runtime, certificate/channel and launch
source first; never assume an earlier runtime is still installed. From a clean,
reviewed, lineage-eligible source:

```sh
./kd cloud deploy --staging --relay
./kd mobile ota publish --staging --platform android
./kd mobile ota status --staging --platform android
./kd mobile ota doctor --staging --platform android
```

Observe download of the exact published update, application through the existing
reload/foreground flow, then OTA launch source/update ID at the same native
runtime. Do not rebuild just to force runtime numbers to match, discard task
0d72b7af's native pairing changes, or bypass staging lineage. If credentials,
lineage, device access or runtime mismatch prevent acceptance, report that limit
and the narrower local evidence. Local integration is not on-device delivery.
The existing physical-iPhone human check above remains unchanged. Production
publication/promotions still require an explicit human request.
