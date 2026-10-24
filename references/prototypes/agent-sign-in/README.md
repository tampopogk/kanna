# Shared agent sign-in probe

Model for [agent sign-in](../../../docs/specs/agent-sign-in.md) and
[Fleet §10](../../../docs/specs/fleet.md#10-build-order), before Fleet 9.
This is a throwaway probe, not Kanna product code.

**Status: shared Claude and Codex holder flows verified (2026-10-25 JST).**
One Mac login supplied two isolated VMs with Codex access tokens through the
experimental app-server `chatgptAuthTokens` interface. [Live evidence](external-token-results.json)
records **six successful requests**: concurrent initial use, replacement after
holder refresh, and recovery from an injected invalid signature. Both workers
kept the same app-server process and thread throughout; neither had a login file
or refresh credential in its isolated home.

The earlier [CLI-token rejection](shared-token-results.json) and successful
[independent-login fallback](fallback-results.json) remain valid. The failure was
the `--with-access-token` import interface, not an inability to share OAuth access
tokens. **This passing route requires app-server integration; it does not establish
sharing for the ordinary Codex terminal UI.** Public docs call it experimental,
while the pinned binary's schema still says internal-only. Fleet 9 must explicitly
account for this version/support mismatch. Product terms checks remain separate.

## Run

Requires Python 3 on this Mac, `claude` and `codex` on PATH, and passwordless SSH/sudo
to r720-1. The current Mac is the holder; there is no Mac mini.

```sh
# Once on r720-1, if Incus/QEMU are absent:
ssh jeremyhale@172.16.0.105 'sudo apt-get update && sudo apt-get install -y incus qemu-system-x86'

# On this Mac, from the repo root:
bash references/prototypes/agent-sign-in/prepare.sh

# THE USER runs Claude's TUI by itself in one terminal:
claude setup-token

# In a SEPARATE terminal, paste the resulting token at the hidden prompt:
python3 references/prototypes/agent-sign-in/probe.py store-claude

# In another terminal, complete Codex's browser flow:
python3 references/prototypes/agent-sign-in/probe.py sign-in-codex

# Verify the holder through app-server (no per-VM sign-in):
python3 references/prototypes/agent-sign-in/external.py > /tmp/agent-sign-in-external-results.json

# Reproduce the original shared-Claude success / Codex CLI rejection:
python3 references/prototypes/agent-sign-in/probe.py run > /tmp/agent-sign-in-results.json
```

To reproduce the independent-login fallback instead, the user runs these
in **two separate terminals**, following each device-code sign-in in their browser:

```sh
bash references/prototypes/agent-sign-in/sign-in-vm.sh agent-sign-in-a
bash references/prototypes/agent-sign-in/sign-in-vm.sh agent-sign-in-b
```

After both complete:

```sh
python3 references/prototypes/agent-sign-in/probe.py run-fallback > /tmp/agent-sign-in-fallback.json
```

These are independent Codex logins created on the VMs, stored with private
permissions under `/home/probe/.local/share/agent-sign-in/codex`. Neither is copied
from the Mac or the other VM. The fallback sends **no credential** over the probe
link and explicitly records `credential_mode: per_machine_login` and
`refresh_credential_present: true`. `fallback_passed` requires two overlapping
pairs of real Codex responses. It must not be reported as holder-token sharing.

`prepare.sh` accepts an alternative SSH destination as its first argument;
`probe.py run` accepts `--host DESTINATION`. It creates `agent-sign-in-a` and
`agent-sign-in-b`, Ubuntu 26.04 Incus **VMs**, each with 2 vCPUs, 4 GiB RAM, and
a 12 GiB disk. `auth-probe` is a directory-backed storage pool. `auth-probe0`
is a private NAT bridge, 10.199.64.1/24. This host uses Wi-Fi, so these probes
use SSH + Incus exec, not Fleet's eventual LAN bridge or encrypted peer link.
No existing Incus profile or host uplink is changed. CLI installs are pinned to
Codex 0.163.0 and Claude Code 2.1.310, matching the Mac at preparation time.

Sign-in stores two entries in the Mac's default Keychain, service
`kanna-model-agent-sign-in`, accounts `claude` and `codex-holder`. The Claude
command prints a setup token in the user's terminal; paste it at the hidden
prompt, never into chat or the repo. Codex signs into a separate temporary home;
the helper stores that login in Keychain and removes the temporary home. It does
not read or replace the user's everyday Claude or Codex login.

Keep Claude's TUI and the token-saving prompt in separate tabs. Running them
consecutively in one terminal mixed the TUI's output with the hidden prompt in
the first attempt. When opening these tabs with `kanna open`, supply `--cwd`
pointing to this worktree and an absolute script path: a task tab can otherwise
start in the main checkout, where this unmerged prototype does not exist yet.

The holder materializes its Codex login into a private temporary directory only
while asking Codex app-server to refresh it with `account/read {refreshToken:true}`.
It saves the updated login back to Keychain before deleting that directory.
**Only the access token and account ID leave the holder** in the app-server probe
(the older CLI probe sends just the token). No login file, refresh token, ID token,
or separate account email is sent to the VMs. The access JWT itself contains identity
claims: workers can read those, so it is still a sensitive credential. Credentials travel on SSH stdin,
never shell command lines. Each worker gets only its own provider's credential.

## What the app-server probe measures

`external.py` starts one app-server in a fresh private home on each VM, opts into
`experimentalApi`, and calls `account/login/start` with `type: chatgptAuthTokens`,
`accessToken` and `chatgptAccountId`. It starts an ephemeral thread and makes three
turns on that same thread/process:

1. Both VMs receive the same current access token from the Mac holder.
2. The Mac forces refresh through its own managed Codex login, saves the rotated
   credential in Keychain, and supplies the new access token to both running workers.
3. The coordinator changes one character in the JWT signature (claims unchanged).
   Codex requests `account/chatgptAuthTokens/refresh` with reason `unauthorized`.
   The worker checks the account ID, asks the Mac, and returns the replacement.
   Concurrent requests share **one** serialized holder refresh; Codex retries.

`passed` requires six real marker responses, overlapping turn lifetimes in all
three phases, changed holder tokens, no worker refresh credentials, and successful
renewal callbacks on both VMs. Only allowlisted evidence is emitted. The worker
never reads either VM's existing fallback login. Tokens travel through SSH stdin
and app-server stdin, not argv or environment. The holder is the only process
allowed to use its rotating refresh credential.

[Evidence](external-token-results.json), recorded 2026-10-24 21:59 UTC / 25 October
JST on Codex 0.163.0:

| Phase | Real responses | Overlap | Worker renewal requests | Holder refreshes on request |
|---|---:|---:|---:|---:|
| Initial token | 2/2 | 2.442 s | 0 | 0 |
| Holder-refreshed token | 2/2 | 3.284 s | 0 | 0 |
| Injected invalid signature | 2/2 | 6.369 s | 2 per VM | 1 total |

No worker `auth.json` was created. The second phase explicitly refreshed once on
the holder before starting its turns. The third refreshed once on demand and
coalesced all four callbacks. Do not assume one callback per user turn: background
requests can also need renewal. The docs give the callback about ten seconds;
a production holder needs fast responses, deduplication and bounded recovery.

`python3 references/prototypes/agent-sign-in/external-selftest.py` checks isolation,
callback forwarding, success parsing and secret redaction with a fake app-server.
It deliberately echoes synthetic secrets in vendor errors and untrusted fields.

This tests recovery from a **deliberately invalid signature**, not waiting for
natural expiry. It does not test a holder outage, refresh revocation, account
removal, VM enrollment offline, cross-process holder locking, or the ordinary
Codex TUI. Timing overlap measures client turn lifetimes, not backend scheduling.

## What the original CLI probe measures

Each round starts both VMs against one credential at a shared future timestamp.
The worker runs as an unprivileged `probe` user in a fresh temporary home with an
allowlisted environment; existing credentials, hooks and user settings cannot
provide a fallback. Requests ask for `KANNA_AUTH_PROBE_OK`, with no tool use.

1. Both Claude CLIs receive the **same** setup token in `CLAUDE_CODE_OAUTH_TOKEN`.
2. Both Codex CLIs receive the **same** holder access token on stdin to
   `codex login --with-access-token`, then run `codex exec` if login succeeds.
3. The Mac holder alone requests a refresh; a second round repeats both providers.
   Claude keeps its original setup token. Codex receives the holder's new access token.

The report contains only an allowlist: CLI versions, login/inference booleans,
known error categories, timestamps, token-change boolean and overlap duration.
Raw stdout/stderr, credentials, JWT claims and token hashes are excluded.
Worker temporary homes, including any imported access-token cache, are removed
after use. The fallback's independent login remains in its private VM state folder.

`passed: true` requires all four pairs of requests to succeed, positive process
overlap for each pair, no refresh credential in either Codex worker's auth cache,
and a changed holder access token in round two. Timing overlap measures concurrent
CLI lifetimes; it is not server-side scheduling telemetry.

The original CLI probe does **not** claim to test natural token expiry in a running
conversation, revocation propagation, enrollment while the holder is offline,
Keychain recovery, API-key accounts, or hundreds of workers. Those need additional
evidence. The app-server probe above adds failure recovery within the same running process/thread.

## Evidence (2026-10-24)

- Host: r720-1, Ubuntu 26.04.1 LTS, x86_64; `/dev/kvm` available. Incus 6.0.5
  and QEMU 10.2.1 installed for this model.
- Both VMs are running; pinned CLIs start as the unprivileged `probe` user.
  [Preflight evidence](preflight.json) records KVM virtualization, synchronized
  clocks and a synthetic-token rejection through the actual SSH/Incus worker
  transport on both VMs. This is infrastructure evidence, not authentication success.
- Mac: Codex 0.163.0; Claude Code 2.1.310. Both required commands exist.
- Keychain storage: create/read/update/delete passed with synthetic data.
- `python3 references/prototypes/agent-sign-in/selftest.py`: passes without accounts
  or network, checking environment isolation, success/rejection parsing, and
  redaction even when a fake CLI echoes a synthetic secret in its output.
- The same Claude setup token completed **four real inference requests**, two on
  each VM. CLI lifetimes overlapped by **5.878 s** in round one and **5.680 s** in
  round two. Both VMs returned the exact requested marker, with exit code zero.
- The Mac holder's `account/read {refreshToken:true}` completed and changed its
  access token. Only the holder retained the refresh credential.
- Both VMs rejected both the original and refreshed **real ChatGPT OAuth access
  tokens** through `codex login --with-access-token`, in the Agent Identity JWT
  path. There were **zero Codex inference requests** in this path, because login
  failed. The report correctly has `passed: false`.
- This is a reproducible incompatibility in Codex 0.163.0, not a hypothetical
  failure inferred solely from CLI help or a synthetic JWT.
- With independent user-created logins, Codex completed **four real inference
  requests**, two per VM, returning the exact marker with exit code zero. CLI
  lifetimes overlapped by **9.953 s** and **10.150 s**. The evidence has
  `fallback_passed: true`. Both refresh credentials remain local to their own VMs;
  each auth file is owned by `probe` with mode `0600`.

## Interface and terms findings

Checked during preparation; these sources can change:

- [Claude authentication](https://code.claude.com/docs/en/authentication#generate-a-long-lived-token)
  documents `setup-token` for scripts, the `CLAUDE_CODE_OAUTH_TOKEN` environment
  variable and a one-year lifetime. Bare mode ignores that variable, so the probe
  does not use `--bare`.
- [Claude legal/compliance](https://code.claude.com/docs/en/legal-and-compliance)
  allows an end user to sign in to unmodified Claude Code, including hosted
  environments, while restricting third-party collection/storage/intermediation
  of Claude.ai credentials. This personal CLI experiment is not approval for
  Kanna's planned subscription-token vault. Fleet 9 needs to resolve that product
  design against current terms, or use the per-machine vendor-flow fallback/API keys.
- [OpenAI authentication](https://learn.chatgpt.com/docs/auth#use-codex-access-tokens-for-enterprise-automation)
  describes `--with-access-token` for Enterprise automation tokens. The flag's
  name alone does not establish support for an ordinary login's OAuth access token.
  [Current source](https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/manager.rs)
  classifies it as a personal access token or an Agent Identity JWT. This source
  is supporting context, not a substitute for probing the installed binaries.
- The more specific [access-token guide](https://learn.chatgpt.com/docs/enterprise/access-tokens)
  currently lists **Business and Enterprise** workspaces. The user creates a
  personal access token in the ChatGPT admin console's Access tokens page, with
  Codex scope and the workspace's token/local-Codex permissions enabled. This is
  a distinct credential, not the `access_token` field extracted from browser login.
  That route has not been tested with this user's account and is not evidence for
  the spec's short-lived OAuth holder design.
- The [public app-server docs](https://learn.chatgpt.com/docs/app-server#authentication-endpoints)
  explicitly document external ChatGPT tokens for hosts owning the auth lifecycle,
  experimental opt-in, and a refresh callback. This led to the passing follow-up
  probe. The installed 0.163.0 schema still labels this **unstable / internal-only**:
  record the discrepancy rather than claiming a stable CLI interface. Generate it
  with `codex app-server generate-json-schema --out /tmp/codex-schema`.
- [OpenAI Terms of Use](https://openai.com/policies/terms-of-use/)
  prohibit making an account available to other people and bypassing usage limits.
  This probe is one person's machines and does not attempt to increase quotas.
  A technical pass does not authorize resale or multi-user credential sharing.

## What Fleet 9 should copy

Copy the Claude evidence contract: real inference on two isolated VMs, overlapping
runs, and the same unchanged setup token working again in fresh processes.
Resolve the product terms question above before shipping a subscription-token vault.

For Codex, copy the **app-server holder contract**: one managed login and one
refresh owner; only access token plus account ID delivered to workers; external
login and renewal RPCs; concurrent requests coalesced into one refresh; real
inference before/after replacement and after an injected authorization failure;
no refresh credential on either worker. Supply tokens through the daemon's private
app-server connection. Keep the per-machine sign-in fallback when that integration
is unavailable. This model is not a TUI auth shim: a product retaining the ordinary
Codex terminal command needs its own passing integration probe.

Keep the negative CLI regression too: ordinary ChatGPT OAuth tokens are rejected
by `--with-access-token` in 0.163.0. Never copy a rotating login file between VMs.
Successful login status alone is insufficient; require real inference.

Do not copy SSH/Incus transport, temporary-home management or this Python Keychain
adapter into the product. The product supplies its peer link, secret storage,
account removal/revocation, sandbox deny lists and user-visible sign-in recovery.

**Surprises:** the token was shareable; the CLI flag was the wrong importer.
Public app-server documentation describes a route the installed schema labels
internal-only. A running thread recovered without restarting, and each VM emitted
two refresh callbacks, all served by one holder refresh. Claude concurrency worked;
its product terms remain separate. Sign-in also needed separate TUI and token-capture
terminals so the hidden prompt was usable.

## Cleanup

The probe deliberately leaves its VMs for reruns. After reviewing the evidence,
the user can remove only these named resources:

```sh
ssh jeremyhale@172.16.0.105 'sudo incus delete --force agent-sign-in-a agent-sign-in-b'
ssh jeremyhale@172.16.0.105 'sudo incus network delete auth-probe0 && sudo incus storage delete auth-probe'
security delete-generic-password -s kanna-model-agent-sign-in -a claude
security delete-generic-password -s kanna-model-agent-sign-in -a codex-holder
```

Deleting the local Keychain entries does not revoke provider tokens. Revoke them
in the providers' account controls if needed. Interrupted temporary directories
may need manual cleanup outside the repo. Production durability and secret-lifetime
hardening are intentionally left to Fleet 9.
