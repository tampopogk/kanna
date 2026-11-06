# Scroll Anchor

**Status:** built · **Tested:** `swift test --filter "ScrollAnchorTests|ZoomAnchorTests"`, e2e `markdown_zoom_anchor`, `web_pinch_anchor`, `web_pinch_comments` · **Builds on:** [markdown pane](app-panes.md), [markdown zoom](markdown-zoom.md), [code viewer](code-viewer.md), [diff viewer](diff-viewer.md), [zoom](zoom.md) · **Why:** zooming and unzooming a long markdown pane crept towards the start of the file, a little each round trip, because the pane kept its scroll as points from the top while the text rewrapped and the diagrams rescaled

A pane keeps its place in the text, not in points. Whatever changes its size or its font, the text at the top of the view stays at the top, the same part of its line showing.

## 1. Rules

- **The anchor:** the character starting the line fragment at the top edge of the view, and how far into that fragment the view starts, as a fraction of its height. A fragment is a wrapped line of text, or a whole diagram or image, so a picture at the top keeps the same fraction of itself showing.
- **Markdown: anything that lays the page out again.** Before a layout that changes the pane's text width (pane zoom and unzoom, a window or split resize, the overview showing or hiding) or the text size (⌘+ ⌘− ⌘0), the pane notes the anchor; after the new layout it scrolls the anchor back to the top. A height change moves nothing, and a pinch and the thread list, which floats over the page, don't lay it out, so none of them is anchored. Sideways scroll at a page zoom stays as it is on a resize, and in proportion on a text-size step.
- **Round trips are exact:** after it restores an anchor the pane remembers it with the scroll it gave, and reuses it until the view is scrolled. Recomputing it from the new layout would take the start of the wrapped line, earlier than the anchor, and each round trip would creep back by one wrap.
- **The top stays the top:** a view at the very top stays there.
- **A reload** (the file changed, an image arrived) keeps the anchor too, as it kept the reading position before.
- **Code and diff panes** don't wrap, so a resize moves nothing; only their font zoom changes the lines' heights. ⌘+ and ⌘− there keep the line (code) or row (diff) at the top with the same fraction of it showing. Before, the code pane snapped to the top line's start and the diff pane kept its points, landing rows away.
- **Web panes keep a point, not the top.** A pinch keeps the page's point under the place it began, however the fingers drift, until they lift; the anchor is taken once, in page coordinates, so small steps don't add up, and a pinch out against the page's edge and back returns to it. ⌘+ and ⌘− keep the point at the view's middle. An edge can stop the page mid-pinch: the anchor comes back when there's room.

## 2. WKWebView's centre point

`setMagnification(_:centeredAt:)` doesn't centre on the point. Measured on macOS 26: it leaves the top-left where it is and then scrolls by minus the point, at the old magnification, rounded to a whole point: the new scroll is `scroll - point / old`, whatever the new magnification, and a page that can't scroll takes it too. A plain point under the cursor therefore pushed the page towards its start on every event of a pinch.

To hold view point `v` going from `old` to `new`, the argument is `v * (old / new - 1)`. That rounding, and WebKit's own rounding of the scroll, add up over a pinch's small steps (6 points over 120 steps, with the remainder carried from step to step). So a pinch reads the page's position once when it begins, and after each step also sets the scroll to `anchor - v / new`. The scale change still carries the relative shift, so the step lands within a point or so before the scroll corrects it, and steps sent before the position arrives only use the shift. Both scripts run in the `.defaultClient` content world, out of reach of the page's own `scrollTo`, and with `behavior: 'instant'`, so a page's `scroll-behavior: smooth` doesn't animate them.

The stock `allowsMagnification` pinch anchors properly but stops at 100%, and web panes zoom down to 50%.

## 3. Interfaces

- `debug.md` returns `top`: `{text, fraction}`, the anchor's line of text (or `[picture at N]`) and the fraction; `""` at the very top.
- `debug.pinch` takes `at: [x, y]`, a point in the page view from its top left, and `drift: [x, y]`, how far the fingers move each event; a web pane reports `spots`, its `[left, top, scale]` after each step.
- `FocusReportingWebView.zoom(to:around:)`.
- `MarkdownPaneView.readingPosition()`, `restoreReadingPosition(_:)`; `CodePaneView.topSpot`, `restore(_:)`; `DiffPaneView.topSpot`, `restore(_:)`.
