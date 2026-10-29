---
name: kanna-new-project
description: Use when you've been told you're the Kanna planning agent and you're in a folder that's empty or isn't a git repo yet: help the user decide what the project is, set it up, and turn the plan into backlog tasks.
---

# A new project in Kanna

Kanna's tasks each work in a git worktree of the project, so a project needs a git repo before it has tasks (`kanna task new` refuses one that isn't). Your job is to get from an idea to a backlog the user can start.

## 1. Decide what it is

Ask, in a few short questions at a time:

- **What it is and who it's for,** in a sentence.
- **What "working" looks like first:** the smallest version worth having.
- **How it's built,** if the user has a preference (language, framework, where it runs); otherwise suggest one, and say why in a line.

Say back what you understood, in a few lines, before setting anything up.

## 2. Set it up, with the user's say-so

- **`git init`** only when the user agrees. It's their folder.
- **A first commit** only when the user agrees, and only of what's there (a README with the sentence from step 1 is a good one). Without one, that's fine: the first task's branch starts empty, and its first commit is the project's.
- **A remote changes where it lives.** A new project is in `~/.kanna-v4/repos/local/NAME`; once it has an `origin` (`gh repo create`), its place is `OWNER/NAME` and Kanna refuses its tasks until it's imported there. Tell the user to run `kanna project import ~/.kanna-v4/repos/local/NAME` from a terminal outside the project: it copies the project to `OWNER/NAME`, stops and restarts this planning terminal in the copy, and you come back to this conversation. The old folder stays as it was; they can delete it.
- **Nothing else.** Scaffolding, dependencies and code are the tasks' work, in their worktrees; you stay in the main checkout.

## 3. Turn the plan into tasks

Make backlog tasks with `kanna task new "NAME" "FIRST MESSAGE"`, as the `kanna-planning` skill says:

- **The first task sets the project up** (scaffold, build, a test that runs), so the others build on it; say in the others' first messages that they come after it.
- **Each first message stands alone:** what the project is (the sentence from step 1), what this task does, the decisions you made together, and how to know it's done.
- **A handful, not everything:** the first working version. More come once it runs.

Then say what's in the backlog, each task by its name (never its ID), and that **Move to Shelf** (or asking you) starts one.
