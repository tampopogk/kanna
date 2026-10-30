# Markdown Pane Keys

**Status:** proposed

A markdown pane is a preview, so its keys are free for moving around. With the pane focused, a pager's keys work (`less`'s, which Vim's mostly share), so you can read a doc without reaching for the mouse. A [code pane](code-viewer.md) takes the same keys.

## Contents

1. [Keys](#1-keys)
2. [Rules](#2-rules)
3. [How it's built](#3-how-its-built)
4. [Open questions](#4-open-questions)

## 1. Keys

| Keys | Move |
|---|---|
| `j` `k`, `↓` `↑` (Vim: `⌃e` `⌃y`) | a line down, up |
| `d` `u` (Vim: `⌃d` `⌃u`) | half a screen down, up |
| `f` `b`, `space` `⇧space` (Vim: `⌃f` `⌃b`) | a screen down, up |
| `g` `G` (Vim: `gg`) | top, bottom |
| `}` `{` | next, previous paragraph |
| `]]` `[[` | next, previous heading |
| `/` `?` | find forward, back |
| `n` `N` | next, previous match |
| `]c` `[c` | next, previous change, in a doc opened with `--diff` ([markdown diff](markdown-diff.md)) |
| `Esc` | stop finding, clear the matches |

## 2. Rules

- **Only when the pane is focused**, and never while you're typing in a text field in it (the find box, a comment).
- **⌘ shortcuts stay the app's.** Only plain and ⌃ keys are taken; menus keep every ⌘ key.
- **less's keys first, Vim's too.** Plain letters are free in a preview, so `less`'s keys work as they are; Vim's ⌃ keys and `gg` do the same, for hands used to them.
- **A count works** as in less and Vim: `5j`, `3}`.
- **Scrolling moves the view, not a cursor:** a preview has none. "A line" is one line of body text at the pane's zoom.
- **Find** searches the rendered text, not the markdown source; matches are highlighted and the view scrolls to keep the current one in the middle.

```mermaid
stateDiagram-v2
    [*] --> reading
    reading --> reading: j k d u f b space g G { } ]] [[ n N
    reading --> finding: / or ?
    finding --> reading: ↵ (keep matches) or Esc (clear)
```

## 3. How it's built

All in the app's markdown view: a key handler on the view, before AppKit's own text keys. The daemon is not involved. Tests drive it with `debug.key`; `debug.md` gains the scroll position and the matches so they can check where it went.

## 4. Open questions

1. Should the same keys work in a web pane when no text field is focused?
2. Visual mode (`v` and moves to select): useful for commenting with the keyboard ([doc markup](doc-markup.md)), or too much?
