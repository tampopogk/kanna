# Workspace pane controls: focused verification

This change extends the existing acknowledged desktop-view lane. Focused tests
cover catalog arguments through the real HTTP request/command/ack route,
renderer command parsing through the real pane controller, the real MainPanel
and contained file viewer opening AGENTS.md in the second displayed pane, and
HTTP acknowledgement serialization. Coverage includes both split directions,
move/state preservation, no reused removed-pane ids, missing/cross-task/stale
identities, workspace changes during delivery, unavailable desktops, and path
containment before delivery. Existing open-view target and containment tests
remain applicable to the unchanged readers.

A full MCP process -> native window event -> real webview -> HTTP acknowledgement
smoke is not run here: the focused request/component tests isolate the changed
boundaries without starting a cold native stack or touching the operator's live
workspace. The remaining native-specific check is exact requested window label
delivery (including a missing window), then inspect -> split -> open -> inspect
in that same window. Run it only through the canonical isolated task-titled
runner with a second isolated workspace window; assert window, task, branch,
pane and tab identities throughout. No installed production/staging window is
a test target. The manager owns the single independent cross-process review.

The concurrent pane-focus/maximize task e9c9b16b owns keyboard actions and the
maximized visible-pane projection. Coordination agreed that it preserves
workspacePresentation(), which reads MainPanel's actual visiblePanes and thus
will also describe the maximized projection without a parallel layout model.
