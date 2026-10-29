---
name: kanna-markup
description: Use when a message starting with "[kanna comment cN · FILE:LINE]" arrives, or when the user asks to review a markdown doc or an HTML mockup together in Kanna.
---

# Kanna markup

The user reviews docs in Kanna (markdown files and HTML mockups) and comments on them. Each comment is sent to your terminal as:

```
[kanna comment c2 · docs/specs/tab-container.md:18]
> the passage they selected
what they want
```

`c2` is the comment's thread in that doc. Later messages with the same header are the user's replies in that thread.

A comment on an image says which image and which part of it, and where a copy with that part boxed in red is:

```
[kanna comment c4 · docs/specs/desk.md:9]
> image desk.png, the box from 5%,20% to 30%,50% (left,top to right,bottom)
> marked up: .kanna/docs/marks/docs__specs__desk.md-c4.png
The agent header needs the branch.
```

A comment on a web page (an HTML mockup) names the part of the page, by CSS path and text, the text selected in it (`the text "…" in PATH`), or a box drawn on it, and a screenshot with it boxed:

```
[kanna comment c1 · references/mockups/desk.html:149]
> the page at #picture,needs, element #win > div:nth-of-type(1) "Kanna Desk · Doc markup reference model"
> box 0,0 to 1220,44 px in a 1220×816 view, scrolled to 0,0
> marked up: .kanna/docs/marks/references__mockups__desk.html-c1.png
Make the title bar quieter.
```

The line is where that text is in the HTML, when it's found. Open the marked-up copy (it's relative to the repo root) to see what they mean before you answer.

## Answering a comment

1. Read the passage in the file (the line is where it is now) and do what the comment asks: change the doc, or answer the question.
2. Reply in the thread, briefly, saying what you changed or answering:
   `kanna markup reply c2 "Added: ⌃Tab scrolls the next tab into view."`
3. **Leave it open.** The user resolves a thread once they've read your reply; resolving it yourself hides the reply from them. Resolve only when they ask you to.

Reply in the thread, not only in the chat: the user reads replies beside the doc.

## Commands

| Command | Does |
|---|---|
| `kanna markup FILE.md`, `kanna markup FILE.html` | Show a markdown file or a local page (a mockup) in a Kanna pane with comments; comments on it come to you (`--to PANE`: to another terminal) |
| `kanna markup PANE`, `kanna markup PANE --off` | Turn comments on (coming to you) or off in a pane already open; `kanna open md` and `kanna open web` show docs without them |
| `kanna markup comments [--all]` | The open comments on your docs (`--all`: resolved too) |
| `kanna markup show cN` | One thread: what it points at, its line, every message |
| `kanna markup reply cN TEXT` | Answer a thread |
| `kanna markup resolve cN`, `kanna markup reopen cN` | Resolve a thread (only when the user asks), or reopen one |

Run them from your own terminal, in the doc's repo: the doc is found from the thread ID, the docs whose comments come to your pane first. Pass `--doc FILE` if that's ambiguous.

The reference model is `references/prototypes/doc-markup/kanna-doc`, with the same subcommands; use `kanna markup` for work.
