import type MarkdownIt from "markdown-it";
import type { BundledLanguage } from "shiki";

/**
 * Shared Shiki + markdown-it setup for previewing file content.
 *
 * `FilePreviewModal.vue` and the tree explorer's preview column both need to
 * syntax-highlight source and render Markdown, so the highlighter and the
 * markdown-it instance live here once, lazily created on first use, instead
 * of each caller standing up its own copy — see the `fuzzyMatch.ts` rule in
 * AGENTS.md, which is the same "don't duplicate the search utility" reasoning
 * applied to renderers.
 */

export type ShikiLanguage = BundledLanguage | "text";
export type ShikiTheme = "github-dark" | "github-light";

type ShikiModule = typeof import("shiki");
export type FilePreviewHighlighter = Awaited<ReturnType<ShikiModule["createHighlighter"]>>;

export function toShikiLanguage(lang: string): ShikiLanguage {
  return lang === "text" ? "text" : (lang as BundledLanguage);
}

let highlighterPromise: Promise<FilePreviewHighlighter> | null = null;

/** Lazy-loaded so a desktop session that never opens a file preview never pays for Shiki's grammars/themes. */
export function getFilePreviewHighlighter(): Promise<FilePreviewHighlighter> {
  if (!highlighterPromise) {
    highlighterPromise = import("shiki").then(({ createHighlighter }) =>
      createHighlighter({ themes: ["github-dark", "github-light"], langs: [] }),
    );
  }
  return highlighterPromise;
}

/**
 * `lang` itself once Shiki can tokenize it, `"text"` when it can't (an
 * unknown extension, or a grammar Shiki has none for) — callers never render
 * a language they failed to load.
 */
export async function resolveHighlightableLanguage(lang: string): Promise<ShikiLanguage> {
  const candidate = toShikiLanguage(lang);
  if (candidate === "text") return "text";
  const highlighter = await getFilePreviewHighlighter();
  // Shiki no-ops a load for a language it already has, so this is cheap to
  // call unconditionally rather than tracking "have we asked for this one
  // before" ourselves.
  try {
    await highlighter.loadLanguage(candidate);
  } catch {
    // No grammar for this language — falls back to "text" below.
  }
  return highlighter.getLoadedLanguages().includes(candidate) ? candidate : "text";
}

/** Highlight a whole file's worth of code to HTML, falling back to plain text for an unknown language. */
export async function highlightFilePreviewCode(
  code: string,
  lang: string,
  theme: ShikiTheme,
): Promise<string> {
  const highlighter = await getFilePreviewHighlighter();
  const useLang = await resolveHighlightableLanguage(lang);
  return highlighter.codeToHtml(code, { lang: useLang, theme });
}

// The markdown-it `highlight` callback is synchronous, so the theme for the
// render in progress is threaded through this module-level slot rather than
// as a constructor option — render() itself is synchronous, so nothing else
// can observe it mid-render.
let markdownRenderTheme: ShikiTheme = "github-dark";

let markdownRendererPromise: Promise<MarkdownIt> | null = null;

function getMarkdownRenderer(): Promise<MarkdownIt> {
  if (!markdownRendererPromise) {
    markdownRendererPromise = (async () => {
      const [{ default: MarkdownIt }, { default: taskLists }, { default: strikethrough }] =
        await Promise.all([
          import("markdown-it"),
          import("markdown-it-task-lists"),
          import("markdown-it-strikethrough-alt"),
        ]);
      const highlighter = await getFilePreviewHighlighter();

      // `html: false` is the sanitization: markdown-it escapes raw HTML in the
      // source instead of passing it through, which is what keeps rendered
      // repo content (untrusted input) from becoming a script-injection vector
      // in the webview.
      const renderer = new MarkdownIt({
        html: false,
        linkify: true,
        typographer: false,
        highlight(code, lang) {
          if (!lang) return highlighter.codeToHtml(code, { lang: "text", theme: markdownRenderTheme });
          const loaded = highlighter.getLoadedLanguages();
          const useLang = loaded.includes(toShikiLanguage(lang)) ? toShikiLanguage(lang) : "text";
          return highlighter.codeToHtml(code, { lang: useLang, theme: markdownRenderTheme });
        },
      });
      renderer.use(taskLists, { enabled: false });
      renderer.use(strikethrough);
      return renderer;
    })();
  }
  return markdownRendererPromise;
}

/** Render Markdown to sanitized HTML, with fenced code blocks Shiki-highlighted. */
export async function renderFilePreviewMarkdown(raw: string, theme: ShikiTheme): Promise<string> {
  const renderer = await getMarkdownRenderer();
  const highlighter = await getFilePreviewHighlighter();

  // markdown-it's `highlight` callback is synchronous, so every fenced
  // language it will need has to be loaded before render() runs.
  const langMatches = raw.matchAll(/^```(\w+)/gm);
  const langs = [...new Set([...langMatches].map((match) => match[1]))];
  await Promise.all(
    langs.map((lang) => highlighter.loadLanguage(toShikiLanguage(lang)).catch(() => undefined)),
  );

  // Set immediately before the (fully synchronous) render call: `renderer` and
  // `highlighter` are shared across every caller, so this module-level slot is
  // only ever read inside render()'s own call stack, with nothing else able to
  // run in between and observe a different value.
  markdownRenderTheme = theme;
  return renderer.render(raw);
}
