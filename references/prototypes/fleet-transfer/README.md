# Fleet transfer model

Reference for [Fleet tasks §3](../../../docs/specs/machine-tasks.md#3-transfer), before Fleet 11,
under [Models first](../../../docs/specs/machines.md#10-build-order). This is a Python script,
not daemon code. SSH/SCP substitutes for the eventual `transfer` stream.

## Run

Requires Python 3.9+, Git, SSH/SCP, and Claude Code and Codex already signed in **on each
machine**. The binaries may be on PATH or in `~/.local/bin`. No dependency install is needed.
The real run makes small model requests against those existing accounts. No login or
credential file is transferred.

From the repo root on this Mac:

```sh
python3 -m unittest discover -s references/prototypes/fleet-transfer -v
python3 references/prototypes/fleet-transfer/run.py --host jeremyhale@172.16.0.105
# Smaller real probes:
python3 references/prototypes/fleet-transfer/run.py --agent claude --direction out
python3 references/prototypes/fleet-transfer/run.py --agent codex --direction back
```

`out` means this Mac → Linux; `back` means Linux → this Mac. Each direction creates a
fresh stopped task, not a new fork of the session being transferred. The run prints its
evidence folder on each machine, under `~/.local/share/kanna-fleet-transfer/<run-id>/`.
`results.json` on the Mac summarizes successful rows. Original and imported transcripts
also remain in the agents' normal session folders. Only these synthetic conversations
are inspected. Runtime logs, random phrases, account metadata, credentials, and agent
databases are **not** repository artifacts.

The script never touches this repo's Git state or a user's existing task. Do not point
its phase commands at a real task: it intentionally understands only its own fixture.
No cleanup is automatic; evidence and sender copies are retained even after `done`.

## What moves

1. Make a no-remote repo with a base commit and a later task commit. Dirty its worktree:
   partially staged text, deletion, untracked file with a space in its name, binary,
   executable, and symlink. An ignored file is deliberately excluded.
2. Start a real agent, give it a random phrase, and wait for its noninteractive turn to
   finish and its process to exit normally. Discover the actual conversation JSONL by
   the returned session ID; validate its structure, and record its digest. This models
   the **already stopped at turn end** boundary; it does not stop Kanna's live agent.
3. Stage the final working tree. Create a WIP commit with the pre-WIP HEAD as its parent,
   persist its ID, and update the task branch. `commit-tree` plus `update-ref` makes the
   WIP identity recoverable across a process exit between creating it and moving the ref.
4. Bundle `task ^<receiver-base>`: task commit plus WIP, with the known receiver base as
   a prerequisite. The fixture supplies `base.bundle` separately to set up the receiver's
   existing clone. Fetch the transfer bundle into that clone, add a worktree at its own
   path, and `git reset --soft <pre-WIP-HEAD>`.
5. Install the **byte-identical** transcript in the receiver's agent session store:

   | Agent | Source and destination |
   |---|---|
   | Claude | `$CLAUDE_CONFIG_DIR/projects/<encoded-worktree>/<session-id>.jsonl`, default `~/.claude`; receiver's absolute worktree path encoded by replacing non-ASCII-alphanumerics with `-` |
   | Codex | `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<session-id>.jsonl`, default `~/.codex`; preserve the rollout basename/date |

   No lock file, auth file, SQLite database, session index, or whole agent home travels.
   Embedded old paths in the transcript are preserved.
6. Launch from the new worktree using `claude -p --resume ID` or
   `codex exec ... resume ID`. Ask for the remembered phrase without supplying it again.
   Claude has tools disabled; the Codex probe rejects tool-use events. Require the
   **same session ID**, a completed turn, and an advanced transcript. A separate tool
   turn runs `pwd` and reads `tracked.txt`, proving the agent actually uses the new tree.
7. Return a receipt containing the transfer ID, session ID, and hash of the recalled
   phrase. Only the sender has the expected phrase for comparison. It remains `packed`
   until the receipt passes; duplicate acknowledgements are harmless.

The receiver index tree must equal the sender's captured tree, HEAD must equal the
pre-WIP commit, and the unstaged diff must be empty. This preserves the final file
contents and Git modes, **not** the old staged/unstaged split: all transported changes
are staged, as §3 requires. Ignored files are not transported by `git add -A`.

## Restarts and boundaries

`state.json` is written with a temporary file, fsync, rename, and directory fsync. The
live runner deliberately exits with code 75 at `prepared` on the sender; `received`,
`fetched`, `restored`, `installed`, and `answered` on the receiver; and `done` on the
sender. Each next phase runs in a fresh process. It repeats pack, receive, resume, and
ack calls to check that completed operations don't run twice.

For a failed run, inspect its state and local logs rather than starting another turn:

```sh
python3 references/prototypes/fleet-transfer/transfer.py pack /absolute/sender
# Run these on the receiver, using its copy of transfer.py:
python3 /absolute/run/transfer.py receive /absolute/receiver
python3 /absolute/run/transfer.py resume /absolute/receiver
python3 /absolute/run/transfer.py check-workdir /absolute/receiver
# Copy receiver/receipt.json back to sender/receipt.json, then on the sender:
python3 references/prototypes/fleet-transfer/transfer.py ack /absolute/sender
```

There is a deliberate conservative boundary: after recording `resuming`, an incomplete
or missing CLI output leaves the transfer waiting. The model will recover a complete
output and continue, but will **not blindly replay an uncertain model turn**. Likewise,
an uncertain initial seed or workdir-check turn requires inspection. Fleet 11 needs to
reconcile session/turn completion and any surviving process before retrying. These are
process-restart checks, not a claim of arbitrary power-loss recovery or exactly-once
external tool effects.

The six offline tests cover Git contents/modes, incremental bundle prerequisites,
checkpoint recovery, changed/running-state refusal, corrupt transcript refusal, wrong
memory refusal, duplicate acknowledgement, and complete versus uncertain resume-output
recovery. Synthetic transcripts in those tests **never count as conversation proof**.

## Observed on 25 October 2026 (JST)

Mac home `/Users/jeremyhale`; r720-1 home `/home/jeremyhale`. Claude Code 2.1.310 on the
Mac, 2.1.306 on Linux; Codex 0.163.0 on both. All four real transfers passed:

| Direction | Agent | Original session retained | Phrase recalled | Receiver files read |
|---|---|---|---|---|
| Mac → Linux | Claude | yes | yes | yes |
| Mac → Linux | Codex | yes | yes | yes |
| Linux → Mac | Claude | yes | yes | yes |
| Linux → Mac | Codex | yes | yes | yes |

See [results.json](results.json) for the sanitized run record, IDs, object hashes, and
paths to local evidence. No transcript text or credentials are included there.

## What Fleet 11 should copy

- Gate transfer on the real stopped boundary (`Backlog::end_agent` plus process exit),
  preserve the session ID, and keep the sender ineligible to run while transfer is pending.
- Persist pre-WIP HEAD, WIP/tree IDs, transfer ID and progress before externally visible
  steps. Negotiate actual receiver objects; verify bundle prerequisites and fetch through
  a private ref. Use a full bundle if no shared base exists (not exercised here).
- Reset **soft** to the recorded parent; compare the index tree and actual files/modes.
  Never push WIP to origin. Keep the sender until the acknowledged continuation succeeds.
- Locate the actual transcript, install it in the receiver's session folder, launch with
  an explicit session ID and receiver working directory, and verify a completed turn.
  Product adapters should track the authoritative transcript path; the model's unique-ID
  glob intentionally refuses ambiguous duplicate Claude copies after earlier transfers.
- Compare product tests with this model's nonce recall, same-ID, dirty-tree, receiver-file,
  restart, and lost-ack assertions. A successful file copy alone is not acceptance.

What surprised me: both agents needed **no transcript path rewriting**, and Codex did
not need its SQLite state copied. Old cwd strings stayed in history while real shell
tools ran at the receiver's new path. Claude also crossed the tested patch versions in
both directions. The `reset --soft` staging change is visible and intentional. The hard
restart boundary is a model turn whose completion is unknown, not the bundle fetch.

## Scope left to the product

This model uses real CLIs in print/exec mode; TUI quit keys, keeper/daemon ownership,
replicated grants, tab reopening, iroh streaming, and authorization are Fleet slices'
work. It does not cover subagents, compacted sessions, attachments, Git LFS objects,
submodules, ignored build artifacts, agent version compatibility beyond the versions
above, or hostile archives/manifests. Those limits must not be inferred as supported.
Security hardening belongs in the product; these fixture-only scripts trust their two
machines and input manifests and are not a general transfer service.
