import { describe, expect, it, vi } from "vitest";
import type { ArtifactFileContent } from "../lib/api/types";
import {
  ARTIFACT_DOCUMENT_POLICY,
  buildArtifactDocument,
  ArtifactBuildCancelled,
  artifactHostOpenPath,
  decodeBase64,
  decodeUtf8,
  encodeBase64,
  encodeUtf8,
  resolveArtifactReference
} from "./buildArtifactDocument";

const MEDIA_TYPES: Record<string, string> = {
  html: "text/html; charset=utf-8",
  css: "text/css; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  png: "image/png",
  woff2: "font/woff2",
  md: "text/markdown; charset=utf-8"
};

function tree(files: Record<string, string | Uint8Array>) {
  const reads: string[] = [];
  const readFile = vi.fn(async (path: string): Promise<ArtifactFileContent> => {
    reads.push(path);
    const content = files[path];
    if (content === undefined) {
      throw new Error(`Remote request failed (404): {"error":"artifact_file_not_found","path":"${path}"}`);
    }
    const bytes = typeof content === "string" ? encodeUtf8(content) : content;
    const extension = path.split(".").pop() ?? "";
    return {
      repoId: "repo-1",
      artifactId: "a".repeat(40),
      path,
      mediaType: MEDIA_TYPES[extension] ?? "application/octet-stream",
      size: bytes.length,
      dataBase64: encodeBase64(bytes)
    };
  });
  return { readFile, reads };
}

function dataUrlText(url: string): string {
  return decodeUtf8(decodeBase64(url.slice(url.indexOf(",") + 1)));
}

const PNG = Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0xff]);

const MOCKUP = {
  "index.html": `<!doctype html>
<html><head>
<link rel="stylesheet" href="css/site.css">
<script src="js/app.js"></script>
<style>.hero { background: url('img/hero.png') }</style>
</head><body>
<!-- <img src="img/commented-out.png"> -->
<img id="logo" src="img/logo.png" srcset="img/logo.png 1x, img/logo.png 2x" alt="logo">
<div style="background-image:url(img/hero.png)"></div>
<a id="about" href="pages/about.html#team">About</a>
<a id="external" href="https://example.com/">External</a>
<a id="fragment" href="#top">Top</a>
<img id="remote" src="https://example.com/tracker.png">
<img id="gone" src="img/missing.png">
<script>document.body.dataset.ran = "<img src='img/in-script.png'>";</script>
</body></html>`,
  "css/site.css": `@import "theme.css";
@font-face { font-family: Brand; src: url("../fonts/brand.woff2") format("woff2"); }
body { background: url(../img/hero.png); }`,
  "css/theme.css": `h1 { color: #123456; }`,
  "js/app.js": `document.title = "mockup — ünïcode";`,
  "img/logo.png": PNG,
  "img/hero.png": PNG,
  "fonts/brand.woff2": Uint8Array.from([1, 2, 3, 4]),
  "pages/about.html": `<link rel="stylesheet" href="../css/site.css"><h1>About</h1>`
};

