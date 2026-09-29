# add a button in the sidebar to add repos for those of us who are keyboard-challe

Add repositories from the sidebar

Add a persistent plus-only (+) button at the right end of the desktop sidebar’s top action row. Keep All / Unread / Needs you on the left and the plus aligned to the right edge, all on one horizontal line. Vertically center all controls and prevent wrapping. Keep this row outside the scrolling list. Match the existing add-task buttons: 24×24px, 14px plus, subtle border, 4px radius and existing theme/hover styling. Show an “Add repository” tooltip using the same title-attribute pattern as add-task buttons. Keep search in the footer.

Click opens the existing Add Repository dialog on Import, with Create available in the same dialog. Reuse existing import/create behavior, validation and errors. Cancel leaves the sidebar unchanged and restores focus to the button. Preserve existing keyboard shortcuts.

Keep the button visible with zero repositories, long scrolling lists, collapsed repositories and active search or attention filters. Its top-level placement and “Add repository” tooltip distinguish it from each repository’s new-task plus action.

Use a native button, localized accessible label “Add repository”, visible focus indicator, and Tab/Enter/Space support. Verify dialog opening, Create access, cancel/focus restoration, and visibility with empty, filtered, long-list and narrow-sidebar states.

The interactive mockup preserves the approved placement: filters on the left, plus on the right, all on one line. Click + to open Import; switch to Create; choose a sample local folder or enter a value; submit to append a sample repository. Cancel, Escape and backdrop dismissal restore focus to +. Repository operations and folder selection are simulated without filesystem or network effects. Application implementation belongs to the build stage.

The runnable prototype is published in the Interactive mockup view, because Kanna’s Prototype position displays this document rather than a preview. It includes default, empty and long-list scenarios, working search, and simulated Import/Create flows. Its source is prototype/index.html in the design scratch repository, committed there with the mockups; no application code was changed.

Verified in headless Chrome: + opens Import; Create submits a sample repository; choosing a sample folder adds it from an empty sidebar; Escape closes the dialog and restores focus to +; the button stays visible while a long list scrolls. At the production minimum sidebar width of 220px, the action row fits without horizontal overflow. Visually inspected the narrow layout. Native folder selection and actual repository operations remain unverified because this prototype simulates them; the build must reuse existing application handlers.

Superseding prototype: the interactive preview now runs copies of production Sidebar.vue and AddRepoModal.vue with their actual theme, translations and helpers, hosted by a small Vue app. The sidebar copy adds the proposed right-aligned +; repository state, invoke, home-directory lookup and folder selection are stubbed. This is the real component UI, not the full App.vue shell or a live Tauri webview. Source and bundled real-prototype/dist/index.html are committed in the disposable scratch repository. Chrome verification passed for dialog opening, sample folder import, Escape and focus restoration; render visually inspected. The earlier hand-written prototype is superseded.

Preview fix: precompile the Vue host template at build time so it does not require runtime code evaluation, which Kanna’s artifact sandbox blocks. Republished the actual-component prototype. Verified in an opaque-origin iframe with sandbox allow-scripts and CSP denying eval and network: sidebar render, dialog opening, sample-folder import, Escape/focus restoration and zero runtime errors.
