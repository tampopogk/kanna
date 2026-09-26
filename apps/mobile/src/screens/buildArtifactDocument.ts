import type { ArtifactFileContent } from "../lib/api/types";
import { buildTaskFilePreviewDocument } from "./buildTaskFilePreviewDocument";
import { escapeTaskFileHtml } from "./taskFileSyntaxHighlight";

/**
 * Render one file of an artifact tree as a self-contained document for an
 * isolated WebView.
 *
 * The desktop frames the artifact store's loopback preview listener, where a
 * page's relative references resolve under a capability URL. A phone cannot
 * reach that listener, so it reads files by exact tree id and puts the page
 * together itself: every relative reference the page makes into its own tree
 * — stylesheets, scripts, images, fonts, media, and the `url()`s inside
 * stylesheets — is read and inlined as a `data:` URL. A link to another file of
 * the same tree becomes a fragment link that asks the host document to show
 * that file (see `isolateArtifactDocument`); the page never navigates for it.
 *
 * The page gets a Content-Security-Policy with no network at all
 * (`connect-src 'none'` and only `data:` subresources), no frames, no forms
 * and no `<base>`. A policy can only be tightened by a later one, so nothing
 * the artifact declares can loosen it.
 */

/** Fragment prefix of a rewritten in-tree link; the rest is the encoded path. */
export const ARTIFACT_LINK_PREFIX = "#kanna-artifact=";

/** The one message a page may send its host: show another file of this tree. */
export const ARTIFACT_NAVIGATE_MESSAGE = "kanna-artifact-navigate";

export const ARTIFACT_DOCUMENT_POLICY = [
  "default-src 'none'",
  "script-src 'unsafe-inline' data:",
  "style-src 'unsafe-inline' data:",
  "img-src data:",
  "font-src data:",
  "media-src data:",
  "connect-src 'none'",
  "frame-src 'none'",
  "child-src 'none'",
  "worker-src 'none'",
  "object-src 'none'",
  "manifest-src 'none'",
  "form-action 'none'",
  "base-uri 'none'"
].join("; ");

/** Bounds on what one rendered page may pull in; beyond them a reference is left unresolved. */
export const MAX_INLINED_FILES = 200;
export const MAX_INLINED_BYTES = 8 * 1024 * 1024;
const MAX_STYLESHEET_DEPTH = 4;

export type ReadArtifactDocumentFile = (path: string) => Promise<ArtifactFileContent>;

export interface ArtifactDocumentOptions {
  path: string;
  readFile: ReadArtifactDocumentFile;
  /** Raw source views of text files scroll here, e.g. from a comment's "line 4". */
  initialLine?: number;
  /** True once the viewer no longer wants this page; no further file is read. */
  isCancelled?: () => boolean;
}

/** Thrown when a build is abandoned before its entry file is read. */
export class ArtifactBuildCancelled extends Error {
  constructor() {
    super("artifact page build cancelled");
    this.name = "ArtifactBuildCancelled";
  }
}

export interface ArtifactDocument {
  html: string;
  /** In-tree references that do not exist in this tree. */
  missing: string[];
  /** References left unresolved because a bound was reached or the file was refused. */
  skipped: string[];
}

const BASE64_ALPHABET =
  "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_VALUES = new Map(
  [...BASE64_ALPHABET].map((character, index) => [character, index])
);

export function decodeBase64(data: string): Uint8Array {
  const clean = data.replace(/[^A-Za-z0-9+/]/g, "");
  const bytes = new Uint8Array(Math.floor((clean.length * 3) / 4));
  let buffer = 0;
  let bits = 0;
  let length = 0;
  for (const character of clean) {
    buffer = (buffer << 6) | (BASE64_VALUES.get(character) ?? 0);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes[length++] = (buffer >> bits) & 0xff;
    }
  }
  return bytes.subarray(0, length);
}

