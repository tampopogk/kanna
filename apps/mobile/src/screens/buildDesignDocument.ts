import {
  DESIGN_EDITOR_CSS,
  DESIGN_EDITOR_SCHEMA_VERSION,
  DESIGN_EDITOR_SCRIPT
} from "./designEditorAssets.generated";

export { DESIGN_EDITOR_SCHEMA_VERSION };

/** Keep bundled code from closing the element it is inlined in. */
function inline(content: string, element: "script" | "style"): string {
  return content.replace(new RegExp(`</${element}`, "gi"), `<\\/${element}`);
}

/**
 * The phone's design page: the bundled editor from packages/design-editor,
 * inlined, with no origin and no network. Its content-security policy
 * forbids every fetch, so the page can reach kanna-server only by asking the
 * app over the bridge; the app holds the task, the connection and the
 * device's pairing, and the page holds none of them.
 */
export function buildDesignDocument(options: { theme: "light" | "dark" }): string {
  const start = JSON.stringify({ theme: options.theme });
  return `<!doctype html>
<html lang="en" data-theme="${options.theme}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; frame-src 'none'; form-action 'none'">
<style>${inline(DESIGN_EDITOR_CSS, "style")}</style>
</head>
<body>
<div id="root"></div>
<script>${inline(DESIGN_EDITOR_SCRIPT, "script")}</script>
<script>window.__kannaDesign && window.__kannaDesign.start(${start});</script>
</body>
</html>`;
}

/** A script the app injects to hand the page a message. */
export function designPageMessageScript(message: unknown): string {
  return `window.__kannaDesign && window.__kannaDesign.receive(${JSON.stringify(message)}); true;`;
}
