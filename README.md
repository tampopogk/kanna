# kanna-skills

Skills for coding agents (Claude Code and Codex), usable in any project.

| Skill | What it does |
|-------|--------------|
| [`reference-driven-design`](skills/reference-driven-design/SKILL.md) | Build from a living reference (mockup, prototype or benchmark) instead of a long spec; integrate first, in small committed steps. |
| [`spec-docs`](skills/spec-docs/SKILL.md) | Keep docs as small specs, one per feature, with an index instead of a monolith; write them in plain English with diagrams and pictures, high level first. |
| [`layer-diagrams`](skills/layer-diagrams/SKILL.md) | Draw architecture as a layer stack (UI on top, like OSI): processes as boxes, layers as bars, colour by owner, clients on top, rendered and checked before showing. |
| [`kanna-markup`](skills/kanna-markup/SKILL.md) | Answer the user's comments on a doc or mockup they review in Kanna, in the comment's thread. |
| [`kanna-explain`](skills/kanna-explain/SKILL.md) | In a Kanna terminal: explain code in comments on it, one part at a time, and the big picture, or anything else the user asks to have explained, in markdown docs with diagrams in the worktree's `.kanna/tmp/`, opened in a tab; the chat stays a line or two. |
| [`kanna-quiz`](skills/kanna-quiz/SKILL.md) | In Kanna: quiz the user on what landed on main, five multiple-choice questions, each answer explained with the code opened beside it; the planning agent offers it. |
| [`kanna-tasks`](skills/kanna-tasks/SKILL.md) | In a Kanna terminal: keep your task's name, name your branch, present your work when it's done, and add follow-up work to the repo's Shelf with a first message that stands alone. |
| [`kanna-planning`](skills/kanna-planning/SKILL.md) | As a Kanna window's planning agent: say what's running and waiting, plan with the user, make Shelf tasks, and start or end them only when asked. |
| [`kanna-new-project`](skills/kanna-new-project/SKILL.md) | As a planning agent in an empty folder or one that isn't a repo yet: decide what the project is, set it up with the user's say-so, and plan its first tasks. |

## Install into a project

```sh
git clone https://github.com/jemdiggity/kanna-skills ~/kanna-skills
~/kanna-skills/install.sh ~/code/my-project        # all skills
~/kanna-skills/install.sh ~/code/my-project spec-docs
~/kanna-skills/install.sh --global                 # for every project, in your home directory
~/kanna-skills/install.sh --link ~/code/my-project # symlink, so `git pull` here updates them
```

Each skill goes in `.agents/skills/<name>/` (where Codex looks), with a
symlink at `.claude/skills/<name>` (where Claude Code looks), so one copy
serves both. Existing skills are skipped unless you pass `--force`.

## Or as a Claude Code plugin

```
/plugin marketplace add jemdiggity/kanna-skills
/plugin install kanna-skills@kanna-skills
```

Plugin skills are namespaced, e.g. `kanna-skills:spec-docs`.

## Adding a skill

Add `skills/<name>/SKILL.md` with `name` and `description` front matter (the
description is what tells an agent when to use the skill), plus any files it
refers to.