export function encodeBase64(bytes: Uint8Array): string {
  let output = "";
  for (let index = 0; index < bytes.length; index += 3) {
    const a = bytes[index];
    const b = bytes[index + 1];
    const c = bytes[index + 2];
    output += BASE64_ALPHABET[a >> 2];
    output += BASE64_ALPHABET[((a & 3) << 4) | ((b ?? 0) >> 4)];
    output += b === undefined ? "=" : BASE64_ALPHABET[((b & 15) << 2) | ((c ?? 0) >> 6)];
    output += c === undefined ? "=" : BASE64_ALPHABET[c & 63];
  }
  return output;
}

/** UTF-8 to a string, replacing malformed sequences rather than throwing. */
export function decodeUtf8(bytes: Uint8Array): string {
  let output = "";
  let index = 0;
  while (index < bytes.length) {
    const first = bytes[index];
    let codePoint = 0xfffd;
    let width = 1;
    if (first < 0x80) {
      codePoint = first;
    } else if (first >= 0xc2 && first < 0xe0 && index + 1 < bytes.length) {
      codePoint = ((first & 0x1f) << 6) | (bytes[index + 1] & 0x3f);
      width = 2;
    } else if (first >= 0xe0 && first < 0xf0 && index + 2 < bytes.length) {
      codePoint =
        ((first & 0x0f) << 12) | ((bytes[index + 1] & 0x3f) << 6) | (bytes[index + 2] & 0x3f);
      width = 3;
    } else if (first >= 0xf0 && first < 0xf5 && index + 3 < bytes.length) {
      codePoint =
        ((first & 0x07) << 18) |
        ((bytes[index + 1] & 0x3f) << 12) |
        ((bytes[index + 2] & 0x3f) << 6) |
        (bytes[index + 3] & 0x3f);
      width = 4;
    }
    output += String.fromCodePoint(codePoint > 0x10ffff ? 0xfffd : codePoint);
    index += width;
  }
  return output;
}

export function encodeUtf8(text: string): Uint8Array {
  const bytes: number[] = [];
  for (const character of text) {
    const codePoint = character.codePointAt(0) ?? 0xfffd;
    if (codePoint < 0x80) bytes.push(codePoint);
    else if (codePoint < 0x800) bytes.push(0xc0 | (codePoint >> 6), 0x80 | (codePoint & 63));
    else if (codePoint < 0x10000)
      bytes.push(0xe0 | (codePoint >> 12), 0x80 | ((codePoint >> 6) & 63), 0x80 | (codePoint & 63));
    else
      bytes.push(
        0xf0 | (codePoint >> 18),
        0x80 | ((codePoint >> 12) & 63),
        0x80 | ((codePoint >> 6) & 63),
        0x80 | (codePoint & 63)
      );
  }
  return Uint8Array.from(bytes);
}

function directoryOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash + 1);
}

function isExternalReference(reference: string): boolean {
  return /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(reference) || reference.startsWith("//");
}

/**
 * Resolve a reference made by the file at `from` to a path inside the tree,
 * the way a browser resolves it under the desktop's capability URL. `null` for
 * an external, fragment-only, root-relative or escaping reference.
 */
