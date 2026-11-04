---
name: kanna-markup
description: Use when a message starting with "[kanna comment cN · FILE:LINE]", "[kanna resolved cN · FILE:LINE]" or "[kanna reopened cN · FILE:LINE]" arrives, or when the user asks to review a markdown doc or an HTML mockup together in Kanna.
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

### Ink

The user can also draw on a doc. Their marks come as one comment, one numbered line per mark, with the note they typed on a mark under it, and a picture of the doc with the ink and the same numbers drawn on:

```
[kanna comment c2 · docs/specs/tab-container.md:7]
> ink, 4 marks, numbered in the picture:
> 1 circle around "Panes only:" (line 7)
> 2 writing beside line 7, read it in the picture
> 3 arrow from "The panes inside stay ordinary pa…" (line 23) to after "A tab container is a slot in a sp…" (line 3)
>   note: say this up front
> 4 strike through "middle-click or × to close it." (line 14)
> marked up: .kanna/docs/marks/docs__specs__tab-container.md-c2.png
```

On a page, marks name parts by CSS path and text, with boxes in page pixels, as page comments do. Always open the picture: it shows what the words can't, such as the user's writing and how far a stroke reaches.

| Mark | Means |
|---|---|
| strike, cross | cut it |
| arrow | move what's at its start to before or after what's at its tip |
| circle, underline | this: see the note, or the writing beside it; with neither, ask what should change |
| writing | read it from the picture: a word, a `?`, a correction |

Answer mark by mark, by number, naming what each touched and how you read it, so a misread shows in your first reply: `kanna markup reply c2 "1, 2: you circled \"Panes only\" with a ? … 3: I'll move \"The panes inside…\" after the opening. 4: I'll cut \"middle-click or ×\"."`. Marks whose text you changed leave the doc by themselves.

## Answering a comment

Kanna types the next comment in only when your turn ends, so a turn spent working holds up every comment after it. Answer first, work after:

1. **Reply first, right away.** Read the passage in the file (the line is where it is now), then reply in the thread before anything else: answer the question, or say what you'll change and how you read the comment, so a misread shows before the work:
   `kanna markup reply c2 "I'll add that ⌃Tab scrolls the next tab into view, under Keys."`
2. **Do the work without holding the turn.** A quick doc edit you make now. Anything longer (code, builds, tests) goes to a subagent or a background command, and your turn ends so the next comment can come in.
3. **Reply again in the same thread when it's done,** saying what landed:
   `kanna markup reply c2 "Added under Keys: ⌃Tab scrolls the next tab into view."`
4. **Leave it open.** The user resolves a thread once they've read your reply; resolving it yourself hides the reply from them. Resolve only when they ask you to. A reply to a thread they've resolved opens it again, so your second reply still reaches them.

Reply in the thread, not only in the chat: the user reads replies beside the doc.

## Resolved and reopened threads

When the user resolves or reopens a thread you get its header alone, several in one message when they do several at once:

```
[kanna resolved c3 · docs/specs/tab-container.md:18]

[kanna reopened c2 · docs/specs/tab-container.md:7]
```

- **Resolved:** the user has closed that thread. Drop any work still pending for it, stop a subagent or background command doing only that, and don't reply in the thread: a reply opens it again. Work that already landed stays.
- **Reopened:** treat it like a new comment. `kanna markup show c2` shows the thread; answer as above.

Your own `kanna markup resolve` and `reopen` aren't sent back to you.

## Commands

| Command | Does |
|---|---|
| `kanna markup FILE.md`, `kanna markup FILE.html` | Show a markdown file or a local page (a mockup) in a new Kanna pane with comments; comments on it come to you (`--to PANE`: to another terminal). If a pane in your workspace already shows it, comments come on there instead |
| `kanna markup PANE`, `kanna markup PANE --off` | Turn comments on (coming to you) or off in a pane already open; `kanna open md` and `kanna open web` show docs without them |
| `kanna markup comments [--all]` | The open comments on your docs (`--all`: resolved too) |
| `kanna markup show cN` | One thread: what it points at, its line, every message |
| `kanna markup reply cN TEXT` | Answer a thread; a resolved one opens again |
| `kanna markup resolve cN`, `kanna markup reopen cN` | Resolve a thread (only when the user asks), or reopen one |

Run them from your own terminal, in the doc's repo: the doc is found from the thread ID, the docs whose comments come to your pane first. Pass `--doc FILE` if that's ambiguous.

The reference model is `references/prototypes/doc-markup/kanna-doc`, with the same subcommands; use `kanna markup` for work.
