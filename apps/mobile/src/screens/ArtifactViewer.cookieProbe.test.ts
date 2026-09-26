import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const walkthroughPath = fileURLToPath(
  new URL("../../../../docs/2026-09-23-section-14-two-account-artifact-walkthrough.md", import.meta.url)
);
const walkthrough = readFileSync(walkthroughPath, "utf8");
const cookieSnippet = walkthrough.match(
  /attempt\("cookies",[\s\S]*?\n(?=attempt\("localStorage")/
)?.[0];

if (!cookieSnippet) throw new Error("Cookie probe snippet is missing from the walkthrough");

type CookieDocument = {
  cookie: string;
};

function securityError(): Error {
  const error = new Error("Cookies are disabled");
  error.name = "SecurityError";
  return error;
}

function runCookieProbe(document: CookieDocument): string {
  let output = "";
  const attempt = (
    name: string,
    action: () => string,
    blocked: (value: string) => boolean
  ) => {
    try {
      const value = action();
      output = `${name}: ${blocked(value) ? `blocked (${value})` : `ALLOWED (${value})`}`;
    } catch (error) {
      output = `${name}: blocked (${(error as Error).name})`;
    }
  };

  Function("document", "attempt", cookieSnippet)(document, attempt);
  return output;
}

describe("walkthrough cookie probe", () => {
  it("passes when reads are empty and writes are ignored", () => {
    const document = Object.defineProperty({}, "cookie", {
      get: () => "",
      set: () => undefined
    }) as CookieDocument;

    expect(runCookieProbe(document)).toBe("cookies: blocked (isolated (write not readable))");
  });

  it("fails when the newly written marker is readable", () => {
    let cookie = "";
    const document = Object.defineProperty({}, "cookie", {
      get: () => cookie,
      set: (value: string) => { cookie = value.split(";", 1)[0]; }
    }) as CookieDocument;

    expect(runCookieProbe(document)).toMatch(/^cookies: ALLOWED \(LEAKED kanna_probe_cookie=\d+\)$/);
  });

  it("fails on a pre-existing readable cookie when writes are ignored", () => {
    const document = Object.defineProperty({}, "cookie", {
      get: () => "session=exposed",
      set: () => undefined
    }) as CookieDocument;

    expect(runCookieProbe(document)).toBe("cookies: ALLOWED (LEAKED session=exposed)");
  });

  it("keeps a pre-existing leak visible when the subsequent write throws", () => {
    const document = Object.defineProperty({}, "cookie", {
      get: () => "session=exposed",
      set: () => { throw securityError(); }
    }) as CookieDocument;

    expect(runCookieProbe(document)).toBe("cookies: ALLOWED (LEAKED session=exposed)");
  });

  it("fails when a marker from a previous load is readable", () => {
    const document = Object.defineProperty({}, "cookie", {
      get: () => "kanna_probe_cookie=previous",
      set: () => undefined
    }) as CookieDocument;

    expect(runCookieProbe(document)).toBe("cookies: ALLOWED (LEAKED kanna_probe_cookie=previous)");
  });

  it("passes when cookie access throws SecurityError", () => {
    const document = Object.defineProperty({}, "cookie", {
      get: () => { throw securityError(); },
      set: () => { throw securityError(); }
    }) as CookieDocument;

    expect(runCookieProbe(document)).toBe("cookies: blocked (SecurityError)");
  });
});
