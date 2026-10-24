---
name: spec-docs
description: Use when the user asks to draft, write, or update a spec, design doc, or technical specification, or when a decision, feature or subsystem needs documenting.
---

# Spec Docs

## Small specs, never a monolith
- One spec per feature or subsystem, in its own file (for example `docs/specs/<topic>.md`), next to its mockup.
- A spec is something a reader takes in at one sitting: aim for under 150 lines. Past about 200, split it into specs of its own.
- The project's top-level doc is an index: a few lines on what the project is, then links to the specs. It holds no feature content of its own.
- Never add a section to a big document. Write or update the topic's own spec and, at most, add one line linking to it.
- When you touch a big document, move the part you need out into its own spec and leave a one-line link in its place. Don't edit it in place.
- Link specs to each other rather than repeat them. Each fact lives in one spec.
- A decision goes in the spec it changes, not in chat, memory or a long changelog section.

## Writing a spec
Write in plain English.
Start with what it is and why, in a few lines, so a reader sees the high-level items at a glance and can dig further in for the details.
Include a table of contents when the spec has more than a few sections.
Include non-common technical terms in a glossary.
Make use of graphics. Use common image generation tools (mermaid etc) to include diagrams showing relationships, structure, states etc. In Kanna, only `flowchart`, `sequenceDiagram`, `stateDiagram-v2`, `classDiagram` and `gitGraph` draw; anything else (`erDiagram`, `gantt`, `pie`, …) shows as unsupported.
After writing or changing a diagram, check it draws: in Kanna, `kanna open md FILE` (or `kanna diagrams check FILE`) prints `FILE:LINE: message` for each diagram that won't; fix every one.
Then look at it before you present it: take a screenshot of the rendered doc (`kanna screenshot PANE` in Kanna), read the image, and fix labels that overlap or are cut off.
Don't describe what something looks like (colours, sizes, layout, styling) in words. That's what pictures are for: put in a screenshot of the mockup, and keep the words for behaviour, rules and interfaces, the things a picture can't show. When you edit a spec that already describes looks in words, replace the description with the picture; don't amend it.
