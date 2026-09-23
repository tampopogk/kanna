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
 * stylesheets — is read and inlined as a `data:` URL. Links to another page of
 * the same tree become `kanna-artifact:<path>` so the host, not the page,
 * decides what opens next; the WebView refuses every other navigation.
 *
 * The page gets a Content-Security-Policy with no network at all
 * (`connect-src 'none'` and only `data:` subresources), no frames, no forms
 * and no `<base>`. A policy can only be tightened by a later one, so nothing
 * the artifact declares can loosen it.
 */

export const ARTIFACT_NAVIGATION_SCHEME = "kanna-artifact:";

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

  constructor(private readonly readFile: ReadArtifactDocumentFile) {}

  read(path: string): Promise<ArtifactFileContent | null> {
    let pending = this.reads.get(path);
    if (!pending) {
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
        rewritten = path ? `${ARTIFACT_NAVIGATION_SCHEME}${encodeArtifactPath(path)}${fragmentOf(value)}` : null;
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
    return `<!doctype html>\n${policy}\n<meta name="viewport" content="width=device-width, initial-scale=1">\n${withoutDoctype}`;
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
  initialLine
}: ArtifactDocumentOptions): Promise<ArtifactDocument> {
  const builder = new ArtifactPageBuilder(readFile);
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
  return { html, missing: [...builder.missing], skipped: [...builder.skipped] };
}

/** The in-tree path a `kanna-artifact:` navigation names, or null for anything else. */
export function artifactNavigationPath(url: string): string | null {
  if (!url.startsWith(ARTIFACT_NAVIGATION_SCHEME)) return null;
  const reference = url.slice(ARTIFACT_NAVIGATION_SCHEME.length).split("#", 1)[0];
  return resolveArtifactReference("", reference);
}

function encodeArtifactPath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

/**
 * The policy of the host document that frames an artifact page. A `srcdoc`
 * frame inherits it, so it is the page's policy except that the host may
 * navigate its one child frame to an in-tree `kanna-artifact:` link.
 */
export const ARTIFACT_HOST_POLICY = ARTIFACT_DOCUMENT_POLICY
  .replace("frame-src 'none'", `frame-src ${ARTIFACT_NAVIGATION_SCHEME}`)
  .replace("child-src 'none'", `child-src ${ARTIFACT_NAVIGATION_SCHEME}`);

/** Sandbox flags for the frame that runs an artifact page: scripts, nothing else. */
export const ARTIFACT_FRAME_SANDBOX = "allow-scripts";

/**
 * Put an artifact page inside a script-free host document, in a frame
 * sandboxed with `allow-scripts` alone.
 *
 * The WebView's navigation callback cannot be the only thing standing between
 * a page and a top-level navigation: on Android, react-native-webview allows a
 * navigation whenever the JS thread has not answered within 250 ms. Inside
 * this frame the engine itself refuses the page's attempts to navigate the
 * top-level document, open a window or submit a form, without asking the JS
 * thread. The frame can only navigate itself, and the host policy lets that
 * happen only for `kanna-artifact:` links, which carry no network request.
 */
export function isolateArtifactDocument(page: string): string {
  const srcdoc = page
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
  return `<!doctype html>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_HOST_POLICY}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>html,body{margin:0;height:100%;background:#fff}iframe{display:block;border:0;width:100%;height:100%}</style>
<iframe sandbox="${ARTIFACT_FRAME_SANDBOX}" referrerpolicy="no-referrer" srcdoc="${srcdoc}"></iframe>`;
}
