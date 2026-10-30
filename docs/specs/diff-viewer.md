# Diff Viewer

**Status:** built · **Mockup:** [`references/mockups/diff.html`](../../references/mockups/diff.html) · **Builds on:** [code viewer](code-viewer.md), [app panes](app-panes.md)

A **diff pane** shows what `git diff` shows, natively, as a tab beside the agent. You review what a task's agent changed without leaving Kanna: file by file, highlighted, following the worktree as the agent works. It's read-only: you read, select, copy, and open a line in a code pane.

![kanna open diff --merge-base main: the files the task changed on the left, each file's changes beside them; the agent just added line 216, which is marked](diff-viewer.png)

## Contents

1. [Opening a diff](#1-opening-a-diff)
2. [Rules](#2-rules)
3. [Keys and mouse](#3-keys-and-mouse)
4. [CLI](#4-cli)
5. [How it's built](#5-how-its-built)
6. [Glossary](#6-glossary)

## 1. Opening a diff

```mermaid
sequenceDiagram
    participant A as agent
    participant D as daemon
    participant G as git
    participant App as Kanna.app
    A->>D: kanna open diff --merge-base main
    D->>App: start {type: diff, path: worktree, args}
    App->>D: diff.subscribe {view: diff-231, path, args}
    D-->>App: ok {content: diff-3f9a1c2e} (a new watcher, or the one already running)
    D->>G: git diff --numstat, git diff, git ls-files --others
    G-->>D: files, patch, untracked files
    D->>App: diff {content, serial, files and hunks}
    App->>App: lay out, highlight, draw
    App->>D: update {state: running}
    D-->>A: opened diff-231
    Note over D,G: the worktree changes (FSEvents)
    D->>G: git diff again
    D->>App: diff {content, serial, the new files and hunks}
    App->>App: redraw at the same place, mark what's new
```

- **`kanna open diff [ARGS]` shows `git diff [ARGS]`**, run in the folder it's opened from (the task's worktree), with git's own arguments:

| You run | Shows |
|---|---|
| `kanna open diff` | what isn't staged |
| `kanna open diff --merge-base main` | everything the task changed since it left `main`, committed or not |
| `kanna open diff --staged` | what's staged |
| `kanna open diff main...HEAD` | the branch's commits only |
| `kanna open diff -- crates/kanna` | any of these, for some paths only |

- **Plus untracked files.** When the diff's new side is the worktree (no `--staged` or `--cached`, and at most one commit named), files git doesn't track yet and doesn't ignore are shown too, as added. An agent's new files are often most of its work, and plain `git diff` leaves them out.
- It takes `open`'s placement options; with none, from a task's terminal it opens a tab on the task's Desk, as `open code` does. Kanna reads its own options (`--tab`, `--split`, `--size`, `--dock`, `--window`, `--name`) and passes everything else to git.
- The tab is named after the command (`diff --merge-base main`); the header shows the git command, the files and the lines added and removed.

## 2. Rules

- **It follows the worktree.** When a file in the worktree or git's state (the index, a commit, a branch moving) changes, the pane runs `git diff` again, keeps your place, and marks what's new: the changed file flashes in the list and its new lines get a bar until the next change. While a re-diff takes a moment the header says **updating**. If git fails (the folder is gone, an argument is wrong) the pane shows git's error, not an old diff.
- **Unified, or Split** (old and new side by side), a switch in the header or `s`; kept with the pane.

![Split: old on the left, new on the right; a change's missing side is hatched](diff-viewer-split.png)

- **Files list** on the left, grouped by folder: each file's status (`M` modified, `A` added or untracked, `D` deleted, `R` renamed) and its lines added and removed. Clicking one scrolls to it; it follows the current line.
- **The list shows only when the diff text keeps 110 columns beside it,** at the pane's font size, so it follows zoom: a split diff is two columns of text, and the code matters more than the list. At the default 12.5 pt that's a pane at least 1,081 pt wide: a Desk filling a 1512 or 1728 pt MacBook screen with its agent at 2/5 leaves the tabs 907 or 1,036 pt, and hides it. Hidden, nothing else changes: the file headers, `[` and `]` and the current file work as before, and the list comes back when the pane grows. The cutoff is one rule over the pane's side panels (`DiffAside`), which drops them in order until the text has its columns; the file list goes first.

![A Desk at 1400 pt with the agent docked: the diff pane keeps its text and hides the list](diff-viewer-narrow.png)
- **Each file has a header** that stays at the top while you're in it, with its path, status and counts. Clicking it folds the file; ⌥-click folds or unfolds them all.
- **Unchanged lines between changes** fold to one row naming how many and the function or block the next change is in, as git's hunk header names it.
- **Highlighting** as in the code pane, by file extension, in light and dark; the words that changed in a changed line are marked more strongly.

![The same diff in light mode](diff-viewer-light.png)

- **Big diffs are folded**, not dropped: a lock file (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `Gemfile.lock`, `poetry.lock`, `go.sum`), a file `.gitattributes` marks `linguist-generated`, or a file with more than 2,000 changed lines shows its header and **Show the diff**. A binary file shows "Binary file changed".

![Cargo.lock and a 6,240-line file folded](diff-viewer-large.png)

- **Read-only.** You select and copy (⌘A, ⌘C); ↩ on a line opens it in a code pane on the same Desk (`kanna open code FILE:LINE`). A removed line isn't in the file any more, so it opens nothing.

![↩ on line 34 opens open.rs there in a code pane](diff-viewer-open-code.png)

- **Nothing to show:** "No changes", with the command.
- **Zoom** (⌘= ⌘− ⌘0), 9 to 32 pt, kept with the pane, as in a code pane.
- **Overview** down the right edge: the whole diff as a thumbnail, with its changes and find hits; the file list gives way to it first ([overview](overview.md)).

## 3. Keys and mouse

| You | Does |
|---|---|
| `j` `k`, ↓ ↑ | next, previous line |
| `n` `p` | next, previous change |
| `]` `[` | next, previous file |
| `g` `G` | first, last line |
| `s` | Unified or Split |
| ↩ | open the line in a code pane |
| Click a line | makes it the current line |
| Click a file in the list | goes to it |
| Click, ⌥-click a file's header | folds it, folds or unfolds every file |
| ⌘F, ⌘G, ⇧⌘G, Esc | find, as in a code pane (each column owns an `NSTextFinder`, so its matches can be marked in the overview) |
| ⌘= ⌘− ⌘0 | font size |

## 4. CLI

| Command | Does |
|---|---|
| `kanna open diff [GIT-DIFF-ARGS] [placement]` | opens a diff pane on `git diff` with those arguments, in the current folder |
| `kanna navigate PANE diff [GIT-DIFF-ARGS]` | shows another diff in the same pane |
| `kanna workspace list --all --json` | shows a diff pane's folder and arguments |
| `kanna capabilities` | lists `diff` among the pane types |
| `kanna raw '{"cmd":"diffs"}'` | the diffs the daemon watches: each one's content ID, folder, arguments, serial and the views subscribed to it |
| `kanna raw '{"cmd":"debug.diff","args":{"pane":"diff-231"}}'` | for tests, from the app: the command, the files (path, status, counts, folded), the rows (kind, old and new line, text), the current line, the mode, whether it's updating; `keys: "nn"` presses keys |

The pane's record in the saved layout is `{type: "diff", path, args, view}`: `path` is the folder git runs in, `args` git's arguments, `view` `unified` or `split`. It comes back after a relaunch where it was.

## 5. How it's built

```mermaid
flowchart TB
    subgraph CLIP["CLI processes"]
        CLI["kanna CLI, agents<br/>open diff · navigate"]
    end
    subgraph APP["Kanna app process"]
        L3["3 · Mux UI: KannaDiff<br/>DiffPaneView · files list<br/>rows · keys"]
        L3C["3 · KannaCode<br/>theme · highlighting · find"]
        L3S["3 · kanna-syntax (Rust)"]
        L3 --> L3C
        L3C --> L3S
    end
    subgraph DAEMON["Kanna daemon process"]
        L4["4 · kanna-mux (as kmux)<br/>+ diff pane records"]
        L4D["4 · diffs: a worker per folder + args<br/>shared by its subscribers"]
        L4K["kanna-diff (Rust)<br/>runs git · parses · untracked · folds"]
        L4 --> L4D
        L4D --> L4K
    end
    subgraph OS["6 · macOS"]
        AK["AppKit<br/>NSTextView"]
        GIT["git"]
        FS["FSEvents"]
    end
    CLI -->|"socket"| L4
    L3 <-->|"app link: start, diff, update"| L4D
    L3 -->|"draws with"| AK
    L4K -->|"runs"| GIT
    L4D -->|"watches"| FS
    classDef kanna fill:#2f6f4f,stroke:#6fcf97,color:#ffffff
    classDef kmux fill:#2f4f8f,stroke:#7fa7ff,color:#ffffff
    classDef os fill:#4a4a55,stroke:#9a9aa8,color:#ffffff
    classDef client fill:#6b4f2f,stroke:#e0a960,color:#ffffff
    class L3,L3C,L3S,L4D,L4K kanna
    class L4 kmux
    class AK,GIT,FS os
    class CLI client
```

| Piece | Where | What's new |
|---|---|---|
| CLI | `crates/kanna` | the `diff` kind of `open`, git's arguments passed through, `navigate` with arguments |
| Pane records | `crates/kanna-mux` | `PaneType::Diff` with `path`, `args`, `view`, `zoom`; `debug.diff` passed to the app |
| Model | `crates/kanna-diff` (new crate) | runs git, parses its patch, adds untracked files, decides what folds |
| Diffs | `crates/kanna-daemon/src/diffs.rs` | a worker per folder + git arguments, shared by every view subscribed to it: diffs, watches the worktree, diffs again, tells the subscribers; the `diffs` command |
| Pane | `apps/kanna/Sources/KannaDiff` (new target) | lays out the rows (unified or split), the view, files list, keys |
| Highlighting, theme, find | `KannaCode`, `kanna-syntax` | shared with the code pane, not copied |

- **Git is the truth.** The daemon runs the `git` on the user's `PATH` with the pane's arguments (adding `--no-color --no-ext-diff`, so a user's pager, colours or external diff tool don't change what's parsed), never its own diff. That's how "follow how git works" holds: whatever `git diff ARGS` means, the pane means.
- **Why the `git` binary, not a git library** (libgit2, gitoxide):

| | `git` binary | libgit2 (git2) | gitoxide (gix) |
|---|---|---|---|
| `git diff ARGS` means what it means in a terminal | yes, it is `git diff` | no: its command line rebuilt on the library | the same |
| User's config, `.gitattributes`, new repo formats (sparse index, sha256) | always | lags git | lags git; diff still young |
| Already in Kanna | yes: worktrees, branches, tasks | no | no |
| Cost per refresh | two processes, milliseconds | in-process | in-process |

  A library would be worth it for something git's command line can't answer cheaply, such as a diff per keystroke or blame; not here.
- **The daemon runs git, not the app**: git runs where the files are, which is the daemon's machine, so an app on another machine (planned) can show a diff too. The app only lays out and draws.
- **Subscriptions, not panes** ([presentation layers](presentation-layers.md#4-content)). A diff is content keyed by its folder and git arguments, with an ID made from them (`diff-3f9a1c2e`). Each view the app shows subscribes (`diff.subscribe {view, path, args}` on the app link; the reply carries the content ID and the latest diff, or an error the pane shows instead of a blank page) and unsubscribes when it goes (`diff.unsubscribe {view}`); `diff.show {view, show: [PATH]}` unfolds files, and the worker unfolds every view's files together. One worker runs per diff, however many views show it, and stops when the last one leaves, or when its app does; there's no grace period. The app subscribes again on every (re)connect, so a restarted daemon diffs again at once. Every change goes to the subscribers as `diff {content, serial, diff}`. Markdown and code panes subscribe to their files the same way ([file panes](file-panes.md)).
- **Off the daemon's loop.** Each diff pane has a worker thread that runs git and parses; the daemon's loop only stores the result and passes it on. `git diff --numstat -z` decides the folds first, so a folded file's patch is never read (an `:(exclude)` pathspec). The app highlights off its main thread and swaps the rows in at the same place.
- **Following the worktree, without polling:** FSEvents on the folder and on its git directory (for a worktree, its own `.git/worktrees/NAME` and the shared refs); git runs only when something there changes, at most once every 200 ms. Changes to files git ignores (a build's `target/`) are dropped (`git check-ignore`). git runs with `GIT_OPTIONAL_LOCKS=0`, so a diff never refreshes the index: its own run can't set off another, and it never holds the lock the agent's git needs.
- **Parsing git's output**, only formats made for it: `--numstat -z` and `ls-files -z` are NUL-separated lists for scripts; the patch is the format `git apply` reads. The pane pins the format so the user's config can't change it: `--no-color --no-ext-diff --no-textconv --src-prefix=a/ --dst-prefix=b/` and `-c core.quotepath=false`.
- **The line a change is in** (the "⋯ 17 unchanged lines `pub enum PaneType {`" row) is git's own hunk header context, as `git diff` prints it.
- **Highlighting** is `kanna-syntax` on each side of each change (its context, removed and added lines, as old and new text), coloured by the code pane's theme. **[Assumption]** Highlighting a change's lines alone is close enough; if it colours wrongly, highlight the whole old and new file instead.
- **Not in kmux**, as the code pane isn't: `open diff` is Kanna's.
- **Checks:** `crates/kanna-diff` tests for the parser and folds, and against real scratch repos (untracked files, `--staged`, two commits, pathspecs, git's errors, ignored paths); `crates/kanna/tests/cli.rs` for passing git's arguments through, and for the daemon diffing a subscribed scratch repo and diffing again when a file changes but not when an ignored one does, two views sharing one watcher, and the watcher stopping when its app goes; `KannaDiffTests` for laying out rows and the list's cutoff (columns, zoom, the MacBook Desks); an e2e scenario (`diff_pane`) that opens a diff in a scratch repo, reads `debug.diff`, edits a file and adds an untracked one and sees both arrive, switches to Split, opens a second view of the same diff (one watcher) and closes it, restarts the daemon (the pane still updates), relaunches the app, navigates to a diff git refuses (the pane shows git's error), and checks the list by `debug.diff`'s `list` and `width`: shown at 1400 pt, hidden zoomed in and at 960 pt (`[` and `]` still move between files), hidden on a 1728 pt Desk with the agent at 2/5 (a 1,036 pt pane) and back with the agent at 1/5.

## 6. Glossary

| Term | Meaning |
|---|---|
| **patch** | git's text form of a diff: each file's header and its hunks |
| **hunk** | one change and the unchanged lines around it, starting at a line in the old and new file |
| **untracked file** | a file in the worktree git doesn't track and doesn't ignore |
| **merge base** | the last commit a branch shares with another: where the task's branch left `main` |
