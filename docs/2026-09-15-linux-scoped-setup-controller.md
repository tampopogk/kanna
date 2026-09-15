# Scoped Linux archive setup controller

Owner authorized archive/account/DNS/Caddy/MBP-key setup and exact retained-B
staging publication. This change implements the missing canonical setup surface
in the existing Ship task. Product B and its prepared artifacts are unchanged.

## Scope and evidence

`kd release setup-linux --staging --mode inspect|plan|apply` targets only the
verified existing staging VM. It reuses the environment registry, pinned SSH
options, storage RPC helper, OpenPGP signer validation, and protected
machine-selector writer. No generalized provisioner, VM/IAM creation, Cloud DNS
enablement, relay image build/pull or native product build is introduced.

Tests use disposable filesystems, keys and synthetic external command results.
They execute host rendering and apply/retry logic with system operations fenced
to fixture functions, and exercise protected local key generation/selector merge
with actual cryptographic validation. Coverage includes remote read-only plan
scope, wrong host/account/changed plan, supplied/authenticated host pins, secret
output suppression, unowned vhost/layout refusal, no duplicate account/Caddy
change on retry, immutable key identity, concurrent config refusal, maintenance
and active-connection refusal, and preserved desktop selector lines. They do not
claim actual host setup or HTTPS acceptance.

## Actual read-only state

MBP authenticated gcloud inventory confirms project kanna-staging,
kanna-relay-staging, us-central1-a, numeric instance ID6655221359129467471,
RUNNING, external34.133.43.193, 30GB disk. Free capacity remains unmeasured.
The Studio token refresh refusal no longer blocks inventory; no login is needed.

Guest hostkeys/ lookup returned authenticated404, with no enabling/retry.
A pre-existing MBP google_compute_known_hosts line2 for
compute.6655221359129467471 supplied the public key retained in
publication-preparation/staging-host-key.pub. It was correlated to the current
numeric instance ID through authenticated inventory. Its historical acquisition
method/time was not independently established. No TOFU, ssh-keyscan or new SSH
trust write has occurred. Actual pinned SSH must still succeed.

Owner completed DNS. Both ns51/ns52.domaincontrol.com and recursive1.1.1.1 and
8.8.8.8 return apt.kanna.build A34.133.43.193; authoritative queries returned no
AAAA/CNAME. No DNS account action/approval is pending. Cloud DNS is disabled and
is not this zone's authority; it was not enabled.

## Real operational constraint before apply

The approved read-only Docker mount requires a Caddy-only recreation. Preserving
the relay process does not preserve its active proxy WebSockets. The tool now
refuses active connections and requires an explicit coordinated maintenance
window. It makes no zero-downtime guarantee and does not quit owner clients.
The desktop/mobile staging.21 ship currently owns the MBP; no Linux selector or
key mutation should overlap it without the coordinated slot. The exact existing
administrator username/key and pinned SSH/sudo reachability still need checking.

After focused PR/MM review: run inspect/plan on the MBP, assess the real host
snapshot and coordinate the Caddy window, apply, then verify the public key over
HTTPS before installing selectors. Finally run canonical retained B staging
ship, status and hand off verified artifact/key URLs, fingerprint and receipt
to website76e93c62. No release URL, installed host setup, public apt key, release
receipt, soak or production claim is made by this controller change.

## Final bounded inventory and invocation

MBP verified existing admin username **jeremyhale** by exact public-key match
between project commonInstanceMetadata ssh-keys and
`/Users/jeremyhale/.ssh/google_compute_engine.pub`. No matching instance-level
key or enable-oslogin/block-project-ssh-keys flags were returned. No SSH or
metadata write occurred. MBP retained raw evidence at its task-owned
`.tmp/linux-admin-username-inventory.json`. Its desktop/mobile commands have
finished and the shared release environment remained untouched.

After this code passes ordinary focused PR/MM assessment, the concrete
read-only command from the current controller checkout on the MBP is:

```sh
./kd release setup-linux --staging --mode inspect \
  --admin-user jeremyhale \
  --admin-identity /Users/jeremyhale/.ssh/google_compute_engine \
  --host-key-file "$PWD/docs/evidence/2026-09-15-linux-bootstrap/publication-preparation/staging-host-key.pub"
```

Use `--mode plan --out .tmp/linux-archive-plan.json` with the same selectors to
retain the exact plan. Do not apply it while relay clients are connected or
without the coordinated Caddy maintenance window. The MBP release completion
is not an assertion that relay clients are disconnected. Actual SSH, sudo,
filesystem capacity, layout and running proxy state remain to be measured.

Validation: seven focused suites (`linux-archive-setup`, `linux-ssh-storage`,
`release-env`, `linux-release-commands`, `linux-release-prepare`, `release-tasks`,
`cloud-deploy`)
passed **97 tests**. Typecheck passed. Plan/host tests use disposable fixtures;
no real account, DNS, key, host, Caddy, release configuration or package was
changed. The PR head uses `[skip ci]` to avoid broad unchanged native rebuilds;
no new native acceptance or publication is claimed.

Future explicitly authorized staging relay deploys retain the managed apt mount
and vhost through the root-owned setup renderer before compose pull/up. The
production deploy path is unchanged. Setup itself refuses a local floating
Caddy image that differs from the running image, so its Caddy-only recreation
cannot silently upgrade the proxy. Fixture tests execute the preservation
renderer after simulated base-template upload.