export function resolveArtifactReference(from: string, reference: string): string | null {
  const trimmed = reference.trim();
  if (!trimmed || trimmed.startsWith("#") || trimmed.startsWith("/") || isExternalReference(trimmed)) {
    return null;
  }
  const withoutSuffix = trimmed.split(/[?#]/, 1)[0];
  if (!withoutSuffix) return null;
  let decoded: string;
  try {
    decoded = decodeURIComponent(withoutSuffix);
  } catch {
    return null;
  }
  const segments: string[] = [];
  for (const segment of `${directoryOf(from)}${decoded}`.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      if (segments.length === 0) return null;
      segments.pop();
      continue;
    }
    segments.push(segment);
  }
  const resolved = segments.join("/");
  if (!resolved) return null;
  return decoded.endsWith("/") ? `${resolved}/index.html` : resolved;
}

function fragmentOf(reference: string): string {
  const index = reference.indexOf("#");
  return index < 0 ? "" : reference.slice(index);
}

function isHtmlPath(path: string): boolean {
  return /\.html?$/i.test(path);
}

function isMarkdownOrText(mediaType: string, path: string): boolean {
  return (
    mediaType.startsWith("text/") ||
    mediaType.startsWith("application/json") ||
    /\.(md|markdown|txt|json|ya?ml|toml|csv|log)$/i.test(path)
  );
}

function decodeEntities(value: string): string {
  return value
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

function dataUrl(mediaType: string, base64: string): string {
  return `data:${mediaType.replace(/\s+/g, "")};base64,${base64}`;
}

class ArtifactPageBuilder {
  readonly missing = new Set<string>();
  readonly skipped = new Set<string>();
  private readonly reads = new Map<string, Promise<ArtifactFileContent | null>>();
  private inlinedBytes = 0;

  constructor(
    private readonly readFile: ReadArtifactDocumentFile,
    private readonly isCancelled: () => boolean = () => false
  ) {}

  read(path: string): Promise<ArtifactFileContent | null> {
    let pending = this.reads.get(path);
    if (!pending) {
      if (this.isCancelled()) return Promise.resolve(null);
      if (this.reads.size >= MAX_INLINED_FILES) {
        this.skipped.add(path);
        return Promise.resolve(null);
      }
      pending = this.readFile(path).then(
        (file) => {
          this.inlinedBytes += file.size;
          if (this.inlinedBytes > MAX_INLINED_BYTES) {
            this.skipped.add(path);
            return null;
          }
          return file;
        },
        (error: unknown) => {
          if (errorCode(error) === "artifact_file_not_found" || /not found|\b404\b/i.test(String(error))) {
            this.missing.add(path);
          } else {
            this.skipped.add(path);
          }
          return null;
        }
      );
      this.reads.set(path, pending);
    }
    return pending;
  }

  /** A reference from `from` as a data URL, or unchanged when it is not an in-tree file. */
  async inline(from: string, reference: string, depth: number): Promise<string> {
    const path = resolveArtifactReference(from, reference);
    if (!path) return reference;
    const file = await this.read(path);
    if (!file) return reference;
    if (file.mediaType.startsWith("text/css") && depth < MAX_STYLESHEET_DEPTH) {
      const css = await this.stylesheet(path, decodeUtf8(decodeBase64(file.dataBase64)), depth + 1);
      return dataUrl(file.mediaType, encodeBase64(encodeUtf8(css)));
    }
    return dataUrl(file.mediaType, file.dataBase64) + fragmentOf(reference);
  }

  /** Rewrite `url(...)` and `@import "..."` inside a stylesheet read from `from`. */
  async stylesheet(from: string, css: string, depth: number): Promise<string> {
    const pattern = /url\(\s*(?:"([^"]*)"|'([^']*)'|([^)'"\s]*))\s*\)|@import\s+(?:"([^"]*)"|'([^']*)')/g;
    return replaceAsync(css, pattern, async (match, ...groups) => {
      const [urlDouble, urlSingle, urlBare, importDouble, importSingle] = groups as (string | undefined)[];
      const reference = urlDouble ?? urlSingle ?? urlBare ?? importDouble ?? importSingle ?? "";
      const inlined = await this.inline(from, reference, depth);
      if (inlined === reference) return match;
      return importDouble !== undefined || importSingle !== undefined
        ? `@import url("${inlined}")`
        : `url("${inlined}")`;
    });
  }

  async attributes(from: string, tag: string, attributes: string): Promise<string> {
    const pattern = /(\s)([a-zA-Z_:][-a-zA-Z0-9_:.]*)(\s*=\s*)(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))/g;
    const lowerTag = tag.toLowerCase();
    return replaceAsync(attributes, pattern, async (match, space, name, equals, doubleQuoted, singleQuoted, bare) => {
      const rawValue = (doubleQuoted ?? singleQuoted ?? bare ?? "") as string;
      const value = decodeEntities(rawValue);
      const attribute = (name as string).toLowerCase();
      let rewritten: string | null = null;
      if (attribute === "style") {
        rewritten = await this.stylesheet(from, value, 1);
      } else if (attribute === "srcset") {
        const candidates = await Promise.all(
          value.split(",").map(async (candidate) => {
            const [url, ...descriptor] = candidate.trim().split(/\s+/);
            return [await this.inline(from, url ?? "", 0), ...descriptor].join(" ");
          })
        );
        rewritten = candidates.join(", ");
      } else if (attribute === "href" && (lowerTag === "a" || lowerTag === "area")) {
        const path = resolveArtifactReference(from, value);
        rewritten = path ? `${ARTIFACT_LINK_PREFIX}${encodeArtifactPath(path)}` : null;
      } else if (attribute === "src" || attribute === "href" || attribute === "poster" || attribute === "data") {
        const inlined = await this.inline(from, value, 0);
        rewritten = inlined === value ? null : inlined;
      }
      if (rewritten === null) return match;
      return `${space}${name}${equals}"${escapeTaskFileHtml(rewritten)}"`;
    });
  }

  async page(path: string, html: string): Promise<string> {
    // Leave comments and script bodies alone; rewrite tags and style bodies.
    const pattern = /<!--[\s\S]*?-->|<(script|style)(\s[^>]*)?>([\s\S]*?)<\/\1\s*>|<([a-zA-Z][a-zA-Z0-9-]*)(\s[^>]*?)?(\/?)>/g;
    const body = await replaceAsync(html, pattern, async (match, rawTag, rawAttributes, content, tag, attributes, selfClosing) => {
      if (match.startsWith("<!--")) return match;
      if (rawTag) {
        const opened = `<${rawTag}${await this.attributes(path, rawTag as string, (rawAttributes as string | undefined) ?? "")}>`;
        const inner = (rawTag as string).toLowerCase() === "style"
          ? await this.stylesheet(path, content as string, 1)
          : (content as string);
        return `${opened}${inner}</${rawTag}>`;
      }
      return `<${tag}${await this.attributes(path, tag as string, (attributes as string | undefined) ?? "")}${selfClosing}>`;
    });
    const policy = `<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_DOCUMENT_POLICY}">`;
    const withoutDoctype = body.replace(/^\s*<!doctype[^>]*>/i, "");
    return `<!doctype html>\n${policy}\n<meta name="viewport" content="width=device-width, initial-scale=1">\n${ARTIFACT_PAGE_LINK_SCRIPT}\n${withoutDoctype}`;
  }
}

