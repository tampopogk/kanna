# Shelf search

**Status:** built · **Mockup:** [`references/mockups/desk.html`](../../references/mockups/desk.html) (Simulate: a long Shelf, then ⌘F; or `#empty&long&q=shelf`) · **Builds on:** [tasks](tasks.md#3-the-shelf), [Desk](desk.md), [Desk view](desk-view.md)

Find a Shelf task by typing part of its name or first message, and start it from the keyboard. A task's ID is `task-` and 8 hex digits ([task IDs](task-ids.md)); `kanna task list` shows it for every task, and commands take any unique start of it.

![Searching the Shelf for "shelf": two of twelve tasks, the first selected](shelf-search.png)

![Two words, in light mode: only the task with both](shelf-search-light.png)

## In the app

- **The field sits in the Shelf header** of the Desk view, between "Shelf" and the count, which reads "2 of 12" while you search. Its placeholder is "Search the Shelf". It shows only when the Shelf has tasks.
- **⌘F** (Edit › Find…) puts the cursor in it while the Desk view shows, from the planning terminal too; it scrolls the field into view.
- **It filters as you type.** A task matches when every word you type appears in its name or first message, ignoring case, in any order. Matches are marked in both. No match: "No Shelf task matches "…"."
- **The first match is selected.** ↑ ↓ move the selection, and ↩ moves the selected task to the Desk, as clicking it does (refused the same way when the Desk is full). Clicking any row still works.
- **esc** clears the search; with nothing typed, it goes back to the planning terminal. The field's **×** clears it too, and shows in place of the ⌘F hint once you've typed.
- **The search clears** when you start a task from it, and when the active task shows again.
- **Rows look as they do unfiltered,** whatever state they show (waiting, running on the Shelf, blocked): the search only hides the ones that don't match.

## CLI

`kanna task list QUERY` lists only the tasks that match QUERY the same way: every word in the name, or in a Shelf task's first message. `kanna task list` alone lists every task, as before; `--json` is filtered too.

```sh
kanna task list resume agent
```

## Rejected

- **Matching the maker or the repo:** the Desk view's Shelf is already one repo's, and the maker is rarely what you remember.
- **Typing anywhere on the page to search:** the planning terminal has the keyboard in the Desk view; ⌘F is the one way in.

## Checked by

e2e `shelf_search`: ⌘F from the planning terminal, filtering by name and by a first message's words in any case, no match, esc twice, ↑↓ at the ends, ↩ starting the selected task and clearing the search, `kanna task list QUERY`, and snapshots in light and dark; the CLI's own test for QUERY and `--json`.