describe("buildArtifactDocument", () => {
  it("inlines every relative asset of a multi-file HTML mockup from its own tree", async () => {
    const { readFile, reads } = tree(MOCKUP);
    const document = await buildArtifactDocument({ path: "index.html", readFile });
    const html = document.html;

    const stylesheet = html.match(/<link rel="stylesheet" href="(data:[^"]+)">/)?.[1];
    expect(stylesheet).toMatch(/^data:text\/css;charset=utf-8;base64,/);
    const css = dataUrlText(stylesheet!);
    // Nested references inside the stylesheet resolve against the stylesheet's own directory.
    expect(css).toMatch(/url\("data:font\/woff2;base64,AQIDBA=="\)/);
    expect(css).toContain(`url("data:image/png;base64,${encodeBase64(PNG)}")`);
    const imported = css.match(/@import url\("(data:[^"]+)"\)/)?.[1];
    expect(dataUrlText(imported!)).toContain("#123456");

    const script = html.match(/<script src="(data:[^"]+)"><\/script>/)?.[1];
    expect(dataUrlText(script!)).toBe(`document.title = "mockup — ünïcode";`);
    expect(html).toContain(`<img id="logo" src="data:image/png;base64,${encodeBase64(PNG)}"`);
    expect(html).toContain(`srcset="data:image/png;base64,${encodeBase64(PNG)} 1x, data:image/png;base64,${encodeBase64(PNG)} 2x"`);
    expect(html).toContain(`<style>.hero { background: url("data:image/png;base64,`);
    expect(html).toContain(`style="background-image:url(&quot;data:image/png;base64,`);

    // In-tree page links are handed to the host; the rest are left for the WebView to refuse.
    // An in-tree link becomes a request to the host, never a navigation.
    expect(html).toContain(`<a id="about" href="#kanna-artifact=pages/about.html">`);
    expect(html).toContain(`<a id="external" href="https://example.com/">`);
    expect(html).toContain(`<a id="fragment" href="#top">`);
    expect(html).toContain(`<img id="remote" src="https://example.com/tracker.png">`);

    // Comments and script bodies are not read as markup.
    expect(html).toContain(`<!-- <img src="img/commented-out.png"> -->`);
    expect(reads).not.toContain("img/commented-out.png");
    expect(reads).not.toContain("img/in-script.png");

    expect(document.missing).toEqual(["img/missing.png"]);
    expect(document.skipped).toEqual([]);
    // Each file is read once however often it is referenced.
    expect(reads.filter((path) => path === "img/hero.png")).toHaveLength(1);
  });

  it("puts a no-network policy ahead of anything the artifact declares", async () => {
    const { readFile } = tree({
      "index.html": `<!DOCTYPE html><head><meta http-equiv="Content-Security-Policy" content="default-src *"><base href="https://evil.example/"></head><p>hi</p>`
    });
    const { html } = await buildArtifactDocument({ path: "index.html", readFile });
    expect(html.startsWith(`<!doctype html>\n<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_DOCUMENT_POLICY}">`)).toBe(true);
    for (const directive of ["connect-src 'none'", "frame-src 'none'", "form-action 'none'", "base-uri 'none'", "img-src data:"]) {
      expect(ARTIFACT_DOCUMENT_POLICY).toContain(directive);
    }
    expect(ARTIFACT_DOCUMENT_POLICY).not.toMatch(/https?:|\*/);
  });

  it("renders a relative page of the same tree with its own relative assets", async () => {
    const { readFile } = tree(MOCKUP);
    const { html, missing } = await buildArtifactDocument({ path: "pages/about.html", readFile });
    const stylesheet = html.match(/href="(data:text\/css[^"]+)"/)?.[1];
    expect(dataUrlText(stylesheet!)).toContain("font-family: Brand");
    expect(missing).toEqual([]);
  });

  it("shows an anchored text file as source at the anchored line", async () => {
    const { readFile } = tree({ "css/site.css": "a{}\nb{}\nheader { height: 120px }\n" });
    const { html } = await buildArtifactDocument({ path: "css/site.css", readFile, initialLine: 3 });
    expect(html).toContain("120px");
    expect(html).toContain("const targetLine = 3;");
  });

  it("reads no further file once the viewer has moved on", async () => {
    const { readFile, reads } = tree(MOCKUP);
    let cancelled = false;
    const reading = buildArtifactDocument({
      path: "index.html",
      readFile: async (path) => {
        const file = await readFile(path);
        cancelled = true;
        return file;
      },
      isCancelled: () => cancelled
    });
    await expect(reading).rejects.toBeInstanceOf(ArtifactBuildCancelled);
    expect(reads).toEqual(["index.html"]);
    await expect(
      buildArtifactDocument({ path: "index.html", readFile, isCancelled: () => true })
    ).rejects.toBeInstanceOf(ArtifactBuildCancelled);
    expect(reads).toEqual(["index.html"]);
  });

  it("names only a file of the tree in a host-open request", () => {
    const files = new Set(["index.html", "pages/a b.html"]);
    expect(artifactHostOpenPath("kanna-host:open?path=pages%2Fa%20b.html", files)).toBe("pages/a b.html");
    expect(artifactHostOpenPath("kanna-host:open?path=..%2Findex.html", files)).toBeNull();
    expect(artifactHostOpenPath("kanna-host:open?path=%E0%A4%A", files)).toBeNull();
    expect(artifactHostOpenPath("kanna-host:open?path=", files)).toBeNull();
    expect(artifactHostOpenPath("https://example.com/?path=index.html", files)).toBeNull();
  });

  it("resolves references the way a browser does under the tree root, and never outside it", () => {
    expect(resolveArtifactReference("pages/about.html", "../css/site.css")).toBe("css/site.css");
    expect(resolveArtifactReference("index.html", "./img/a%20b.png?v=2#x")).toBe("img/a b.png");
    expect(resolveArtifactReference("index.html", "docs/")).toBe("docs/index.html");
    expect(resolveArtifactReference("index.html", "../../etc/passwd")).toBeNull();
    expect(resolveArtifactReference("index.html", "/abs.css")).toBeNull();
    expect(resolveArtifactReference("index.html", "//cdn.example/x.js")).toBeNull();
    expect(resolveArtifactReference("index.html", "javascript:alert(1)")).toBeNull();
    expect(resolveArtifactReference("index.html", "#only")).toBeNull();
  });

  it("round-trips base64 and UTF-8 without platform codecs", () => {
    const text = "plain ascii — ünïcode 🎨";
    expect(decodeUtf8(decodeBase64(encodeBase64(encodeUtf8(text))))).toBe(text);
    expect(encodeBase64(Uint8Array.from([1, 2]))).toBe("AQI=");
    expect(Array.from(decodeBase64("AQI="))).toEqual([1, 2]);
  });
});