function errorCode(error: unknown): string | null {
  if (typeof error === "object" && error !== null && "code" in error) {
    const code = (error as { code?: unknown }).code;
    if (typeof code === "string") return code;
  }
  const match = String(error instanceof Error ? error.message : error).match(/"error"\s*:\s*"([a-z_]+)"/);
  return match?.[1] ?? null;
}

async function replaceAsync(
  input: string,
  pattern: RegExp,
  replace: (match: string, ...groups: unknown[]) => Promise<string>
): Promise<string> {
  const pending: Promise<string>[] = [];
  input.replace(pattern, (match, ...rest) => {
    pending.push(replace(match, ...rest.slice(0, -2)));
    return match;
  });
  const replacements = await Promise.all(pending);
  let index = 0;
  return input.replace(pattern, () => replacements[index++]);
}

function shell(title: string, body: string): string {
  return `<!doctype html>
<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_DOCUMENT_POLICY}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeTaskFileHtml(title)}</title>
<style>html,body{margin:0;background:#050b14;color:#d7e2f0;font-family:-apple-system,sans-serif}body{padding:16px}img,video{max-width:100%;height:auto}</style>
${body}`;
}

export async function buildArtifactDocument({
  path,
  readFile,
  initialLine,
  isCancelled = () => false
}: ArtifactDocumentOptions): Promise<ArtifactDocument> {
  if (isCancelled()) throw new ArtifactBuildCancelled();
  const builder = new ArtifactPageBuilder(readFile, isCancelled);
  const entry = await readFile(path);
  const mediaType = entry.mediaType.toLowerCase();
  let html: string;
  if (isHtmlPath(entry.path) || mediaType.startsWith("text/html")) {
    html = await builder.page(entry.path, decodeUtf8(decodeBase64(entry.dataBase64)));
  } else if (mediaType.startsWith("image/")) {
    html = shell(entry.path, `<img alt="${escapeTaskFileHtml(entry.path)}" src="${dataUrl(entry.mediaType, entry.dataBase64)}">`);
  } else if (mediaType.startsWith("video/") || mediaType.startsWith("audio/")) {
    const element = mediaType.startsWith("video/") ? "video" : "audio";
    html = shell(entry.path, `<${element} controls src="${dataUrl(entry.mediaType, entry.dataBase64)}"></${element}>`);
  } else if (isMarkdownOrText(mediaType, entry.path)) {
    const line = typeof initialLine === "number" && initialLine > 0 ? initialLine : undefined;
    html = buildTaskFilePreviewDocument({
      path: entry.path,
      content: decodeUtf8(decodeBase64(entry.dataBase64)),
      mode: /\.md$/i.test(entry.path) && !line ? "rendered" : "raw",
      initialLine: line
    });
  } else {
    html = shell(entry.path, `<p>${escapeTaskFileHtml(entry.path)} (${escapeTaskFileHtml(entry.mediaType)}) cannot be previewed here.</p>`);
  }
  if (isCancelled()) throw new ArtifactBuildCancelled();
  return { html, missing: [...builder.missing], skipped: [...builder.skipped] };
}

