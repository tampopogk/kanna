# "Consultation" → "research": what was renamed, and what still answers to the old name

*2026-09-19*

Owner directive: the concept is **research**. Nothing about stage behavior,
gate semantics, visibility, or agent authority changed — only the word. This
note records which `consultation` hits a future `grep` should leave alone,
following the same pattern as
[the pipeline → workflow rename](2026-08-19-workflow-rename-remaining-debt.md).

## What was renamed

| Surface | Was | Now |
|---|---|---|
| Public product workflow | `consultation` | `research` |
| Its stage | `consultation` | `research` |
| Its agent | `consultant` | `researcher` |
| Internal architect workflow | `architect-consultation` | `architect-research` |
| Its stage | `consultation` | `research` |
| Spec | `docs/specs/architect-consultations.md` | `docs/specs/architect-research.md` |

`architect` is a role, not the retired word, so the agent keeps its name.
`architect-research` keeps `"visibility": "internal"` — an internal definition
that loses that field is silently promoted into the new-task picker and the
advertised tool catalog.

## Kept as deprecated aliases

A task stores its workflow by *name* in `pipeline_item.pipeline` and pins the
full definition in `pipeline_item.pipeline_def`, and pinned definitions are
supposed to survive byte-for-byte. There were open `consultation` and
`architect-consultation` tasks on the owner's machine when this landed, so
both names had to keep resolving rather than be migrated away.

| Retired name | Resolves to | Table |
|---|---|---|
| `consultation` | `research` | `LEGACY_BUILTIN_WORKFLOWS` |
| `architect-consultation` | `architect-research` | `LEGACY_BUILTIN_WORKFLOWS` |
| `consultant` | `researcher` | `LEGACY_BUILTIN_AGENTS` |

Both tables live in `crates/kanna-server/src/task_creator/definitions.rs` and
already carried the earlier retirements (`default`, `qa`, `qa-dispatch`,
`pr-triage`, `config-factory`). They are **resolution** aliases only: a
retired name never appears in `workflow_names()`, `agents()`, the repo
manifest, the desktop's new-task picker, or the tool catalog's advertised
lineup — the same listing-vs-resolution split `visibility` implements. A repo
shipping its own `.kanna/workflows/consultation.json` or
`.kanna/agents/consultant/` still wins over the alias, and the agent alias also
carries repo `EXTEND.md` files and `agentProviders` entries written under the
old name (`agent_repo_dirs`).

No migration rewrites stored `pipeline` or `pipeline_def` values.

## Stage names, which resolve differently

A pinned definition keeps its stage spelled `consultation`, and
`pipeline_item.stage` keeps matching it, so no alias is needed: the stage is
read out of the task's own pinned document. What *is* needed is a display slot
for the retired spelling, so both names appear in `DEFAULT_STAGE_ORDER`
(`packages/core/src/config/repo-config.ts`) and in the mobile card palette
(`apps/mobile/src/theme/taskStageTheme.ts`).

## Deliberately left alone

Dated notes, investigation write-ups, task specs under `docs/task-specs/`, and
spec status lines that cite a past consultation by task id. These record events
that happened under the old name; rewriting them would make the record wrong.

## Coverage

- `task_creator::tests::core::retired_workflow_names_resolve_to_the_renamed_definitions`
  — both retired workflow names and the retired agent name resolve, stay
  unlisted, and the retired internal name still serves an internal definition.
- `task_creator::tests::stage::a_task_pinned_to_the_retired_consultation_definition_still_reads_and_advances`
  — a task pinned to the pre-rename definition still spawns its stage's agent
  through the alias and still advances.
- `task_creator::tests::core::legacy_builtin_workflow_names_still_resolve_for_committed_repo_config`
  — both names added to the existing retired-name table test.
- `http_api::tests::repo_definitions` — the manifest and definition endpoints,
  for listing and for alias resolution.
