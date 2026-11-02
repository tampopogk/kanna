# Scroll Anchor

**Status:** built · **Tested:** `swift test --filter "ScrollAnchorTests|ZoomAnchorTests"`, e2e `markdown_zoom_anchor` · **Builds on:** [markdown pane](app-panes.md), [code viewer](code-viewer.md), [diff viewer](diff-viewer.md), [zoom](zoom.md) · **Why:** zooming and unzooming a long markdown pane crept towards the start of the file, a little each round trip, because the pane kept its scroll as points from the top while the text rewrapped and the diagrams rescaled

A pane keeps its place in the text, not in points. Whatever changes its size or its font, the text at the top of the view stays at the top, the same part of its line showing.

## 1. Rules

- **The anchor:** the character starting the line fragment at the top edge of the view, and how far into that fragment the view starts, as a fraction of its height. A fragment is a wrapped line of text, or a whole diagram or image, so a picture at the top keeps the same fraction of itself showing.
- **Markdown: any change of width.** Before a layout that changes the text column's width or the magnification (pane zoom and unzoom, a window or split resize, the overview or the threads panel showing or hiding, ⌘+ ⌘− ⌘0, a pinch), the pane notes the anchor; after the new layout it scrolls the anchor back to the top. A height change alone moves nothing and isn't anchored.
- **Round trips are exact:** after it restores an anchor the pane remembers it with the scroll it gave, and reuses it until the view is scrolled. Recomputing it from the new layout would take the start of the wrapped line, earlier than the anchor, and each round trip would creep back by one wrap.
- **The top stays the top:** a view at the very top stays there.
- **⌘+ and ⌘− keep the top, not the middle.** Before, markdown zoomed around the centre of the view; now it keeps the anchor, the same as a resize.
- **A reload** (the file changed, an image arrived) keeps the anchor too, as it kept the reading position before.
- **Code and diff panes** don't wrap, so a resize moves nothing; only their font zoom changes the lines' heights. ⌘+ and ⌘− there keep the line (code) or row (diff) at the top with the same fraction of it showing. Before, the code pane snapped to the top line's start and the diff pane kept its points, landing rows away.

## 2. Interfaces

- `debug.md` returns `top`: `{text, fraction}`, the anchor's line of text (or `[picture at N]`) and the fraction; `""` at the very top.
- `MarkdownPaneView.readingPosition()`, `restoreReadingPosition(_:)`; `CodePaneView.topSpot`, `restore(_:)`; `DiffPaneView.topSpot`, `restore(_:)`.