/**
 * Registered first in every HTML page, in the page's own sandboxed context: a
 * click on a rewritten in-tree link asks the parent to show that path. This is
 * a request, not a navigation; the host decides.
 */
const ARTIFACT_PAGE_LINK_SCRIPT = `<script>(function(){var P=${JSON.stringify(ARTIFACT_LINK_PREFIX)};document.addEventListener("click",function(e){var n=e.target;while(n&&n.nodeName!=="A"&&n.nodeName!=="AREA")n=n.parentNode;if(!n||!n.getAttribute)return;var h=n.getAttribute("href")||"";if(h.indexOf(P)!==0)return;e.preventDefault();var p;try{p=decodeURIComponent(h.slice(P.length))}catch(x){return}parent.postMessage({kind:${JSON.stringify(ARTIFACT_NAVIGATE_MESSAGE)},path:p},"*")},true)})();</script>`;

function encodeArtifactPath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

/** Sandbox flags for the frame that runs an artifact page: scripts, nothing else. */
export const ARTIFACT_FRAME_SANDBOX = "allow-scripts";

/**
 * The top-level navigation the host document makes to ask the viewer for
 * another file of the tree, where the viewer's navigation callback is always
 * answered before the engine proceeds (iOS). Only the host can make it: the
 * sandboxed frame can neither navigate the top-level document nor its own
 * frame.
 */
export const ARTIFACT_HOST_OPEN_PREFIX = "kanna-host:open?path=";

/**
 * The host document's own title while it asks the viewer for another file of
 * the tree on Android: this prefix and the encoded path. Nothing there may
 * navigate for it — react-native-webview lets through a navigation the JS
 * thread has not answered within 250 ms, and an unknown scheme then fails on
 * screen with its URL logged. So the host sets its title and then changes its
 * own URL fragment with `history.replaceState`, which loads nothing and asks no
 * navigation callback. The library reports that change to `onLoadStart`
 * (`doUpdateVisitedHistory`) with the WebView's title, whenever the JS thread
 * gets to it; the URL it reports is the document's load URL, never the
 * fragment. Only the host can set this title: the page's frame is another
 * origin.
 */
export const ARTIFACT_HOST_OPEN_TITLE_PREFIX = "kanna-host-open:";

/** How the host asks the viewer for a file: a refused navigation, or its title. */
export type ArtifactHostOpen = "navigation" | "title";

function hostOpenStatement(mode: ArtifactHostOpen): string {
  // Each request gets a fragment of its own, so each is a change to report.
  return mode === "title"
    ? `document.title=${JSON.stringify(ARTIFACT_HOST_OPEN_TITLE_PREFIX)}+encodeURIComponent(d.path);history.replaceState(null,"","#kanna-host-open-"+(++opens))`
    : `location.href=${JSON.stringify(ARTIFACT_HOST_OPEN_PREFIX)}+encodeURIComponent(d.path)`;
}

