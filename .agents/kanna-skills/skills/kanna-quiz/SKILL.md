---
name: kanna-quiz
description: Use when you've been told you're the Kanna quiz agent (the agent of a Quiz window), or as the Kanna planning agent offering the user a quiz on what landed on main: five multiple-choice questions about the codebase, asked one at a time, each answer explained with the code opened at the line.
---

# Kanna quiz

Agents write code faster than the user can read it. A quiz is a short break that keeps them up with what landed: five multiple-choice questions about main's last day of commits, asked one at a time, each answer explained, with the code shown beside you on a miss. About two minutes. No score to protect.

Two parts: the planning agent **offers** a round and opens the Quiz window; the agent there, told it's the Kanna quiz agent in its first message, **asks** it.

## Offering (the planning agent)

When your session starts (once per session):

1. `git log --oneline main --since=1.day` (`master` if there's no `main`). With nothing, say nothing about a quiz.
2. Otherwise offer once, in a line, after whatever else you say: what landed (the work, not the hashes), then "Quiz on today's changes? Five questions, about two minutes."
3. On yes: `kanna quiz open`, from the repo, with the agent you are (`--agent codex` for Codex; Claude is the default). It brings the repo's Quiz window forward if it's open, else opens one: the quiz agent docked on the left, in the repo's main checkout, told it's the quiz agent, and what it opens as tabs beside it. It isn't a task. Say it's open, and carry on.

## Asking (the quiz agent)

You're in the repo's own folder, on main, with no task: a quiz changes nothing. Don't name a task, rename the branch, commit or edit files.

### Read first

- `kanna quiz history --json`: this repo's past rounds, newest first, each with the commit of main it asked about (`head`) and its answers. With a round, what's new is `git log --stat LAST_HEAD..main` (at most 30 commits); with none, or nothing since, `git log --stat main --since=1.day`. With none of those either, say so and offer the last 30 commits instead.
- `kanna quiz history --misses --json`: the misses not asked again yet. Bring back up to two a round, the oldest first, asked fresh: new wording, the choices shuffled, the lines read again on main.
- The commit messages, the diff (`git show`, `git diff`), and the specs and docs those commits touch. Only what's true on main: never an agent's conversation, never another branch.
- If the checkout isn't on main (`git rev-parse --abbrev-ref HEAD`), read with `git show main:PATH`, and tell the user the code you open is the checkout's, not main's.
- Pick five topics, spread across files, broadest first: how the pieces fit before a line's detail; the misses you bring back count among the five. Say what landed in a line and that the first question is coming; don't make the user wait for all five.
- Start the round before the first question: `kanna quiz start --json '{"commits": ["67b287c", …], "agent": "claude"}'` prints its ID (`r12`), for each answer.

### The kinds

| Kind | `--kind` | Asks | Example |
|---|---|---|---|
| Architecture | `architecture` | how the pieces fit: who owns what, what talks to what | Which process owns a pane's terminal: the app, the daemon, or the CLI? |
| Pick the drawing | `drawing` | the same, as three diagrams, two wrong | Which drawing shows how a terminal's bytes reach Kanna.app? |
| Behaviour | `behaviour` | what happens when | When you Move to Shelf, what happens to the agent? |
| Where it lives | `where` | which file or function does it | Which file decides a pane's status? |
| Why | `why` | a decision, from the specs | Why does an instance have its own daemon? |
| Spot the bug | `bug` | a few lines of today's code with one changed; which line? | Which of these lines has a bug put in? |

Mix them; a round has at least one architecture or drawing question, first.

### One question at a time

Write each question just before you ask it:

- **Four choices** (three for a drawing), one right, the wrong ones plausible: things a reader who skimmed might believe. Not trick questions.
- **About some lines:** read the file and pick the lines that answer it (a function, a spec's diagram, the place one piece hands off to another), so you can open them on a miss with the right line numbers.
- **Ask it with your multiple-choice tool** (Claude's AskUserQuestion: the question, choices as options with a short description; header `Quiz N/5`). An agent without one: a numbered list, and the user answers with a number.
- Head it, in the chat: `**N of 5** · KIND · where it comes from` (a commit's short hash, or a spec's path).
- **Spot the bug:** put the lines in the question itself, labelled A to D with their line numbers, with exactly one changed to put a bug in; the choices are A to D. Not only in the chat before it: the question tool can cover what you wrote.
- **Pick the drawing:** write three mermaid diagrams under the headings `## A`, `## B`, `## C`, with the question above them, to `$TMPDIR/kanna-quiz/drawings.md`, and open it in a tab: `kanna open md "$TMPDIR/kanna-quiz/drawings.md"`. The choices are A, B and C, each described only as "Drawing A" and so on: describing a drawing in words gives it away. Leave it open until the round ends, like the code you open.
- **Never hint,** never reword a choice once asked, and never say the answer before the user picks.

### Answering

- Say **Right.** or **Not quite:** it's N, and the answer; then why, in a sentence or two. Every answer gets the why.
- **Record it** straight away, with the lines it's about and what was asked:

  ```sh
  kanna quiz answer r12 --kind where --file crates/kanna-daemon/src/status.rs:88 --wrong \
    --json '{"question": "Which file decides the status of a pane?", "choices": ["…", "…", "…", "…"], "pick": 3, "answer": 1, "explanation": "…", "topic": "pane status"}'
  ```

  The JSON goes in single quotes, so write a `'` in it as `\u0027`. A miss brought back from an earlier round adds `--again a7` (its `answer` ID in the history), right or wrong: it stops being a miss, and a new miss stands for it. Don't tell the user it's recorded.
- **On a miss,** open the code straight away, with the why on its lines:

  ```sh
  kanna comment crates/kanna-daemon/src/agents.rs:514 "The daemon keeps each file as it installed it and runs git merge-file on yours, the base and the new one."
  ```

  It opens as a tab beside you, so your pane stays whole (`FILE:LINE` takes the whole scope at that line; `FILE:FIRST-LAST` takes just those lines). A comment on the same file reuses that tab. Leave the panes you open there until the round ends: the user may still be reading one when you ask the next question.
- **On a right answer,** offer to open the code; open it if asked.
- **Follow questions** the user asks between questions: answer from the code, briefly (comment on it if that's clearer), then go on to the next question.

### At the end

- Say what they knew: "You knew 4 of 5 about today's changes." Then one line per question: ✓ or ✗, its topic, and its file and line. Only this round: never a total, a trend or past rounds; the history is for picking questions, not a score.
- Ask: close this window, or another round? Another round is a new `kanna quiz start`, and picks new topics from the same commits, or older ones once those run out.
- To close: close your own window (`kanna close --window WINDOW`, the one `kanna workspace list` names first); the tabs you opened close with it.

Kanna keeps every round and answer (in its database, through `kanna quiz`), so misses come back and the next round starts from what's new since this one.

## Commands

Run the `kanna` on your PATH, from this terminal: its `KANNA_*` variables tell Kanna which pane is asking, so what you open goes in a tab beside you.

| Command | Does |
|---|---|
| `kanna quiz open [--agent CMD]` | The planning agent: bring the repo's Quiz window forward, or open it |
| `kanna comment FILE:LINE[-LAST] TEXT` | Show the code in a tab beside you with TEXT on those lines |
| `kanna open md FILE` | Show a markdown file (with mermaid diagrams) in a tab beside you |
| `kanna close PANE` | Close a pane you opened |
| `kanna focus PANE` | Bring a pane forward |
| `kanna workspace list` | Your window: its tabs and panes, with their IDs, what's focused and zoomed (`--all`: every window, workspace and pane) |
| `kanna quiz start [--json DOC]` | Start a round on main's commit; prints its ID (`r12`) |
| `kanna quiz answer ROUND --kind KIND --file FILE:LINE --right\|--wrong [--again ANSWER] [--json DOC]` | Record an answer; prints its ID (`a7`) |
| `kanna quiz history [--misses] [--json]` | This repo's rounds and answers, newest first; `--misses`: the misses not asked again |
