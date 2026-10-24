# Shared agent sign-in probe

Model for [agent sign-in](../../../docs/specs/agent-sign-in.md) and
[Fleet §10](../../../docs/specs/fleet.md#10-build-order), before Fleet 9.
This is a throwaway probe, not Kanna product code.

**Status: VMs and harness ready; awaiting the user's sign-in. No live inference result yet.**
Fleet 9 must not treat preparation, CLI help, or synthetic tokens as a passing model.

## Run

Requires Python 3 on this Mac, `claude` and `codex` on PATH, and passwordless SSH/sudo
to r720-1. The current Mac is the holder; there is no Mac mini.

```sh
# Once on r720-1, if Incus/QEMU are absent:
ssh jeremyhale@172.16.0.105 'sudo apt-get update && sudo apt-get install -y incus qemu-system-x86'

# On this Mac, from the repo root:
bash references/prototypes/agent-sign-in/prepare.sh

# THE USER runs this interactively, completing both providers' browser flows:
python3 references/prototypes/agent-sign-in/probe.py sign-in

# After sign-in, the agent or user runs:
python3 references/prototypes/agent-sign-in/probe.py run > /tmp/agent-sign-in-results.json
```

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

The holder materializes its Codex login into a private temporary directory only
while asking Codex app-server to refresh it with `account/read {refreshToken:true}`.
It saves the updated login back to Keychain before deleting that directory.
**Only `tokens.access_token` leaves the holder.** No login file, refresh token,
ID token, or account email is sent to the VMs. Credentials travel on SSH stdin,
never shell command lines. Each worker gets only its own provider's credential.

## What the probe measures

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
Worker temporary homes, including any access-token cache, are removed after use.

`passed: true` requires all four pairs of requests to succeed, positive process
overlap for each pair, no refresh credential in either Codex worker's auth cache,
and a changed holder access token in round two. Timing overlap measures concurrent
CLI lifetimes; it is not server-side scheduling telemetry.

This deliberately does **not** claim to test natural token expiry in a running
conversation, revocation propagation, enrollment while the holder is offline,
Keychain recovery, API-key accounts, or hundreds of workers. Those need additional
evidence. A refreshed token accepted by a new process is not mid-run renewal.

## Evidence so far

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
- A synthetic JWT passed to the installed Codex `--with-access-token` command
  failed in its **Agent Identity** parser. This is not evidence that a real
  ChatGPT OAuth access token succeeds or fails; that live check is pending.
- No subscription credentials have been used and no live inference has passed yet.

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
- The installed 0.163.0 app-server schema exposes `chatgptAuthTokens`, but labels it
  **unstable / for OpenAI internal use only**. Do not silently substitute that
  interface and call the spec's CLI path verified. Generate the schema with
  `codex app-server generate-json-schema --out /tmp/codex-schema` to inspect it.
- [OpenAI Terms of Use](https://openai.com/policies/terms-of-use/)
  prohibit making an account available to other people and bypassing usage limits.
  This probe is one person's machines and does not attempt to increase quotas.
  A technical pass does not authorize resale or multi-user credential sharing.

## What Fleet 9 should copy, once observed

Copy the evidence contract: real inference on two isolated VMs, overlapping runs,
one unchanged Claude setup token, holder-only Codex refresh, access-only worker
payloads, and another successful pair after refresh. Preserve failures as failures;
successful `login status` is insufficient.

Do not copy SSH/Incus transport, temporary-home management or this Python Keychain
adapter into the product. The product supplies its peer link, secret storage,
account removal/revocation, sandbox deny lists and user-visible sign-in recovery.

**Surprise so far:** Codex's access-token flag and its ChatGPT login access token
are not documented as the same credential type. The model must settle that before
Fleet 9 chooses its sharing path. The Claude product-credential restrictions also
need to be kept separate from the narrower technical concurrency result.

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
