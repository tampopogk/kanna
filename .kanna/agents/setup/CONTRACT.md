# setup Contract

The `setup` role configures a repository by composing built-in Kanna roles and tested flavor variants.

Required behavior:

- It must inspect the repository before asking questions, including git remote URL, available GitHub auth through `gh auth status`, existing CI configuration, and existing `.kanna/` files.
- It must ask only for decisions that inspection cannot determine safely.
- It must write `.kanna/config.json` selections, and repo-local `EXTEND.md` files only for behavior that does not match stock flavors.
- Machine-specific choices go in ignored `.kanna/config.local.json`. It must preserve an existing local config bootstrap if valid, and must not mandate installing a script or local skeleton.
- It must not write copied stock `AGENT.md` files for roles such as `pr` or `merge`.
- For the stock GitHub flow, it must select a built-in workflow (`no-review`, `single-reviewer`, or `specialized-reviewers`) plus `merge@github`, not author a workflow file of its own. A workflow file is written only for stages the built-ins do not offer.
- The stock GitHub flow must not select `pr@draft-pr`. `merge@github` cannot merge a draft, so a draft PR requires a deliberate repo-local decision about what readies it; drafts are offered only when the user asks for them.
- Its answers must compose. Every built-in workflow ends with a `pr` stage plus an `approve` post, and `approve` resolves the PR with `gh pr view` and fails when none exists, so direct built-in selection is valid only for the ordinary-PR flow:
  - `pr@push-only` creates no PR, so it must never be paired with a built-in workflow. It implies manual merge plus a repo-local workflow matching the chosen review depth with the `approve` post omitted.
  - Manual merge likewise requires omitting the `approve` post, because nothing consumes the merge signal.
  - `pr@draft-pr` with a merge agent must also write a repo-local `.kanna/agents/approve/EXTEND.md` that readies the draft before signaling.
- It must validate changed JSON files and the local-config sync script, and verify the local config stays ignored, before reporting success.
- It must finish with `kanna_complete_stage` status `success`, or `failure` when setup is blocked.
- On rerun, it must revise only the requested area, explaining affected dependencies, and preserve prior choices and reasons, deferred items, valid commands/configuration, coherent provider/model selections, and user-authored documentation instead of repeating setup from scratch or authoring a replacement documentation corpus.
- Portable team/project choices belong in committed `.kanna/config.json`, workflows, and agent definitions/extensions; machine-specific choices belong in ignored `.kanna/config.local.json`, which supports only `agentProviders`, `workflow`, `ports`, `setup`, `teardown`, `test`, and `$schema`. Entry maps merge; arrays and other values replace. It must not move local settings into shared configuration, and must use the running version's schemas/parsers rather than an older repo-local schema as final authority.
- Provider/model/effort choices stay coherent within their provider layer; explicit overrides outrank repo preferences, which outrank layered agent defaults. It must not change them merely to match its own provider, and must not invent a model-id allowlist.
- `kanna_doctor` is a deterministic read-only check of `.kanna` syntax, resolution, and explicit rules; it never runs setup/test/dev commands, launches services, or proves custom prose correct. It must run doctor after edits, fix what the agreed scope covers, and not impose a broad test/dev gate as substitute proof.
- Written candidate files are not automatically active: shared definitions resolve from origin's recorded default-branch snapshot, machine-local overrides from the registered checkout. It must explain the required integration or remaining activation limitation rather than publishing or inventing a remote, and must not let an existing task's pinned workflow, or an inherited product-work stage, be treated as already carrying this setup's changes or publishing authority.
- Its completion report must state the agreed scope, files/settings changed, preserved choices and reasons, deferred decisions, doctor results, other validation performed, limitations, and whether configuration is effective or awaiting integration. A deferred area is a valid outcome, not grounds to manufacture defaults or another task.
