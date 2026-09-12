# OpenCode models in Kanna

OpenCode owns provider connections, credentials, model definitions and tool
permissions on the **machine running the task**. Kanna chooses a native
`provider/model` identifier for a run. A loopback endpoint on another machine
is not the operator's local model server.

Configure providers in OpenCode's global or project `opencode.json` / JSONC,
using OpenCode's `/connect` for cloud credentials. For example, a local
OpenAI-compatible server can be registered as follows (replace the model ID,
port and limits with those your server actually supports):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "provider": {
    "local": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Local model server",
      "options": { "baseURL": "http://127.0.0.1:8000/v1" },
      "models": {
        "YOUR_LOCAL_MODEL_ID": {
          "name": "Local coding model",
          "tool_call": true,
          "limit": { "context": 32768, "output": 8192 }
        }
      }
    }
  }
}
```

Use persistent model storage and a stable endpoint in your serving application
(oMLX, Ollama, or another compatible server). Kanna does not download weights,
start or restart servers, or change their memory/cache settings. Include the
repository instructions and MCP tools when sizing the context window.

## Selecting a model

In **New Task**, choose OpenCode and use the model field. The suggestions come
from the execution machine's installed OpenCode CLI, in the repository's
configuration context. You can refresh them or enter a native ID directly.
Blank keeps normal Kanna/OpenCode model resolution; it does not reset defaults.
Discovery runs `opencode --pure models --verbose` and `--pure debug config`,
with automatic updates and model-catalog refresh disabled. It does not run
inference. Plugin-provided inventory may differ because discovery excludes
external plugins. A repository checkout may also differ from a later stage's
worktree; suggestions are advisory, not an allowlist.

The local task header's **Next stage model…** control reads this task's pinned
workflow. Local and cloud IDs use the same field. At a direct stage advance,
it sends the existing one-time override with operator provenance; the stage
after it resolves its own configuration normally.

When the current stage has a post, **Save model for…** changes only the next
stage's provider selector in this task's pinned workflow. It uses the existing
compare-and-set workflow API and records an operator-authored workflow change.
It neither advances the task nor skips the post: advance normally when ready.
This saved choice also applies to future reruns of that stage. Concurrent edits
are rejected without retrying or overwriting them. A model ID ambiguous with
the workflow's effort-suffix syntax is refused; configure a native model alias
without that suffix to save it. The final stage has no next model to select.

For defaults on newly created tasks, edit the repository workflow stage selectors. For
example, add `"agent_provider": "opencode-local/YOUR_LOCAL_MODEL_ID"` to the
implementation stage, and
`"agent_provider": "opencode-anthropic/YOUR_CLOUD_MODEL_ID"` to review. Keep
the workflow's prompts, policies and posts. Stage transitions start fresh
sessions/worktrees from committed work; they are not mid-turn model swaps.
Repo `agentProviders` and machine-local `.kanna/config.local.json` remain
available for defaults. No second connection/profile registry is introduced.

The desktop controls are for locally owned tasks. The existing
server/CLI/MCP stage-override API remains usable by other clients; this change
does not add a mobile model picker.

## Runtime and permissions

An explicit model is passed on the argv and in process-local OpenCode config.
Kanna also sets `small_model` to that ID and `enabled_providers` to its provider,
so auxiliary inference cannot silently select a different provider. An
unavailable or incompatible selection fails in OpenCode; Kanna does not retry
it on a cloud provider. This is provider selection, not a network sandbox:
user-defined tools, plugins and provider implementations still own their I/O.

Kanna owns `OPENCODE_CONFIG_CONTENT` for MCP/model overrides. Put connection
and permission settings in native config files (or use OpenCode's supported
config-file environment variable), rather than replacing that Kanna-owned
inline payload. Files are merged by OpenCode. No global configuration or auth
file is written by Kanna's model selection.

OpenCode 1.4.3 rejects `--auto`; MCP subprocess variables use `environment`,
not the generic MCP input's `env`. Kanna sends neither `--auto` nor a blanket
`permission: {"*":"allow"}` replacement. Native permissions are retained,
including for Kanna's default and `dontAsk` modes. A TUI may ask for permission;
headless runs remain subject to OpenCode's own noninteractive permission
behavior. Kanna's generic allow/disallow tool lists are not translated into
OpenCode grants. Configure tool permissions in OpenCode itself; do not remove
MCP completion access from agents that must report a result.

The header says **Launched with** using the recorded stage-run provider/model.
It is not a claim about later model changes inside the TUI. Connection origins
and declared context appear in the picker; credentials, URL user info, paths,
and queries are not returned. “Local connection” means a configured loopback
address, not proof that inference ran or that a server is ready.

Native configuration reference: https://opencode.ai/docs/config/
