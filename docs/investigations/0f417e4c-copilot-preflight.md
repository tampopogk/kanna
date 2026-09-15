# Copilot no-inference extension preflight

2026-09-15, task `0f417e4c`. **Same-session attachment passed.** The extension
loaded and `joinSession()` returned the exact UUID of its parent TUI in 2.43
seconds. **The proposed `source: "system"` send contract is not supported by the
inspected high-level SDK serializers:** both locally cached versions omit that
option. No send or inference was attempted, so no delivery claim follows.

## Authorization and actual execution

The owner explicitly authorized the proposed bounded 90-second disposable
Copilot NO-INFERENCE preflight, limited to extension loading, `joinSession`,
process/session identity and installed SDK inspection. No further permission
gate applied to this check. Paid/live inference beyond the earlier Claude
experiment remained separate. This note supersedes the comparison's historical
pending-authorization statement for this preflight only.

One disposable TUI was launched through the existing
`tests/cli-contract/helpers/pty-bridge.py`. No keystrokes, prompts, SDK sends,
tools or model requests were submitted. The fixture extension called only
`joinSession()`; it read `sessionId` and inspected `send.toString()` without
calling it. It added no tools, permission handlers or alternate agent runtime.

The installed CLI's `help environment` documents `COPILOT_OFFLINE=true` and
`COPILOT_PROVIDER_BASE_URL` for local-provider operation without GitHub
authentication. Used those with a local HTTP fixture that rejects every request.
The fixture recorded **zero requests**. No account credential was needed or
copied. Inherited Kanna, Claude, Anthropic, Codex, Copilot, GitHub, Google,
OpenAI and Node configuration variables were excluded by the runner's recorded
prefix filter. `COPILOT_HOME`, XDG paths, GH config and TMPDIR were redirected
inside `.tmp/copilot-preflight-0f417e4c`; HOME was not changed.

Actual command (absolute fixture paths expanded in the evidence):

```sh
copilot -C "$EXPERIMENT/workspace" \
  --no-auto-update --no-remote-export --no-custom-instructions \
  --disable-builtin-mcps --experimental \
  --session-id 3cc8cdaf-94a3-4df9-921d-ce12c1c577ae \
  --model mai-code-1.1-flash \
  --log-dir "$EXPERIMENT/logs" --log-level debug
```

The model name was configuration only; **no model ran and usage was zero**.
Runtime 1.0.64 warned that the name was absent from its built-in catalog, using
default token limits for the local provider. It did not substitute or invoke a
different model. That warning is not an account-availability finding.

The extension was project-local under the isolated fixture repository's
`.github/extensions/kanna-preflight/extension.mjs`. The disposable config
trusted only that fixture folder, disabled memory/banner/updates, and enabled
experimental features. No account or global configuration was changed.

## Process and session identity

| Observed item | Value |
| --- | --- |
| PTY bridge PID | `20974` |
| Parent Copilot TUI PID | `21288` |
| Copilot-owned extension PID / PPID | `25468` / `21288` |
| Launch UUID, extension `SESSION_ID`, returned `session.sessionId` | `3cc8cdaf-94a3-4df9-921d-ce12c1c577ae` in all three places |
| Extension stdin/stdout | Pipes, not TTYs |
| Host operation recorded by the CLI | `session.resume` for that UUID, `disableResume: true`, permission/user-input handlers disabled |
| Host readiness record | `Extension ready` |
| Active test duration | `2.43s` of the allowed `90s` |

The extension process used the same executable to run the host's
`preloads/extension_bootstrap.mjs`. This is the extension child, not a second
interactive/headless agent session. The only observed owned process tree was
bridge → TUI → extension. The session was created solely for this experiment.

## Runtime and SDK compatibility finding

Plain `copilot --version` reports **1.0.83** on this machine. However,
`copilot --no-auto-update --version` reports **1.0.64**, and the experiment's
TUI banner, debug log and SDK resolver all corroborate **1.0.64**. A read-only
version check with offline/local-provider settings but without the flag still
reported 1.0.83. Thus the launch flags materially affect which installed runtime
runs here; the preflight must not be reported as a live 1.0.83 result.

Loaded SDK:
`~/Library/Caches/copilot/pkg/darwin-arm64/1.0.64/copilot-sdk/extension.js`, SHA-256
`c4d7588911b6feb44522489bb77712120a7bdedc44a82e32f58e280bfebdd9c1`.
The cached 1.0.83 SDK was inspected read-only afterward, SHA-256
`7c6498e44e5e6d7718bdfb14ffa3b03b0eb07f51e2fc178c9a1771a6482b945d`.
Neither extracted SDK directory exposes a package.json with an independent
package version; identify them by runtime path/hash, not an invented SDK semver.
Both declare SDK protocol version 3.

Both high-level `send()` implementations explicitly construct the request with
`sessionId`, `prompt`, `displayPrompt`, `attachments`, `mode`, `agentMode`,
`requestHeaders` and trace context. Neither forwards `options.source`. Therefore
passing `source: "system"` as in the newer documented candidate would silently
omit that field in these installed SDKs. The raw runtime's behavior if given
that field was **not tested**. No private RPC workaround, SDK override, download,
upgrade or second TUI experiment was attempted.

## Result, limits and cleanup

This resolves the local attachment question for the actual 1.0.64 runtime and
identifies a specific serializer mismatch in both inspected SDKs. It does not
prove 1.0.83 TUI attachment, message enqueue, model-visible source handling,
draft/cursor preservation, busy/permission behavior, or recovery after an
uncertain send. No mailbox subscription was created or mutated.

Do not enable the candidate claiming native `source: system` support on this
evidence. Any implementation must explicitly resolve its provenance contract
and actual runtime selection first; do not silently downgrade, use PTY fallback,
or modify ordinary explicit input. No additional model test is needed merely to
establish the already-observed serializer omission.

[Machine-readable evidence](../evidence/0f417e4c-copilot-no-inference-preflight.json)
includes exact argv/environment, fixture source/hash, process tree, SDK hashes
and static method bodies, session-resume/readiness log excerpts and cleanup
assertions. Checked UUID equality, parent PID, empty provider-request log,
absence of all owned PIDs, closed fixture listener, JSON validity and local
links. Removed the disposable repo, extension/config, session store, temp and
XDG/GH directories. No credentials were used. Raw synthetic traces and runner
remain in `.tmp`; shared installed SDK caches were not removed. No background
processes remain. Only this report/evidence and comparison update are committed;
the parent's unfinished product source changes remain separate.

Public contract references: [extension overview](https://docs.github.com/en/enterprise-cloud@latest/copilot/concepts/agents/copilot-cli/about-cli-extensions)
and [extension tutorial](https://docs.github.com/en/enterprise-cloud@latest/copilot/tutorials/create-an-extension).