/**
 * The trusted host document's own script. It shows the page it was given in
 * its frame. When the frame asks for another file, it accepts the request only
 * from its own frame, only of the one kind, and only for a path that is a file
 * of this tree other than the one shown; then it asks the viewer for that file
 * (`hostOpenStatement`), which the viewer answers by rendering the file.
 * Anything else is ignored. The host has no bridge to reach: the WebView gets
 * no `onMessage`.
 */
function artifactHostScript(mode: ArtifactHostOpen): string {
  return `(function(){var opens=0;var data=JSON.parse(document.getElementById("kanna-artifact-host").textContent);var files=new Set(data.files);var frame=document.getElementById("kanna-artifact-frame");frame.setAttribute("data-path",data.current);frame.srcdoc=data.page;window.addEventListener("message",function(e){if(e.source!==frame.contentWindow)return;var d=e.data;if(!d||typeof d!=="object"||d.kind!==${JSON.stringify(ARTIFACT_NAVIGATE_MESSAGE)}||typeof d.path!=="string"||d.path===data.current||!files.has(d.path))return;frame.setAttribute("data-requested",d.path);${hostOpenStatement(mode)}})})();`;
}

export interface ArtifactHostOptions {
  /** The rendered page to show. */
  page: string;
  /** Its path in the tree. */
  current: string;
  /** Every file path of the tree, from the artifact's own descriptor. */
  files: readonly string[];
  /** How the host asks for another file; `navigation` unless the viewer says otherwise. */
  hostOpen?: ArtifactHostOpen;
}

/**
 * Put one rendered artifact page inside a trusted host document, the page in a
 * frame sandboxed with `allow-scripts` alone.
 *
 * The WebView's navigation callback cannot be the barrier against top-level
 * navigation: on Android, react-native-webview allows a navigation the JS
 * thread has not answered within 250 ms. Inside this frame the engine itself
 * refuses the page's attempts to navigate the top-level document, open a
 * window or submit a form. The host policy is the page's policy, so the frame
 * cannot navigate itself either (`frame-src 'none'`). An in-tree link is a
 * request to the host, which forwards only a file of this tree to the viewer.
 */
export function isolateArtifactDocument({ page, current, files, hostOpen = "navigation" }: ArtifactHostOptions): string {
  // JSON inside a script element: `<` escaped so no `</script>` can end it.
  const data = JSON.stringify({ page, current, files }).replace(/</g, "\\u003c");
  return `<!doctype html>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_DOCUMENT_POLICY}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>html,body{margin:0;height:100%;background:#fff}iframe{display:block;border:0;width:100%;height:100%}</style>
<iframe id="kanna-artifact-frame" sandbox="${ARTIFACT_FRAME_SANDBOX}" referrerpolicy="no-referrer"></iframe>
<script type="application/json" id="kanna-artifact-host">${data}</script>
<script>${artifactHostScript(hostOpen)}</script>`;
}

/**
 * The file a host-open navigation names, if it is exactly one of `files`;
 * null for any other URL.
 */
export function artifactHostOpenPath(url: string, files: ReadonlySet<string>): string | null {
  if (!url.startsWith(ARTIFACT_HOST_OPEN_PREFIX)) return null;
  let path: string;
  try {
    path = decodeURIComponent(url.slice(ARTIFACT_HOST_OPEN_PREFIX.length));
  } catch {
    return null;
  }
  return files.has(path) ? path : null;
}

/**
 * The file the host document's title asks for, if it is exactly one of
 * `files`; null for any other title.
 */
export function artifactHostOpenTitlePath(title: string, files: ReadonlySet<string>): string | null {
  if (!title.startsWith(ARTIFACT_HOST_OPEN_TITLE_PREFIX)) return null;
  let path: string;
  try {
    path = decodeURIComponent(title.slice(ARTIFACT_HOST_OPEN_TITLE_PREFIX.length));
  } catch {
    return null;
  }
  return files.has(path) ? path : null;
}
