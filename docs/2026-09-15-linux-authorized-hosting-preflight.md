# Authorized Linux staging hosting: execution preflight

Owner authorization received verbatim: **“go ahead with your recommendations
for staging release, linux apt hosting etc.”** The accompanying directive
explicitly approves dedicated apt.kanna.build on existing relay infrastructure,
scoped DNS/Caddy/unprivileged publisher setup, protected apt signing custody on
the trusted MBP, and exact retained B staging publication once gates pass.
Production promotion and soak override remain unauthorized. This supersedes
older reports saying hosting approval itself is pending.

## Actual execution and result

- Own clean controller fast-forwarded to merged PR1514 `dc75f7a3b`.
- Kanna info: effective http://127.0.0.1:48121, authoritative Jeremy's Mac
  Studio, staging 0.3.0-staging.20; separately advertised 0.0.0.0:48121.
- Authenticated read-only command:
  `gcloud compute instances list --project kanna-staging --format='json(name,zone,status,machineType,networkInterfaces.networkIP,networkInterfaces.accessConfigs.natIP,disks.source)'`
  **Exit 1, no inventory returned:**
  `There was a problem refreshing your current auth tokens: Reauthentication failed. cannot prompt during non-interactive execution.`
  Diagnostic requests `gcloud auth login`. No credential retry, token print,
  host selection, host-key bootstrap or mutation followed this refusal.
- `./kd release status --platform linux`: promotion.allowed=false; local
  backend/root/baseUrl/validForHours/publicKeyPath/fingerprint unset. This is
  actual missing configuration, not a withdrawn setup authorization.
- Coordinated with MBP Ship6a8d78d0: requested existing authenticated inventory
  and safe key/config presence information; warned against concurrent release
  environment edits or a full relay deploy. Its response remains pending at
  this report. No apt secrets were requested in messages.

## Named host proposal and canonical setup boundary

The registry declares staging `kanna-relay-staging`, project `kanna-staging`,
zone us-central1-a, and production `kanna-relay-vm`, project `kanna-build`.
These are source declarations, not successful live inventory. Prefer the
staging host for this authorized staging archive after authenticated checks;
no production-service deployment is inferred.

The approved setup topology remains unprivileged kanna-apt, local POSIX
/srv/kanna-apt/archive, pinned SSH transport and MBP-only signing, with a
read-only Caddy mount. Current canonical `cloud.relay-provision` only returns a
broad new-VM/IAM plan. `cloud.deploy --relay` builds/pushes a relay image,
rewrites its environment, pulls and starts the full compose stack. Neither is
a scoped existing-host apt account/DNS/key setup operation. The repository's
setup proposal supplies desired configuration, but no such executing kd
command. Do not replace this gap with raw provisioning or deploy the relay
merely to create an apt archive.

Next concrete requirements: obtain successful authenticated inventory on the
intended trusted execution host (reauthenticate there if needed), then provide
the bounded canonical setup path for the approved existing-host changes,
including independent host-key pinning and MBP protected key/config handling.
The retained-B publication command is already implemented and documented in
2026-09-15-linux-publication-next-step.md; it needs no native rebuild.

No verified archive/public-key/package URL or fingerprint exists from this
attempt. No package, key, DNS, Caddy, host, publication receipt or soak changed.
B stays a9df2f1d46fb08bcf53200b738e0fdc3c127b636 / 0.2.0 staging.2. The four
prepared A/B artifacts and floor acceptance stay intact. Ship remains open;
website76e93c62 receives actual verified resources only after publication.

## Update: trusted MBP authentication confirmed; Studio repair unnecessary

Ship6a8d78d0 supplied successful authenticated MBP inventory:

| Project / VM | State / zone | External IP | Disk |
| --- | --- | --- | --- |
| kanna-staging / kanna-relay-staging | RUNNING / us-central1-a | 34.133.43.193 | kanna-relay-staging, 30GB |
| kanna-build / kanna-relay-vm | RUNNING / us-central1-a | 34.133.233.111 | kanna-relay-vm, 30GB |

Both report internal 10.128.0.2 in their respective projects. Select the staging
VM for the approved archive; there is no unresolved owner host choice.
The Studio's failed credential refresh is historical and no longer blocks
inventory. No login, credential transfer or Studio reauthentication is needed.
The stale authentication attention badge has been cleared.

MBP reports existing admin SSH public keys (RSA3072 and ED25519), but these
are neither apt signing keys nor an approved dedicated publisher identity.
No ~/.gnupg exists; this alone does not establish whether a suitable apt key
exists elsewhere. No key was created/exported or selected. The shared release
environment file remains untouched; MBP owns simultaneous desktop/mobile
release work at dc75f7a3, independent of Linux B. Linux selector installation
must be coordinated as a narrow merge preserving all existing selectors.

The selected-host inert configuration is
`docs/evidence/2026-09-15-linux-bootstrap/publication-preparation/staging-archive.env.example`.
It deliberately fails configuration validation until a real apt fingerprint is
supplied; approved URL and credential paths are desired configuration, not
claims of provisioning. Do not install/source this template unchanged.

### Exact remaining engineering boundary

Provide a canonical existing-host apt setup operation covering the approved
account/archive, authenticated SSH host-key pin, dedicated protected MBP keys,
DNS ownership/readback and Caddy-only configuration. It must inventory and pin
the current relay image/config first, refuse unexpected existing resources,
retain rollback material, validate Caddy configuration and preserve the relay
container/environment. It must not invoke the current full relay build/deploy,
create a new VM/IAM role, access mobile signing secrets, or print private keys.
DNS provider/account ownership and VM free space/current service configuration
still need authenticated read-only inspection through the MBP before mutation.
The static 30GB disk size is not available-capacity evidence.

No further hosting approval is requested. This is a missing canonical setup
implementation, not a missing owner decision or credential blocker. Current
repo rules forbid inventing a raw release/provisioning workaround. Publication
can use the already merged retained-B route once this boundary and real setup
are complete. No verified public URLs/fingerprint/receipt are available yet.
