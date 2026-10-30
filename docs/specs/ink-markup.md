# Ink Markup

**Status:** proposed · **Mockup:** [`references/mockups/ink-markup.html`](../../references/mockups/ink-markup.html) · **Builds on:** [Doc Markup](doc-markup.md)

**Ink markup** lets you draw on a doc in Kanna (circle a word, strike out a sentence, draw an arrow from a paragraph to where it belongs, scribble a note in the margin) and have the agent beside it work out what the marks mean and make the changes. It is another way to post a [Doc Markup](doc-markup.md) thread: the same agent, the same reply and resolve. Today it's the Mac with a mouse or trackpad; the design carries over to an iPad with Apple Pencil.

![Ink on a markdown spec: a circle with a ? beside it, an arrow moving a paragraph up, a strike, numbered, waiting to be sent](ink-markup.png)

## Contents

1. [The loop](#1-the-loop)
2. [Drawing](#2-drawing)
3. [What the agent gets](#3-what-the-agent-gets)
4. [After it's sent](#4-after-its-sent)
5. [iPad and Pencil](#5-ipad-and-pencil)
6. [How it's built](#6-how-its-built)
7. [Glossary](#7-glossary)

## 1. The loop

```mermaid
sequenceDiagram
    participant You
    participant Doc as doc pane
    participant F as thread file
    participant A as agent
    You->>Doc: Ink on, draw strokes
    Doc->>Doc: recognise each stroke, number it
    You->>Doc: optional note, ⌘↵
    Doc->>F: new thread c3 with its marks
    Doc->>A: types c3 (marks as text + marked-up picture)
    A->>A: reads the picture, edits the file
    A->>F: kanna markup reply c3 "1: … 2: …"
    F->>Doc: c3 shows the reply, marks it acted on leave the page
    You->>Doc: read it, resolve
```

## 2. Drawing

- **Where:** markdown files and HTML pages with comments on ([Doc Markup](doc-markup.md#2-rules)): they already anchor marks to text and parts of a page. Terminals, the diff viewer and code panes come later, once ink anchors to their lines.
- **Ink mode:** the **Ink** button in the pane's toolbar turns it on; then dragging draws and the page doesn't respond to the pointer. ⌥-drag draws one stroke without it. Esc turns it off.
- **One pen.** No palette: you draw, and Kanna recognises each stroke, keeps it as you drew it, numbers it, and labels it with what it saw. Click the label to correct it; × removes the mark.

| You draw | Recognised as | It points at |
|---|---|---|
| A closed loop | **circle** | the words or part inside it |
| A line under words | **underline** | those words |
| A line through words, or a zigzag over them | **strike** | those words |
| A line or curve, with or without a head | **arrow** | from what's at its start to before or after what's at its tip |
| Anything else, in a run of strokes | **writing** | the nearest block; read from the picture |

- An arrow drawn without a head gets one, pointing the way you drew it; a small V near an arrow's tip is taken as its head.
- **A batch, then send.** Strokes collect in a **New ink** card at the top of the thread list, numbered, with a box for an optional typed note. ⌘↵ (or **Send**) posts them all as one thread; **Discard** drops them. ⌘Z undoes the last stroke.

## 3. What the agent gets

Both a picture and words: the picture shows what you meant (the gesture, your writing), the words say where it is in the source.

![What c3 types into the agent, beside the marked-up picture it points to](ink-markup-sent.png)

- **The message** is a Doc Markup comment: one `>` line per mark, numbered as in the picture, then your note.

```
[kanna comment c3 · docs/specs/tab-container.md:7]
> ink, 4 marks, numbered in the picture:
> 1 circle around "Panes only:" (line 7)
> 2 writing beside line 7, read it in the picture
> 3 arrow from "The panes inside stay ordinary panes…" (line 23) to after "A tab container is a slot…" (line 3)
> 4 strike through "middle-click or × to close it." (line 14)
> marked up: .kanna/docs/marks/docs__specs__tab-container.md-c3.png
```

- **On a page**, marks name parts by CSS path and text with a box in page pixels, as [page comments](doc-markup.md#3-reaching-the-agent) do.
- **The picture** is the pane as shown, with the ink and its numbers drawn on, saved beside the threads (as `kanna screenshot` takes it).
- **What marks mean is a skill.** The `kanna-markup` skill gives the conventions (strike: cut it; arrow: move it there; circle or underline: this, see the note or writing; writing: read it from the picture) and tells the agent to answer mark by mark, saying how it read each, so a misread shows in the reply.

## 4. After it's sent

![The agent's answer: the paragraph moved, the strike gone, the circle and ? still marked, waiting for an answer](ink-markup-answered.png)

```mermaid
stateDiagram-v2
    [*] --> drawing: first stroke
    drawing --> [*]: Discard
    drawing --> sent: ⌘↵
    sent --> replied: kanna markup reply
    replied --> sent: you reply
    sent --> resolved: you resolve
    replied --> resolved: you resolve
    resolved --> sent: reopen
```

- **Ink stays on what it touched**, the way a comment's mark does: each stroke is kept relative to the block or part under it, so it follows edits, reloads and zoom.
- **Marks the agent acted on leave the page.** When the text or part under a mark changes, its ink goes; the thread keeps the picture as sent, so you can still see what you drew.
- **Resolving clears the rest.** A resolved thread's ink is hidden, and shows faintly when you open the thread from **Show resolved**.
- **Erasing:** before sending, ⌘Z, × or Discard. After, ink goes with its thread: resolve it.
- **The thread card** shows the picture cropped to the marks and the numbered list in place of a quote ([threads UI](doc-markup.md#2-rules)).

![Ink on a mockup page: an arrow moving a row, a zigzag over another, a circle round Save with a typed note](ink-markup-page.png)

## 5. iPad and Pencil

| | Mac | iPad |
|---|---|---|
| Draw | Ink on, or ⌥-drag | Pencil always draws; fingers scroll and click |
| Undo | ⌘Z | double-tap the Pencil |
| Stroke | points | points, with pressure and tilt kept |
| Recognition, anchors, numbering, message, picture, thread | same | same |

Only the input differs, so the recognition, the anchors and the thread format are one shared piece, ready for the iOS app (task-05a7, "Kanna for iOS").

## 6. How it's built

- **A thread with marks.** An ink thread is a Doc Markup thread whose target is a list of marks instead of a quote; each mark has its kind, its strokes (points relative to the block or part it's anchored to) and what it points at. The same thread file, CLI and skill.
- **Recognition runs where you draw** (the app, later the iPad), from the stroke alone plus what's under it; the label you correct wins.
- **The picture** is the pane's own capture with the ink layer on, the same as [`kanna screenshot`](screenshot.md).
- **Not yet decided:** whether the thread store moves to the daemon first (task-4053), which this would follow.

## 7. Glossary

| Term | Meaning |
|---|---|
| **Ink** | Strokes drawn on a doc |
| **Stroke** | One drag of the pointer or Pencil |
| **Mark** | One or more strokes recognised as one thing (a circle, an arrow with its head, a word of writing), numbered |
| **Batch** | The marks drawn before you send; they become one thread |
| **Picture** | The pane as shown with the batch's ink and numbers, sent with the message |
